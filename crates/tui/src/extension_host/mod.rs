//! Experimental TypeScript extension host — phase 1 (`[features] extension_host`).
//!
//! Codewhale's Rust core stays closed and authoritative: one turn loop, one
//! event authority, one store, one prompt authority, one approval gate. The
//! extension host (`crates/tui/extension-host`, a Bun or Node process running
//! the embedded bundle) is the only extensible surface, and in phase 1 it can do
//! exactly one thing: contribute tools, which become ordinary registry
//! `ToolSpec`s ([`tool::HostToolSpec`]) behind the existing gate.
//!
//! Lifecycle: a reviewed, enabled plugin with a `native` entry (activation
//! policy v4, selected by the flag) makes [`ExtensionHostManager::reconcile_in_background`]
//! spawn the host in the background — never on the first-prompt path — and
//! activate one owner per plugin. Tools join the per-turn registry at the next
//! rebuild, deferred. Disabling, revoking or updating the plugin revokes its
//! registrations synchronously before the host is asked to tear down.
//!
//! Engines: the manager and its one host are process-wide, but each engine
//! holds its own [`HostAttachment`] carrying the plugin snapshot of its
//! workspace. Reconcile activates the union of what every attached snapshot
//! desires and revokes only owners no attachment desires. Each snapshot is
//! re-verified against persisted plugin state on every reconcile, so a
//! disable or revoke made through any registry revokes the plugin for every
//! engine. An engine installs only the tools of owners its own snapshot
//! desires, so a workspace never sees another workspace's project plugins.
//! Dropping an attachment detaches without revoking; the next reconcile
//! revokes whatever no remaining attachment desires. Engines without a
//! plugin snapshot of their own (isolated chats, the empty fallback) never
//! attach.
//!
//! Known limitations (phase 1, by design — see the design doc §8):
//! * Tools only: no commands, hooks, skills, prompt sections, MCP, or
//!   `core/call` (the host cannot ask the core to do anything).
//! * Heartbeat and bounded automatic restart preserve the shared crash budget
//!   across engine creation and replay. Dead-host calls fail with a typed
//!   error and are never replayed. Three crashes in five minutes require an
//!   explicit plugin change/reload to retry. Two dirty teardowns within ten
//!   minutes retire the process once non-heartbeat calls are idle, without
//!   resetting or consuming the unexpected-crash budget.
//! * Every awaited core→host request is bounded by its method's deadline
//!   (`protocol::CoreRequest::deadline`) and then cancelled with `$/cancel`
//!   (`supervisor::HostProcess::call`). Cancellation is a request: a plugin
//!   that ignores its abort signal keeps running in the host until the host
//!   is torn down, though the core has already failed the call.
//! * One host per engine process and one trust tier. Under its OS sandbox
//!   (Seatbelt on macOS; bubblewrap on Linux when a launch-time probe shows it
//!   works) the host has no direct network, and cannot read the Codewhale
//!   home (except the bundle, its data dir and plugin code), the Codex and
//!   DSH credential homes, or the default credential stores
//!   (`supervisor::plan_launch`, whose module docs list what bubblewrap does
//!   not cover). Other files the user can read — including project `.env`
//!   files — stay readable, and Mach services are not restricted. On Windows,
//!   and on Linux where bwrap is missing or cannot start, it runs unsandboxed
//!   with the user's permissions, and `/plugin`, doctor and the start
//!   diagnostic say why. Either way the flag is Experimental.
//! * The runtime (`[extension_host] runtime`) defaults to Node. Bun is an
//!   opt-in (`bun`, or `auto`, which prefers a Bun >= 1.4.0 and uses Node
//!   when none is found) and is qualified on macOS only. The runtime is
//!   pinned once a host on it completes the handshake; restarts reuse it
//!   without probing it again, and the handshake refuses a host reporting a
//!   different runtime or version. Under `auto`, a Bun that fails to launch or
//!   handshake before anything is pinned is reported once and Node is used
//!   for the rest of the session; an explicit `bun`/`node` never falls back.
//!   The 1 GiB memory cap is applied through `RLIMIT_DATA` on Linux, the Job
//!   Object on Windows and, for a Bun host on macOS, a jetsam limit the host
//!   applies to itself; a macOS Node host is checked at each heartbeat
//!   instead. The Rust host tests have not run a Bun host on Linux or
//!   Windows (`supervisor` module docs say what was measured where).
//! * In-process native code is taken away from plugins by the host
//!   (`extension-host/src/runtime.ts`), for the entry points found so far; a
//!   native-code entry point a newer runtime adds is not covered until it is
//!   added there. A process a plugin starts is outside that policy and runs
//!   under the same OS sandbox (none on Windows, or where bwrap fails).
//! * The owner token is a bug/staleness guard, not a boundary between
//!   plugins that share the process: one plugin can alter another's
//!   behaviour, which the approval card discloses.
//! * Extension tool names that any name-keyed approval table special-cases
//!   are refused (`registry::core_special_case`), so an extension tool never
//!   shares an approval key, summary or category with a built-in.
//! * The host's process tree (Unix process group / Windows Job Object,
//!   shared with hooks via `crate::process_tree`) is killed as a whole. On
//!   Windows the host is assigned to its job just after spawn, not created
//!   suspended as hooks are. On Unix a plugin child that calls `setsid` leaves
//!   the group and is not killed with it. When the core goes away, the host
//!   kills its own group at stdin EOF, and a watchdog thread does the same
//!   when its parent process changes, even if a plugin blocks the event loop.
//! * A running engine's snapshot is replaced only by its own workspace
//!   switch or by [`plugins_changed`] for the same workspace. A plugin newly
//!   enabled through another workspace's registry reaches an engine at its
//!   next snapshot, not at once; disables and revokes always reach it at the
//!   next reconcile, through the persisted-state check.
//! * The host re-hashes each `native` entry file before importing it; other
//!   files in the staged snapshot are covered by Rust's per-call receipt
//!   check, not re-hashed by the host.

pub(crate) mod protocol;
pub(crate) mod registry;
pub(crate) mod supervisor;
pub(crate) mod tool;

#[cfg(test)]
pub(crate) mod tests;

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};

use self::protocol::{
    ActivateParams, ActivateResult, CoreRequest, DeactivateParams, DeactivateResult, EntryRef,
    OwnerRef, RegisterResult,
};
use self::registry::{OwnerRegistry, OwnerState, ToolRegistration};
use self::supervisor::{HostEvents, HostProcess};
use crate::plugins::PluginRegistry;
use crate::plugins::activation::{self, PluginActivationCapability};
use crate::plugins::types::PluginAuthority;

/// The host bundle, embedded so every distribution channel carries it.
const BUNDLE: &[u8] = include_bytes!("../../extension-host/dist/codewhale-extension-host.mjs");
const BUNDLE_FILE_NAME: &str = "codewhale-extension-host.mjs";
const MAX_DIAGNOSTICS: usize = 64;
const MAX_DIAGNOSTIC_BYTES: usize = 2048;
const DIRTY_RESTART_REASON: &str = "planned restart after repeated dirty teardowns";

struct Diagnostic {
    plugin_id: Option<String>,
    message: String,
}

