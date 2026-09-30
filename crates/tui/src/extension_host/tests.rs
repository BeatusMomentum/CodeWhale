//! Extension host tests.
//!
//! Unit tests (protocol corpus, registry rules, tool gating) need no Node.
//! Integration tests spawn the *committed* bundle under a real Node ≥22.19:
//! they skip with a printed reason when none is found, unless
//! `CODEWHALE_EXT_HOST_TESTS=1` is set (CI), where a missing Node fails.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::protocol::{
    self, OwnerRef, RegisterKind, RegisterParams, ToolSpecWire, parse_core_message,
    parse_host_message,
};
use super::registry::{OwnerRegistry, OwnerState};
use super::{ExtensionHostManager, ExtensionHostOptions, HostAttachment, HostStatus};
use crate::plugins::PluginRegistry;
use crate::plugins::activation::TestPolicyGuard;
use crate::plugins::discovery::{DiscoveryConfig, discover_with_config};
use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolError, ToolSpec};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extension_host")
}

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

#[test]
fn protocol_corpus_parses_and_round_trips_in_both_directions() {
    let dir = fixtures_dir().join("protocol");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("corpus dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort();
    let (mut valid, mut invalid) = (0, 0);
    for path in entries {
        let case: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let frame = case["frame"].clone();
        let direction = case["direction"].as_str().unwrap();
        let expect_valid = case["valid"].as_bool().unwrap();
        let reencoded = match direction {
            "host_to_core" => parse_host_message(frame.clone()).map(|m| m.to_value()),
            "core_to_host" => parse_core_message(frame.clone()).map(|m| m.to_value()),
            other => panic!("unknown direction {other}"),
        };
        let name = path.file_name().unwrap().to_string_lossy();
        if expect_valid {
            let reencoded = reencoded.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(reencoded, frame, "{name} must round-trip exactly");
            let bytes = protocol::encode_frame(&frame).unwrap();
            assert_eq!(&bytes[..4], b"CWX1");
            valid += 1;
        } else {
            assert!(reencoded.is_err(), "{name} must be rejected");
            invalid += 1;
        }
    }
    assert!(
        valid >= 15 && invalid >= 8,
        "corpus too small: {valid} valid, {invalid} invalid"
    );
}

#[tokio::test]
async fn frames_decode_in_order_and_violations_are_typed() {
    let mut bytes = protocol::encode_frame(&json!({"a": 1})).unwrap();
    bytes.extend(protocol::encode_frame(&json!({"b": "ü"})).unwrap());
    let mut reader = bytes.as_slice();
    assert_eq!(
        protocol::read_frame(&mut reader).await.unwrap(),
        Some(json!({"a": 1}))
    );
    assert_eq!(
        protocol::read_frame(&mut reader).await.unwrap(),
        Some(json!({"b": "ü"}))
    );
    assert_eq!(protocol::read_frame(&mut reader).await.unwrap(), None);

    let mut bad = b"NOPE\0\0\0\0".as_slice();
    assert!(matches!(
        protocol::read_frame(&mut bad).await,
        Err(protocol::FrameError::BadMagic)
    ));
    let mut huge = Vec::from(*b"CWX1");
    huge.extend(((protocol::MAX_FRAME + 1) as u32).to_le_bytes());
    assert!(matches!(
        protocol::read_frame(&mut huge.as_slice()).await,
        Err(protocol::FrameError::TooLarge(_))
    ));
}

// ---------------------------------------------------------------------------
// Owner registry
// ---------------------------------------------------------------------------

fn fake_authority(plugin_id: &str) -> crate::plugins::types::PluginAuthority {
    crate::plugins::types::PluginAuthority {
        plugin_id: crate::plugins::types::PluginId(plugin_id.to_string()),
        plugin_name: plugin_id.to_string(),
        workspace: PathBuf::from("/w"),
        state_path: PathBuf::from("/s"),
        source_manifest: PathBuf::from("/m"),
        staged_manifest: PathBuf::from("/sm"),
        content_hash: format!("hash-{plugin_id}"),
        capability_hash: "cap".to_string(),
        state_generation: 1,
    }
}

fn register(registry: &mut OwnerRegistry, owner: &OwnerRef, name: &str) -> Result<u64, String> {
    registry.register_tool(&RegisterParams {
        owner: owner.clone(),
        kind: RegisterKind::Tool,
        spec: ToolSpecWire {
            name: name.to_string(),
            description: "d".to_string(),
            input_schema: json!({"type": "object", "properties": {}})
                .as_object()
                .unwrap()
                .clone(),
        },
    })
}

#[test]
fn registry_refuses_shadowing_and_foreign_names_and_undoes_exactly_one_entry() {
    let mut registry = OwnerRegistry::new();
    registry.add_native_names(["grep_files"]);
    let a = registry.begin_owner("a", "a", fake_authority("a"), "hash-a");
    let b = registry.begin_owner("b", "b", fake_authority("b"), "hash-b");

    // Built-ins (static, snapshot, and case-folded) and reserved prefixes.
    for name in [
        "read_file",
        "grep_files",
        "READ",
        "tool_search",
        "mcp_x_y",
        "ext_z",
    ] {
        assert!(
            register(&mut registry, &a, name).is_err(),
            "{name} must be refused"
        );
    }
    assert!(register(&mut registry, &a, "bad name").is_err());

    let first = register(&mut registry, &a, "shared_name").unwrap();
    // Another owner cannot take it, in any case.
    assert!(register(&mut registry, &b, "Shared_Name").is_err());
    // Same owner re-registering retires the old handle.
    let second = register(&mut registry, &a, "shared_name").unwrap();
    assert_ne!(first, second);
    registry.mark_active(&a);
    registry.unregister(&a, first); // stale: must not remove the newer entry
    assert_eq!(registry.live_tools().len(), 1);
    assert!(registry.is_live(second, &a));
    // A foreign owner cannot unregister it either.
    registry.unregister(&b, second);
    assert!(registry.is_live(second, &a));

    // A stale token is refused.
    let mut stale = a.clone();
    stale.owner_token = "not-the-token".to_string();
    assert!(register(&mut registry, &stale, "other").is_err());

    // Revocation is synchronous and total.
    assert_eq!(registry.revoke_owner("a"), Some(a.clone()));
    assert!(!registry.is_live(second, &a));
    assert!(registry.live_tools().is_empty());
    assert!(register(&mut registry, &a, "after_revoke").is_err());
}

/// Names that the approval tables key by name must never reach an extension:
/// a `fetch_url` session grant for github.com is `net:github.com`, and a
/// plugin tool called `web_fetch` would otherwise get that same key.
#[test]
fn registry_refuses_names_the_approval_tables_special_case() {
    let mut registry = OwnerRegistry::new();
    let a = registry.begin_owner("a", "a", fake_authority("a"), "hash-a");
    // Special-cased by name somewhere in the approval path; some are also
    // natives in some modes.
    for name in [
        "web_fetch",
        "exec_wait",
        "exec_interact",
        "task_shell_start",
        "web_search",
        "run_tests",
        "run_verifiers",
        "fim_edit",
        "Bash",
        "read_workspace_deps",
        "list_things",
        "get_secret",
        "start_mcp_server",
    ] {
        let refused =
            register(&mut registry, &a, name).expect_err(&format!("{name} must be refused"));
        assert!(
            refused.contains("reserved") || refused.contains("collides with a built-in"),
            "{name}: {refused}"
        );
    }
    // Not natives in any mode: only the classifier probe refuses these.
    for name in [
        "web_fetch",
        "exec_wait",
        "exec_interact",
        "read_workspace_deps",
    ] {
        let refused = register(&mut registry, &a, name).unwrap_err();
        assert!(refused.contains("reserved"), "{name}: {refused}");
    }
    // The fetch-family key really is shared by name: this is what the refusal
    // protects.
    let input = json!({"url": "https://github.com/x"});
    assert_eq!(
        crate::tools::approval_cache::build_approval_grouping_key("web_fetch", &input),
        crate::tools::approval_cache::build_approval_grouping_key("fetch_url", &input),
    );
    // Opaque names are admitted and keyed as themselves.
    for name in ["load_workspace_dependencies", "slow_wait", "probe_read"] {
        register(&mut registry, &a, name).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn registry_enforces_schema_and_count_caps() {
    let mut registry = OwnerRegistry::new();
    let a = registry.begin_owner("a", "a", fake_authority("a"), "hash-a");
    let mut params = RegisterParams {
        owner: a.clone(),
        kind: RegisterKind::Tool,
        spec: ToolSpecWire {
            name: "big".to_string(),
            description: "x".repeat(super::registry::MAX_DESCRIPTION_BYTES + 1),
            input_schema: json!({"type": "object"}).as_object().unwrap().clone(),
        },
    };
    assert!(
        registry
            .register_tool(&params)
            .unwrap_err()
            .contains("description")
    );
    params.spec.description = "ok".to_string();
    params.spec.input_schema =
        json!({"type": "object", "description": "y".repeat(super::registry::MAX_SCHEMA_BYTES)})
            .as_object()
            .unwrap()
            .clone();
    assert!(
        registry
            .register_tool(&params)
            .unwrap_err()
            .contains("schema")
    );
    params.spec.input_schema = json!({"type": "string"}).as_object().unwrap().clone();
    assert!(
        registry
            .register_tool(&params)
            .unwrap_err()
            .contains("object")
    );
    for index in 0..super::registry::MAX_TOOLS_PER_OWNER {
        register(&mut registry, &a, &format!("t{index}")).unwrap();
    }
    assert!(
        register(&mut registry, &a, "one_too_many")
            .unwrap_err()
            .contains("at most")
    );
}

// ---------------------------------------------------------------------------
// Integration: real Node, real bundle
// ---------------------------------------------------------------------------

/// A Node for the integration tests, or `None` (skip) when there is none and
/// the tests were not explicitly required.
pub(crate) fn node_for_tests(test: &str) -> Option<PathBuf> {
    let resolution = crate::dependencies::resolve_node_for_extension_host(None);
    match resolution.selected {
        Some((path, _)) => Some(path),
        None if std::env::var_os("CODEWHALE_EXT_HOST_TESTS").is_some() => panic!(
            "{test}: CODEWHALE_EXT_HOST_TESTS is set but no Node ^22.19 || >=24 was found: {}",
            resolution.describe_rejections()
        ),
        None => {
            eprintln!(
                "skipping {test}: no Node ^22.19 || >=24 ({})",
                resolution.describe_rejections()
            );
            None
        }
    }
}

/// Fixture plugins installed into a private user plugin dir through the
/// reviewed installer (`plugins::install`, local path), then reviewed
/// (trusted) and enabled through the real registry. Callers must hold a
/// `TestPolicyGuard::extension_host(true)` on this thread.
pub(crate) struct FixturePlugins {
    _temp: tempfile::TempDir,
    pub config: DiscoveryConfig,
    pub root: PathBuf,
}

impl FixturePlugins {
    pub(crate) async fn new(names: &[&str]) -> Self {
        use crate::plugins::install::{
            DEFAULT_MAX_SIZE_BYTES, PluginInstallOutcome, PluginInstallSource, install,
        };
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project");
        let user = temp.path().join("user");
        std::fs::create_dir_all(&workspace).unwrap();
        for name in names {
            let outcome = install(
                PluginInstallSource::LocalPath(fixtures_dir().join(name)),
                &user,
                DEFAULT_MAX_SIZE_BYTES,
                &crate::network_policy::NetworkPolicy::default(),
                false,
                &|_| None,
            )
            .await
            .unwrap_or_else(|error| panic!("install {name}: {error:#}"));
            assert!(
                matches!(outcome, PluginInstallOutcome::Installed(ref installed) if installed.name == *name),
                "install {name}: {outcome:?}"
            );
        }
        let config = DiscoveryConfig {
            workspace: workspace.clone(),
            user_plugins_dir: user,
            workspace_plugins_dir: workspace.join(".codewhale/plugins"),
            builtin_plugin_dirs: Vec::new(),
            state_path: temp.path().join("state/plugin-state.json"),
        };
        let mut registry = discover_with_config(&config);
        for name in names {
            registry
                .trust(name)
                .unwrap_or_else(|e| panic!("trust {name}: {e}"));
            registry
                .enable(name)
                .unwrap_or_else(|e| panic!("enable {name}: {e}"));
        }
        let root = temp.path().join("home");
        let fixture = Self {
            _temp: temp,
            config,
            root,
        };
        let registry = fixture.registry();
        for name in names {
            assert!(
                registry.is_active(name),
                "{name} must be active under policy v4"
            );
        }
        fixture
    }

    pub(crate) fn registry(&self) -> Arc<PluginRegistry> {
        Arc::new(discover_with_config(&self.config))
    }

    pub(crate) fn disable(&self, name: &str) -> Arc<PluginRegistry> {
        let mut registry = discover_with_config(&self.config);
        registry.disable(name).unwrap();
        self.registry()
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.config.workspace
    }

    pub(crate) fn manager(&self, node: PathBuf) -> Arc<ExtensionHostManager> {
        Arc::new(ExtensionHostManager::new(ExtensionHostOptions {
            node_override: Some(node),
            root: Some(self.root.clone()),
            ..Default::default()
        }))
    }
}

pub(crate) fn host_tool(
    engine: &HostAttachment,
    workspace: &Path,
    name: &str,
) -> Arc<dyn ToolSpec> {
    let mut registry =
        crate::tools::registry::ToolRegistryBuilder::new().build(ToolContext::new(workspace));
    let installed = engine.install_tools(&mut registry);
    assert!(
        installed.contains(&name.to_string()),
        "{name} not installed: {installed:?}"
    );
    registry.get(name).unwrap()
}

fn rss_kib(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[tokio::test]
async fn dsh_plugin_runs_end_to_end_behind_the_approval_gate() {
    let Some(node) = node_for_tests("dsh_plugin_runs_end_to_end_behind_the_approval_gate") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["dsh-workspace-deps"]).await;
    let manager = fixture.manager(node);
    assert_eq!(
        manager.status(),
        HostStatus::Idle,
        "nothing starts before sync"
    );

    let started = Instant::now();
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let elapsed = started.elapsed();
    let pid = manager.host_pid().expect("host running");
    eprintln!(
        "extension host: spawn + handshake + activation {:.1} ms; RSS {} KiB",
        elapsed.as_secs_f64() * 1000.0,
        rss_kib(pid).map_or_else(|| "?".to_string(), |kib| kib.to_string())
    );
    assert_eq!(
        manager.live_tool_names(),
        vec!["load_workspace_dependencies"]
    );
    assert_eq!(
        manager.owner_state(
            fixture
                .registry()
                .get("dsh-workspace-deps")
                .unwrap()
                .id
                .as_str()
        ),
        Some(OwnerState::Active)
    );

    let tool = host_tool(&engine, fixture.workspace(), "load_workspace_dependencies");
    assert_eq!(tool.registration_origin(), "extension:dsh-workspace-deps");
    // The plugin declares `presentCall: kind 'read'`; approval stays Required.
    assert_eq!(
        tool.approval_requirement_for(&json!({})),
        ApprovalRequirement::Required
    );
    assert!(!tool.is_read_only_for(&json!({})));
    assert!(tool.defer_loading());
    let context = ToolContext::new(fixture.workspace());
    let prepared = tool.prepare(json!({}), &context).unwrap();
    assert_eq!(prepared.approval, ApprovalRequirement::Required);
    assert!(
        prepared
            .description
            .contains("extension:dsh-workspace-deps")
    );

    let result = tool.execute(json!({}), &context).await.unwrap();
    assert!(result.success);
    let payload: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(payload["pythonDistributions"]["numpy"], "2.1.0");
    assert!(payload["python"].as_str().unwrap().contains("dependencies"));
    manager.shutdown().await;
}

#[tokio::test]
async fn execute_tools_refuses_extension_tools_before_any_host_call() {
    let Some(node) = node_for_tests("execute_tools_refuses_extension_tools_before_any_host_call")
    else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let mut registry = crate::tools::registry::ToolRegistryBuilder::new()
        .build(ToolContext::new(fixture.workspace()));
    engine.install_tools(&mut registry);
    let context = ToolContext::new(fixture.workspace());
    let started = Instant::now();
    let result = crate::tools::codemode::execute_tools_tool(
        &json!({"code": "return await tools.call('slow_wait', { ms: 5000 })"}),
        &registry,
        &context,
    )
    .await
    .unwrap();
    // Refused at the gate: had the call reached the host it would take 5 s.
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(!result.success, "{}", result.content);
    assert!(
        result.content.contains("can mutate") || result.content.contains("needs approval"),
        "{}",
        result.content
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn disabling_mid_call_revokes_at_once_and_teardown_waits_for_async_disposers() {
    let Some(node) = node_for_tests("disabling_mid_call") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let tool = host_tool(&engine, fixture.workspace(), "slow_wait");
    let context = ToolContext::new(fixture.workspace());
    let call = tokio::spawn(async move { tool.execute(json!({}), &context).await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let disabled = fixture.disable("slow-tool");
    // The revocation scan (a full re-hash of the staged tree) runs first;
    // the clock for the 500 ms bound starts when the registry drops the
    // handle, which is the moment revocation takes effect. A plain thread
    // watches for it, because this runtime is single-threaded (the policy
    // override is thread-local) and would only look when `sync` yields.
    let watcher = {
        let manager = Arc::clone(&manager);
        std::thread::spawn(move || {
            let started = Instant::now();
            while !manager.live_tool_names().is_empty() {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "revocation never happened"
                );
                std::thread::yield_now();
            }
            Instant::now()
        })
    };
    let sync_started = Instant::now();
    engine.set_plugins(disabled);
    let sync = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move { manager.reconcile().await })
    };
    let outcome = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("call resolves")
        .unwrap();
    let resolved_at = Instant::now();
    let revoked_at = watcher.join().unwrap();
    let call_resolved = resolved_at.saturating_duration_since(revoked_at);
    assert!(
        matches!(outcome, Err(ToolError::Cancelled { .. })),
        "{outcome:?}"
    );
    eprintln!(
        "extension host: in-flight call resolved as cancelled {:.1} ms after revocation",
        call_resolved.as_secs_f64() * 1000.0
    );
    assert!(
        call_resolved < Duration::from_millis(500),
        "{call_resolved:?}"
    );
    sync.await.unwrap().unwrap();
    let teardown = sync_started.elapsed();
    assert!(
        teardown >= Duration::from_millis(300),
        "ack must wait for the 300 ms async disposer (got {teardown:?})"
    );
    let diagnostics = manager.diagnostics();
    assert!(
        !diagnostics.iter().any(|d| d.contains("teardown")),
        "disposed with nothing leaked: {diagnostics:?}"
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn killed_host_fails_calls_once_and_replays_with_fresh_owners() {
    let Some(node) = node_for_tests("killed_host") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    assert_eq!(manager.spawn_attempts(), 1);
    let pid = manager.host_pid().unwrap();
    let tool = host_tool(&engine, fixture.workspace(), "slow_wait");
    let context = ToolContext::new(fixture.workspace());
    let call = tokio::spawn(async move { tool.execute(json!({}), &context).await });
    tokio::time::sleep(Duration::from_millis(150)).await;
    #[cfg(unix)]
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    #[cfg(windows)]
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let outcome = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("call resolves")
        .unwrap();
    match outcome {
        Err(ToolError::NotAvailable { message }) => {
            assert!(message.contains("extension host exited"), "{message}")
        }
        other => panic!("expected a typed not-available error, got {other:?}"),
    }
    wait_host(&manager, || {
        manager.spawn_attempts() == 2 && manager.live_tool_names().contains(&"slow_wait".into())
    })
    .await;
    assert!(matches!(manager.status(), HostStatus::Ready { .. }));
    assert_ne!(manager.host_pid(), Some(pid));
    assert_eq!(manager.shared.supervision.lock().unwrap().crashes.len(), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn approval_providing_plugin_fails_activation_and_leaves_nothing_registered() {
    let Some(node) = node_for_tests("approval_providing_plugin") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["refuses-approval", "clash-native"]).await;
    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let registry = fixture.registry();
    for (name, needle) in [
        ("refuses-approval", "approval"),
        ("clash-native", "read_file"),
    ] {
        let id = registry.get(name).unwrap().id.as_str().to_string();
        match manager.owner_state(&id) {
            Some(OwnerState::Failed(reason)) => {
                assert!(reason.contains(needle), "{name}: {reason}")
            }
            other => panic!("{name}: expected failed activation, got {other:?}"),
        }
    }
    assert!(manager.live_tool_names().is_empty());
    // A failed activation of the same bytes is not retried every turn.
    let attempts = manager.spawn_attempts();
    engine.sync().await.unwrap();
    assert_eq!(manager.spawn_attempts(), attempts);
    manager.shutdown().await;
}

/// Stands in for a `~/.codewhale/tools` script tool.
struct FakeScriptTool;

#[async_trait::async_trait]
impl ToolSpec for FakeScriptTool {
    fn name(&self) -> &str {
        "fixture_script_tool"
    }
    fn registration_origin(&self) -> std::borrow::Cow<'_, str> {
        "plugin script fixture_script_tool".into()
    }
    fn description(&self) -> &str {
        "script"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn capabilities(&self) -> Vec<crate::tools::spec::ToolCapability> {
        Vec::new()
    }
    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<crate::tools::spec::ToolResult, ToolError> {
        Ok(crate::tools::spec::ToolResult::success("from the script"))
    }
}

#[tokio::test]
async fn an_extension_named_like_a_script_tool_is_skipped_at_turn_build() {
    let Some(node) = node_for_tests("script_name_clash") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["clash-script"]).await;
    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    assert_eq!(manager.live_tool_names(), vec!["fixture_script_tool"]);
    let mut registry = crate::tools::registry::ToolRegistryBuilder::new()
        .build(ToolContext::new(fixture.workspace()));
    registry.register(Arc::new(FakeScriptTool));
    let installed = engine.install_tools(&mut registry);
    assert!(installed.is_empty());
    assert_eq!(
        registry
            .get("fixture_script_tool")
            .unwrap()
            .registration_origin(),
        "plugin script fixture_script_tool",
        "the script tool is unaffected"
    );
    assert!(
        manager
            .diagnostics()
            .iter()
            .any(|d| d.contains("fixture_script_tool") && d.contains("skipped")),
        "{:?}",
        manager.diagnostics()
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn with_no_native_plugin_the_host_is_never_spawned() {
    let _policy = TestPolicyGuard::extension_host(true);
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(PluginRegistry::empty(temp.path()));
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions {
        node_override: None,
        root: Some(temp.path().join("home")),
        ..Default::default()
    }));
    let engine = manager.attach(registry);
    engine.sync().await.unwrap();
    assert_eq!(manager.spawn_attempts(), 0);
    assert_eq!(manager.status(), HostStatus::Idle);
    assert!(!temp.path().join("home").exists(), "nothing materialized");
}

async fn probe(tool: &Arc<dyn ToolSpec>, path: &Path, context: &ToolContext) -> Value {
    let result = tool
        .execute(json!({"path": path.to_string_lossy()}), context)
        .await
        .unwrap();
    serde_json::from_str(&result.content).unwrap()
}

#[tokio::test]
async fn sandboxed_host_cannot_read_codewhale_secrets_or_write_outside_its_data_dir() {
    let Some(node) = node_for_tests("sandboxed_host") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["secret-probe"]).await;
    // Created before launch: the deny-list records the canonical spelling of
    // paths that exist (macOS `/var` → `/private/var`).
    let secrets = fixture.root.join("secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    let token = secrets.join("token");
    std::fs::write(&token, "s3cret-value").unwrap();
    // Any other entry of the Codewhale home is denied too (config backups,
    // OAuth tokens, state), not only the named stores.
    let backup = fixture.root.join("config.toml.bak-20260925");
    std::fs::write(&backup, "api_key = \"s3cret-backup\"").unwrap();
    let tokens = fixture.root.join("tokens");
    std::fs::create_dir_all(&tokens).unwrap();
    std::fs::write(tokens.join("codex.json"), "s3cret-oauth").unwrap();
    // Outside the Codewhale home, ordinary files stay readable.
    let readable = fixture.workspace().join("readable.txt");
    std::fs::write(&readable, "plain").unwrap();

    let manager = fixture.manager(node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let HostStatus::Ready { sandbox, .. } = manager.status() else {
        panic!("host not ready: {:?}", manager.status());
    };
    let Some(sandbox) = sandbox else {
        assert!(
            cfg!(windows) || crate::sandbox::get_platform_sandbox().is_none(),
            "an OS sandbox is available here, so the host must run under it"
        );
        eprintln!("skipping sandbox assertions: no OS sandbox for the host on this platform");
        manager.shutdown().await;
        return;
    };
    assert!(super::render_status(&manager).contains(&format!("{sandbox} sandbox")));

    let context = ToolContext::new(fixture.workspace());
    let read = host_tool(&engine, fixture.workspace(), "probe_read");
    let write = host_tool(&engine, fixture.workspace(), "probe_write");

    let plain = probe(&read, &readable, &context).await;
    assert_eq!(
        plain,
        json!({"ok": true, "text": "plain"}),
        "ordinary reads work"
    );
    for denied in [token, backup, tokens.join("codex.json")] {
        let secret = probe(&read, &denied, &context).await;
        assert_eq!(secret["ok"], false, "{} was readable", denied.display());
        assert!(
            !secret.to_string().contains("s3cret"),
            "{} leaked its contents",
            denied.display()
        );
    }
    // A store created after the host started is denied by name.
    let state = fixture.root.join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("late.json"), "s3cret-late").unwrap();
    let late = probe(&read, &state.join("late.json"), &context).await;
    assert_eq!(
        late["ok"], false,
        "a store created after start was readable"
    );
    // The migrated history can first appear after host launch too. Its name
    // must be denied before enumeration can observe the file.
    let history = fixture.root.join("composer_history.jsonl");
    std::fs::write(&history, "\"private synthetic prompt\"\n").unwrap();
    let late_history = probe(&read, &history, &context).await;
    assert_eq!(late_history["ok"], false, "new history was readable");
    // The Codex credential file Codewhale itself reads, when this machine has
    // one. Only `ok` is reported, never the content.
    let codex_auth = crate::oauth::auth_file_path();
    if codex_auth.is_file() {
        let codex = probe(&read, &codex_auth, &context).await;
        assert_eq!(codex["ok"], false, "code: {}", codex["code"]);
    }

    let data = fixture.root.join("extension-host/data/probe.txt");
    assert_eq!(probe(&write, &data, &context).await["ok"], true);
    // Outside the data dir and the temp dirs (the fixture itself lives under
    // TMPDIR, which the profile leaves writable): the crate's source dir,
    // unless the checkout itself sits in a temp dir.
    let crate_dir = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
    let in_temp = [std::env::temp_dir(), PathBuf::from("/tmp")]
        .iter()
        .filter_map(|dir| std::fs::canonicalize(dir).ok())
        .any(|dir| crate_dir.starts_with(dir));
    if !in_temp {
        let escape = crate_dir.join(format!(".ext-host-probe-{}", uuid::Uuid::new_v4().simple()));
        let escaped = probe(&write, &escape, &context).await;
        let leaked = escape.exists();
        let _ = std::fs::remove_file(&escape);
        assert_eq!(escaped["ok"], false, "{escaped}");
        assert!(!leaked);
    }
    manager.shutdown().await;
}

// ---------------------------------------------------------------------------
// Receipt-bound approval keys (design §4.3)
// ---------------------------------------------------------------------------

fn keys_for(
    manager: &ExtensionHostManager,
    registration: super::registry::ToolRegistration,
    input: &Value,
) -> (String, String) {
    let name = registration.name.clone();
    let mut registry = crate::tools::ToolRegistry::new(ToolContext::new(Path::new("/w")));
    registry.register(Arc::new(super::tool::HostToolSpec::new(
        registration,
        Arc::clone(&manager.shared),
    )));
    let (exact, grouping) =
        crate::tools::approval_cache::approval_keys_for_call(Some(&registry), &name, input);
    (exact.0, grouping.0)
}

/// A session grant for an extension tool covers one reviewed plugin build:
/// an update of the plugin, or another plugin that later registers the same
/// tool name, gets a different key and is asked again.
#[test]
fn extension_approval_keys_are_bound_to_the_plugin_receipt() {
    let manager = ExtensionHostManager::new(ExtensionHostOptions::default());
    let input = json!({"path": "x"});
    let mut owners = OwnerRegistry::new();
    let live = |owners: &mut OwnerRegistry, owner: &OwnerRef| {
        register(owners, owner, "shared_tool").unwrap();
        owners.mark_active(owner);
        owners.live_tools().pop().unwrap()
    };

    let first = owners.begin_owner("a", "a", fake_authority("a"), "hash-a1");
    let first = keys_for(&manager, live(&mut owners, &first), &input);
    assert!(
        first.0.starts_with("ext:a@hash-a1:shared_tool:"),
        "{first:?}"
    );
    assert_eq!(first.0, first.1, "a grant covers the exact call only");
    let generic = crate::tools::approval_cache::build_approval_grouping_key("shared_tool", &input);
    assert_ne!(first.1, generic.0, "never the name-derived family key");

    // Same plugin, same input, updated bytes: a different grant.
    let updated = owners.begin_owner("a", "a", fake_authority("a"), "hash-a2");
    let updated = keys_for(&manager, live(&mut owners, &updated), &input);
    assert_ne!(first.1, updated.1);

    // Another plugin takes the name once the first is gone.
    owners.revoke_owner("a");
    let other = owners.begin_owner("b", "b", fake_authority("b"), "hash-a1");
    let other = keys_for(&manager, live(&mut owners, &other), &input);
    assert_ne!(first.1, other.1);
    assert_ne!(updated.1, other.1);

    // Tools without a scope keep their existing keys.
    let shell = json!({"command": "cargo build --release"});
    let (exact, grouping) =
        crate::tools::approval_cache::approval_keys_for_call(None, "exec_shell", &shell);
    assert_eq!(
        exact,
        crate::tools::approval_cache::build_approval_key("exec_shell", &shell)
    );
    assert_eq!(
        grouping,
        crate::tools::approval_cache::build_approval_grouping_key("exec_shell", &shell)
    );
}

// ---------------------------------------------------------------------------
// The native-entry rule at validate / review time
// ---------------------------------------------------------------------------

fn native_bundle(user: &Path, name: &str, native_path: &str, files: &[&str]) {
    let root = user.join(name);
    for file in files {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "export const name = 'x'\nexport function apply() {}\n",
        )
        .unwrap();
    }
    std::fs::write(
        root.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/plugin.json",
            "name": name,
            "version": "0.1.0",
            "description": "native entry rule fixture",
            "license": "MIT",
            "extensions": {"net.codewhale": {"native": {"path": native_path}}}
        }))
        .unwrap(),
    )
    .unwrap();
}

/// `/plugin validate` and the review screen read plugin diagnostics, so an
/// entry that activation would refuse must fail there too, with the flag on;
/// with it off `native` is inventory-only and any path stays valid.
#[test]
fn native_entry_rule_fails_validation_when_the_host_is_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let user = temp.path().join("user");
    native_bundle(&user, "dir-entry", "lib", &["lib/index.mjs"]);
    native_bundle(&user, "ts-entry", "index.ts", &["index.ts"]);
    native_bundle(&user, "good-entry", "index.mjs", &["index.mjs"]);
    native_bundle(&user, "typed-entry", "index.mts", &["index.mts"]);
    let config = DiscoveryConfig {
        workspace: temp.path().join("project"),
        user_plugins_dir: user,
        workspace_plugins_dir: temp.path().join("project/.codewhale/plugins"),
        builtin_plugin_dirs: Vec::new(),
        state_path: temp.path().join("state/plugin-state.json"),
    };
    let native_errors = |registry: &PluginRegistry, name: &str| -> Vec<String> {
        registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} not discovered: {:?}", registry.diagnostics()))
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "native-entry-invalid")
            .map(|diagnostic| {
                assert_eq!(
                    diagnostic.level,
                    crate::plugins::types::PluginDiagnosticLevel::Error
                );
                diagnostic.message.clone()
            })
            .collect()
    };

    {
        let _policy = TestPolicyGuard::extension_host(true);
        let registry = discover_with_config(&config);
        for name in ["dir-entry", "ts-entry"] {
            let errors = native_errors(&registry, name);
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert!(
                errors[0].contains(".mjs, .js or .mts"),
                "{name}: {errors:?}"
            );
        }
        assert!(native_errors(&registry, "good-entry").is_empty());
        assert!(native_errors(&registry, "typed-entry").is_empty());
        assert!(!registry.validation_is_clean());
    }
    let _policy = TestPolicyGuard::extension_host(false);
    let registry = discover_with_config(&config);
    for name in ["dir-entry", "ts-entry", "good-entry", "typed-entry"] {
        assert!(native_errors(&registry, name).is_empty(), "{name}");
    }
}

