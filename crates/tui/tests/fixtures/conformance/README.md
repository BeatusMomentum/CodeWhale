# Conformance fixtures

The gate for the Rust → TypeScript edge migration (`CURRENT_DECISIONS.md` §26;
TS-EXTENSION-HOST-DESIGN §9.3). A subsystem that moves to the extension host —
MCP client, hook orchestration, script tools, tool adapters, slash commands —
flips its flag only after it reproduces these goldens on these fixtures. The
goldens are what the Rust implementation does today; they are not a spec
written in the abstract.

Everything here is language-neutral: JSON, JSONL, raw SSE bytes, and POSIX
shell. No fixture was captured from a real provider or contains a secret.

The Rust runner is `crates/tui/src/conformance/` (one test per family):

```sh
cargo test -p codewhale-tui --lib -- conformance::
# Re-record after an intended change, then review the diff like source:
CODEWHALE_CONFORMANCE_UPDATE=1 cargo test -p codewhale-tui --lib -- conformance::
```

Default is compare-only. Update mode rewrites drifted goldens and refuses to
run when `CI` is set. A golden change is a behavior change and is reviewed as
one. Harness deadlines, incomplete turns, broken invariants, and empty cases
are failures in both modes and cannot be recorded as goldens. Provider timeout
errors are legitimate observable outcomes; a harness timing out is missing
evidence.

Platforms: `sse` and `mcp` run everywhere. `events`, `prompt` and `hooks`
goldens were recorded on Unix and compile only there — hook fixtures are POSIX
shell, and a Windows turn differs in shell and path facts no golden covers
yet. Recording Windows goldens is open work, not an implied capability.

## Families

Each case is `<family>/<name>.case.json` (input, hand-written) plus one or
more `<name>.golden.*` files (output, recorded).

| family | input | production path | proves |
|---|---|---|---|
| `sse/` | `<name>.sse` recorded bytes + route | `CodewhaleClient::create_message_stream` via a loopback HTTP server | wire adapter: bytes → normalized stream events, incl. tool-call deltas, reasoning, usage, mid-stream errors, truncation |
| `events/` | scripted provider + host driver | `Engine::run` → `Engine::run_turn` | one turn → protocol `EventMsg` sequence + provider request count + workspace side effects |
| `mcp/` | scripted MCP server transcript + host steps | `McpPool` behind `Engine::execute_mcp_tool_with_pool` | catalog normalization and call results for one dispatch |
| `prompt/` | session config + workspace files | first `MessageRequest` of a real turn | model-visible prefix bytes (system prompt + tool catalog) |
| `hooks/` | user hook config + one tool call | `run_tool_call_before_hooks` / `HookExecutor::execute` | hook verdict fold, env contract, schema-1 stdin |

The host protocol corpus that Rust serde and the TypeScript host both parse
already lives in `../extension_host/protocol/`; it is not duplicated here.

## Normalized formats

### Stream events (`sse` goldens, `events` provider scripts)

One JSON object per event in exactly the serde shape of
`codewhale_models::StreamEvent` — the Anthropic Messages event vocabulary:
`message_start`, `content_block_start` (`text` / `thinking` / `tool_use` /
`server_tool_use`), `content_block_delta` (`text_delta` / `thinking_delta` /
`input_json_delta` / `signature_delta` / `reasoning_state_delta`),
`content_block_stop`, `message_delta` (stop reason + usage), `message_stop`,
`ping`, `error`, `tool_projection_warning`. The Rust runner proves every golden
event line deserializes into `StreamEvent` and serializes back unchanged, so an
`sse` golden can be pasted into an `events` script.

An `sse` golden's first line is `{"request": [{"method", "path"}]}` — the
request the adapter sent. A stream that errors yields
`{"type": "stream_failure", "detail": …}`; a request that fails before a
stream yields `{"type": "open_failure", "detail": …}`.

### Turn events (`events` goldens)

Each line is `codewhale_protocol::EventMsg` as serialized (`"event"` tag), with:

- the per-session `thread_id` / `session_id` envelope removed;
- `tool_call_heartbeat` dropped (a liveness pulse, timing-dependent);
- UUIDs → `<uuid:N>` numbered by first appearance; RFC 3339 timestamps →
  `<timestamp>`; the local date `<turn_meta>` states → `<today>`; `created_at`, `duration_ms`, `first_token_ms`, `request_ms`,
  `elapsed_ms`, `pinned_combined_hash` → `"<masked>"`; temp paths →
  `<WORKSPACE>` / `<HOME>` / `<TMP>`;
- `tool_catalog` and `system_prompt` bodies → `"<pinned by the prompt family>"`;
- runs of adjacent `tool_call_complete` events ordered by `tool_call_id`
  (parallel completions race);
