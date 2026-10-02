//! Owned registrations: the Rust side is the authority (design §3).
//!
//! Every registration belongs to exactly one `(plugin_id, generation)` owner.
//! Revocation is synchronous and never waits for the host: removing an owner
//! removes its tools from every later turn's registry at once, and
//! `HostToolSpec` re-checks liveness before each call. Handles are never
//! reused, so undoing one registration can never touch a newer one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use super::protocol::{OwnerRef, RegisterKind, RegisterParams};
use crate::plugins::types::PluginAuthority;

/// Largest accepted tool input schema, serialized.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Largest accepted tool description.
pub const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
pub const MAX_TOOLS_PER_OWNER: usize = 128;
pub const MAX_TOOLS_PER_HOST: usize = 1024;
/// Commands are listed in the palette and `/help`, so the caps are tighter.
pub const MAX_COMMANDS_PER_OWNER: usize = 64;
pub const MAX_COMMANDS_PER_HOST: usize = 256;
/// A command description is one palette line; a hint is a short placeholder.
pub const MAX_COMMAND_DESCRIPTION_BYTES: usize = 1024;
pub const MAX_COMMAND_HINT_BYTES: usize = 256;

/// Name prefixes no extension may use: MCP's namespace, and one kept free
/// for future core-issued extension names.
const RESERVED_PREFIXES: &[&str] = &["mcp_", "ext_"];
const NAME_HINT: &str = "use a plugin-specific prefix, for example `myplugin_read_x`";

/// Core tool names that exist outside the native registry builder (catalog
/// meta-tools) and so never show up in a registry snapshot.
const RESERVED_NAMES: &[&str] = &[
    "tool_search",
    "tool_search_tool_regex",
    "tool_search_tool_bm25",
    "retrieve_tool_result",
    "execute_tools",
    "code_execution",
    "js_execution",
    "request_user_input",
    "multi_tool_use.parallel",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    Activating,
    Active,
    Failed(String),
    Faulted(String),
    Revoked,
}

#[derive(Debug, Clone)]
pub struct OwnerEntry {
    pub owner: OwnerRef,
    pub plugin_name: String,
    pub authority: PluginAuthority,
    pub content_hash: String,
    /// Digest of the plugin config this activation was given (empty until
    /// [`OwnerRegistry::set_config_hash`]). A change of config is a different
    /// activation: reconcile revokes the owner and activates a new generation.
    pub config_hash: String,
    pub state: OwnerState,
}

/// Most schema violations one refused call reports back to the model.
const MAX_REPORTED_INPUT_ERRORS: usize = 5;
/// Bound on the refusal text (violations quote the offending values).
const MAX_INPUT_ERROR_BYTES: usize = 2048;

/// A tool's input schema, compiled once at registration. The core checks every
/// call's input against it before anything is sent to the host: a plugin that
/// registers a plain object receives only input its own schema admits, without
/// having to validate it itself.
///
/// Compiled by `jsonschema` (already linked for Workflow `responseSchema`),
/// which resolves no external `$ref` here (no network or file resolver is
/// enabled); a schema it cannot compile is refused at registration.
#[derive(Clone)]
pub struct InputValidator(Arc<jsonschema::Validator>);

impl InputValidator {
    /// Compile `schema`, or say why it cannot be.
    pub fn compile(schema: &Value) -> Result<Self, String> {
        jsonschema::validator_for(schema)
            .map(|validator| Self(Arc::new(validator)))
            .map_err(|error| error.to_string())
    }

    /// `Ok` when `input` satisfies the schema; otherwise the violations, as
    /// text a model can correct its call from.
    pub fn check(&self, input: &Value) -> Result<(), String> {
        let mut errors = self.0.iter_errors(input);
        let Some(first) = errors.next() else {
            return Ok(());
        };
        let describe = |error: &jsonschema::ValidationError<'_>| {
            let at = error.instance_path().to_string();
            if at.is_empty() {
                error.to_string()
            } else {
                format!("{error} (at {at})")
            }
        };
        let mut reasons = vec![describe(&first)];
        let mut more = 0usize;
        for error in errors {
            if reasons.len() < MAX_REPORTED_INPUT_ERRORS {
                reasons.push(describe(&error));
            } else {
                more += 1;
            }
        }
        let mut text = reasons.join("; ");
        if more > 0 {
            text.push_str(&format!("; and {more} more"));
        }
        if text.len() > MAX_INPUT_ERROR_BYTES {
            let mut end = MAX_INPUT_ERROR_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        Err(text)
    }
}

