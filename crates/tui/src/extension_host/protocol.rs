//! Extension-host protocol v1 — the phase-1 subset, and its source of truth.
//!
//! Frame: 4-byte magic `CWX1`, u32 little-endian payload length, UTF-8 JSON.
//! Envelope: JSON-RPC 2.0. A length prefix (not NDJSON) makes a plugin that
//! writes raw bytes to the channel detectable: bad magic or length ends the
//! host instead of desynchronising it.
//!
//! Host→core types use `deny_unknown_fields` — host output is untrusted
//! input. Core→host types are what this side writes.
//!
//! **This file is the protocol's single source.** [`METHODS`] is every method
//! either side may send; both parsers admit nothing else. The TypeScript
//! side's constants, method table, params shapes and wire types are generated
//! from the types here into `crates/tui/extension-host/src/protocol.generated.ts`
//! by `protocol/tests.rs`, which fails when the committed file drifts. What
//! stays hand-written in `protocol.ts`: the frame codec, the JSON-RPC envelope
//! checks, and the one rule [`parse_host_message`] applies beyond the types
//! (`host/hello`'s runtime name). Both sides also parse the shared corpus in
//! `tests/fixtures/extension_host/protocol`. Known limit: result shapes are
//! generated as TypeScript types only; the host does not validate the
//! results the core sends it, and the core validates what it reads.
//!
//! There is deliberately no method that expresses approval, and nothing a
//! host can send makes the core *do* anything in phase 1: registrations are
//! admitted or refused, and tool calls only flow core→host after the gate.
//! The authority lint in `protocol/tests.rs` keeps [`METHODS`] that way.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAGIC: [u8; 4] = *b"CWX1";
pub const HEADER_LEN: usize = 8;
/// Enough for base64 screenshots later; anything larger is refused, never truncated.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;
/// Requests in flight per direction.
pub const MAX_INFLIGHT: usize = 256;

/// JSON-RPC error codes used on this channel.
pub mod error_code {
    pub const INVALID_PARAMS: i64 = -32602;
    /// Unknown, revoked, or not-yet-active handle.
    pub const NOT_AVAILABLE: i64 = -32001;
    /// Cancelled by `$/cancel`.
    pub const CANCELLED: i64 = -32800;

    /// Every code on the channel, by the name the TypeScript side uses. The
    /// core interprets only the three above; the host also answers with the
    /// standard JSON-RPC codes and `ExecutionFailed` (a tool body threw),
    /// which reach the caller as `HostCallError::Rpc`.
    #[cfg(test)]
    pub const ALL: &[(&str, i64)] = &[
        ("ParseError", -32700),
        ("InvalidRequest", -32600),
        ("MethodNotFound", -32601),
        ("InvalidParams", INVALID_PARAMS),
        ("Internal", -32603),
        ("ExecutionFailed", -32000),
        ("NotAvailable", NOT_AVAILABLE),
        ("Cancelled", CANCELLED),
    ];
}

/// Which side sends a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    CoreToHost,
    HostToCore,
}

impl Direction {
    /// The corpus and TypeScript spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoreToHost => "core_to_host",
            Self::HostToCore => "host_to_core",
        }
    }
}

/// One method either side may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub direction: Direction,
    /// A request carries an id and gets a response; a notification does not.
    pub request: bool,
}

const fn row(direction: Direction, name: &'static str, request: bool) -> MethodSpec {
    MethodSpec {
        name,
        direction,
        request,
    }
}

/// The whole protocol surface. A method missing here is refused by both
/// parsers; a method added here must pass the authority lint.
pub const METHODS: &[MethodSpec] = &[
    row(Direction::CoreToHost, "host/initialize", true),
    row(Direction::CoreToHost, "host/ping", true),
    row(Direction::CoreToHost, "host/shutdown", true),
    row(Direction::CoreToHost, "ext/activate", true),
    row(Direction::CoreToHost, "ext/deactivate", true),
    row(Direction::CoreToHost, "tool/call", true),
    row(Direction::CoreToHost, "$/cancel", false),
    row(Direction::HostToCore, "host/hello", false),
    row(Direction::HostToCore, "host/ready", false),
    row(Direction::HostToCore, "registry/register", true),
    row(Direction::HostToCore, "registry/unregister", true),
    row(Direction::HostToCore, "ext/faulted", false),
    row(Direction::HostToCore, "log", false),
    row(Direction::HostToCore, "$/cancel", false),
];