#[test]
fn owner_reports_keep_bounded_attributed_logs_and_ignore_stale_hosts() {
    use super::supervisor::HostEvents;

    let manager = ExtensionHostManager::new(ExtensionHostOptions::default());
    manager
        .shared
        .host_generation
        .store(2, std::sync::atomic::Ordering::SeqCst);
    {
        let mut registry = manager.shared.registry.lock().unwrap();
        for id in ["alpha", "beta"] {
            let owner = registry.begin_owner(id, id, fake_authority(id), "hash");
            registry.mark_active(&owner);
            register(&mut registry, &owner, &format!("{id}_probe")).unwrap();
        }
    }
    let current = super::Events {
        shared: Arc::downgrade(&manager.shared),
        generation: 2,
    };
    let stale = super::Events {
        shared: Arc::downgrade(&manager.shared),
        generation: 1,
    };
    for index in 0..25 {
        current.log(&protocol::LogParams {
            level: "warn".into(),
            msg: format!("alpha message {index}"),
            plugin_id: Some("alpha".into()),
        });
    }
    let mut log = protocol::LogParams {
        level: "error".into(),
        msg: "beta only".into(),
        plugin_id: Some("beta".into()),
    };
    current.log(&log);
    log.msg = "stale message".into();
    stale.log(&log);
    log.plugin_id = Some("unknown".into());
    current.log(&log);
    log.plugin_id = Some("alpha".into());
    log.level = "debug".into();
    current.log(&log);
    let alpha = manager.owner_report("alpha").unwrap();
    assert!(matches!(alpha.state, Some(OwnerState::Active)));
    assert_eq!(alpha.tools, ["alpha_probe"]);
    assert_eq!(alpha.diagnostics.len(), 20);
    assert_eq!(alpha.diagnostics[0], "warn: alpha message 5");
    assert_eq!(
        manager.owner_report("beta").unwrap().diagnostics,
        ["error: beta only"]
    );
    assert!(manager.owner_report("unknown").is_none());
    manager
        .shared
        .plugin_diagnostic(&"x".repeat(10_000), "oversized id".into());
    assert!(
        manager
            .shared
            .diagnostics
            .lock()
            .unwrap()
            .back()
            .unwrap()
            .plugin_id
            .is_none()
    );
    for _ in 0..80 {
        manager.shared.plugin_diagnostic("alpha", "🦀".repeat(3000));
    }
    assert_eq!(manager.diagnostics().len(), 64);
    assert!(
        manager
            .diagnostics()
            .iter()
            .all(|line| line.len() <= super::MAX_DIAGNOSTIC_BYTES + '…'.len_utf8())
    );
    assert!(
        manager
            .owner_report("alpha")
            .unwrap()
            .diagnostics
            .iter()
            .all(|line| line.ends_with('…'))
    );
}