- keys sorted.

The last line is `{"harness_summary": {model_requests, non_streaming_requests,
workspace_after}}`: how many provider requests the turn made and every file
left in the workspace (path → sha256 prefix). `invariants` in a case are
checked independently of the golden (`workspace_file_absent`,
`workspace_file_present`, `max_model_requests`).

`events/provider_error_after_tool_call` requires the C02-05 (#6561) authority:
a failed response executes no collected tool call and issues no retry. Its
invariants are checked before recording, with no exemption for known defects.

### MCP transcripts (`mcp` goldens)

The case's `server` object scripts a Streamable HTTP MCP server: one answer
per list method (`initialize`, `tools/list`, `resources/list`,
`resources/templates/list`, `prompts/list`) and, for `tools/call`,
`resources/read` and `prompts/get`, a list of `{"match": {…params}, …}`
entries answering with `result`, `error`, `progress` (notifications sent as
SSE before the result), or `hold` (never answer until the client goes away).
Notifications get `202`, a `GET` gets `405`, unknown methods get `-32601`.

`steps` are what a host does: `catalog`, or `call` a model-facing tool name
with `input` (optionally `"cancel": "after_server_holds"`). The golden records
`boot`, the catalog (`codewhale_models::Tool` list), each call as
`{"ok": {success, content, metadata, content_blocks}}` (JSON content parsed) or
`{"err": {kind, detail}}`, and `server_received`: every `tools/call`,
`resources/read` and `prompts/get` the server actually saw. The server URL is
masked. The golden does not name the dispatch: every dispatch must produce the
same file.

### Prompt bytes (`prompt` goldens)

`<name>.golden.json` holds sha256 of the system prompt JSON, of the tool
catalog JSON, and of both (`prefix_sha256`), with sizes and tool names in
catalog order; `.system.golden.txt` and `.tools.golden.json` are the readable
bodies. Bytes are hashed as serialized — key order is part of the cached
prefix. Masks: temp paths, and the environment block's `- platform:` and
`- shell:` lines. Tools admitted only after probing the host for a binary
(`code_execution`, `image_ocr`, `js_execution`, `pandoc_convert`) are removed
from the pinned catalog and pinned individually in
`host_probed_tools.golden.json`, checked whenever the probe succeeds. The
runner also fails if two identical sessions produce different prefixes.

### Hook receipts (`hooks` goldens)

`hooks` is user configuration in the `[[hooks.hooks]]` shape. `{{capture}} NAME`
in a command runs a helper that saves the hook's stdin and every documented
environment variable (`CODEWHALE_*` / `DEEPSEEK_*`, see `HOOK_ENV_CONTRACT`).
The golden records the outcome — `{"admit": {requires_approval, updated_input,
additional_context}}` or `{"refuse": {kind, detail}}` for `tool_call_before`,
the observer results (including exact stdout, stderr and error) for
`tool_call_after` — and, per hook, whether it ran,
its stdin (the schema-1 document, parsed) and its contract environment. The
hook session id and temp paths are masked. POSIX shell: Unix only.

## Running a TypeScript implementation against these fixtures

A TS implementation proves parity by producing byte-identical normalized
output — same masks, same key order (sorted, except prompt bytes) — for every
case, and by passing the Rust runner once it is wired in:

- **MCP (Phase 2).** Implement `HostMcpDispatch` and add it to `DISPATCHES` in
  `crates/tui/src/conformance/mcp.rs`. The transcript server is a real loopback
  URL, so the Node host connects to it with the official SDK exactly as it
  would to a user's server. The same `mcp/*.golden.json` must pass unchanged.
- **Hooks (Phase 2).** Run the hook orchestration under test with each case's
  `hooks` and tool call; the `{{capture}}` helper is plain `sh`, so the
  captured stdin/env must match the golden without changes.
- **Tool adapters, slash commands, MCP catalog (Phases 2–3).** Whatever moves,
  `prompt/*` must not change unless the change is intended and re-recorded in
  the same PR — a prefix change invalidates every user's KV cache.
- **Provider wire adapters (Phase 4, gated).** Replay each `sse/*.sse` with its
  case's chunking/framing and emit the stream-event format above.
- **Turn events.** The turn loop stays in Rust (D10); `events/*` is the
  regression net around it while its edges move. A host-side consumer of
  `EventMsg` can read these goldens as its contract.

Fields named `detail` carry human-readable error text and are compared exactly,
including by a second implementation. Only the explicitly documented masks
apply. No whitespace, line-ending, error-text, or event-order normalization may
be added to make a drift pass. `.gitattributes` pins fixture checkout bytes to LF.
