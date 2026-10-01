//! Recorded-MCP family: one transcript, every dispatch implementation.
//!
//! A transcript (`<case>.case.json`) scripts an MCP server — what it answers
//! to `initialize`, `tools/list`, `resources/*`, `prompts/*`, and to each
//! `tools/call` (a result, an `isError` result, a JSON-RPC error, progress
//! notifications before the result, or no answer at all until the caller
//! cancels) — plus the steps a host takes against it. The harness serves the
//! transcript as a real Streamable HTTP MCP server on loopback, so any client
//! that can open a URL can be pointed at it, including a child Node process.
//!
//! [`DISPATCHES`] is the seam for the TypeScript migration
//! (TS-EXTENSION-HOST-DESIGN §5.2, §9.3): today it holds the Rust pool path
//! production uses (`Engine::execute_mcp_tool_with_pool`); `HostMcpDispatch`
//! joins it in Phase 2 and must produce the *same* golden, because the
//! golden does not name the dispatch. Production MCP code is not touched.
//!
//! Normalization: the server URL/port is masked; results are
//! `{"ok": {success, content, metadata, content_blocks}}` with JSON content
//! parsed, or `{"err": {kind, detail}}` with exact detail bytes.
//! `server_received` lists the side-effecting requests the server saw
//! (`tools/call`, `resources/read`, `prompts/get`) — a call that must not be
//! sent, or must not be replayed, shows up there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::golden::{self, Failures, Sandbox};
use crate::core::engine::Engine;
use crate::core::events::Event;
use crate::mcp::{McpConfig, McpPool};
use crate::tools::spec::{RichToolResult, ToolError};

const FAMILY: &str = "mcp";
const CALL_DEADLINE: Duration = Duration::from_secs(30);
/// A held `tools/call` is released after this long even if nobody cancels,
/// so a dispatch that ignores cancellation fails the case instead of hanging.
const HOLD_LIMIT: Duration = Duration::from_secs(20);

// === The dispatch seam ===

/// One implementation of "call a tool on an external MCP server", as the
/// engine sees it. Planning, hooks and approval happen before `call`;
/// implementors never gate (design §5.2).
#[async_trait::async_trait]
trait McpDispatchUnderTest: Send + Sync {
    /// Connect to every configured server.
    async fn boot(&self) -> Result<(), String>;
    /// The model-visible tool catalog this dispatch advertises.
    async fn catalog(&self) -> Vec<codewhale_models::Tool>;
    /// Call one model-facing tool; `cancel` is the turn interrupt.
    async fn call(
        &self,
        model_name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<RichToolResult, ToolError>;
    async fn shutdown(&self);
}

#[tokio::test]
async fn harness_timeout_rejects_a_real_unanswered_mcp_call() {
    let mut case = golden::read_case(FAMILY, "tools_resources_prompts");
    let held = case["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["cancel"] == "after_server_holds")
        .expect("held-call fixture")
        .clone();
    let mut held = held;
    held.as_object_mut().expect("step").remove("cancel");
    case["steps"] = json!([held]);
    let _sandbox = Sandbox::new(&Value::Null);
    let outcome =
        run_transcript_with_deadline(&case, McpPoolDispatch::boxed, Duration::from_secs(1)).await;
    let mut failures = Failures::default();
    match outcome {
        Err(error) => failures.push("unanswered_call", error),
        Ok(_) => panic!("an unanswered MCP call became recordable output"),
    }
    assert!(failures.contains("harness timeout: MCP call"));
}

type DispatchFactory = fn(config: McpConfig) -> Box<dyn McpDispatchUnderTest>;

/// Every dispatch runs every transcript against the same golden.
const DISPATCHES: &[(&str, DispatchFactory)] = &[("mcp_pool", McpPoolDispatch::boxed)];

/// The Rust path production uses today: the shared pool behind the engine's
/// direct MCP execution seam.
struct McpPoolDispatch {
    pool: Arc<AsyncMutex<McpPool>>,
    tx_event: mpsc::Sender<Event>,
    // Kept open so status events the dispatch emits never hit a closed channel.
    _rx_event: Mutex<mpsc::Receiver<Event>>,
}

impl McpPoolDispatch {
    fn boxed(config: McpConfig) -> Box<dyn McpDispatchUnderTest> {
        let (tx_event, rx_event) = mpsc::channel(64);
        Box::new(Self {
            pool: Arc::new(AsyncMutex::new(McpPool::new(config))),
            tx_event,
            _rx_event: Mutex::new(rx_event),
        })
    }
}

#[async_trait::async_trait]
impl McpDispatchUnderTest for McpPoolDispatch {
    async fn boot(&self) -> Result<(), String> {
        let errors = self.pool.lock().await.connect_all().await;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors
                .iter()
                .map(|(name, error)| format!("{name}: {error:#}"))
                .collect::<Vec<_>>()
                .join("; "))
        }
    }