#[tokio::test]
async fn typed_author_example_is_reviewed_before_its_tool_can_execute() {
    use crate::plugins::install::{DEFAULT_MAX_SIZE_BYTES, PluginInstallSource, install};

    let Some(node) = node_for_tests("typed author example") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&[]).await;
    let example =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/plugins/hello-extension");
    install(
        PluginInstallSource::LocalPath(example),
        &fixture.config.user_plugins_dir,
        DEFAULT_MAX_SIZE_BYTES,
        &crate::network_policy::NetworkPolicy::default(),
        false,
        &|_| None,
    )
    .await
    .unwrap();
    let mut plugins = discover_with_config(&fixture.config);
    assert!(!plugins.is_active("hello-extension"));
    plugins.trust("hello-extension").unwrap();
    assert!(
        !plugins.is_active("hello-extension"),
        "trust alone does not enable code"
    );
    plugins.enable("hello-extension").unwrap();
    // This tests plugin review and typed loading, not hang detection. The
    // 600 ms watchdog used by supervision fault tests can kill a healthy
    // typed-plugin load on a busy runner before registration completes.
    let manager = fixture.manager(node);
    let engine = manager.attach(Arc::new(plugins));
    engine.sync().await.unwrap();
    let tool = host_tool(&engine, fixture.workspace(), "hello_greet");
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Required);
    let result = tool
        .execute(
            json!({"name": "Codewhale"}),
            &ToolContext::new(fixture.workspace()),
        )
        .await
        .unwrap();
    let payload: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(payload["greeting"], "Hello, Codewhale!");
    assert!(payload["callId"].as_str().is_some_and(|id| !id.is_empty()));
    manager.shutdown().await;
}