/// Admit `method` travelling in `direction` from [`METHODS`]: anything not
/// in the table is refused, a request must carry an id and a notification
/// must not. Returns the id.
fn admit(
    direction: Direction,
    method: &str,
    id: Option<u64>,
) -> Result<Option<u64>, ProtocolError> {
    let spec = METHODS
        .iter()
        .find(|spec| spec.direction == direction && spec.name == method)
        .ok_or_else(|| perr(format!("unknown {} method `{method}`", direction.as_str())))?;
    match (spec.request, id) {
        (true, Some(_)) | (false, None) => Ok(id),
        (true, None) => Err(perr(format!("`{method}` must be a request (with id)"))),
        (false, Some(_)) => Err(perr(format!("`{method}` must be a notification (no id)"))),
    }
}

fn undecoded(method: &str) -> ProtocolError {
    perr(format!(
        "`{method}` is in the method table but has no decoder"
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("bad frame magic")]
    BadMagic,
    #[error("frame of {0} bytes exceeds MAX_FRAME")]
    TooLarge(usize),
    #[error("frame payload is not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("channel read failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProtocolError(pub String);

fn perr(message: impl Into<String>) -> ProtocolError {
    ProtocolError(message.into())
}

/// Encode one message into a `CWX1` frame.
pub fn encode_frame(message: &Value) -> Result<Vec<u8>, FrameError> {
    let payload = serde_json::to_vec(message)?;
    if payload.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&MAGIC);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Read one frame. `Ok(None)` is a clean EOF at a frame boundary. The payload
/// buffer is allocated only after the length is checked against `MAX_FRAME`.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Value>, FrameError> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        let read = reader.read(&mut header[filled..]).await?;
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()));
        }
        filled += read;
    }
    if header[..4] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge(length));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(serde_json::from_slice(&payload)?))
}

/// `Some(value)` even for an explicit JSON `null`, so `"result": null` is a
/// present result and round-trips.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Identity of one activation. `generation` bumps on every (re)activation;
/// `owner_token` is 128+ random bits minted by the core and handed over only
/// in `ext/activate`. It catches bugs and stale fibers; it is not a boundary
/// against a malicious plugin in the same process (see the design, §4.4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct OwnerRef {
    pub plugin_id: String,
    pub generation: u64,
    pub owner_token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RpcErrorWire {
    pub code: i64,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub data: Option<Value>,
}