    async fn catalog(&self) -> Vec<codewhale_models::Tool> {
        self.pool.lock().await.to_api_tools()
    }

    async fn call(
        &self,
        model_name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<RichToolResult, ToolError> {
        // The engine interrupts a tool by dropping its future; do the same.
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(ToolError::cancelled("tool call interrupted by the host")),
            result = Engine::execute_mcp_tool_with_pool(
                Arc::clone(&self.pool),
                &self.tx_event,
                model_name,
                input,
                &[],
                // Conformance probes cannot supply a person's card decision.
                None,
            ) => result,
        }
    }

    async fn shutdown(&self) {
        self.pool.lock().await.shutdown_all().await;
    }
}

// === The transcript server ===

struct TranscriptServer {
    url: String,
    addr: String,
    received: Arc<Mutex<Vec<Value>>>,
    held: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TranscriptServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TranscriptServer {
    async fn start(spec: Value) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind transcript server");
        let addr = listener.local_addr().expect("addr").to_string();
        let url = format!("http://{addr}/mcp");
        let received = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(Notify::new());
        let spec = Arc::new(spec);
        let (log, notify) = (Arc::clone(&received), Arc::clone(&held));
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = connections.join_next(), if !connections.is_empty() => continue,
                };
                let Ok((socket, _)) = accepted else {
                    break;
                };
                let (spec, log, notify) =
                    (Arc::clone(&spec), Arc::clone(&log), Arc::clone(&notify));
                connections.spawn(async move {
                    answer(socket, &spec, &log, &notify).await;
                });
            }
        });
        Self {
            url,
            addr,
            received,
            held,
            task,
        }
    }
}