impl PartialEq for InputValidator {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for InputValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputValidator").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRegistration {
    pub handle: u64,
    pub owner: OwnerRef,
    pub plugin_name: String,
    /// The reviewed bundle content hash of the owner that registered it: the
    /// receipt its approval grants are bound to.
    pub content_hash: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// `input_schema`, compiled: every call is checked against it.
    pub input_validator: InputValidator,
}

/// An admitted slash command. Owned exactly like a tool: one owner
/// generation, a never-reused handle, removed with its owner.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRegistration {
    pub handle: u64,
    pub owner: OwnerRef,
    pub plugin_name: String,
    /// The reviewed bundle content hash of the registering owner.
    pub content_hash: String,
    /// The slash-command name, without the slash (lower case).
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Default)]
pub struct OwnerRegistry {
    next_handle: u64,
    next_generation: u64,
    owners: HashMap<String, OwnerEntry>,
    tools: BTreeMap<u64, ToolRegistration>,
    /// Lower-cased tool name → handle, so `Read` cannot impersonate `read`.
    by_name: HashMap<String, u64>,
    commands: BTreeMap<u64, CommandRegistration>,
    /// Command name → handle. Commands and tools are separate namespaces: a
    /// tool is called by the model, a command by the user.
    commands_by_name: HashMap<String, u64>,
    /// Lower-cased names of every native tool any engine's turn build has
    /// reported, plus the static set. Only ever grows: engines in one
    /// process build different native surfaces, and a name that is native
    /// anywhere is refused everywhere.
    native_names: HashSet<String>,
}

fn mint_token() -> String {
    // Two v4 UUIDs: 244 random bits from the OS generator.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// DSH's command grammar (`^[a-z][a-z0-9_-]*$`), bounded. Lower case only:
/// the user's input is lower-cased before it is looked up.
fn valid_command_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Whether a built-in command answers to `name`: a canonical name, an alias,
/// or one of the fixed mode aliases dispatched ahead of the registry.
fn builtin_command(name: &str) -> bool {
    matches!(name, "jihua" | "zidong") || crate::commands::registry().get(name).is_some()
}

/// Why the core would treat a tool called `name` as something other than an
/// opaque extension tool, if it would.
///
/// Approval keys, approval-card summaries, and the approval/auto-review
/// category are all derived from the tool *name*. A name any of them
/// special-cases would let an extension borrow a native tool's identity: a
/// session grant for `fetch_url` on a host (`net:<host>`) would approve a
/// plugin tool named `web_fetch`, and a `read_*` name would be classified as
/// a read. Such names need not be registered natives (`web_fetch`,
/// `exec_wait`, and mode-dependent tools such as `task_shell_start` are not),
/// so they are refused by probing the classifiers themselves rather than by a
/// hand-kept list that would drift from them.
fn core_special_case(name: &str) -> Option<&'static str> {
    use crate::core::authority::{ToolCategory, get_tool_category_for_call};
    use crate::tools::approval_cache::{build_approval_grouping_key, build_approval_key};
    let empty = Value::Object(serde_json::Map::new());
    let spellings = [name.to_string(), name.to_ascii_lowercase()];
    for spelling in &spellings {
        let generic = format!("tool:{spelling}:");
        if !build_approval_key(spelling, &empty).0.starts_with(&generic)
            || !build_approval_grouping_key(spelling, &empty)
                .0
                .starts_with(&generic)
        {
            return Some("the approval cache keys it as a built-in tool family");
        }
        if crate::tools::canonical_action::canonical_action_alias(spelling, &empty) != spelling {
            return Some("it is an alias of a built-in tool");
        }
        if crate::tools::approval_summary::approval_summary(spelling, &empty, None)
            != format!("Use the {spelling} tool")
        {
            return Some("approval cards describe it as a built-in tool");
        }
        if get_tool_category_for_call(spelling, &empty) != ToolCategory::Unknown {
            return Some(
                "the approval policy classifies it by name (read, write, shell, network, MCP or agent)",
            );
        }
    }
    None
}