// ---------------------------------------------------------------------------
// Engines sharing one process-wide host
// ---------------------------------------------------------------------------

#[test]
fn attachment_changes_discard_the_complete_scan_before_owner_side_effects() {
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions::default()));
    let old = Arc::new(PluginRegistry::empty(Path::new("/old")));
    let current = Arc::new(PluginRegistry::empty(Path::new("/current")));
    let attachment = manager.attach(Arc::clone(&old));
    let scan = |plugins: &Arc<PluginRegistry>| super::DesiredScan {
        attachments: vec![(
            attachment.id,
            Arc::clone(plugins),
            [("plugin".into(), "hash-plugin".into())].into(),
        )],
        owners: [(
            "plugin".into(),
            super::DesiredOwner {
                plugin_name: "plugin".into(),
                authority: fake_authority("plugin"),
                entries: Vec::new(),
            },
        )]
        .into(),
        errors: Vec::new(),
    };

    // An old scan finishes after the engine has already changed workspace.
    attachment.set_plugins(Arc::clone(&current));
    let mut attachments = manager.shared.attachments.lock().unwrap();
    assert!(scan(&old).publish(&mut attachments).is_none());
    assert!(attachments[&attachment.id].desired.is_empty());
    // A current scan publishes both the engine view and owner union.
    let (owners, _) = scan(&current).publish(&mut attachments).unwrap();
    assert!(owners.contains_key("plugin"));
    assert_eq!(attachments[&attachment.id].desired["plugin"], "hash-plugin");
    drop(attachments);

    // A newly attached engine also invalidates the complete scan, even when
    // it uses the same snapshot: otherwise its owners could be revoked.
    let other = manager.attach(Arc::clone(&current));
    assert!(
        scan(&current)
            .publish(&mut manager.shared.attachments.lock().unwrap())
            .is_none()
    );
    drop(other);
    let stale = scan(&current);
    drop(attachment);
    assert!(
        stale
            .publish(&mut manager.shared.attachments.lock().unwrap())
            .is_none()
    );
}