fn bounded_diagnostic(mut message: String) -> String {
    if message.len() > MAX_DIAGNOSTIC_BYTES {
        let mut end = MAX_DIAGNOSTIC_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    message
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// SHA-256 of the embedded bundle.
#[must_use]
pub fn bundle_sha256() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST.get_or_init(|| hex(Sha256::digest(BUNDLE)))
}

/// Write the embedded bundle to `<root>/extension-host/<sha256>/` unless an
/// identical copy is already there, then re-verify the bytes on disk. The
/// file is never overwritten in place: a different build writes a different
/// directory. Blocking.
fn materialize_bundle(root: &Path) -> Result<PathBuf, String> {
    let digest = bundle_sha256();
    let dir = supervisor::bundle_dir(root, digest);
    let path = dir.join(BUNDLE_FILE_NAME);
    let matches = |path: &Path| {
        std::fs::read(path)
            .map(|bytes| hex(Sha256::digest(&bytes)) == digest)
            .unwrap_or(false)
    };
    if !matches(&path) {
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let staging = dir.join(format!(
            ".{BUNDLE_FILE_NAME}.{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&staging, BUNDLE)
            .map_err(|error| format!("cannot write {}: {error}", staging.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o400));
        }
        if let Err(error) = std::fs::rename(&staging, &path) {
            let _ = std::fs::remove_file(&staging);
            if !matches(&path) {
                return Err(format!("cannot publish {}: {error}", path.display()));
            }
        }
    }
    // Re-check what will actually be executed.
    if !matches(&path) {
        return Err(format!(
            "{} does not match the embedded bundle digest {digest}",
            path.display()
        ));
    }
    Ok(path)
}

/// Options fixed for the life of one manager.
#[derive(Debug, Clone, Default)]
pub struct ExtensionHostOptions {
    /// `[extension_host] runtime`: `node` (the default), `bun`, or `auto`
    /// (Bun first).
    pub runtime: crate::config::ExtensionHostRuntime,
    /// `[extension_host] node`: tried before every `node` on `PATH`.
    pub node_override: Option<PathBuf>,
    /// `[extension_host] bun`: tried before every `bun` on `PATH`.
    pub bun_override: Option<PathBuf>,
    /// Where the bundle is materialized; defaults to the Codewhale home.
    pub root: Option<PathBuf>,
    /// Per-manager timings; tests can shorten them without global state.
    pub supervision: SupervisionOptions,
}

#[derive(Debug, Clone)]
pub struct SupervisionOptions {
    pub heartbeat_interval: Duration,
    pub ping_timeout: Duration,
    pub hang_timeout: Duration,
    pub restart_backoff: Duration,
    pub crash_window: Duration,
    pub crash_limit: usize,
    pub start_retry_cooldown: Duration,
    pub dirty_window: Duration,
    pub dirty_limit: usize,
    /// Host memory cap in bytes (`supervisor::HOST_MEMORY_CAP`).
    pub memory_cap: u64,
    /// How long one extension tool call may take before the core cancels it
    /// (`tool::TOOL_CALL_DEADLINE`); sent to the host as `deadline_ms`. The
    /// other methods' deadlines are fixed (`CoreRequest::deadline`).
    pub tool_call_deadline: Duration,
}

impl ExtensionHostOptions {
    /// Options for the `[extension_host]` table (paths `~`-expanded).
    #[must_use]
    pub fn from_config(table: Option<&crate::config::ExtensionHostConfig>) -> Self {
        let expand = |path: Option<&String>| {
            path.map(|path| PathBuf::from(shellexpand::tilde(path).as_ref()))
        };
        Self {
            runtime: table.map_or_else(Default::default, |table| table.effective_runtime()),
            node_override: expand(table.and_then(|table| table.node.as_ref())),
            bun_override: expand(table.and_then(|table| table.bun.as_ref())),
            ..Default::default()
        }
    }
}

impl Default for SupervisionOptions {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(3),
            ping_timeout: Duration::from_secs(3),
            hang_timeout: supervisor::PING_DEADLINE,
            restart_backoff: Duration::from_millis(250),
            crash_window: Duration::from_secs(5 * 60),
            crash_limit: 3,
            start_retry_cooldown: Duration::from_secs(60),
            dirty_window: Duration::from_secs(10 * 60),
            dirty_limit: 2,
            memory_cap: supervisor::HOST_MEMORY_CAP,
            tool_call_deadline: tool::TOOL_CALL_DEADLINE,
        }
    }
}

#[derive(Default)]
struct SupervisionState {
    crashes: VecDeque<Instant>,
    last_start: Option<Instant>,
    launch_failed: bool,
    retry_ticket: u64,
    policy: bool,
    dirty_teardowns: VecDeque<Instant>,
    dirty_restart_pending: bool,
    planned_restart: Option<u64>,
}

impl SupervisionState {
    fn record_dirty_teardown(&mut self, now: Instant, options: &SupervisionOptions) {
        while self
            .dirty_teardowns
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= options.dirty_window)
        {
            self.dirty_teardowns.pop_front();
        }
        self.dirty_teardowns.push_back(now);
        let limit = options.dirty_limit.max(1);
        self.dirty_restart_pending |= self.dirty_teardowns.len() >= limit;
        while self.dirty_teardowns.len() > limit {
            self.dirty_teardowns.pop_front();
        }
    }

    fn record_crash(&mut self, now: Instant, options: &SupervisionOptions) -> bool {
        while self
            .crashes
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= options.crash_window)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        self.launch_failed = false;
        self.retry_ticket += 1;
        self.crashes.len() < options.crash_limit
    }
}

/// Observable host state: the one source for `/plugin` and for the error a
/// tool call routed to the host gets while it is down ([`fmt::Display`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    /// `[features] extension_host` is off for this process.
    Disabled,
    /// Not started: nothing has needed it yet.
    Idle,
    Starting,
    Ready {
        pid: Option<u32>,
        /// `bun` or `node`.
        runtime: &'static str,
        /// As reported by the host (`host/hello`).
        runtime_version: String,
        sandbox: supervisor::HostSandbox,
        /// How the memory cap is enforced for this process.
        memory: supervisor::MemoryEnforcement,
    },
    Unresponsive {
        pid: Option<u32>,
    },
    /// Crashed (or retired for maintenance); the supervisor restarts it.
    Restarting {
        reason: String,
    },
    /// Start refused (`start failed: …`) or crash budget exhausted; only an
    /// explicit plugin change or reload retries.
    Failed {
        reason: String,
        stderr_tail: String,
    },
}

/// Why the host can or cannot take a call, in one phrase.
impl fmt::Display for HostStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled by config ([features] extension_host is off)"),
            Self::Idle => f.write_str(
                "not started (it starts in the background when a reviewed plugin with host code is enabled)",
            ),
            Self::Starting => f.write_str("starting"),
            Self::Ready { .. } => f.write_str("running"),
            Self::Unresponsive { pid } => write!(
                f,
                "unresponsive (pid {} missed its heartbeat; the supervisor restarts it if it stays silent)",
                pid.map_or_else(|| "?".to_string(), |pid| pid.to_string())
            ),
            Self::Restarting { reason } => write!(f, "restarting after: {reason}"),
            Self::Failed { reason, .. } => {
                write!(f, "{reason} (change or reload a plugin to retry)")
            }
        }
    }
}

enum HostSlot {
    Idle,
    Starting,
    Ready(Arc<HostProcess>),
    Unresponsive(Arc<HostProcess>),
    Restarting { reason: String },
    Failed { reason: String, stderr_tail: String },
}

impl HostSlot {
    fn status(&self) -> HostStatus {
        match self {
            Self::Idle => HostStatus::Idle,
            Self::Starting => HostStatus::Starting,
            Self::Ready(host) if host.is_retiring() => HostStatus::Restarting {
                reason: DIRTY_RESTART_REASON.to_string(),
            },
            // The exit watcher reports the exit a moment later.
            Self::Ready(host) | Self::Unresponsive(host) if host.has_exited() => {
                HostStatus::Restarting {
                    reason: "the host process exited".to_string(),
                }
            }
            Self::Ready(host) => HostStatus::Ready {
                pid: host.pid,
                runtime: host.runtime.kind.name(),
                runtime_version: host.runtime_version.get().cloned().unwrap_or_default(),
                sandbox: host.sandbox.clone(),
                memory: host.memory(),
            },
            Self::Unresponsive(host) => HostStatus::Unresponsive { pid: host.pid },
            Self::Restarting { reason } => HostStatus::Restarting {
                reason: reason.clone(),
            },
            Self::Failed {
                reason,
                stderr_tail,
            } => HostStatus::Failed {
                reason: reason.clone(),
                stderr_tail: stderr_tail.clone(),
            },
        }
    }
}

struct DesiredOwner {
    plugin_name: String,
    authority: PluginAuthority,
    entries: Vec<(PathBuf, String)>,
}

/// One engine's view: its plugin snapshot and the owners (plugin id →
/// reviewed content hash) that snapshot desired at the last reconcile.
struct AttachmentState {
    plugins: Arc<PluginRegistry>,
    desired: BTreeMap<String, String>,
}

