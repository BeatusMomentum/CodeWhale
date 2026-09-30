# Writing an extension tool

The experimental TypeScript extension host runs reviewed plugin code in a
shared Node process. Rust still owns sessions, tool admission, approval and
execution. Extensions currently contribute tools; they cannot provide an
approval service, run a second agent loop or replace built-in tools.

## Quick start

Start with [hello-extension](examples/plugins/hello-extension/hello.mts) and its
[plugin.json](examples/plugins/hello-extension/plugin.json). The example is
executed unchanged by the host tests. It returns a greeting and does not read
files or use the network.

1. Explicitly enable `[features] extension_host = true` in your configuration.
2. Install the example's directory using `/plugin install <local-directory>`.
3. Run `/plugin validate hello-extension`, then `/plugin show hello-extension`.
4. Run `/plugin enable hello-extension` to open its content/capability review.
   Read the review and personally run its exact `/plugin trust <token>` command,
   then enable the plugin. Trust alone does not execute the code.
5. Ask Codewhale to use `hello_greet`. The tool follows the normal approval gate.

An authoring agent should stop after installation and validation and present
the review to the person. Do not trust or enable a bundle automatically.

## Locations, installation and trust

User bundles live under `~/.codewhale/plugins/`; workspace bundles live under
`<workspace>/.codewhale/plugins/`. New bundles use Agent Plugins v1.0.0
`plugin.json`. Declare `extensions["net.codewhale"].native.path` as one regular
`.mjs`, `.js` or `.mts` file inside the bundle. Directories and other extensions
fail validation when the host is enabled. With the flag off, native entries
remain inventory-only.

Node must satisfy `^22.19 || >=24`. `.mts` supports Node's built-in removal of
erasable types; enums, decorators and syntax needing transformation require a
separate author build to JavaScript. Do not assume Node reads `tsconfig.json`.
See [Node's TypeScript rules](https://nodejs.org/docs/latest-v22.x/api/typescript.html).
Include local imports in the bundle; trust stages reviewed content, and the
entry is rehashed before import. Changes to reviewed bytes or capabilities
require another review. See [bundle rules](PLUGIN_BUNDLES.md).

## Imports and services

Export a Cordis plugin function or an object with `apply`. The host supplies
one shared Cordis and the supported DSH compatibility services. The example's
`inject = ['tools']` asks for the existing tool registry. The supplied service
names are `tools`, `logger`, `events`, `reflect` and `registry`; commands,
`core/call`, persistent extension storage,
hooks, skills and prompt providers are not host services yet. A required
service that is unavailable fails activation with a diagnostic.

Package runtime dependencies and local imports within the reviewed bundle.
Do not install packages or fetch code during activation. Register cleanup
through `ctx.effect`; asynchronous disposers are awaited with a deadline.

## Tool rules

Register a unique name with `ctx.tools.register`, a description, a JSON object
input schema and an `execute(input, exec)` function. Names start with a letter,
contain only letters, digits, `_` and `-`, and are at most 64 characters. Use a
plugin-specific prefix, such as `hello_greet`. Core names, core approval-name
families, and `mcp_`/`ext_` prefixes are reserved. Refusals explain the rule.

Every extension tool is `Required` and never treated as read-only based on a
plugin's claim. Full Access, Bypass or an exact session grant for the reviewed
plugin receipt may satisfy that requirement without another prompt. Tools are
deferred by default; `[tools].always_load` can pin a tool through the normal
tool configuration. Registration never grants permission to execute it.

Return a JSON value or text; the host renders it into ordinary tool output.
Avoid secrets in descriptions, logs and results. An approval card's wording
and identity come from Rust, never from plugin-supplied labels.

## Execution context

`exec.signal` is the call's cancellation signal; check it and propagate it to
asynchronous operations. `exec.callId` identifies the call, and `exec.args`
contains its input. Neither a call id nor plugin trust grants new privileges.
The calling workspace path is not currently exposed as an execution field;
the host process's working directory is its data directory, not the caller's
workspace. Do not use `process.chdir` in this shared process.

## Lifecycle, diagnostics and restarts

Use `/plugin show <name>` for that plugin's owner state, live tool names and
up to 20 recent retained diagnostic messages. The host retains a bounded 64-entry
shared diagnostic ring; this view is not a persistent per-plugin log. Warnings,
errors, activation/refusal/fault messages and teardown outcomes are attributed
to the plugin when known. Display text is escaped. `/plugin list` includes
overall host health and bounded diagnostics.

Do not block Node's event loop. Heartbeats mark an unanswered host Unresponsive
after 3 seconds and kill it after 10 seconds. Unexpected exits restart with a
short backoff; the third crash within 5 minutes stops automatic recovery.
Opening another engine does not reset that budget. Explicit plugin changes or
reload retry deliberately. Launch failures also allow a newly attached engine
to retry after a one-minute cooldown.

Recovery verifies current attachments and persisted trust again, then creates
fresh owner tokens and tool handles. Outstanding calls fail and are never
replayed; failed/faulted receipts remain suppressed until explicit retry or
changed authority. Two dirty teardowns within 10 minutes request maintenance
when active calls finish. This restart preserves the unexpected-crash budget.
Dispose within 2 seconds and avoid leaving background work behind.

## Sandbox

Trust is not a complete security boundary. Plugins share one process and can
interfere with each other. On macOS, the existing Seatbelt profile denies direct
network access, writes outside host data/temp paths and reads of the protected
credential locations. Other user-readable files, including project `.env`
files, remain readable. Linux and Windows currently run this host with the
user's permissions. Review the [current design limits](design/TS_EXTENSION_HOST.md)
before enabling third-party code.

## Limits

The host has a 256 MiB Node heap limit, 32 MiB frame limit, 256 in-flight request
limit (plus a reserved heartbeat), 128 tools per owner and 1024 per host. Tool
descriptions are at most 4 KiB and schemas 64 KiB. Tool calls have a 120-second
deadline. The feature stays Experimental and off by default; local fixture
tests do not establish sandbox parity, provider behavior or release readiness.
