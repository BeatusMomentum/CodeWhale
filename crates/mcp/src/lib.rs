//! Bounded, inert MCP server guidance shared by the active TUI transports.
//!
//! The legacy client pool and aggregating stdio proxy were removed in Phase 0.
//! Client/session authority stays in the existing TUI MCP pool; native server
//! mode uses its shared Rust tool registry through `serve --mcp`.

use serde_json::Value;

/// Upper bound, in bytes, on the `instructions` an MCP server may supply at
/// `initialize`. Longer guidance is cut on a character boundary and ends with
/// [`SERVER_INSTRUCTIONS_TRUNCATED_MARKER`].
pub const MAX_SERVER_INSTRUCTIONS_BYTES: usize = 4096;
/// Appended to guidance cut at [`MAX_SERVER_INSTRUCTIONS_BYTES`].
pub const SERVER_INSTRUCTIONS_TRUNCATED_MARKER: &str = "\n[truncated]";

/// Normalize the optional `instructions` string of an MCP `initialize` result.
///
/// The spec defines it as free-form usage guidance from the server. It is
/// third-party text, so it is kept only in a bounded, inert form: a non-string
/// value is ignored (with a debug log) rather than failing the handshake,
/// control and bidirectional-override characters other than newline and tab
/// are dropped, surrounding whitespace is trimmed, empty guidance becomes
/// `None`, and anything past [`MAX_SERVER_INSTRUCTIONS_BYTES`] is truncated
/// with an explicit marker.
#[must_use]
pub fn sanitize_server_instructions(server_name: &str, value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::Null => return None,
        Value::String(text) => text,
        other => {
            let kind = match other {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::Array(_) => "array",
                _ => "object",
            };
            tracing::debug!(
                server = server_name,
                kind,
                "ignoring non-string MCP initialize instructions"
            );
            return None;
        }
    };
    let cleaned: String = raw
        .chars()
        .filter(|ch| {
            matches!(ch, '\n' | '\t')
                || !(ch.is_control()
                    || matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= MAX_SERVER_INSTRUCTIONS_BYTES {
        return Some(trimmed.to_string());
    }
    let mut end = MAX_SERVER_INSTRUCTIONS_BYTES - SERVER_INSTRUCTIONS_TRUNCATED_MARKER.len();
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!(
        "{}{SERVER_INSTRUCTIONS_TRUNCATED_MARKER}",
        trimmed[..end].trim_end()
    ))
}
