//! Golden-events family: one full turn through the one turn loop.
//!
//! Each case drives `Engine::run` (and so `Engine::run_turn`) with a scripted
//! provider — a queue of normalized stream events per model request, written
//! in the [`super::stream_json`] format — and a small host driver that
//! answers approvals or cancels on cue. The golden is the protocol projection
//! (`protocol_parity::event_to_protocol`, the `EventMsg` wire shape every
//! frontend consumes), normalized, plus one trailing `harness_summary` line
//! with the provider request count and the workspace after the turn.
//!
//! Normalization (also stated in the fixture README): the per-session
//! `thread_id`/`session_id` routing envelope is dropped; liveness heartbeats
//! are dropped; UUIDs, timestamps, temp paths and durations are masked; the
//! system prompt and tool catalog bodies are replaced by a marker because the
//! prompt family owns those bytes; runs of adjacent `tool_call_complete`
//! events are ordered by tool call id because parallel completions race.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use codewhale_config::AppMode;
use codewhale_execpolicy::ApprovalMode;
use codewhale_models::{MessageRequest, MessageResponse, StreamEvent};
use codewhale_protocol::ids::{SessionId, ThreadId};

use super::golden::{self, Failures, Masker, Sandbox};
use super::stream_json;
use crate::compaction::CompactionConfig;
use crate::config::Config;
use crate::core::engine::{Engine, EngineConfig};
use crate::core::events::Event;
use crate::core::ops::{Op, TurnSpec, UserInputProvenance};
use crate::core::protocol_parity::{ProtocolIds, event_to_protocol};
use crate::llm_client::{LlmClient, StreamEventBox};

const FAMILY: &str = "events";
pub(super) const PREFIX_OWNED_BY_PROMPT_FAMILY: &str = "<pinned by the prompt family>";

/// Keys whose values are clocks or durations.
const VOLATILE_KEYS: &[&str] = &[
    "created_at",
    "duration_ms",
    "first_token_ms",
    "request_ms",
    "elapsed_ms",
    "pinned_combined_hash",
];

fn event_deadline() -> Duration {
    if cfg!(windows) {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(20)
    }
}

enum Step {
    Stream {
        events: Vec<StreamEvent>,
        hang: bool,
    },
    Refuse(String),
}

/// The scripted fake provider: one queued step per model request. Unlike the
/// general-purpose `MockLlmClient` it appends nothing — a script without
/// `message_stop` is a truncated stream, exactly as written.
pub(super) struct ScriptedProvider {
    model: String,
    steps: Mutex<VecDeque<Step>>,
    stream_requests: AtomicUsize,
    other_requests: AtomicUsize,
    captured: Mutex<Vec<MessageRequest>>,
}