pub(crate) struct ManagerShared {
    options: ExtensionHostOptions,
    attachments: Mutex<BTreeMap<u64, AttachmentState>>,
    next_attachment: AtomicU64,
    registry: Mutex<OwnerRegistry>,
    host: Mutex<HostSlot>,
    host_generation: AtomicU64,
    spawn_attempts: AtomicU64,
    sync_lock: tokio::sync::Mutex<()>,
    diagnostics: Mutex<VecDeque<Diagnostic>>,
    /// Lock order: host, supervision, registry. Never held across await.
    supervision: Mutex<SupervisionState>,
    /// Never held with another lock.
    runtime: Mutex<RuntimePin>,
}

/// Which runtime this manager's hosts run on.
#[derive(Default)]
struct RuntimePin {
    /// Set by the first host that completes the handshake and reused by
    /// every restart, so a session never switches runtime (a `bun` install
    /// or removal mid-session changes nothing until the next process; a
    /// binary replaced at the pinned path is refused at the handshake). With
    /// the one-line summary of why it was chosen, for `/plugin`.
    pinned: Option<(crate::dependencies::HostRuntime, String)>,
    /// `runtime = "auto"` only: a Bun host failed to launch or handshake
    /// before anything was pinned, so every later launch this session
    /// resolves Node.
    bun_failed: bool,
}

/// Resolve the runtime unless one is pinned, materialize the bundle and plan
/// the launch. Returns the summary to pin once a host on this runtime
/// completes the handshake, or `None` when the runtime was already pinned.
/// Blocking.
fn prepare_launch(
    options: &ExtensionHostOptions,
    pinned: Option<(crate::dependencies::HostRuntime, String)>,
    bun_failed: bool,
) -> Result<(supervisor::HostLaunch, Option<String>), String> {
    let (runtime, summary) = match pinned {
        Some((runtime, _)) => (runtime, None),
        None => {
            let choice = if bun_failed {
                crate::config::ExtensionHostRuntime::Node
            } else {
                options.runtime
            };
            let mut resolution = crate::dependencies::resolve_extension_host_runtime(
                choice,
                options.node_override.as_deref(),
                options.bun_override.as_deref(),
            );
            // Report the configured choice, not the Node-only fallback.
            resolution.choice = options.runtime;
            let note = if bun_failed {
                "; Bun failed to start this session (see diagnostics)"
            } else {
                ""
            };
            let Some(runtime) = resolution.selected.clone() else {
                return Err(format!("{}{note}", resolution.failure()));
            };
            (runtime, Some(format!("{}{note}", resolution.summary())))
        }
    };
    let root = host_root(options)?;
    let bundle = materialize_bundle(&root)?;
    let launch = supervisor::plan_launch(&runtime, &bundle, &root, options.supervision.memory_cap)?;
    Ok((launch, summary))
}

/// The Codewhale home the host lives under.
fn host_root(options: &ExtensionHostOptions) -> Result<PathBuf, String> {
    match options.root.clone() {
        Some(root) => Ok(root),
        None => codewhale_config::codewhale_home()
            .map_err(|error| format!("Codewhale home unavailable: {error}")),
    }
}

/// The OS sandbox a host started now on `runtime` would run under, planned —
/// and on Linux probed — exactly as a launch does (for doctor). Blocking;
/// creates the host's data dir, as a launch would.
pub(crate) fn planned_sandbox(
    options: &ExtensionHostOptions,
    runtime: &crate::dependencies::HostRuntime,
) -> Result<supervisor::HostSandbox, String> {
    supervisor::planned_sandbox(runtime, &host_root(options)?)
}

impl ManagerShared {
    async fn deactivate_owner(&self, host: &Arc<HostProcess>, owner: &OwnerRef) {
        let diagnostic = match host
            .call(
                CoreRequest::Deactivate(DeactivateParams {
                    owner: owner.clone(),
                }),
                None,
            )
            .await
            .map(serde_json::from_value::<DeactivateResult>)
        {
            Ok(Ok(ack)) if ack.disposed && ack.leaked.is_empty() => return,
            Ok(Ok(ack)) => format!(
                "extension `{}` teardown incomplete (disposed: {}, leaked: {:?})",
                owner.plugin_id, ack.disposed, ack.leaked
            ),
            Ok(Err(error)) => format!(
                "extension `{}` teardown answer malformed: {error}",
                owner.plugin_id
            ),
            Err(error) => format!("extension `{}` teardown failed: {error}", owner.plugin_id),
        };
        self.plugin_diagnostic(&owner.plugin_id, diagnostic);
        self.record_dirty_teardown(host);
    }

    fn record_dirty_teardown(&self, host: &Arc<HostProcess>) {
        let slot = self.host.lock().expect("host lock");
        if !matches!(&*slot, HostSlot::Ready(current) | HostSlot::Unresponsive(current)
            if Arc::ptr_eq(current, host))
        {
            return;
        }
        self.supervision
            .lock()
            .expect("supervision lock")
            .record_dirty_teardown(Instant::now(), &self.options.supervision);
    }

    fn diagnostic(&self, message: String) {
        self.record_diagnostic(None, message);
    }

    fn plugin_diagnostic(&self, plugin_id: &str, message: String) {
        // Refused registrations can contain an arbitrary host-supplied id.
        // Keep an oversized id global instead of retaining unbounded metadata
        // or truncating it into another plugin's identity.
        let plugin_id = (plugin_id.len() <= MAX_DIAGNOSTIC_BYTES).then(|| plugin_id.to_string());
        self.record_diagnostic(plugin_id, message);
    }

    fn record_diagnostic(&self, plugin_id: Option<String>, message: String) {
        let message = bounded_diagnostic(message);
        tracing::info!(target: "extension_host", "{message}");
        let mut diagnostics = self.diagnostics.lock().expect("diagnostics lock");
        diagnostics.push_back(Diagnostic { plugin_id, message });
        while diagnostics.len() > MAX_DIAGNOSTICS {
            diagnostics.pop_front();
        }
    }

    /// The host, when it can take a call; otherwise why not.
    fn ready_host(&self) -> Result<Arc<HostProcess>, HostStatus> {
        match &*self.host.lock().expect("host lock") {
            HostSlot::Ready(host) if !host.has_exited() && !host.is_retiring() => {
                Ok(Arc::clone(host))
            }
            slot => Err(slot.status()),
        }
    }

    /// Re-check everything a call depends on, immediately before it is sent:
    /// a running host (first, so a call to a host that is down says why),
    /// exact owner generation, the reviewed receipt and staged bytes, the
    /// Native adapter in this build's policy, and the host again.
    pub(crate) async fn live_host_for(
        &self,
        registration: &ToolRegistration,
    ) -> Result<Arc<HostProcess>, String> {
        let policy = activation::extension_host_policy_enabled();
        if !policy {
            return Err(host_down(&HostStatus::Disabled));
        }
        self.ready_host().map_err(|status| host_down(&status))?;
        let authority = {
            let registry = self.registry.lock().expect("registry lock");
            if !registry.is_live(registration.handle, &registration.owner) {
                return Err(format!(
                    "extension tool `{}` from `{}` is no longer registered",
                    registration.name, registration.plugin_name
                ));
            }
            registry
                .authority_for(&registration.owner)
                .ok_or_else(|| "extension owner has no authority".to_string())?
        };
        tokio::task::spawn_blocking(move || {
            let _scope = activation::PolicyScope::propagate(policy);
            crate::plugins::registry::verify_plugin_component_authority(
                &authority,
                PluginActivationCapability::Native,
            )
        })
        .await
        .map_err(|error| format!("authority check failed: {error}"))??;
        self.ready_host().map_err(|status| host_down(&status))
    }
}

/// The error a host tool call gets while the host cannot take it.
fn host_down(status: &HostStatus) -> String {
    format!("extension host is down: {status}")
}

/// Channel callbacks. Holds a `Weak` so the host process (which owns the
/// callbacks) never keeps the manager alive.
struct Events {
    shared: Weak<ManagerShared>,
    generation: u64,
}