fn installed(engine: &HostAttachment, workspace: &Path) -> Vec<String> {
    let mut registry =
        crate::tools::registry::ToolRegistryBuilder::new().build(ToolContext::new(workspace));
    engine.install_tools(&mut registry)
}

fn plugin_id(fixture: &FixturePlugins, name: &str) -> String {
    fixture
        .registry()
        .get(name)
        .unwrap()
        .id
        .as_str()
        .to_string()
}

/// Two engines for different workspaces in one process: syncing either
/// keeps the other's plugin active and its in-flight call running, neither
/// receives the other's tools, and detaching one revokes only its plugin.
#[tokio::test]
async fn engines_in_one_process_never_revoke_each_others_plugins() {
    let Some(node) = node_for_tests("engines_in_one_process") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let slow = FixturePlugins::new(&["slow-tool"]).await;
    let deps = FixturePlugins::new(&["dsh-workspace-deps"]).await;
    let (slow_id, deps_id) = (
        plugin_id(&slow, "slow-tool"),
        plugin_id(&deps, "dsh-workspace-deps"),
    );
    let manager = slow.manager(node);
    let first = manager.attach(slow.registry());
    first.sync().await.unwrap();
    let tool = host_tool(&first, slow.workspace(), "slow_wait");
    let context = ToolContext::new(slow.workspace());
    let call = tokio::spawn(async move { tool.execute(json!({"ms": 600}), &context).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A second workspace's engine attaches and syncs mid-call.
    let second = manager.attach(deps.registry());
    second.sync().await.unwrap();
    assert_eq!(manager.owner_state(&slow_id), Some(OwnerState::Active));
    assert_eq!(manager.owner_state(&deps_id), Some(OwnerState::Active));
    let result = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("call resolves")
        .unwrap()
        .expect("the first engine's in-flight call completes");
    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("600"), "{}", result.content);

    assert_eq!(installed(&first, slow.workspace()), vec!["slow_wait"]);
    assert_eq!(
        installed(&second, deps.workspace()),
        vec!["load_workspace_dependencies"]
    );
    first.sync().await.unwrap();
    assert_eq!(manager.owner_state(&deps_id), Some(OwnerState::Active));
    assert!(
        !manager.diagnostics().iter().any(|d| d.contains("revoked")),
        "{:?}",
        manager.diagnostics()
    );

    // Detaching does not revoke by itself; the next reconcile revokes only
    // what no remaining engine desires.
    drop(second);
    assert_eq!(manager.owner_state(&deps_id), Some(OwnerState::Active));
    first.sync().await.unwrap();
    assert_eq!(manager.owner_state(&deps_id), None);
    assert_eq!(manager.owner_state(&slow_id), Some(OwnerState::Active));
    assert_eq!(manager.spawn_attempts(), 1);
    manager.shutdown().await;
}

/// Each snapshot is re-verified against persisted plugin state, so a disable
/// made through one engine's registry revokes the plugin for an engine still
/// holding the older snapshot.
#[tokio::test]
async fn a_disable_through_either_registry_revokes_for_every_engine() {
    let Some(node) = node_for_tests("a_disable_through_either_registry") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let id = plugin_id(&fixture, "slow-tool");
    let manager = fixture.manager(node);
    let first = manager.attach(fixture.registry());
    let second = manager.attach(fixture.registry());
    first.sync().await.unwrap();
    assert_eq!(installed(&first, fixture.workspace()), vec!["slow_wait"]);
    assert_eq!(installed(&second, fixture.workspace()), vec!["slow_wait"]);

    second.set_plugins(fixture.disable("slow-tool"));
    second.sync().await.unwrap();
    assert_eq!(manager.owner_state(&id), None, "revoked and forgotten");
    assert!(installed(&first, fixture.workspace()).is_empty());
    assert!(installed(&second, fixture.workspace()).is_empty());
    manager.shutdown().await;
}

fn fast_supervision() -> super::SupervisionOptions {
    super::SupervisionOptions {
        heartbeat_interval: Duration::from_millis(50),
        ping_timeout: Duration::from_millis(150),
        hang_timeout: Duration::from_millis(600),
        restart_backoff: Duration::from_millis(25),
        ..Default::default()
    }
}

fn supervised_manager(fixture: &FixturePlugins, node: PathBuf) -> Arc<ExtensionHostManager> {
    Arc::new(ExtensionHostManager::new(ExtensionHostOptions {
        node_override: Some(node),
        root: Some(fixture.root.clone()),
        supervision: fast_supervision(),
    }))
}

async fn wait_host(manager: &ExtensionHostManager, predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "host wait timed out: {:?}; {:?}",
            manager.status(),
            manager.diagnostics()
        )
    });
}