impl ScriptedProvider {
    pub(super) fn from_script(model: &str, script: &Value) -> Self {
        let steps = script
            .as_array()
            .expect("provider_script is an array")
            .iter()
            .map(|step| {
                if let Some(message) = step.get("request_error").and_then(Value::as_str) {
                    return Step::Refuse(message.to_string());
                }
                let events = step["events"]
                    .as_array()
                    .expect("provider_script step has events")
                    .iter()
                    .map(|event| {
                        stream_json::from_json(event)
                            .unwrap_or_else(|error| panic!("script event {event}: {error}"))
                    })
                    .collect();
                Step::Stream {
                    events,
                    hang: step.get("then").and_then(Value::as_str) == Some("hang"),
                }
            })
            .collect();
        Self {
            model: model.to_string(),
            steps: Mutex::new(steps),
            stream_requests: AtomicUsize::new(0),
            other_requests: AtomicUsize::new(0),
            captured: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn stream_requests(&self) -> usize {
        self.stream_requests.load(Ordering::SeqCst)
    }

    pub(super) fn other_requests(&self) -> usize {
        self.other_requests.load(Ordering::SeqCst)
    }

    pub(super) fn captured(&self) -> Vec<MessageRequest> {
        self.captured.lock().expect("captured requests").clone()
    }
}

impl LlmClient for ScriptedProvider {
    fn provider_name(&self) -> &'static str {
        "conformance"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn create_message(&self, _request: MessageRequest) -> Result<MessageResponse> {
        self.other_requests.fetch_add(1, Ordering::SeqCst);
        Err(anyhow!(
            "conformance provider serves streaming turns only; a non-streaming request is not scripted"
        ))
    }

    async fn create_message_stream(&self, request: MessageRequest) -> Result<StreamEventBox> {
        let call = self.stream_requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.captured
            .lock()
            .expect("captured requests")
            .push(request);
        let step = self.steps.lock().expect("script").pop_front();
        match step {
            None => Err(anyhow!(
                "conformance script exhausted: model request #{call} has no scripted response"
            )),
            Some(Step::Refuse(message)) => Err(anyhow!(message)),
            Some(Step::Stream { events, hang }) => {
                let stream: StreamEventBox = Box::pin(async_stream::stream! {
                    for event in events {
                        yield Ok::<StreamEvent, anyhow::Error>(event);
                    }
                    if hang {
                        std::future::pending::<()>().await;
                    }
                });
                Ok(stream)
            }
        }
    }
}

fn app_mode(case: &Value) -> AppMode {
    match case["mode"].as_str().unwrap_or("agent") {
        "agent" => AppMode::Agent,
        "plan" => AppMode::Plan,
        "operate" => AppMode::Operate,
        other => panic!("unknown mode `{other}`"),
    }
}

fn approval_mode(case: &Value) -> ApprovalMode {
    match case["approval_mode"].as_str().unwrap_or("suggest") {
        "suggest" => ApprovalMode::Suggest,
        "auto" => ApprovalMode::Auto,
        "bypass" => ApprovalMode::Bypass,
        "never" => ApprovalMode::Never,
        other => panic!("unknown approval_mode `{other}`"),
    }
}

pub(super) fn send_message_op(case: &Value, config: &Config) -> Op {
    let model = case["model"].as_str().expect("case.model");
    let route =
        crate::route_runtime::resolve_runtime_route(config, config.api_provider(), Some(model))
            .expect("resolve conformance route");
    Op::SendMessage(TurnSpec {
        max_output_tokens: None,
        content: case["user_message"]
            .as_str()
            .expect("case.user_message")
            .to_string(),
        images: Vec::new(),
        mode: app_mode(case),
        route: Box::new(route),
        compaction: Box::new(CompactionConfig::default()),
        initial_routed_usage: Box::default(),
        goal_objective: None,
        goal_token_budget: None,
        goal_status: crate::tools::goal::GoalStatus::Active,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        auto_model: false,
        allow_shell: case["allow_shell"].as_bool().unwrap_or(true),
        trust_mode: false,
        auto_approve: case["auto_approve"].as_bool().unwrap_or(false),
        approval_mode: approval_mode(case),
        translation_enabled: false,
        allowed_tools: None,
        dynamic_tools: Vec::new(),
        hook_executor: None,
        verbosity: None,
        provenance: UserInputProvenance::ExternalUser,
    })
}

/// Relative path → sha256 prefix for every file left in the workspace.
fn workspace_listing(workspace: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let relative = path
                    .strip_prefix(root)
                    .expect("inside workspace")
                    .to_string_lossy()
                    .replace('\\', "/");
                let digest = crate::hashing::sha256_hex(&bytes);
                out.insert(relative, digest[..16].to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(workspace, workspace, &mut out);
    out
}

pub(super) struct TurnRecord {
    pub(super) events: Vec<Event>,
    pub(super) timed_out: bool,
}

/// Drive one scripted turn inside `sandbox` on a fresh current-thread
/// runtime, dropping the runtime (and its blocking post-turn work) before
/// returning so nothing outlives the sandboxed environment.
pub(super) fn run_scripted_turn(
    sandbox: &Sandbox,
    case: &Value,
    script: &Value,
) -> (TurnRecord, std::sync::Arc<ScriptedProvider>) {
    let model = case["model"].as_str().expect("case.model");
    let provider = std::sync::Arc::new(ScriptedProvider::from_script(model, script));
    let driver = case["driver"].as_array().cloned().unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let record = runtime.block_on(async {
        let config = Config::default();
        let engine_config = EngineConfig {
            workspace: sandbox.workspace.clone(),
            snapshots_enabled: false,
            subagents_enabled: false,
            session_id: Some("conformance-session".to_string()),
            ..EngineConfig::default()
        };
        let client: crate::core::model_client::SharedModelClient = provider.clone();
        let (engine, handle) = Engine::new_with_model_client(engine_config, &config, client);
        let op = send_message_op(case, &config);
        drive_turn(engine, handle, op, &driver).await
    });
    drop(runtime);
    (record, provider)
}

/// Run one turn and return every engine event up to and including
/// `TurnComplete`, then everything the engine emits while it shuts down.
async fn drive_turn(
    engine: Engine,
    handle: crate::core::engine::EngineHandle,
    op: Op,
    driver: &[Value],
) -> TurnRecord {
    let run = tokio::spawn(engine.run());
    handle.send(op).await.expect("send conformance turn");
    let rx = handle.rx_event.clone();
    let ids = protocol_ids();
    let mut events = Vec::new();
    let mut fired = vec![false; driver.len()];
    let mut timed_out = false;
    loop {
        let next =
            tokio::time::timeout(event_deadline(), async { rx.write().await.recv().await }).await;
        let event = match next {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        };
        let projected = serde_json::to_value(event_to_protocol(&event, &ids)).expect("EventMsg");
        for (index, rule) in driver.iter().enumerate() {
            if fired[index] || !rule_matches(&rule["when"], &projected) {
                continue;
            }
            fired[index] = true;
            match rule["action"].as_str().expect("driver action") {
                "cancel" => handle.cancel(),
                action @ ("approve" | "deny") => {
                    let Event::ApprovalRequired { id, .. } = &event else {
                        panic!("driver `{action}` must match an approval_required event");
                    };
                    let answered = if action == "approve" {
                        handle.approve_tool_call(id.clone()).await
                    } else {
                        handle.deny_tool_call(id.clone()).await
                    };
                    answered.expect("answer approval");
                }
                other => panic!("unknown driver action `{other}`"),
            }
        }
        let terminal = matches!(event, Event::TurnComplete { .. });
        events.push(event);
        if terminal {
            break;
        }
    }
    let _ = handle.send(Op::Shutdown).await;
    let _ = tokio::time::timeout(event_deadline(), run).await;
    let mut rx = rx.write().await;
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    TurnRecord { events, timed_out }
}

fn rule_matches(when: &Value, projected: &Value) -> bool {
    if when["event"].as_str() != projected["event"].as_str() {
        return false;
    }
    if let Some(needle) = when.get("delta_contains").and_then(Value::as_str) {
        return projected["delta"]
            .as_str()
            .is_some_and(|delta| delta.contains(needle));
    }
    true
}

fn protocol_ids() -> ProtocolIds {
    ProtocolIds {
        thread_id: ThreadId::from_string("conformance-thread"),
        session_id: SessionId::from_string("conformance-session"),
    }
}

/// Project, drop, mask and order engine events into golden lines.
pub(super) fn normalize_events(events: &[Event], masker: &mut Masker) -> Vec<Value> {
    let ids = protocol_ids();
    let mut lines: Vec<Value> = Vec::new();
    for event in events {
        let mut value = serde_json::to_value(event_to_protocol(event, &ids)).expect("EventMsg");
        let kind = value["event"].as_str().unwrap_or_default().to_string();
        if kind == "tool_call_heartbeat" {
            continue;
        }
        if let Some(map) = value.as_object_mut() {
            map.remove("thread_id");
            map.remove("session_id");
            for owned in ["tool_catalog", "system_prompt"] {
                if map.get(owned).is_some_and(|value| !value.is_null()) {
                    map.insert(owned.to_string(), json!(PREFIX_OWNED_BY_PROMPT_FAMILY));
                }
            }
        }
        masker.value(&mut value);
        lines.push(golden::canonical(&value));
    }
    order_parallel_completions(&mut lines);
    lines
}

fn order_parallel_completions(lines: &mut [Value]) {
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        while end < lines.len() && lines[end]["event"] == "tool_call_complete" {
            end += 1;
        }
        if end - start > 1 {
            lines[start..end].sort_by(|left, right| {
                left["tool_call_id"]
                    .as_str()
                    .cmp(&right["tool_call_id"].as_str())
            });
        }
        start = end.max(start + 1);
    }
}

fn check_invariants(case: &Value, workspace: &Path, provider: &ScriptedProvider) -> Vec<String> {
    let mut violations = Vec::new();
    for invariant in case["invariants"].as_array().into_iter().flatten() {
        if let Some(file) = invariant
            .get("workspace_file_absent")
            .and_then(Value::as_str)
            && workspace.join(file).exists()
        {
            violations.push(format!(
                "`{file}` exists: a tool side effect ran that the scenario forbids"
            ));
        }
        if let Some(file) = invariant
            .get("workspace_file_present")
            .and_then(Value::as_str)
            && !workspace.join(file).exists()
        {
            violations.push(format!(
                "`{file}` is gone: a tool side effect ran that the scenario forbids"
            ));
        }
        if let Some(max) = invariant.get("max_model_requests").and_then(Value::as_u64)
            && provider.stream_requests() as u64 > max
        {
            violations.push(format!(
                "{} model requests were issued; at most {max} allowed",
                provider.stream_requests()
            ));
        }
    }
    violations
}

fn run_case(name: &str, case: &Value, failures: &mut Failures) {
    let sandbox = Sandbox::new(case);
    let workspace = sandbox.workspace.clone();
    let (record, provider) = run_scripted_turn(&sandbox, case, &case["provider_script"]);
    let mut masker = sandbox.masker(VOLATILE_KEYS);
    let mut lines = normalize_events(&record.events, &mut masker);
    let mut summary = json!({
        "model_requests": provider.stream_requests(),
        "non_streaming_requests": provider.other_requests(),
        "workspace_after": workspace_listing(&workspace),
    });
    if record.timed_out {
        summary["harness_timeout"] =
            json!(format!("no engine event within {:?}", event_deadline()));
    }
    lines.push(json!({ "harness_summary": summary }));

    let violations = check_invariants(case, &workspace, &provider);
    let expected_change = case.get("expected_to_change");
    match (violations.is_empty(), expected_change) {
        (false, None) => {
            failures.push(name, format!("invariant broken: {}", violations.join("; ")))
        }
        (false, Some(change)) => eprintln!(
            "conformance: `{name}` pins current behavior that {} is changing ({}): {}",
            change["owner"].as_str().unwrap_or("an audit slice"),
            change["finding"].as_str().unwrap_or("unnamed finding"),
            violations.join("; ")
        ),
        (true, Some(change)) if !golden::update_mode() => failures.push(
            name,
            format!(
                "the invariant now holds, so {} has landed: re-record this golden with {}=1 \
                 and delete `expected_to_change` from the case",
                change["finding"].as_str().unwrap_or("the expected change"),
                golden::UPDATE_ENV
            ),
        ),
        _ => {}
    }

    failures.record(
        name,
        golden::check_golden(
            &golden::family_dir(FAMILY).join(format!("{name}.golden.jsonl")),
            &golden::jsonl(&lines),
        ),
    );
}

#[test]
fn golden_turn_events_match() {
    let names = golden::case_names(FAMILY);
    let mut failures = Failures::default();
    for name in &names {
        let case = golden::read_case(FAMILY, name);
        run_case(name, &case, &mut failures);
    }
    failures.finish(FAMILY, names.len());
}