impl OwnerRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut native_names: HashSet<String> = RESERVED_NAMES
            .iter()
            .chain(crate::core::engine::tool_catalog::DEFAULT_ACTIVE_NATIVE_TOOLS)
            .map(|name| name.to_ascii_lowercase())
            .collect();
        for (family, _, alias) in crate::tools::canonical_action::CANONICAL_ACTION_ALIASES {
            native_names.insert(family.to_ascii_lowercase());
            native_names.insert(alias.to_ascii_lowercase());
        }
        Self {
            native_names,
            ..Self::default()
        }
    }

    /// Add native tool names from one engine's turn build (the registry
    /// before scripts, plugins or extensions are added). Never removes one.
    pub fn add_native_names<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        self.native_names
            .extend(names.into_iter().map(str::to_ascii_lowercase));
    }

    /// Start a new activation for `plugin_id`, superseding any previous one.
    pub fn begin_owner(
        &mut self,
        plugin_id: &str,
        plugin_name: &str,
        authority: PluginAuthority,
        content_hash: &str,
    ) -> OwnerRef {
        self.revoke_owner(plugin_id);
        self.next_generation += 1;
        let owner = OwnerRef {
            plugin_id: plugin_id.to_string(),
            generation: self.next_generation,
            owner_token: mint_token(),
        };
        self.owners.insert(
            plugin_id.to_string(),
            OwnerEntry {
                owner: owner.clone(),
                plugin_name: plugin_name.to_string(),
                authority,
                content_hash: content_hash.to_string(),
                config_hash: String::new(),
                state: OwnerState::Activating,
            },
        );
        owner
    }

    /// Record which config `owner`'s activation was given.
    pub fn set_config_hash(&mut self, owner: &OwnerRef, hash: &str) {
        if let Some(entry) = self.owners.get_mut(&owner.plugin_id)
            && entry.owner == *owner
        {
            entry.config_hash = hash.to_string();
        }
    }

    #[must_use]
    pub fn owner(&self, plugin_id: &str) -> Option<&OwnerEntry> {
        self.owners.get(plugin_id)
    }

    pub fn owners(&self) -> impl Iterator<Item = &OwnerEntry> {
        self.owners.values()
    }

    /// The exact current, not-yet-revoked owner (token and generation match).
    fn current(&self, owner: &OwnerRef) -> Option<&OwnerEntry> {
        self.owners.get(&owner.plugin_id).filter(|entry| {
            entry.owner == *owner
                && matches!(entry.state, OwnerState::Activating | OwnerState::Active)
        })
    }

    pub fn mark_active(&mut self, owner: &OwnerRef) -> bool {
        match self.owners.get_mut(&owner.plugin_id) {
            Some(entry) if entry.owner == *owner && entry.state == OwnerState::Activating => {
                entry.state = OwnerState::Active;
                super::command::bump_epoch();
                true
            }
            _ => false,
        }
    }

    /// Mark an owner failed or faulted and drop everything it registered.
    pub fn mark_failed(&mut self, owner: &OwnerRef, state: OwnerState) -> bool {
        let matches = self
            .owners
            .get(&owner.plugin_id)
            .is_some_and(|entry| entry.owner == *owner);
        if matches {
            self.remove_registrations_of(&owner.plugin_id);
            if let Some(entry) = self.owners.get_mut(&owner.plugin_id) {
                entry.state = state;
            }
        }
        matches
    }

    /// Admit or refuse one `registry/register`, whatever its kind.
    pub fn register(&mut self, params: &RegisterParams) -> Result<u64, String> {
        match params.kind {
            RegisterKind::Tool => self.register_tool(params),
            RegisterKind::Command => self.register_command(params),
        }
    }

    /// Admit or refuse one command registration. An extension command never
    /// shadows a built-in command or another plugin's command; a clash with
    /// a user, workspace or manifest (markdown) command is resolved when the
    /// user registry loads (the markdown command wins and the extension
    /// command is not loaded), because only that registry knows the workspace.
    pub fn register_command(&mut self, params: &RegisterParams) -> Result<u64, String> {
        let entry = self
            .current(&params.owner)
            .ok_or_else(|| "stale or unknown owner".to_string())?;
        let plugin_name = entry.plugin_name.clone();
        let content_hash = entry.content_hash.clone();
        let spec = &params.spec;
        let name = spec.name.as_str();
        const COMMAND_HINT: &str = "a command name is lower case, starts with a letter, and uses only a-z, 0-9, `_` and `-` (at most 64 characters)";
        if !valid_command_name(name) {
            return Err(format!(
                "command name `{}` is invalid: {COMMAND_HINT}",
                crate::safe_label::SafeLabel::identifier(name)
            ));
        }
        if builtin_command(name) {
            return Err(format!(
                "command `/{name}` collides with a built-in command; extensions never shadow core commands; use a plugin-specific name, for example `/myplugin-{name}`"
            ));
        }
        if spec.input_schema.is_some() {
            return Err(format!("command `/{name}` has no input schema"));
        }
        let description = spec.description.trim();
        if description.is_empty() {
            return Err(format!("command `/{name}` needs a description"));
        }
        if description.len() > MAX_COMMAND_DESCRIPTION_BYTES {
            return Err(format!(
                "command `/{name}` description exceeds {MAX_COMMAND_DESCRIPTION_BYTES} bytes"
            ));
        }
        let hint = spec.argument_hint.as_deref().map(str::trim);
        if hint.is_some_and(str::is_empty) {
            return Err(format!("command `/{name}` argument hint must not be empty"));
        }
        if hint.is_some_and(|hint| hint.len() > MAX_COMMAND_HINT_BYTES) {
            return Err(format!(
                "command `/{name}` argument hint exceeds {MAX_COMMAND_HINT_BYTES} bytes"
            ));
        }
        // The palette, `/help` and the composer print these verbatim.
        if description.chars().any(char::is_control)
            || hint.is_some_and(|h| h.chars().any(char::is_control))
        {
            return Err(format!(
                "command `/{name}` description and argument hint must be single-line text without control characters"
            ));
        }
        let mut replaced = None;
        if let Some(existing) = self
            .commands_by_name
            .get(name)
            .and_then(|handle| self.commands.get(handle))
        {
            if existing.owner.plugin_id != params.owner.plugin_id {
                return Err(format!(
                    "command `/{name}` is already registered by extension `{}`; use a plugin-specific name",
                    existing.plugin_name
                ));
            }
            // Same owner re-registering a name: the new handle retires the old.
            replaced = Some(existing.handle);
        }
        let owned = self
            .commands
            .values()
            .filter(|command| command.owner.plugin_id == params.owner.plugin_id)
            .count()
            - usize::from(replaced.is_some());
        if owned >= MAX_COMMANDS_PER_OWNER {
            return Err(format!(
                "an extension may register at most {MAX_COMMANDS_PER_OWNER} commands"
            ));
        }
        if self.commands.len() - usize::from(replaced.is_some()) >= MAX_COMMANDS_PER_HOST {
            return Err(format!(
                "the extension host holds at most {MAX_COMMANDS_PER_HOST} commands"
            ));
        }
        if let Some(old) = replaced {
            self.commands.remove(&old);
        }
        self.next_handle += 1;
        let handle = self.next_handle;
        self.commands.insert(
            handle,
            CommandRegistration {
                handle,
                owner: params.owner.clone(),
                plugin_name,
                content_hash,
                name: name.to_string(),
                description: description.to_string(),
                argument_hint: hint.map(str::to_string),
            },
        );
        self.commands_by_name.insert(name.to_string(), handle);
        super::command::bump_epoch();
        Ok(handle)
    }

    /// Admit or refuse one tool registration.
    pub fn register_tool(&mut self, params: &RegisterParams) -> Result<u64, String> {
        let entry = self
            .current(&params.owner)
            .ok_or_else(|| "stale or unknown owner".to_string())?;
        let plugin_name = entry.plugin_name.clone();
        let content_hash = entry.content_hash.clone();
        let spec = &params.spec;
        let name = spec.name.as_str();
        if !valid_tool_name(name) {
            return Err(format!(
                "tool name `{}` must match ^[A-Za-z][A-Za-z0-9_-]{{0,63}}$; {NAME_HINT}",
                crate::safe_label::SafeLabel::identifier(name)
            ));
        }
        let key = name.to_ascii_lowercase();
        if RESERVED_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
        {
            return Err(format!(
                "tool name `{name}` uses a reserved prefix; {NAME_HINT}"
            ));
        }
        if self.native_names.contains(&key) {
            return Err(format!(
                "tool name `{name}` collides with a built-in tool; extensions never shadow core tools; {NAME_HINT}"
            ));
        }
        if let Some(reason) = core_special_case(name) {
            return Err(format!(
                "tool name `{name}` is reserved: {reason}; extension tools never borrow a built-in's approval identity; {NAME_HINT}"
            ));
        }
        if spec.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(format!(
                "tool `{name}` description exceeds {MAX_DESCRIPTION_BYTES} bytes"
            ));
        }
        let schema = Value::Object(
            spec.input_schema
                .clone()
                .ok_or_else(|| format!("tool `{name}` needs an input schema"))?,
        );
        let schema_bytes = serde_json::to_vec(&schema)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if schema_bytes > MAX_SCHEMA_BYTES {
            return Err(format!(
                "tool `{name}` input schema exceeds {MAX_SCHEMA_BYTES} bytes"
            ));
        }
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!(
                "tool `{name}` input schema must be a JSON object schema (`\"type\": \"object\"`)"
            ));
        }
        let input_validator = InputValidator::compile(&schema).map_err(|reason| {
            format!("tool `{name}` input schema is not a valid JSON Schema: {reason}")
        })?;
        let mut replaced = None;
        if let Some(existing) = self
            .by_name
            .get(&key)
            .and_then(|handle| self.tools.get(handle))
        {
            if existing.owner.plugin_id != params.owner.plugin_id {
                return Err(format!(
                    "tool name `{name}` is already registered by extension `{}`; {NAME_HINT}",
                    existing.plugin_name
                ));
            }
            // Same owner re-registering a name: the new handle retires the old.
            replaced = Some(existing.handle);
        }
        let owned = self
            .tools
            .values()
            .filter(|tool| tool.owner.plugin_id == params.owner.plugin_id)
            .count()
            - usize::from(replaced.is_some());
        if owned >= MAX_TOOLS_PER_OWNER {
            return Err(format!(
                "an extension may register at most {MAX_TOOLS_PER_OWNER} tools"
            ));
        }
        if self.tools.len() - usize::from(replaced.is_some()) >= MAX_TOOLS_PER_HOST {
            return Err(format!(
                "the extension host holds at most {MAX_TOOLS_PER_HOST} tools"
            ));
        }
        if let Some(old) = replaced {
            self.tools.remove(&old);
        }
        self.next_handle += 1;
        let handle = self.next_handle;
        self.tools.insert(
            handle,
            ToolRegistration {
                handle,
                owner: params.owner.clone(),
                plugin_name,
                content_hash,
                name: name.to_string(),
                description: spec.description.clone(),
                input_schema: schema,
                input_validator,
            },
        );
        self.by_name.insert(key, handle);
        Ok(handle)
    }

    /// Undo exactly one registration. Idempotent; a stale or foreign handle is a no-op.
    pub fn unregister(&mut self, owner: &OwnerRef, handle: u64) {
        if self
            .commands
            .get(&handle)
            .is_some_and(|command| command.owner == *owner)
            && let Some(command) = self.commands.remove(&handle)
        {
            if self.commands_by_name.get(&command.name) == Some(&handle) {
                self.commands_by_name.remove(&command.name);
            }
            super::command::bump_epoch();
            return;
        }
        let owned = self
            .tools
            .get(&handle)
            .is_some_and(|tool| tool.owner == *owner);
        if !owned {
            return;
        }
        if let Some(tool) = self.tools.remove(&handle) {
            let key = tool.name.to_ascii_lowercase();
            if self.by_name.get(&key) == Some(&handle) {
                self.by_name.remove(&key);
            }
        }
    }

    fn remove_commands_of(&mut self, plugin_id: &str) {
        // Called by `remove_registrations_of`, so every revocation path that
        // drops an owner's tools drops its commands too.
        let handles: Vec<u64> = self
            .commands
            .values()
            .filter(|command| command.owner.plugin_id == plugin_id)
            .map(|command| command.handle)
            .collect();
        for handle in handles {
            if let Some(command) = self.commands.remove(&handle)
                && self.commands_by_name.get(&command.name) == Some(&handle)
            {
                self.commands_by_name.remove(&command.name);
            }
        }
        super::command::bump_epoch();
    }

    fn remove_registrations_of(&mut self, plugin_id: &str) -> Vec<u64> {
        self.remove_commands_of(plugin_id);
        let handles: Vec<u64> = self
            .tools
            .values()
            .filter(|tool| tool.owner.plugin_id == plugin_id)
            .map(|tool| tool.handle)
            .collect();
        for handle in &handles {
            if let Some(tool) = self.tools.remove(handle) {
                let key = tool.name.to_ascii_lowercase();
                if self.by_name.get(&key) == Some(handle) {
                    self.by_name.remove(&key);
                }
            }
        }
        handles
    }

    /// Revoke an owner synchronously. Returns the owner that was live, if any.
    pub fn revoke_owner(&mut self, plugin_id: &str) -> Option<OwnerRef> {
        self.remove_registrations_of(plugin_id);
        let entry = self.owners.get_mut(plugin_id)?;
        let was_live = matches!(entry.state, OwnerState::Activating | OwnerState::Active);
        entry.state = OwnerState::Revoked;
        was_live.then(|| entry.owner.clone())
    }

    /// Forget an owner entirely (after revocation, when its plugin is gone).
    pub fn forget_owner(&mut self, plugin_id: &str) {
        self.remove_registrations_of(plugin_id);
        self.owners.remove(plugin_id);
    }

    /// Forget owners that are not live (failed, faulted, revoked) so a new
    /// explicit plugin mutation retries them.
    pub fn forget_inactive(&mut self) {
        self.owners
            .retain(|_, entry| matches!(entry.state, OwnerState::Activating | OwnerState::Active));
    }

    /// A crash drops live registrations, preserves failed/faulted receipts,
    /// and blames the sole activating owner. Other owners are replayable only
    /// after reconciliation verifies their current persisted authority again.
    pub fn host_exited(&mut self, reason: &str) {
        self.tools.clear();
        self.by_name.clear();
        self.commands.clear();
        self.commands_by_name.clear();
        super::command::bump_epoch();
        let activating: Vec<_> = self
            .owners
            .values()
            .filter(|entry| entry.state == OwnerState::Activating)
            .map(|entry| entry.owner.plugin_id.clone())
            .collect();
        if let [plugin] = activating.as_slice() {
            self.owners.get_mut(plugin).expect("activating owner").state =
                OwnerState::Failed(format!("host crashed during activation: {reason}"));
        }
        self.owners.retain(|_, entry| {
            matches!(entry.state, OwnerState::Failed(_) | OwnerState::Faulted(_))
        });
    }

    /// Planned test shutdown drops all tools and fails the remaining live owners.
    #[cfg(test)]
    pub fn revoke_all(&mut self, reason: &str) {
        self.tools.clear();
        self.by_name.clear();
        self.commands.clear();
        self.commands_by_name.clear();
        super::command::bump_epoch();
        for entry in self.owners.values_mut() {
            if matches!(entry.state, OwnerState::Activating | OwnerState::Active) {
                entry.state = OwnerState::Failed(reason.to_string());
            }
        }
    }

    /// Tools of active owners, in handle order.
    #[must_use]
    pub fn live_tools(&self) -> Vec<ToolRegistration> {
        self.tools
            .values()
            .filter(|tool| {
                self.owners.get(&tool.owner.plugin_id).is_some_and(|entry| {
                    entry.owner == tool.owner && entry.state == OwnerState::Active
                })
            })
            .cloned()
            .collect()
    }

    /// Commands of active owners, in handle order.
    #[must_use]
    pub fn live_commands(&self) -> Vec<CommandRegistration> {
        self.commands
            .values()
            .filter(|command| {
                self.owners
                    .get(&command.owner.plugin_id)
                    .is_some_and(|entry| {
                        entry.owner == command.owner && entry.state == OwnerState::Active
                    })
            })
            .cloned()
            .collect()
    }

    /// The command behind `handle`, if it is still admitted for exactly this
    /// owner generation of `plugin_id`. A stale reference finds nothing.
    #[must_use]
    pub fn live_command(
        &self,
        handle: u64,
        plugin_id: &str,
        generation: u64,
    ) -> Option<CommandRegistration> {
        let command = self.commands.get(&handle)?;
        (command.owner.plugin_id == plugin_id
            && command.owner.generation == generation
            && self.owners.get(plugin_id).is_some_and(|entry| {
                entry.owner == command.owner && entry.state == OwnerState::Active
            }))
        .then(|| command.clone())
    }

    /// Whether `handle` is still admitted for exactly this owner generation.
    #[must_use]
    pub fn is_live(&self, handle: u64, owner: &OwnerRef) -> bool {
        self.tools
            .get(&handle)
            .is_some_and(|tool| tool.owner == *owner)
            && self
                .owners
                .get(&owner.plugin_id)
                .is_some_and(|entry| entry.owner == *owner && entry.state == OwnerState::Active)
    }

    /// Active owners other than `plugin_id` sharing the one host process.
    #[must_use]
    pub fn other_active_owners(&self, plugin_id: &str) -> usize {
        self.owners
            .values()
            .filter(|entry| entry.owner.plugin_id != plugin_id && entry.state == OwnerState::Active)
            .count()
    }

    #[must_use]
    pub fn authority_for(&self, owner: &OwnerRef) -> Option<PluginAuthority> {
        self.current(owner).map(|entry| entry.authority.clone())
    }
}