#[test]
fn crash_budget_is_bounded_and_expires_only_with_the_window() {
    let options = super::SupervisionOptions::default();
    let mut state = super::SupervisionState::default();
    let now = Instant::now();
    assert!(state.record_crash(now, &options));
    assert!(state.record_crash(now + Duration::from_secs(1), &options));
    assert!(!state.record_crash(now + Duration::from_secs(2), &options));
    assert_eq!(state.crashes.len(), 3);
    assert!(state.record_crash(
        now + options.crash_window + Duration::from_secs(3),
        &options
    ));
    assert_eq!(state.crashes.len(), 1);
}

#[test]
fn dirty_teardown_window_is_bounded_and_a_requested_restart_waits_for_idle() {
    let options = super::SupervisionOptions::default();
    let mut state = super::SupervisionState::default();
    let now = Instant::now();
    state.record_dirty_teardown(now, &options);
    assert!(!state.dirty_restart_pending);
    state.record_dirty_teardown(now + options.dirty_window, &options);
    assert!(!state.dirty_restart_pending, "the first event expired");
    state.record_dirty_teardown(
        now + options.dirty_window + Duration::from_secs(1),
        &options,
    );
    assert!(state.dirty_restart_pending);
    for second in 2..100 {
        state.record_dirty_teardown(
            now + options.dirty_window + Duration::from_secs(second),
            &options,
        );
    }
    assert_eq!(state.dirty_teardowns.len(), 2);
    state.record_dirty_teardown(now + options.dirty_window * 3, &options);
    assert!(
        state.dirty_restart_pending,
        "an idle request does not expire"
    );
    assert!(state.crashes.is_empty());
}

#[tokio::test]
async fn ordinary_exit_rejects_requests_from_a_drained_calls_waker() {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Wake, Waker};

    use super::supervisor::{HostCallError, HostProcess};

    struct AdmissionProbe {
        host: Arc<HostProcess>,
        result: Mutex<Option<Result<(), HostCallError>>>,
        woke: tokio::sync::Notify,
    }

    impl Wake for AdmissionProbe {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let mut result = self.result.lock().unwrap();
            if result.is_none() {
                // oneshot send wakes synchronously: probe the exact gap after
                // the exit watcher drains pending and starts failing its calls.
                *result = Some(
                    self.host
                        .start_request(protocol::CoreRequest::Ping, None)
                        .map(|(id, _)| self.host.forget(id)),
                );
                self.woke.notify_one();
            }
        }
    }

    let Some(node) = node_for_tests("ordinary exit admission") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions {
        node_override: Some(node),
        root: Some(fixture.root.clone()),
        supervision: super::SupervisionOptions {
            heartbeat_interval: Duration::from_secs(60),
            ..Default::default()
        },
    }));
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let host = manager.shared.ready_host().unwrap();
    let registration = manager.shared.registry.lock().unwrap().live_tools()[0].clone();
    let (_, mut call) = host
        .start_request(
            protocol::CoreRequest::ToolCall(protocol::ToolCallParams {
                handle: registration.handle,
                call_id: "exit-admission".into(),
                input: json!({"ms": 30_000}),
                deadline_ms: 60_000,
            }),
            Some(registration.owner.plugin_id),
        )
        .unwrap();
    // A following response proves the writer flushed the slow call and is
    // waiting for another frame, so a closed outbound queue cannot mask the bug.
    host.request_with_deadline(protocol::CoreRequest::Ping, None, Duration::from_secs(5))
        .await
        .unwrap();
    let probe = Arc::new(AdmissionProbe {
        host: Arc::clone(&host),
        result: Mutex::new(None),
        woke: tokio::sync::Notify::new(),
    });
    let waker = Waker::from(Arc::clone(&probe));
    assert!(
        Pin::new(&mut call)
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(
        !host.is_retiring(),
        "this exercises exit, not maintenance sealing"
    );
    host.terminate("ordinary exit admission regression".into());
    tokio::time::timeout(Duration::from_secs(5), probe.woke.notified())
        .await
        .expect("exit must fail the pending call");
    assert!(matches!(
        probe.result.lock().unwrap().take().unwrap(),
        Err(HostCallError::Exited(_))
    ));
    assert!(matches!(call.await.unwrap(), Err(HostCallError::Exited(_))));
    manager.shutdown().await;
}