async fn answer(
    socket: tokio::net::TcpStream,
    spec: &Value,
    log: &Mutex<Vec<Value>>,
    held: &Notify,
) {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let method = line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let mut content_length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if reader.read_exact(&mut body).await.is_err() {
        return;
    }
    let mut socket = reader.into_inner();
    if method != "POST" {
        // No server-initiated stream: the spec's answer for a GET.
        let _ = socket
            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nAllow: POST\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        let _ = socket
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return;
    };
    let rpc_method = message["method"].as_str().unwrap_or_default().to_string();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = message.get("id").cloned() else {
        // Notifications (initialized, cancelled, progress) are accepted.
        let _ = socket
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    match rpc_method.as_str() {
        "tools/call" => log.lock().expect("log").push(json!({
            "method": "tools/call",
            "name": params["name"],
            "arguments": params.get("arguments").cloned().unwrap_or(Value::Null),
        })),
        "resources/read" => log
            .lock()
            .expect("log")
            .push(json!({ "method": "resources/read", "uri": params["uri"] })),
        "prompts/get" => log
            .lock()
            .expect("log")
            .push(json!({ "method": "prompts/get", "name": params["name"] })),
        _ => {}
    }

    let entry = scripted_entry(spec, &rpc_method, &params);
    if entry.get("hold").and_then(Value::as_bool) == Some(true) {
        held.notify_one();
        // Answer nothing until the client goes away (or the hold limit).
        let mut byte = [0u8; 1];
        let _ = tokio::time::timeout(HOLD_LIMIT, socket.read(&mut byte)).await;
        return;
    }
    let reply = match entry.get("error") {
        Some(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        None => json!({ "jsonrpc": "2.0", "id": id, "result": entry["result"] }),
    };
    let session = if rpc_method == "initialize" {
        "Mcp-Session-Id: conformance-session\r\n"
    } else {
        ""
    };
    let progress = entry.get("progress").and_then(Value::as_array);
    let (content_type, payload) = match progress {
        Some(steps) => {
            let token = params
                .pointer("/_meta/progressToken")
                .cloned()
                .unwrap_or_else(|| json!("conformance"));
            let mut sse = String::new();
            for step in steps {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": { "progressToken": token, "progress": step["progress"], "total": step["total"] },
                });
                sse.push_str(&format!("event: message\ndata: {notification}\n\n"));
            }
            sse.push_str(&format!("event: message\ndata: {reply}\n\n"));
            ("text/event-stream", sse)
        }
        None => ("application/json", reply.to_string()),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The transcript's answer for one request: a fixed entry per list method,
/// or the first `match`ing entry for calls. Unknown methods get -32601.
fn scripted_entry(spec: &Value, method: &str, params: &Value) -> Value {
    let not_found =
        || json!({ "error": { "code": -32601, "message": format!("method not found: {method}") } });
    let Some(entry) = spec.get(method) else {
        return not_found();
    };
    let Some(candidates) = entry.as_array() else {
        return entry.clone();
    };
    candidates
        .iter()
        .find(|candidate| {
            candidate["match"]
                .as_object()
                .is_none_or(|fields| fields.iter().all(|(key, value)| params.get(key) == Some(value)))
        })
        .cloned()
        .unwrap_or_else(|| json!({ "error": { "code": -32602, "message": format!("no scripted {method} answer for {params}") } }))
}

// === Running a transcript ===

fn normalize_call(result: &Result<RichToolResult, ToolError>) -> Value {
    match result {
        Ok(rich) => json!({ "ok": {
            "success": rich.result.success,
            "content": serde_json::from_str::<Value>(&rich.result.content)
                .unwrap_or_else(|_| Value::String(rich.result.content.clone())),
            "metadata": rich.result.metadata,
            "content_blocks": rich.content_blocks,
        } }),
        Err(error) => {
            json!({ "err": { "kind": golden::tool_error_kind(error), "detail": error.to_string() } })
        }
    }
}

async fn run_transcript(
    case: &Value,
    factory: DispatchFactory,
) -> Result<(Value, TranscriptServer), String> {
    run_transcript_with_deadline(case, factory, CALL_DEADLINE).await
}

async fn run_transcript_with_deadline(
    case: &Value,
    factory: DispatchFactory,
    deadline: Duration,
) -> Result<(Value, TranscriptServer), String> {
    let scripted_steps = case["steps"].as_array().expect("case.steps");
    if scripted_steps.is_empty() {
        return Err("MCP transcript has no host steps".to_string());
    }
    let server = TranscriptServer::start(case["server"].clone()).await;
    let server_name = case["server_name"].as_str().unwrap_or("conformance");
    let config: McpConfig = serde_json::from_value(json!({
        "servers": { server_name: {
            "url": server.url,
            "connect_timeout": 10,
            "execute_timeout": 10,
        } }
    }))
    .expect("transcript MCP config");
    let dispatch = factory(config);
    let mut steps = Vec::new();
    let boot = match golden::complete_within("MCP boot", deadline, dispatch.boot()).await? {
        Ok(()) => json!("ok"),
        Err(detail) => return Err(format!("MCP transcript could not boot: {detail}")),
    };
    steps.push(json!({ "op": "boot", "outcome": boot }));
    for step in scripted_steps {
        match step["op"].as_str().expect("step.op") {
            "catalog" => {
                let catalog = serde_json::to_value(
                    golden::complete_within("MCP catalog", deadline, dispatch.catalog()).await?,
                )
                .expect("catalog");
                steps.push(json!({ "op": "catalog", "tools": catalog }));
            }
            "call" => {
                let tool = step["tool"].as_str().expect("step.tool");
                let cancel = CancellationToken::new();
                let canceller =
                    (step["cancel"].as_str() == Some("after_server_holds")).then(|| {
                        let (held, cancel) = (Arc::clone(&server.held), cancel.clone());
                        tokio::spawn(async move {
                            held.notified().await;
                            cancel.cancel();
                        })
                    });
                let outcome = golden::complete_within(
                    "MCP call",
                    deadline,
                    dispatch.call(tool, step["input"].clone(), cancel),
                )
                .await;
                if let Some(canceller) = canceller {
                    canceller.abort();
                }
                let outcome = normalize_call(&outcome?);
                steps.push(json!({ "op": "call", "tool": tool, "outcome": outcome }));
            }
            other => panic!("unknown transcript op `{other}`"),
        }
    }
    golden::complete_within("MCP shutdown", deadline, dispatch.shutdown()).await?;
    let received = server.received.lock().expect("log").clone();
    Ok((
        json!({ "steps": steps, "server_received": received }),
        server,
    ))
}

#[test]
fn mcp_transcripts_match_goldens_for_every_dispatch() {
    let dir = golden::family_dir(FAMILY);
    let names = golden::case_names(FAMILY);
    let mut failures = Failures::default();
    for name in &names {
        let case = golden::read_case(FAMILY, name);
        for (dispatch, factory) in DISPATCHES {
            // The pool may touch its state directory; keep that hermetic.
            let sandbox = Sandbox::new(&Value::Null);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let (mut outcome, server) = match runtime.block_on(run_transcript(&case, *factory)) {
                Ok(completed) => completed,
                Err(error) => {
                    failures.push(&format!("{name} via {dispatch}"), error);
                    continue;
                }
            };
            let mut masker = sandbox
                .masker(&[])
                .literal(&server.url, "<SERVER_URL>")
                .literal(&server.addr, "<SERVER_ADDR>");
            drop(server);
            drop(runtime);
            masker.value(&mut outcome);
            failures.record(
                &format!("{name} via {dispatch}"),
                golden::check_golden(
                    &dir.join(format!("{name}.golden.json")),
                    &golden::pretty(&golden::canonical(&outcome)),
                ),
            );
        }
    }
    failures.finish(FAMILY, names.len() * DISPATCHES.len());
}