impl HostEvents for Events {
    fn register(&self, params: &protocol::RegisterParams) -> RegisterResult {
        let Some(shared) = self.shared.upgrade() else {
            return RegisterResult::Refused {
                refused: "extension host manager is gone".to_string(),
            };
        };
        let slot = shared.host.lock().expect("host lock");
        if shared.host_generation.load(Ordering::SeqCst) != self.generation {
            return RegisterResult::Refused {
                refused: "stale host generation".to_string(),
            };
        }
        let result = shared
            .registry
            .lock()
            .expect("registry lock")
            .register_tool(params);
        drop(slot);
        match result {
            Ok(handle) => RegisterResult::Admitted { handle },
            Err(reason) => {
                shared.plugin_diagnostic(
                    &params.owner.plugin_id,
                    format!(
                        "extension `{}` tool `{}` refused: {reason}",
                        params.owner.plugin_id, params.spec.name
                    ),
                );
                RegisterResult::Refused { refused: reason }
            }
        }
    }

    fn unregister(&self, params: &protocol::UnregisterParams) {
        if let Some(shared) = self.shared.upgrade() {
            shared
                .registry
                .lock()
                .expect("registry lock")
                .unregister(&params.owner, params.handle);
        }
    }

    fn faulted(&self, params: &protocol::FaultedParams) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let slot = shared.host.lock().expect("host lock");
        if shared.host_generation.load(Ordering::SeqCst) != self.generation {
            return;
        }
        if !shared
            .registry
            .lock()
            .expect("registry lock")
            .mark_failed(&params.owner, OwnerState::Faulted(params.error.clone()))
        {
            return;
        }
        if let HostSlot::Ready(host) | HostSlot::Unresponsive(host) = &*slot {
            host.revoke_calls_of(&params.owner.plugin_id);
        }
        drop(slot);
        shared.plugin_diagnostic(
            &params.owner.plugin_id,
            format!(
                "extension `{}` faulted and was disposed: {}",
                params.owner.plugin_id, params.error
            ),
        );
    }

    fn exited(&self, host_generation: u64, reason: String, stderr_tail: String) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let retry = {
            let mut slot = shared.host.lock().expect("host lock");
            if shared.host_generation.load(Ordering::SeqCst) != host_generation
                || !matches!(&*slot, HostSlot::Ready(_) | HostSlot::Unresponsive(_))
            {
                return;
            }
            let mut supervision = shared.supervision.lock().expect("supervision lock");
            let planned = supervision.planned_restart.take() == Some(host_generation)
                && reason == DIRTY_RESTART_REASON;
            let restart = if planned {
                supervision.retry_ticket += 1;
                true
            } else {
                supervision.record_crash(Instant::now(), &shared.options.supervision)
            };
            shared
                .registry
                .lock()
                .expect("registry lock")
                .host_exited(&reason);
            *slot = if restart {
                HostSlot::Restarting {
                    reason: reason.clone(),
                }
            } else {
                HostSlot::Failed {
                    reason: format!("crash budget exhausted: {reason}"),
                    stderr_tail,
                }
            };
            restart.then_some((supervision.retry_ticket, supervision.policy))
        };
        shared.diagnostic(format!("extension host {reason}"));
        if let Some((ticket, policy)) = retry {
            schedule_restart(&shared, host_generation, ticket, policy);
        }
    }

    fn log(&self, params: &protocol::LogParams) {
        if !matches!(params.level.as_str(), "warn" | "error") {
            return;
        }
        let Some(plugin_id) = params.plugin_id.as_deref() else {
            return;
        };
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let _slot = shared.host.lock().expect("host lock");
        if shared.host_generation.load(Ordering::SeqCst) != self.generation
            || shared
                .registry
                .lock()
                .expect("registry lock")
                .owner(plugin_id)
                .is_none()
        {
            return;
        }
        shared.plugin_diagnostic(plugin_id, format!("{}: {}", params.level, params.msg));
    }
}

/// One scheduled retry owns a ticket, so explicit retry/shutdown and a newer
/// host generation invalidate it. The existing reconcile lock owns replay.
fn schedule_restart(shared: &Arc<ManagerShared>, generation: u64, ticket: u64, policy: bool) {
    let weak = Arc::downgrade(shared);
    let backoff = shared.options.supervision.restart_backoff;
    tokio::spawn(async move {
        tokio::time::sleep(backoff).await;
        let Some(shared) = weak.upgrade() else {
            return;
        };
        {
            let _serial = shared.sync_lock.lock().await;
            let mut slot = shared.host.lock().expect("host lock");
            if shared.host_generation.load(Ordering::SeqCst) != generation
                || shared
                    .supervision
                    .lock()
                    .expect("supervision lock")
                    .retry_ticket
                    != ticket
                || !matches!(&*slot, HostSlot::Restarting { .. })
            {
                return;
            }
            *slot = HostSlot::Idle;
        }
        let manager = ExtensionHostManager { shared };
        if let Err(error) = manager.reconcile_with_policy(policy).await {
            manager.shared.diagnostic(error);
        }
    });
}

fn set_host_health(shared: &ManagerShared, generation: u64, unresponsive: bool) -> bool {
    let mut slot = shared.host.lock().expect("host lock");
    if shared.host_generation.load(Ordering::SeqCst) != generation {
        return false;
    }
    let host = match &*slot {
        HostSlot::Ready(host) | HostSlot::Unresponsive(host) => Arc::clone(host),
        _ => return false,
    };
    *slot = if unresponsive {
        HostSlot::Unresponsive(host)
    } else {
        HostSlot::Ready(host)
    };
    true
}

fn restart_dirty_host_when_idle(
    shared: &ManagerShared,
    host: &Arc<HostProcess>,
    generation: u64,
) -> bool {
    // Reconciliation owns activation/deactivation between wire requests too.
    let Ok(_serial) = shared.sync_lock.try_lock() else {
        return false;
    };
    let slot = shared.host.lock().expect("host lock");
    if shared.host_generation.load(Ordering::SeqCst) != generation
        || !matches!(&*slot, HostSlot::Ready(current) if Arc::ptr_eq(current, host))
    {
        return false;
    }
    let mut supervision = shared.supervision.lock().expect("supervision lock");
    if !supervision.dirty_restart_pending || !host.terminate_if_idle(DIRTY_RESTART_REASON) {
        return false;
    }
    supervision.planned_restart = Some(generation);
    true
}

/// A monitor never owns the manager. Dropping the manager or changing host
/// generation stops its monitor; pending calls are never retried here.
fn monitor_host(shared: &Arc<ManagerShared>, host: &Arc<HostProcess>, generation: u64) {
    let weak = Arc::downgrade(shared);
    let host = Arc::clone(host);
    let options = shared.options.supervision.clone();
    tokio::spawn(async move {
        let mut blocked_since = None;
        loop {
            tokio::time::sleep(options.heartbeat_interval).await;
            let Some(shared) = weak.upgrade() else {
                return;
            };
            if shared.host_generation.load(Ordering::SeqCst) != generation || host.has_exited() {
                return;
            }
            if restart_dirty_host_when_idle(&shared, &host, generation) {
                return;
            }
            // Only where no kernel limit is planned (a macOS Node host).
            if host.memory() == supervisor::MemoryEnforcement::Heartbeat
                && let Some(resident) = host.pid.and_then(supervisor::resident_bytes)
                && resident > host.memory_cap
            {
                shared.diagnostic(format!(
                    "extension host exceeded its memory cap ({} MiB resident, cap {} MiB); killed",
                    resident / (1024 * 1024),
                    host.memory_cap / (1024 * 1024)
                ));
                drop(shared);
                host.terminate("exceeded the memory cap".into());
                return;
            }
            drop(shared);
            let (id, mut answer) = match host.start_request(CoreRequest::Ping, None) {
                Ok(request) => {
                    blocked_since = None;
                    request
                }
                Err(supervisor::HostCallError::Busy) => {
                    // A full outbound queue can itself be caused by a hung
                    // host. Bound that wait too instead of skipping forever.
                    let elapsed = blocked_since.get_or_insert_with(Instant::now).elapsed();
                    if elapsed >= options.hang_timeout {
                        host.terminate("heartbeat queue remained blocked".into());
                        return;
                    }
                    if elapsed >= options.ping_timeout {
                        let Some(shared) = weak.upgrade() else {
                            return;
                        };
                        if !set_host_health(&shared, generation, true) {
                            return;
                        }
                    }
                    continue;
                }
                Err(_) => return,
            };
            let result = match tokio::time::timeout(options.ping_timeout, &mut answer).await {
                Ok(result) => result,
                Err(_) => {
                    let Some(shared) = weak.upgrade() else {
                        host.forget(id);
                        return;
                    };
                    if !set_host_health(&shared, generation, true) {
                        host.forget(id);
                        return;
                    }
                    drop(shared);
                    let remaining = options.hang_timeout.saturating_sub(options.ping_timeout);
                    match tokio::time::timeout(remaining, answer).await {
                        Ok(result) => result,
                        Err(_) => {
                            host.forget(id);
                            host.terminate("heartbeat timed out".into());
                            return;
                        }
                    }
                }
            };
            host.forget(id);
            if !matches!(result, Ok(Ok(serde_json::Value::Object(ref object))) if object.is_empty())
            {
                if !host.has_exited() {
                    host.terminate("invalid heartbeat response".into());
                }
                return;
            }
            let Some(shared) = weak.upgrade() else {
                return;
            };
            if !set_host_health(&shared, generation, false) {
                return;
            }
        }
    });
}