#[tokio::test]
async fn idle_retirement_seals_admission_and_does_not_wait_for_heartbeat() {
    use futures_util::FutureExt;

    let Some(node) = node_for_tests("idle admission") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["slow-tool"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let host = manager.shared.ready_host().unwrap();
    let registration = manager.shared.registry.lock().unwrap().live_tools()[0].clone();
    let (_, call) = host
        .start_request(
            protocol::CoreRequest::ToolCall(protocol::ToolCallParams {
                handle: registration.handle,
                call_id: "idle-admission".into(),
                input: json!({"ms": 100}),
                deadline_ms: 5000,
            }),
            Some(registration.owner.plugin_id),
        )
        .unwrap();
    assert!(!host.terminate_if_idle(super::DIRTY_RESTART_REASON));
    assert!(call.await.unwrap().is_ok());
    // No await between admission and retirement: this heartbeat is still in
    // the pending map, but it does not make the process busy.
    let (_, _heartbeat) = host
        .start_request(protocol::CoreRequest::Ping, None)
        .unwrap();
    assert!(host.terminate_if_idle(super::DIRTY_RESTART_REASON));
    assert!(manager.shared.ready_host().is_none());
    assert!(matches!(manager.status(), HostStatus::Restarting { .. }));
    // Poll without yielding to the exit watcher. A reconcile in this exact
    // gap must not start activation on the sealed process and falsely fail
    // a valid receipt before replay.
    assert!(manager.ensure_host(true).now_or_never().unwrap().is_err());
    assert_eq!(
        manager.owner_state(&plugin_id(&fixture, "slow-tool")),
        Some(OwnerState::Active)
    );
    assert!(matches!(
        host.start_request(protocol::CoreRequest::Ping, None),
        Err(super::supervisor::HostCallError::Exited(_))
    ));
    assert!(!host.terminate_if_idle(super::DIRTY_RESTART_REASON));
    manager.shutdown().await;
}

#[tokio::test]
async fn two_dirty_teardowns_wait_for_a_live_call_then_replay_without_spending_crash_budget() {
    let Some(node) = node_for_tests("dirty teardown") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["dirty-dispose", "slow-tool"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    // An existing unexpected crash must survive planned maintenance.
    manager
        .shared
        .supervision
        .lock()
        .unwrap()
        .record_crash(Instant::now(), &manager.shared.options.supervision);
    engine.set_plugins(fixture.disable("dirty-dispose"));
    engine.sync().await.unwrap();
    assert_eq!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .dirty_teardowns
            .len(),
        1
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        manager.spawn_attempts(),
        1,
        "one dirty event is insufficient"
    );

    let mut enabled = discover_with_config(&fixture.config);
    enabled.enable("dirty-dispose").unwrap();
    engine.set_plugins(Arc::new(enabled));
    engine.sync().await.unwrap();
    let old = host_tool(&engine, fixture.workspace(), "slow_wait");
    let host = manager.shared.ready_host().unwrap();
    let registration = manager.shared.registry.lock().unwrap().live_tools()[0].clone();
    let (_, call) = host
        .start_request(
            protocol::CoreRequest::ToolCall(protocol::ToolCallParams {
                handle: registration.handle,
                call_id: "survives-dirty-teardown".into(),
                input: json!({"ms": 4000}),
                deadline_ms: 10000,
            }),
            Some(registration.owner.plugin_id.clone()),
        )
        .unwrap();
    engine.set_plugins(fixture.disable("dirty-dispose"));
    engine.sync().await.unwrap();
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .dirty_restart_pending
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(manager.spawn_attempts(), 1, "a live call defers retirement");
    let completed = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed["structured"]["waited"], 4000);
    wait_host(&manager, || {
        manager.spawn_attempts() == 2 && manager.live_tool_names().contains(&"slow_wait".into())
    })
    .await;
    assert_eq!(manager.shared.supervision.lock().unwrap().crashes.len(), 1);
    assert!(
        !manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .dirty_restart_pending
    );
    let replayed = manager.shared.registry.lock().unwrap().live_tools()[0].clone();
    assert_ne!(registration.owner, replayed.owner);
    assert_ne!(registration.handle, replayed.handle);
    assert!(matches!(
        old.execute(json!({"ms": 1}), &ToolContext::new(fixture.workspace()))
            .await,
        Err(ToolError::NotAvailable { .. })
    ));
    // A delayed outcome from the retired process cannot dirty its replacement.
    manager.shared.record_dirty_teardown(&host);
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .dirty_teardowns
            .is_empty()
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn failed_activation_cleanup_also_records_a_dirty_teardown() {
    let Some(node) = node_for_tests("failed activation teardown") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["clash-script"]).await;
    native_bundle(
        &fixture.config.user_plugins_dir,
        "failed-disposal",
        "index.mjs",
        &["index.mjs"],
    );
    std::fs::write(
        fixture.config.user_plugins_dir.join("failed-disposal/index.mjs"),
        "export const name = 'failed-disposal';\nexport function apply(ctx) {\n  ctx.effect(() => () => new Promise(() => {}), 'unfinished activation cleanup');\n  throw new Error('fixture activation failure');\n}\n",
    ).unwrap();
    let mut plugins = discover_with_config(&fixture.config);
    plugins.trust("failed-disposal").unwrap();
    plugins.enable("failed-disposal").unwrap();
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(Arc::new(plugins));
    tokio::time::timeout(Duration::from_secs(15), engine.sync())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        manager.owner_state(&plugin_id(&fixture, "failed-disposal")),
        Some(OwnerState::Failed(_))
    ));
    assert_eq!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .dirty_teardowns
            .len(),
        1
    );
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .crashes
            .is_empty()
    );
    assert_eq!(manager.spawn_attempts(), 1);
    assert!(
        manager
            .live_tool_names()
            .contains(&"fixture_script_tool".into())
    );
    manager.shutdown().await;
}

#[test]
fn host_exit_preserves_failed_receipts_and_blames_only_the_activating_owner() {
    let mut registry = OwnerRegistry::new();
    let active = registry.begin_owner(
        "healthy",
        "healthy",
        fake_authority("healthy"),
        "hash-healthy",
    );
    registry.mark_active(&active);
    register(&mut registry, &active, "healthy_probe").unwrap();
    let failed = registry.begin_owner("failed", "failed", fake_authority("failed"), "hash-failed");
    registry.mark_failed(&failed, OwnerState::Faulted("existing fault".into()));
    registry.begin_owner(
        "activating",
        "activating",
        fake_authority("activating"),
        "hash-activating",
    );
    registry.host_exited("fixture crash");
    assert!(registry.owner("healthy").is_none());
    assert!(registry.live_tools().is_empty());
    assert!(matches!(
        registry.owner("failed").unwrap().state,
        OwnerState::Faulted(_)
    ));
    assert!(matches!(
        registry.owner("activating").unwrap().state,
        OwnerState::Failed(_)
    ));
    let replay = registry.begin_owner(
        "healthy",
        "healthy",
        fake_authority("healthy"),
        "hash-healthy",
    );
    assert_ne!(replay.generation, active.generation);
    assert_ne!(replay.owner_token, active.owner_token);
}

#[test]
fn opening_an_engine_never_resets_a_crash_budget() {
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions::default()));
    {
        *manager.shared.host.lock().unwrap() = super::HostSlot::Failed {
            reason: "budget".into(),
            stderr_tail: String::new(),
        };
        let mut state = manager.shared.supervision.lock().unwrap();
        for _ in 0..3 {
            state.record_crash(Instant::now(), &manager.shared.options.supervision);
        }
    }
    let _engine = manager.attach(Arc::new(PluginRegistry::empty(Path::new("/fixture"))));
    assert_eq!(manager.shared.supervision.lock().unwrap().crashes.len(), 3);
    assert!(matches!(manager.status(), HostStatus::Failed { .. }));
    manager.retry();
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .crashes
            .is_empty()
    );
    assert_eq!(manager.status(), HostStatus::Idle);
}

