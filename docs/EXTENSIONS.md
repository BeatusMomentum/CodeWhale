# Writing an extension tool

The experimental TypeScript extension host runs reviewed plugin code in a
shared Node process (or, as an opt-in, Bun). Rust still owns sessions, tool admission, approval and
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

## Runtime

`[extension_host] runtime` selects what runs the host. Node is the default.
Bun is an opt-in: it is qualified on macOS only, and becomes the default only
after an explicit, recorded cutover.

```toml
[extension_host]
runtime = "node"   # default: Node only
# runtime = "bun"  # Bun only; fails rather than falling back to Node
# runtime = "auto" # Bun >= 1.4.0 if one is found and starts, otherwise Node
# bun = "~/.bun/bin/bun"          # the only Bun tried when set
# node = "/opt/homebrew/bin/node" # the only Node tried when set
```

Node must satisfy `^22.19 || >=24`; Bun must be 1.4.0 or newer. Leaving
`runtime` unset means `node`, except that a table which sets only `bun` means
`bun`. A configured `node` or `bun` path is the only candidate for that
runtime: if it does not run or is below the floor, the host fails with that
reason instead of searching `PATH`. Without one, every `node` (or `bun`, then
`$BUN_INSTALL/bin`, default `~/.bun/bin`) on `PATH` is tried in order, and one
inside a `node_modules` directory or the working directory is skipped without
being run; set the path explicitly to use such a runtime. The working-directory
check does not apply when Codewhale starts in a directory that contains your
home directory, such as `~` or `/`.

Under `auto`, if the Bun it found fails to start the host, `/plugin` reports
why once and Node runs the host for the rest of the session. The runtime is
pinned once a host on it completes its handshake, and restarts reuse it. A
runtime binary replaced at that path mid-session is refused; restart Codewhale
to use it. `codewhale doctor` and `/plugin` show which runtime runs the host,
its version and path, how its memory cap is enforced, and, when `auto` used
Node, why Bun was not used.

When the host cannot take a call, `/plugin` and the failing extension tool
call give the same reason: not started, starting, unresponsive, restarting
after a crash (with how it exited), failed to start or out of crash budget
(with the reason; change or reload a plugin to retry), or disabled by config.
Every call into the host has a deadline, 120 s for a tool call; past it
Codewhale cancels the call, reports a timeout, and the host keeps serving
other calls. A plugin that ignores cancellation keeps running inside the host
until the host is torn down.

Write extensions for both runtimes. Use Node's APIs (Bun implements them) and
only erasable TypeScript in `.mts`. Enums, decorators and syntax that needs
transformation require a separate author build to JavaScript. Bun would accept
more, but Node would not. Node never reads a `tsconfig.json` for the host. Bun
does: a `tsconfig.json` next to or above an extension's files applies its
`paths`, `baseUrl` and JSX settings to that extension's imports, so an import
that resolves under Bun can fail under Node. Do not rely on it. Known limit:
that includes a `tsconfig.json` in a directory above the reviewed bundle,
which is not part of what you reviewed. See
[Node's TypeScript rules](https://nodejs.org/docs/latest-v22.x/api/typescript.html).
Under Bun the host runs with `--no-install`: a missing package fails the import;
it is never downloaded.

Extensions cannot run native code inside the host. On both runtimes
`process.dlopen`, `process.execve` and Worker threads are unavailable (a Worker
is a new JavaScript realm that would start without these restrictions). Under
Bun, `bun:ffi`, `Bun.FFI`, `bun:sqlite`, `node:sqlite` and `ShadowRealm` are
unavailable too; under Node, `node:sqlite` and `node:ffi` are switched off
(SQLite extensions and FFI load native libraries). The host refuses to start
when one of these restrictions does not hold on the installed runtime. Known
limit: these are the native-code entry points found so far (Bun 1.4, Node 22
and 26); one a newer runtime adds is not covered until it is added. A
process an extension starts is outside this policy; on macOS and Linux it runs
under the same sandbox as the host.
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

Do not block the event loop. Heartbeats mark an unanswered host Unresponsive
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
interfere with each other. On macOS the existing Seatbelt profile, and on Linux
bubblewrap (`/usr/bin/bwrap`), deny direct network access, writes outside host
data/temp paths and reads of the protected credential locations. Other
user-readable files, including project `.env` files, remain readable. On Linux
each start first checks that bwrap actually runs; where it is missing or cannot
create its namespaces (for example Ubuntu 24.04's
`kernel.apparmor_restrict_unprivileged_userns`), the host runs with the user's
permissions and `/plugin`, `codewhale doctor` and the start diagnostic say
`UNSANDBOXED` with bwrap's own error. Under bubblewrap a default credential
store created after the host started stays readable until it restarts.
Windows runs this host with the user's permissions. Review the
[current design limits](design/TS_EXTENSION_HOST.md) before enabling
third-party code.

## Limits

The host process has a 1 GiB memory cap. On Linux it is `RLIMIT_DATA` (or a
lower hard limit Codewhale itself inherited) and on Windows the Job Object's
per-process limit, so an allocation past it fails; both also apply to each
process an extension starts. Those two have been tested with a Node host only.
On macOS the Bun host applies a jetsam limit to itself before any extension
loads, and the kernel kills it past the cap; processes it starts are not
covered. A Node host on macOS has no kernel limit: Codewhale checks its
resident size at each 3-second heartbeat and kills it past the cap. Under Node
the JavaScript heap is also limited to 256 MiB. The host has a
32 MiB frame limit, 256 in-flight request
limit (plus a reserved heartbeat), 128 tools per owner and 1024 per host. Tool
descriptions are at most 4 KiB and schemas 64 KiB. Tool calls have a 120-second
deadline. The feature stays Experimental and off by default; local fixture
tests do not establish sandbox parity, provider behavior or release readiness.