/// Supervises at most one extension host for this engine process.
pub struct ExtensionHostManager {
    shared: Arc<ManagerShared>,
}

impl ExtensionHostManager {
    #[must_use]
    pub fn new(options: ExtensionHostOptions) -> Self {
        Self {
            shared: Arc::new(ManagerShared {
                options,
                attachments: Mutex::new(BTreeMap::new()),
                next_attachment: AtomicU64::new(0),
                registry: Mutex::new(OwnerRegistry::new()),
                host: Mutex::new(HostSlot::Idle),
                host_generation: AtomicU64::new(0),
                spawn_attempts: AtomicU64::new(0),
                sync_lock: tokio::sync::Mutex::new(()),
                diagnostics: Mutex::new(VecDeque::new()),
                supervision: Mutex::new(SupervisionState::default()),
                runtime: Mutex::new(RuntimePin::default()),
            }),
        }
    }

    #[must_use]
    pub fn status(&self) -> HostStatus {
        self.shared.host.lock().expect("host lock").status()
    }

    /// The pinned runtime's one-line summary, once a host on it has completed
    /// the handshake.
    #[must_use]
    pub fn runtime_summary(&self) -> Option<String> {
        self.shared
            .runtime
            .lock()
            .expect("runtime lock")
            .pinned
            .as_ref()
            .map(|(_, summary)| summary.clone())
    }