#[tokio::test]
async fn three_crashes_stop_replay_until_explicit_retry() {
    let Some(node) = node_for_tests("crash budget") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["crash-tool", "refuses-approval"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let failed_id = plugin_id(&fixture, "refuses-approval");
    let mut previous = None;
    for crash in 1..=3 {
        let tool = host_tool(&engine, fixture.workspace(), "crash_probe");
        let registration = manager
            .shared
            .registry
            .lock()
            .unwrap()
            .live_tools()
            .into_iter()
            .find(|t| t.name == "crash_probe")
            .unwrap();
        if let Some(old) = previous {
            assert_ne!(registration.owner, old);
        }
        previous = Some(registration.owner);
        let outcome = tool
            .execute(json!({}), &ToolContext::new(fixture.workspace()))
            .await;
        assert!(matches!(outcome, Err(ToolError::NotAvailable { .. })));
        if crash < 3 {
            wait_host(&manager, || {
                manager.spawn_attempts() == crash + 1
                    && manager.live_tool_names().contains(&"crash_probe".into())
            })
            .await;
            assert!(matches!(
                manager.owner_state(&failed_id),
                Some(OwnerState::Failed(_))
            ));
        } else {
            wait_host(&manager, || {
                matches!(manager.status(), HostStatus::Failed { .. })
            })
            .await;
        }
    }
    assert_eq!(manager.spawn_attempts(), 3);
    let _another = manager.attach(fixture.registry());
    engine.sync().await.ok();
    assert_eq!(manager.spawn_attempts(), 3);
    manager.retry();
    engine.sync().await.unwrap();
    assert_eq!(manager.spawn_attempts(), 4);
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .crashes
            .is_empty()
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn an_activation_crash_does_not_prevent_other_receipts_replaying() {
    let Some(node) = node_for_tests("activation crash") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["crash-activation", "clash-script"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    wait_host(&manager, || {
        manager.spawn_attempts() == 2
            && manager
                .live_tool_names()
                .contains(&"fixture_script_tool".into())
    })
    .await;
    assert!(matches!(
        manager.owner_state(&plugin_id(&fixture, "crash-activation")),
        Some(OwnerState::Failed(_))
    ));
    assert_eq!(manager.shared.supervision.lock().unwrap().crashes.len(), 1);
    manager.shutdown().await;
}

#[tokio::test]
async fn heartbeat_recovers_a_delayed_pong_then_kills_a_hung_host() {
    let Some(node) = node_for_tests("heartbeat") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["hang-tool"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let tool = host_tool(&engine, fixture.workspace(), "hang_probe");
    let workspace = fixture.workspace().to_path_buf();
    let task = tokio::spawn(async move {
        tool.execute(json!({"ms": 400}), &ToolContext::new(&workspace))
            .await
    });
    wait_host(&manager, || {
        matches!(manager.status(), HostStatus::Unresponsive { .. })
    })
    .await;
    assert!(task.await.unwrap().is_ok());
    wait_host(&manager, || {
        matches!(manager.status(), HostStatus::Ready { .. })
    })
    .await;
    assert_eq!(manager.spawn_attempts(), 1);
    let tool = host_tool(&engine, fixture.workspace(), "hang_probe");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        tool.execute(json!({}), &ToolContext::new(fixture.workspace())),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(ToolError::NotAvailable { .. })));
    wait_host(&manager, || {
        manager.spawn_attempts() == 2 && manager.live_tool_names().contains(&"hang_probe".into())
    })
    .await;
    assert_eq!(manager.shared.supervision.lock().unwrap().crashes.len(), 1);
    manager.shutdown().await;
}

#[test]
fn old_host_callbacks_cannot_fault_or_remove_a_new_owner() {
    use super::supervisor::HostEvents;
    let manager = ExtensionHostManager::new(ExtensionHostOptions::default());
    manager
        .shared
        .host_generation
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let owner = {
        let mut registry = manager.shared.registry.lock().unwrap();
        let owner = registry.begin_owner(
            "fixture",
            "fixture",
            fake_authority("fixture"),
            "hash-fixture",
        );
        registry.mark_active(&owner);
        register(&mut registry, &owner, "fixture_probe").unwrap();
        owner
    };
    let old = super::Events {
        shared: Arc::downgrade(&manager.shared),
        generation: 1,
    };
    old.faulted(&protocol::FaultedParams {
        owner,
        error: "stale fault".into(),
    });
    old.exited(1, "stale exit".into(), String::new());
    assert_eq!(manager.owner_state("fixture"), Some(OwnerState::Active));
    assert_eq!(manager.live_tool_names(), ["fixture_probe"]);
    assert!(
        manager
            .shared
            .supervision
            .lock()
            .unwrap()
            .crashes
            .is_empty()
    );
}

#[test]
fn a_new_attachment_retries_only_cooled_down_launch_failures() {
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions::default()));
    *manager.shared.host.lock().unwrap() = super::HostSlot::Failed {
        reason: "missing Node".into(),
        stderr_tail: String::new(),
    };
    {
        let mut state = manager.shared.supervision.lock().unwrap();
        state.launch_failed = true;
        state.last_start = Some(Instant::now());
    }
    let plugins = Arc::new(PluginRegistry::empty(Path::new("/fixture")));
    let _first = manager.attach(Arc::clone(&plugins));
    assert!(matches!(manager.status(), HostStatus::Failed { .. }));
    manager.shared.supervision.lock().unwrap().last_start =
        Some(Instant::now() - Duration::from_secs(61));
    let _later = manager.attach(plugins);
    assert_eq!(manager.status(), HostStatus::Idle);
}

#[tokio::test]
async fn replay_rechecks_persisted_disable_and_keeps_workspace_tools_separate() {
    let Some(node) = node_for_tests("replay authority") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let a = FixturePlugins::new(&["crash-tool"]).await;
    let b = FixturePlugins::new(&["clash-script"]).await;
    let manager = Arc::new(ExtensionHostManager::new(ExtensionHostOptions {
        node_override: Some(node),
        root: Some(a.root.clone()),
        supervision: super::SupervisionOptions {
            restart_backoff: Duration::from_secs(1),
            ..fast_supervision()
        },
    }));
    let first = manager.attach(a.registry());
    let second = manager.attach(b.registry());
    first.sync().await.unwrap();
    let tool = host_tool(&first, a.workspace(), "crash_probe");
    assert!(matches!(
        tool.execute(json!({}), &ToolContext::new(a.workspace()))
            .await,
        Err(ToolError::NotAvailable { .. })
    ));
    wait_host(&manager, || {
        matches!(manager.status(), HostStatus::Restarting { .. })
    })
    .await;
    // Keep the engine's snapshot stale deliberately. Replay must consult the
    // persisted state rather than restoring the previous owner's authority.
    a.disable("crash-tool");
    wait_host(&manager, || {
        manager.spawn_attempts() == 2
            && manager
                .live_tool_names()
                .contains(&"fixture_script_tool".into())
    })
    .await;
    assert!(installed(&first, a.workspace()).is_empty());
    assert_eq!(installed(&second, b.workspace()), ["fixture_script_tool"]);
    assert!(manager.owner_state(&plugin_id(&a, "crash-tool")).is_none());
    manager.shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(manager.status(), HostStatus::Idle);
    assert_eq!(
        manager.spawn_attempts(),
        2,
        "planned shutdown never restarts"
    );
}

#[tokio::test]
async fn explicit_retry_refreshes_same_byte_authority_without_inheriting_old_handles() {
    let Some(node) = node_for_tests("same byte retry") else {
        return;
    };
    let _policy = TestPolicyGuard::extension_host(true);
    let fixture = FixturePlugins::new(&["clash-script"]).await;
    let manager = supervised_manager(&fixture, node);
    let engine = manager.attach(fixture.registry());
    engine.sync().await.unwrap();
    let old = host_tool(&engine, fixture.workspace(), "fixture_script_tool");
    let old_owner = manager.shared.registry.lock().unwrap().live_tools()[0]
        .owner
        .clone();
    let mut updated = discover_with_config(&fixture.config);
    updated.enable("clash-script").unwrap();
    manager.refresh_workspace(&Arc::new(updated));
    manager.retry();
    engine.sync().await.unwrap();
    let current_owner = manager.shared.registry.lock().unwrap().live_tools()[0]
        .owner
        .clone();
    assert_ne!(old_owner, current_owner);
    assert!(matches!(
        old.execute(json!({}), &ToolContext::new(fixture.workspace()))
            .await,
        Err(ToolError::NotAvailable { .. })
    ));
    let current = host_tool(&engine, fixture.workspace(), "fixture_script_tool");
    assert!(
        current
            .execute(json!({}), &ToolContext::new(fixture.workspace()))
            .await
            .is_ok()
    );
    assert_eq!(
        manager.spawn_attempts(),
        1,
        "a healthy process need not restart"
    );
    manager.shutdown().await;
}