// ---------------------------------------------------------------------------
// Host → core (strict)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProtocolRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HelloParams {
    pub protocol: ProtocolRange,
    pub host_version: String,
    pub bundle_sha256: String,
    /// The runtime actually running the host. Under Bun this comes from
    /// `process.versions.bun`, not the Node version Bun emulates.
    pub runtime: HelloRuntime,
    /// A kernel memory limit the host applied to itself before loading any
    /// plugin, in MiB: macOS + Bun, when the core asked for one
    /// (`supervisor::MemoryEnforcement::Jetsam`). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_mib: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HelloRuntime {
    /// `bun` or `node`.
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RegisterKind {
    /// The only kind in phase 1.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolSpecWire {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RegisterParams {
    pub owner: OwnerRef,
    pub kind: RegisterKind,
    pub spec: ToolSpecWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct UnregisterParams {
    pub owner: OwnerRef,
    pub handle: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FaultedParams {
    pub owner: OwnerRef,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LogParams {
    pub level: String,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CancelParams {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostRequest {
    Register(RegisterParams),
    Unregister(UnregisterParams),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostNotification {
    Hello(HelloParams),
    Ready,
    Faulted(FaultedParams),
    Log(LogParams),
    Cancel(CancelParams),
}

/// One decoded, validated host→core message.
#[derive(Debug, Clone, PartialEq)]
pub enum HostMessage {
    Request {
        id: u64,
        request: HostRequest,
    },
    Notification(HostNotification),
    Response {
        id: u64,
        outcome: Result<Value, RpcErrorWire>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    jsonrpc: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default, deserialize_with = "present")]
    params: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcErrorWire>,
}

fn decode_envelope(value: Value) -> Result<Envelope, ProtocolError> {
    let envelope: Envelope =
        serde_json::from_value(value).map_err(|e| perr(format!("envelope: {e}")))?;
    if envelope.jsonrpc != "2.0" {
        return Err(perr("envelope: jsonrpc must be \"2.0\""));
    }
    Ok(envelope)
}

/// Params are always named: an array would otherwise decode positionally
/// into a struct (`[]` into [`EmptyParams`]), which the TypeScript side
/// rejects.
fn params<T: DeserializeOwned>(method: &str, params: Option<Value>) -> Result<T, ProtocolError> {
    let params = params.unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Err(perr(format!("{method}: params must be an object")));
    }
    serde_json::from_value(params).map_err(|e| perr(format!("{method}: {e}")))
}

fn decode_response(
    envelope: Envelope,
) -> Result<(u64, Result<Value, RpcErrorWire>), ProtocolError> {
    let id = envelope.id.ok_or_else(|| perr("response: missing id"))?;
    if envelope.params.is_some() {
        return Err(perr("response: unexpected params"));
    }
    match (envelope.result, envelope.error) {
        (Some(result), None) => Ok((id, Ok(result))),
        (None, Some(error)) => Ok((id, Err(error))),
        _ => Err(perr("response: needs exactly one of result or error")),
    }
}

/// Parse and strictly validate one host→core message.
pub fn parse_host_message(value: Value) -> Result<HostMessage, ProtocolError> {
    let envelope = decode_envelope(value)?;
    let Some(method) = envelope.method.clone() else {
        let (id, outcome) = decode_response(envelope)?;
        return Ok(HostMessage::Response { id, outcome });
    };
    if envelope.result.is_some() || envelope.error.is_some() {
        return Err(perr(format!(
            "`{method}`: a request carries no result or error"
        )));
    }
    let id = admit(Direction::HostToCore, &method, envelope.id)?;
    let p = envelope.params;
    let message = match (method.as_str(), id) {
        ("registry/register", Some(id)) => HostMessage::Request {
            id,
            request: HostRequest::Register(params(&method, p)?),
        },
        ("registry/unregister", Some(id)) => HostMessage::Request {
            id,
            request: HostRequest::Unregister(params(&method, p)?),
        },
        ("host/hello", None) => {
            let hello: HelloParams = params(&method, p)?;
            if !matches!(hello.runtime.name.as_str(), "bun" | "node") {
                return Err(perr(format!(
                    "host/hello.runtime.name: unknown runtime `{}`",
                    hello.runtime.name
                )));
            }
            HostMessage::Notification(HostNotification::Hello(hello))
        }
        ("host/ready", None) => {
            let _: EmptyParams = params(&method, p)?;
            HostMessage::Notification(HostNotification::Ready)
        }
        ("ext/faulted", None) => {
            HostMessage::Notification(HostNotification::Faulted(params(&method, p)?))
        }
        ("log", None) => HostMessage::Notification(HostNotification::Log(params(&method, p)?)),
        ("$/cancel", None) => {
            HostMessage::Notification(HostNotification::Cancel(params(&method, p)?))
        }
        _ => return Err(undecoded(&method)),
    };
    Ok(message)
}

fn request_value(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification_value(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn response_value(id: u64, outcome: &Result<Value, RpcErrorWire>) -> Value {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("protocol types serialize")
}

#[cfg(test)]
impl HostMessage {
    /// Re-encode (used by the conformance corpus round-trip).
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Request { id, request } => match request {
                HostRequest::Register(p) => request_value(*id, "registry/register", to_value(p)),
                HostRequest::Unregister(p) => {
                    request_value(*id, "registry/unregister", to_value(p))
                }
            },
            Self::Notification(notification) => match notification {
                HostNotification::Hello(p) => notification_value("host/hello", to_value(p)),
                HostNotification::Ready => notification_value("host/ready", json!({})),
                HostNotification::Faulted(p) => notification_value("ext/faulted", to_value(p)),
                HostNotification::Log(p) => notification_value("log", to_value(p)),
                HostNotification::Cancel(p) => notification_value("$/cancel", to_value(p)),
            },
            Self::Response { id, outcome } => response_value(*id, outcome),
        }
    }
}

// ---------------------------------------------------------------------------
// Core → host (written by this side)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct HostLimits {
    pub max_frame: u64,
    pub max_inflight: u64,
    pub dispose_deadline_ms: u64,
    pub activate_deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct InitializeParams {
    pub protocol: u32,
    pub limits: HostLimits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct EntryRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ActivateParams {
    pub owner: OwnerRef,
    pub plugin_name: String,
    pub entry: EntryRef,
    #[serde(default = "empty_object")]
    pub config: Value,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DeactivateParams {
    pub owner: OwnerRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ToolCallParams {
    pub handle: u64,
    pub call_id: String,
    pub input: Value,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoreRequest {
    Ping,
    Initialize(InitializeParams),
    /// Production relies on stdin EOF at process exit (the host is shared by
    /// every engine in the process); the bounded shutdown is test-driven.
    #[cfg(test)]
    Shutdown,
    Activate(ActivateParams),
    Deactivate(DeactivateParams),
    ToolCall(ToolCallParams),
}

impl CoreRequest {
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Ping => "host/ping",
            Self::Initialize(_) => "host/initialize",
            #[cfg(test)]
            Self::Shutdown => "host/shutdown",
            Self::Activate(_) => "ext/activate",
            Self::Deactivate(_) => "ext/deactivate",
            Self::ToolCall(_) => "tool/call",
        }
    }

    #[must_use]
    pub fn params(&self) -> Value {
        match self {
            Self::Ping => json!({}),
            Self::Initialize(p) => to_value(p),
            #[cfg(test)]
            Self::Shutdown => json!({}),
            Self::Activate(p) => to_value(p),
            Self::Deactivate(p) => to_value(p),
            Self::ToolCall(p) => to_value(p),
        }
    }

    /// How long the core waits for this request's answer before it sends
    /// `$/cancel`, forgets the call and fails it with
    /// `HostCallError::Timeout` (`HostProcess::call`). The match is
    /// exhaustive, so no request can be added without a deadline.
    ///
    /// | method | deadline | why |
    /// |---|---|---|
    /// | `host/initialize` | `HANDSHAKE_DEADLINE` (30 s) | the whole handshake has the same budget |
    /// | `host/ping` | `PING_DEADLINE` (10 s) | the heartbeat supervises pings with its own `ping_timeout`/`hang_timeout` and kills a silent host; this bounds any other caller |
    /// | `host/shutdown` | 2 s | tests only |
    /// | `ext/activate` | `ACTIVATE_DEADLINE` + 1 s | the host enforces activation's own deadline; 1 s for its answer to arrive |
    /// | `ext/deactivate` | `DISPOSE_DEADLINE` + 500 ms | the same, for disposal |
    /// | `tool/call` | its `deadline_ms` | the host is told the bound the core enforces (`SupervisionOptions::tool_call_deadline`, 120 s) |
    #[must_use]
    pub fn deadline(&self) -> Duration {
        use super::supervisor::{
            ACTIVATE_DEADLINE, DISPOSE_DEADLINE, HANDSHAKE_DEADLINE, PING_DEADLINE,
        };
        match self {
            Self::Ping => PING_DEADLINE,
            Self::Initialize(_) => HANDSHAKE_DEADLINE,
            #[cfg(test)]
            Self::Shutdown => Duration::from_secs(2),
            Self::Activate(_) => ACTIVATE_DEADLINE + Duration::from_secs(1),
            Self::Deactivate(_) => DISPOSE_DEADLINE + Duration::from_millis(500),
            Self::ToolCall(params) => Duration::from_millis(params.deadline_ms),
        }
    }

    #[must_use]
    pub fn to_value(&self, id: u64) -> Value {
        request_value(id, self.method(), self.params())
    }
}

/// `registry/register` answer: a handle, or a refusal the host reports as a
/// failed activation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum RegisterResult {
    Admitted { handle: u64 },
    Refused { refused: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActivateResult {
    Ok { tools: Vec<String> },
    Failed { diagnostic: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct DeactivateResult {
    pub disposed: bool,
    pub leaked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockWire {
    Text { text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ToolResultWire {
    pub content: Vec<ContentBlockWire>,
    pub is_error: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub structured: Option<Value>,
}

/// One core→host message, parsed back (tests and the corpus only).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub enum CoreMessage {
    Request {
        id: u64,
        request: CoreRequest,
    },
    Cancel(CancelParams),
    Response {
        id: u64,
        outcome: Result<Value, RpcErrorWire>,
    },
}

/// Parse a core→host message (the shape this side writes).
#[cfg(test)]
pub fn parse_core_message(value: Value) -> Result<CoreMessage, ProtocolError> {
    let envelope = decode_envelope(value)?;
    let Some(method) = envelope.method.clone() else {
        let (id, outcome) = decode_response(envelope)?;
        return Ok(CoreMessage::Response { id, outcome });
    };
    let p = envelope.params;
    let Some(id) = admit(Direction::CoreToHost, &method, envelope.id)? else {
        return match method.as_str() {
            "$/cancel" => Ok(CoreMessage::Cancel(params(&method, p)?)),
            _ => Err(undecoded(&method)),
        };
    };
    let request = match method.as_str() {
        "host/initialize" => CoreRequest::Initialize(params(&method, p)?),
        "host/shutdown" => {
            let _: EmptyParams = params(&method, p)?;
            CoreRequest::Shutdown
        }
        "host/ping" => {
            let _: EmptyParams = params(&method, p)?;
            CoreRequest::Ping
        }
        "ext/activate" => CoreRequest::Activate(params(&method, p)?),
        "ext/deactivate" => CoreRequest::Deactivate(params(&method, p)?),
        "tool/call" => CoreRequest::ToolCall(params(&method, p)?),
        _ => return Err(undecoded(&method)),
    };
    Ok(CoreMessage::Request { id, request })
}

#[cfg(test)]
impl CoreMessage {
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Request { id, request } => request.to_value(*id),
            Self::Cancel(p) => notification_value("$/cancel", to_value(p)),
            Self::Response { id, outcome } => response_value(*id, outcome),
        }
    }
}

#[must_use]
pub fn cancel_value(id: u64) -> Value {
    notification_value("$/cancel", json!({ "id": id }))
}

#[must_use]
pub fn response_ok(id: u64, result: Value) -> Value {
    response_value(id, &Ok(result))
}

#[cfg(test)]
mod tests;