    /// How many times this manager has tried to start a host process.
    #[must_use]
    pub fn spawn_attempts(&self) -> u64 {
        self.shared.spawn_attempts.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<String> {
        self.shared
            .diagnostics
            .lock()
            .expect("diagnostics lock")
            .iter()
            .map(|entry| entry.message.clone())
            .collect()
    }

    /// Recent retained diagnostics for exactly this plugin, not a persistent
    /// log. The command renderer escapes each field before displaying it.
    #[must_use]
    pub fn owner_report(&self, plugin_id: &str) -> Option<OwnerReport> {
        let registry = self.shared.registry.lock().expect("registry lock");
        let state = registry.owner(plugin_id).map(|entry| match &entry.state {
            OwnerState::Failed(reason) => OwnerState::Failed(bounded_diagnostic(reason.clone())),
            OwnerState::Faulted(reason) => OwnerState::Faulted(bounded_diagnostic(reason.clone())),
            state => state.clone(),
        });
        let tools = registry
            .live_tools()
            .into_iter()
            .filter(|tool| tool.owner.plugin_id == plugin_id)
            .map(|tool| tool.name)
            .collect();
        drop(registry);
        let mut diagnostics: Vec<_> = self
            .shared
            .diagnostics
            .lock()
            .expect("diagnostics lock")
            .iter()
            .rev()
            .filter(|entry| entry.plugin_id.as_deref() == Some(plugin_id))
            .take(20)
            .map(|entry| entry.message.clone())
            .collect();
        diagnostics.reverse();
        if state.is_none() && diagnostics.is_empty() {
            return None;
        }
        Some(OwnerReport {
            state,
            tools,
            diagnostics,
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn live_tool_names(&self) -> Vec<String> {
        self.shared
            .registry
            .lock()
            .expect("registry lock")
            .live_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[cfg(test)]
    #[must_use]
    pub fn owner_state(&self, plugin_id: &str) -> Option<OwnerState> {
        self.shared
            .registry
            .lock()
            .expect("registry lock")
            .owner(plugin_id)
            .map(|entry| entry.state.clone())
    }

    /// An explicit plugin mutation retries failed receipts and clears the
    /// shared crash budget. Merely opening another engine never does this.
    pub fn retry(&self) {
        let mut slot = self.shared.host.lock().expect("host lock");
        let mut supervision = self.shared.supervision.lock().expect("supervision lock");
        supervision.crashes.clear();
        supervision.launch_failed = false;
        supervision.retry_ticket += 1;
        if matches!(
            &*slot,
            HostSlot::Failed { .. } | HostSlot::Restarting { .. }
        ) {
            *slot = HostSlot::Idle;
        }
        self.shared
            .registry
            .lock()
            .expect("registry lock")
            .forget_inactive();
    }

    fn retry_launch_on_attach(&self) {
        let mut slot = self.shared.host.lock().expect("host lock");
        let mut supervision = self.shared.supervision.lock().expect("supervision lock");
        if matches!(&*slot, HostSlot::Failed { .. })
            && supervision.launch_failed
            && supervision.last_start.is_some_and(|at| {
                at.elapsed() >= self.shared.options.supervision.start_retry_cooldown
            })
        {
            *slot = HostSlot::Idle;
            supervision.retry_ticket += 1;
        }
    }

    /// Record native tool names from one engine's turn build (the registry
    /// before scripts, plugins and extensions are added) so
    /// `registry/register` refuses collisions. Additive: engines in one
    /// process report different native surfaces and none may shrink the set.
    pub fn note_native_names<'a>(&self, names: impl IntoIterator<Item = &'a str>) {
        self.shared
            .registry
            .lock()
            .expect("registry lock")
            .add_native_names(names);
    }

    /// Attach an engine whose workspace plugin snapshot is `plugins`. Nothing
    /// is reconciled until [`HostAttachment::sync`] or a background sync.
    #[must_use]
    pub fn attach(self: &Arc<Self>, plugins: Arc<PluginRegistry>) -> HostAttachment {
        self.retry_launch_on_attach();
        let id = self.shared.next_attachment.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared
            .attachments
            .lock()
            .expect("attachments lock")
            .insert(
                id,
                AttachmentState {
                    plugins,
                    desired: BTreeMap::new(),
                },
            );
        HostAttachment {
            id,
            manager: Arc::clone(self),
        }
    }

    /// How many engines are attached.
    #[must_use]
    pub fn attached_engines(&self) -> usize {
        self.shared
            .attachments
            .lock()
            .expect("attachments lock")
            .len()
    }

    /// Replace the snapshot of every engine attached to `plugins`'s
    /// workspace: a plugin was enabled, disabled, trusted or revoked there.
    fn refresh_workspace(&self, plugins: &Arc<PluginRegistry>) {
        for state in self
            .shared
            .attachments
            .lock()
            .expect("attachments lock")
            .values_mut()
        {
            if state.plugins.workspace() == plugins.workspace() {
                state.plugins = Arc::clone(plugins);
                state.desired.clear();
            }
        }
    }

    /// Add the live tools of the owners attachment `id` desires to
    /// `tool_registry`, *after* natives and `~/.codewhale/tools` scripts. A
    /// name already present is skipped with a diagnostic —
    /// `ToolRegistry::register` would silently overwrite it. Returns the
    /// names added.
    fn install_tools_for(
        &self,
        id: u64,
        tool_registry: &mut crate::tools::ToolRegistry,
    ) -> Vec<String> {
        let desired = self
            .shared
            .attachments
            .lock()
            .expect("attachments lock")
            .get(&id)
            .map(|state| state.desired.clone())
            .unwrap_or_default();
        if desired.is_empty() {
            return Vec::new();
        }
        let tools: Vec<ToolRegistration> = self
            .shared
            .registry
            .lock()
            .expect("registry lock")
            .live_tools()
            .into_iter()
            .filter(|tool| desired.get(&tool.owner.plugin_id) == Some(&tool.content_hash))
            .collect();
        if tools.is_empty() {
            return Vec::new();
        }
        let taken: HashSet<String> = tool_registry
            .names()
            .into_iter()
            .map(str::to_ascii_lowercase)
            .collect();
        let mut installed = Vec::new();
        for registration in tools {
            if taken.contains(&registration.name.to_ascii_lowercase()) {
                let origin = tool_registry
                    .get(&registration.name)
                    .map(|existing| existing.registration_origin().into_owned())
                    .unwrap_or_else(|| "another tool".to_string());
                self.shared.plugin_diagnostic(&registration.owner.plugin_id, format!(
                    "extension tool `{}` from `{}` skipped: the name is already registered by {origin}",
                    registration.name, registration.plugin_name
                ));
                continue;
            }
            installed.push(registration.name.clone());
            tool_registry.register(Arc::new(tool::HostToolSpec::new(
                registration,
                Arc::clone(&self.shared),
            )));
        }
        installed
    }

    /// Reconcile without waiting (turn builds, session start,
    /// plugin changes).
    pub fn reconcile_in_background(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let manager = Arc::clone(self);
        let policy = activation::extension_host_policy_enabled();
        tokio::spawn(async move {
            if let Err(error) = manager.reconcile_with_policy(policy).await {
                manager.shared.diagnostic(error);
            }
        });
    }

    /// Reconcile host owners with the reviewed, enabled plugins that declare
    /// `native` entries in any attached engine's snapshot: revoke what no
    /// attachment desires any more or what changed (synchronously, then ask
    /// the host to tear down), spawn the host if needed, activate the rest.
    #[cfg(test)]
    pub async fn reconcile(&self) -> Result<(), String> {
        self.reconcile_with_policy(activation::extension_host_policy_enabled())
            .await
    }

    async fn reconcile_with_policy(&self, policy: bool) -> Result<(), String> {
        let shared = &self.shared;
        let _serial = shared.sync_lock.lock().await;
        let (desired, errors) = loop {
            let snapshots: Vec<(u64, Arc<PluginRegistry>)> = shared
                .attachments
                .lock()
                .expect("attachments lock")
                .iter()
                .map(|(id, state)| (*id, Arc::clone(&state.plugins)))
                .collect();
            let scan = tokio::task::spawn_blocking(move || {
                let _scope = activation::PolicyScope::propagate(policy);
                union_of_desired_owners(snapshots)
            })
            .await
            .map_err(|error| format!("plugin scan failed: {error}"))?;
            let published = scan.publish(&mut shared.attachments.lock().expect("attachments lock"));
            if let Some(published) = published {
                break published;
            }
            // Attach, detach, or workspace refresh raced the blocking scan.
            // Rescan before changing either engine views or global owners.
        };
        for error in errors {
            shared.diagnostic(error);
        }

        // 1. Revoke first — never waits for the host.
        let mut revoked: Vec<OwnerRef> = Vec::new();
        let mut to_activate: Vec<(String, DesiredOwner)> = Vec::new();
        {
            let mut registry = shared.registry.lock().expect("registry lock");
            let existing: Vec<(String, PluginAuthority)> = registry
                .owners()
                .map(|entry| (entry.owner.plugin_id.clone(), entry.authority.clone()))
                .collect();
            for (plugin_id, authority) in &existing {
                // An explicit trust/enable transition revokes the persisted
                // authority even when the bytes stay identical. Refresh that
                // owner instead of retaining a tool that can only fail closed.
                let keep = desired.get(plugin_id).is_some_and(|want| {
                    want.authority.content_hash == authority.content_hash
                        && want.authority.capability_hash == authority.capability_hash
                        && want.authority.state_generation == authority.state_generation
                        && want.authority.state_path == authority.state_path
                });
                if !keep {
                    if let Some(owner) = registry.revoke_owner(plugin_id) {
                        revoked.push(owner);
                    }
                    registry.forget_owner(plugin_id);
                }
            }
            for (plugin_id, want) in desired {
                // A failed or faulted activation of these exact bytes is not
                // retried every turn; changed authority or an explicit
                // plugin mutation/reload permits another attempt.
                if registry.owner(&plugin_id).is_none() {
                    to_activate.push((plugin_id, want));
                }
            }
        }
        let host = shared.ready_host().ok();
        for owner in revoked {
            shared.plugin_diagnostic(
                &owner.plugin_id,
                format!("extension `{}` revoked", owner.plugin_id),
            );
            if let Some(host) = &host {
                host.revoke_calls_of(&owner.plugin_id);
                shared.deactivate_owner(host, &owner).await;
            }
        }

        if to_activate.is_empty() {
            return Ok(());
        }
        let host = self.ensure_host(policy).await?;
        for (plugin_id, want) in to_activate {
            if host.has_exited() {
                break;
            }
            self.activate_owner(&host, &plugin_id, want).await;
        }
        Ok(())
    }

    async fn activate_owner(&self, host: &Arc<HostProcess>, plugin_id: &str, want: DesiredOwner) {
        let shared = &self.shared;
        let owner = shared.registry.lock().expect("registry lock").begin_owner(
            plugin_id,
            &want.plugin_name,
            want.authority.clone(),
            &want.authority.content_hash,
        );
        let mut failure = None;
        let mut tools = Vec::new();
        for (path, sha256) in &want.entries {
            let request = CoreRequest::Activate(ActivateParams {
                owner: owner.clone(),
                plugin_name: want.plugin_name.clone(),
                entry: EntryRef {
                    path: path.to_string_lossy().into_owned(),
                    sha256: sha256.clone(),
                },
                config: json!({}),
            });
            let outcome = host.call(request, Some(plugin_id.to_string())).await;
            match outcome.map(serde_json::from_value::<ActivateResult>) {
                Ok(Ok(ActivateResult::Ok { tools: mut names })) => tools.append(&mut names),
                Ok(Ok(ActivateResult::Failed { diagnostic })) => {
                    failure = Some(diagnostic);
                    break;
                }
                Ok(Err(error)) => {
                    failure = Some(format!("malformed activation answer: {error}"));
                    break;
                }
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        match failure {
            None => {
                let active = shared
                    .registry
                    .lock()
                    .expect("registry lock")
                    .mark_active(&owner);
                if active {
                    shared.plugin_diagnostic(
                        plugin_id,
                        format!(
                            "extension `{}` active (tools: {})",
                            want.plugin_name,
                            tools.join(", ")
                        ),
                    );
                }
            }
            Some(reason) => {
                // All-or-nothing: drop anything half-registered on this side,
                // and dispose any entry the host did activate.
                shared
                    .registry
                    .lock()
                    .expect("registry lock")
                    .mark_failed(&owner, OwnerState::Failed(reason.clone()));
                shared.plugin_diagnostic(
                    plugin_id,
                    format!(
                        "extension `{}` failed to activate: {reason}",
                        want.plugin_name
                    ),
                );
                shared.deactivate_owner(host, &owner).await;
            }
        }
    }

    async fn ensure_host(&self, policy: bool) -> Result<Arc<HostProcess>, String> {
        let shared = &self.shared;
        let mut generation = {
            let mut slot = shared.host.lock().expect("host lock");
            match &*slot {
                HostSlot::Ready(host) if !host.has_exited() && !host.is_retiring() => {
                    return Ok(Arc::clone(host));
                }
                HostSlot::Idle => {}
                other => return Err(host_down(&other.status())),
            }
            *slot = HostSlot::Starting;
            let mut supervision = shared.supervision.lock().expect("supervision lock");
            supervision.last_start = Some(Instant::now());
            supervision.policy = policy;
            supervision.launch_failed = false;
            supervision.dirty_teardowns.clear();
            supervision.dirty_restart_pending = false;
            supervision.planned_restart = None;
            shared.host_generation.fetch_add(1, Ordering::SeqCst) + 1
        };
        // At most two launches: under `auto`, before anything is pinned, a
        // Bun that cannot start is reported once and Node is resolved for the
        // rest of the session. An explicit `bun` or `node` never falls back.
        let spawned = loop {
            shared.spawn_attempts.fetch_add(1, Ordering::SeqCst);
            let options = shared.options.clone();
            let (pinned, bun_failed) = {
                let runtime = shared.runtime.lock().expect("runtime lock");
                (runtime.pinned.clone(), runtime.bun_failed)
            };
            let prepared =
                tokio::task::spawn_blocking(move || prepare_launch(&options, pinned, bun_failed))
                    .await
                    .map_err(|error| format!("extension host preparation failed: {error}"))
                    .and_then(|result| result);
            let (launch, summary) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => break Err(error),
            };
            let events: Arc<dyn HostEvents> = Arc::new(Events {
                shared: Arc::downgrade(shared),
                generation,
            });
            let reason =
                match HostProcess::spawn(generation, &launch, bundle_sha256(), events).await {
                    Ok(host) => break Ok((host, summary)),
                    Err(reason) => reason,
                };
            if shared.options.runtime != crate::config::ExtensionHostRuntime::Auto
                || launch.runtime.kind != crate::dependencies::HostRuntimeKind::Bun
                || summary.is_none()
            {
                break Err(reason);
            }
            shared.runtime.lock().expect("runtime lock").bun_failed = true;
            shared.diagnostic(format!(
                "extension host: Bun {} at {} failed to start ({reason}); runtime = \"auto\" uses Node for the rest of this session",
                launch.runtime.version_string(),
                launch.runtime.path.display()
            ));
            // A fresh generation, so the failed Bun host's exit report can
            // never be taken for the Node host's.
            generation = {
                let slot = shared.host.lock().expect("host lock");
                if shared.host_generation.load(Ordering::SeqCst) != generation
                    || !matches!(&*slot, HostSlot::Starting)
                {
                    break Err("extension host startup was superseded".into());
                }
                shared.host_generation.fetch_add(1, Ordering::SeqCst) + 1
            };
        };
        let mut slot = shared.host.lock().expect("host lock");
        if shared.host_generation.load(Ordering::SeqCst) != generation
            || !matches!(&*slot, HostSlot::Starting)
        {
            drop(slot);
            if let Ok((host, _)) = spawned {
                host.terminate("host startup superseded".into());
            }
            return Err("extension host startup was superseded".into());
        }
        match spawned {
            Ok((host, summary)) => {
                *slot = HostSlot::Ready(Arc::clone(&host));
                drop(slot);
                // Pinned only once a host on this runtime has completed the
                // handshake; every restart then reuses it.
                if let Some(summary) = summary {
                    let mut runtime = shared.runtime.lock().expect("runtime lock");
                    if runtime.pinned.is_none() {
                        runtime.pinned = Some((host.runtime.clone(), summary.clone()));
                        drop(runtime);
                        shared.diagnostic(format!("extension host runtime: {summary}"));
                    }
                }
                if host.has_exited() {
                    Events {
                        shared: Arc::downgrade(shared),
                        generation,
                    }
                    .exited(
                        generation,
                        "exited immediately after handshake".into(),
                        host.stderr_tail(),
                    );
                    return Err("extension host exited immediately after handshake".into());
                }
                monitor_host(shared, &host, generation);
                shared.diagnostic(format!(
                    "extension host started (pid {}, {} {}, sandbox {})",
                    host.pid
                        .map_or_else(|| "?".to_string(), |pid| pid.to_string()),
                    host.runtime.kind.name(),
                    host.runtime_version.get().map_or("?", String::as_str),
                    host.sandbox.label()
                ));
                Ok(host)
            }
            Err(reason) => {
                shared
                    .supervision
                    .lock()
                    .expect("supervision lock")
                    .launch_failed = true;
                *slot = HostSlot::Failed {
                    reason: format!("start failed: {reason}"),
                    stderr_tail: String::new(),
                };
                drop(slot);
                shared.diagnostic(format!("extension host failed to start: {reason}"));
                Err(reason)
            }
        }
    }

    /// Bounded shutdown of the host process, if one is running. Production
    /// has no such call: the host is shared by every engine in the process,
    /// so no single engine's shutdown may stop it. When this process ends the
    /// host sees stdin EOF and kills its own process tree; if a plugin blocks
    /// its event loop, its watchdog thread does so when the parent changes.
    #[cfg(test)]
    pub async fn shutdown(&self) {
        let host = {
            let mut slot = self.shared.host.lock().expect("host lock");
            self.shared.host_generation.fetch_add(1, Ordering::SeqCst);
            self.shared
                .supervision
                .lock()
                .expect("supervision lock")
                .retry_ticket += 1;
            match std::mem::replace(&mut *slot, HostSlot::Idle) {
                HostSlot::Ready(host) | HostSlot::Unresponsive(host) => Some(host),
                _ => None,
            }
        };
        if let Some(host) = host {
            self.shared
                .registry
                .lock()
                .expect("registry lock")
                .revoke_all("extension host shut down");
            host.shutdown().await;
        }
    }

    #[cfg(test)]
    pub(crate) fn host_requests_started(&self) -> Option<u64> {
        self.shared
            .ready_host()
            .ok()
            .map(|host| host.requests_started())
    }

    #[cfg(test)]
    pub(crate) fn host_pid(&self) -> Option<u32> {
        self.shared.ready_host().ok().and_then(|host| host.pid)
    }
}

/// Human-readable host section for `/plugin`.
pub(crate) fn render_status(manager: &ExtensionHostManager) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Extension host (experimental): ");
    let status = manager.status();
    match status.clone() {
        HostStatus::Ready {
            pid,
            runtime,
            runtime_version,
            sandbox,
            memory: _,
        } => {
            let _ = write!(
                out,
                "running · pid {} · {runtime} {runtime_version} · {sandbox}",
                pid.map_or_else(|| "?".to_string(), |pid| pid.to_string()),
            );
        }
        HostStatus::Failed { stderr_tail, .. } => {
            let _ = write!(out, "{status}");
            let tail = stderr_tail.trim();
            if !tail.is_empty() {
                let start = tail
                    .char_indices()
                    .rev()
                    .nth(599)
                    .map_or(0, |(index, _)| index);
                let _ = write!(out, "\n  stderr: {}", &tail[start..]);
            }
        }
        down => {
            let _ = write!(out, "{down}");
        }
    }
    if let Some(summary) = manager.runtime_summary() {
        let _ = write!(out, "\n  runtime: {summary}");
        // How the running host's cap is enforced, as its handshake settled
        // it; with no host running there is nothing enforced to describe.
        if let HostStatus::Ready { memory, .. } = &status {
            let cap = manager.shared.options.supervision.memory_cap;
            let _ = write!(out, " · {}", memory.describe(cap));
        }
    }
    let _ = write!(
        out,
        "\n  spawn attempts: {} · engines attached: {}",
        manager.spawn_attempts(),
        manager.attached_engines()
    );
    let (tools, owners) = {
        let registry = manager.shared.registry.lock().expect("registry lock");
        let owners = registry
            .owners()
            .filter(|entry| entry.state == OwnerState::Active)
            .count();
        (registry.live_tools(), owners)
    };
    if owners > 1 {
        let _ = write!(
            out,
            "\n  {owners} plugins share this one host process and can alter each other's behaviour"
        );
    }
    for tool in tools {
        let _ = write!(
            out,
            "\n  tool {} (extension:{}; needs approval, which your approval mode or a session grant for this exact call of this plugin build may give)",
            tool.name, tool.plugin_name
        );
    }
    let diagnostics = manager.diagnostics();
    for diagnostic in diagnostics.iter().rev().take(5).rev() {
        let _ = write!(out, "\n  · {diagnostic}");
    }
    out
}

/// A plugin was enabled, disabled, trusted, revoked or removed: reconcile the
/// host now, so a disabled plugin's calls are cancelled and its code torn
/// down without waiting for the next turn. No-op with the flag off.
pub fn plugins_changed(plugins: Arc<PluginRegistry>) {
    if activation::extension_host_policy_enabled() {
        let manager = manager();
        manager.refresh_workspace(&plugins);
        manager.retry();
        manager.reconcile_in_background();
    }
}

/// The `/plugin` section. With the experimental host off it is one line
/// saying so.
#[must_use]
pub fn status_report() -> String {
    if !activation::extension_host_policy_enabled() {
        return format!("Extension host (experimental): {}", HostStatus::Disabled);
    }
    render_status(&manager())
}

pub struct OwnerReport {
    pub state: Option<OwnerState>,
    pub tools: Vec<String>,
    pub diagnostics: Vec<String>,
}

/// The resolved plugin id is supplied by the existing `/plugin show` facet.
pub fn owner_report(plugin_id: &str) -> Option<OwnerReport> {
    if !activation::extension_host_policy_enabled() {
        return None;
    }
    manager().owner_report(plugin_id)
}

/// Reviewed, enabled plugins with `native` entries, keyed by plugin id, read
/// from Codewhale's immutable staged snapshot. Blocking.
fn desired_owners(plugins: &PluginRegistry) -> (BTreeMap<String, DesiredOwner>, Vec<String>) {
    let (sources, mut errors) = crate::plugins::runtime::active_component_sources(
        plugins,
        PluginActivationCapability::Native,
    );
    let mut desired: BTreeMap<String, DesiredOwner> = BTreeMap::new();
    let mut broken: BTreeSet<String> = BTreeSet::new();
    for source in sources {
        let plugin_id = source.authority.plugin_id.as_str().to_string();
        // The rule discovery reports, re-checked on the staged copy: the
        // name here, and file-ness by the read itself.
        let bytes = match crate::plugins::runtime::native_entry_problem(&source.path, true) {
            None => std::fs::read(&source.path).map_err(|error| error.to_string()),
            Some(problem) => Err(problem.to_string()),
        };
        match bytes {
            Ok(bytes) => desired
                .entry(plugin_id)
                .or_insert_with(|| DesiredOwner {
                    plugin_name: source.plugin_name.clone(),
                    authority: source.authority.clone(),
                    entries: Vec::new(),
                })
                .entries
                .push((source.path.clone(), hex(Sha256::digest(&bytes)))),
            Err(reason) => {
                errors.push(format!(
                    "Plugin `{}` native entry {} was denied: {reason}",
                    source.plugin_name,
                    source.path.display()
                ));
                broken.insert(plugin_id);
            }
        }
    }
    // All-or-nothing per plugin: one unusable entry keeps the whole plugin out.
    for plugin_id in broken {
        desired.remove(&plugin_id);
    }
    (desired, errors)
}

/// One scan of the complete attachment set. Its per-engine views and global
/// owner union must be published together, against those same snapshots.
struct DesiredScan {
    attachments: Vec<(u64, Arc<PluginRegistry>, BTreeMap<String, String>)>,
    owners: BTreeMap<String, DesiredOwner>,
    errors: Vec<String>,
}

impl DesiredScan {
    fn publish(
        self,
        current: &mut BTreeMap<u64, AttachmentState>,
    ) -> Option<(BTreeMap<String, DesiredOwner>, Vec<String>)> {
        if current.len() != self.attachments.len()
            || self.attachments.iter().any(|(id, scanned, _)| {
                !current
                    .get(id)
                    .is_some_and(|state| Arc::ptr_eq(&state.plugins, scanned))
            })
        {
            return None;
        }
        for (id, _, desired) in self.attachments {
            current.get_mut(&id).expect("validated attachment").desired = desired;
        }
        Some((self.owners, self.errors))
    }
}

/// Scan every attached snapshot (engines sharing one snapshot scan it once)
/// and merge what they desire. Blocking.
///
/// Two snapshots can disagree about one plugin id only while one of them is
/// stale; the stale one then fails its persisted-state check and desires
/// nothing, so the first valid scan wins and the per-attachment hashes keep
/// each engine's tools to the bytes it desires.
fn union_of_desired_owners(snapshots: Vec<(u64, Arc<PluginRegistry>)>) -> DesiredScan {
    let mut union: BTreeMap<String, DesiredOwner> = BTreeMap::new();
    let mut per_attachment = Vec::with_capacity(snapshots.len());
    let mut scanned: Vec<(Arc<PluginRegistry>, BTreeMap<String, String>)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (id, plugins) in snapshots {
        if let Some((_, hashes)) = scanned.iter().find(|(seen, _)| Arc::ptr_eq(seen, &plugins)) {
            per_attachment.push((id, Arc::clone(&plugins), hashes.clone()));
            continue;
        }
        let (desired, scan_errors) = desired_owners(&plugins);
        for error in scan_errors {
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
        let hashes: BTreeMap<String, String> = desired
            .iter()
            .map(|(plugin_id, want)| (plugin_id.clone(), want.authority.content_hash.clone()))
            .collect();
        for (plugin_id, want) in desired {
            union.entry(plugin_id).or_insert(want);
        }
        per_attachment.push((id, Arc::clone(&plugins), hashes.clone()));
        scanned.push((plugins, hashes));
    }
    DesiredScan {
        attachments: per_attachment,
        owners: union,
        errors,
    }
}

/// One engine's hold on the process-wide extension host.
///
/// The engine publishes its workspace plugin snapshot here and installs only
/// the tools of owners that snapshot desires. Dropping it detaches without
/// revoking anything: the next reconcile revokes owners no remaining
/// attachment desires, so an engine being replaced never tears down plugins
/// its successor is about to use.
pub struct HostAttachment {
    id: u64,
    manager: Arc<ExtensionHostManager>,
}

impl HostAttachment {
    #[must_use]
    pub fn manager(&self) -> &Arc<ExtensionHostManager> {
        &self.manager
    }

    /// The engine switched workspace: publish the new snapshot.
    pub fn set_plugins(&self, plugins: Arc<PluginRegistry>) {
        if let Some(state) = self
            .manager
            .shared
            .attachments
            .lock()
            .expect("attachments lock")
            .get_mut(&self.id)
        {
            state.plugins = plugins;
            state.desired.clear();
        }
    }

    /// Reconcile the host against every attachment, waiting for it.
    #[cfg(test)]
    pub async fn sync(&self) -> Result<(), String> {
        self.manager.reconcile().await
    }

    /// Reconcile the host against every attachment, without waiting.
    pub fn sync_in_background(&self) {
        self.manager.reconcile_in_background();
    }

    /// Add this engine's live extension tools to `tool_registry`.
    pub fn install_tools(&self, tool_registry: &mut crate::tools::ToolRegistry) -> Vec<String> {
        self.manager.install_tools_for(self.id, tool_registry)
    }
}

impl Drop for HostAttachment {
    fn drop(&mut self) {
        if let Ok(mut attachments) = self.manager.shared.attachments.lock() {
            attachments.remove(&self.id);
        }
    }
}

impl std::fmt::Debug for HostAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostAttachment")
            .field("id", &self.id)
            .finish()
    }
}

static GLOBAL: OnceLock<Arc<ExtensionHostManager>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_MANAGER: std::cell::RefCell<Option<Arc<ExtensionHostManager>>> =
        const { std::cell::RefCell::new(None) };
}

/// Configure the process-wide manager once, at boot, from user config.
pub fn configure(options: ExtensionHostOptions) {
    let _ = GLOBAL.set(Arc::new(ExtensionHostManager::new(options)));
}

/// The manager for this engine process (one host per process and tier).
#[must_use]
pub fn manager() -> Arc<ExtensionHostManager> {
    #[cfg(test)]
    if let Some(manager) = TEST_MANAGER.with(|cell| cell.borrow().clone()) {
        return manager;
    }
    Arc::clone(
        GLOBAL.get_or_init(|| Arc::new(ExtensionHostManager::new(ExtensionHostOptions::default()))),
    )
}

/// Test-only: route [`manager`] on this thread to `manager`.
#[cfg(test)]
pub(crate) struct TestManagerGuard(Option<Arc<ExtensionHostManager>>);

#[cfg(test)]
impl TestManagerGuard {
    pub(crate) fn install(manager: Arc<ExtensionHostManager>) -> Self {
        Self(TEST_MANAGER.with(|cell| cell.replace(Some(manager))))
    }
}

#[cfg(test)]
impl Drop for TestManagerGuard {
    fn drop(&mut self) {
        TEST_MANAGER.with(|cell| *cell.borrow_mut() = self.0.take());
    }
}
