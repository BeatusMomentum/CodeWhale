# RFC: Evaluate consolidating interpreter tools into Shell

**Status:** Proposed; maintainer decision required before implementation.

**Date:** 2026-10-02

## Purpose

Ask maintainers to first evaluate removing the duplicate `code_execution`
(Python) and `js_execution` (Node.js) entrypoints and using the existing Shell
execution tools instead.

Only if these tools have essential, independently justified value that Shell
cannot reasonably replace should we proceed with integrating them into the
shared permission-aware process-launch mechanism.

This is a documentation-only proposal. It does not remove tools, change
permissions, or implement a sandbox fix.

## Current problems

### 1. The core capabilities overlap with Shell

Both tools accept model-authored code, launch a local interpreter, and return
stdout, stderr, and an exit code. Shell can also run Python and Node.js.

The dedicated tools provide convenience: interpreter discovery, temporary
script handling, and a consistent result format. Their inspected execution
paths are one-shot subprocesses, not persistent analysis environments with
stateful variables or integrated artifact presentation.

Maintainers should decide whether this convenience justifies two additional
tool surfaces and their execution plumbing. Historical existence alone is not
a reason to keep extending them.

### 2. The separate launch paths do not apply the session sandbox policy

The engine resolves a session sandbox policy and carries the effective
per-call context through approval. Interpreter dispatch then calls
`execute_code_execution_tool` and `execute_js_execution_tool` with only the
input and workspace path, without passing that context.

The runners directly construct local Python/Node commands, set the working
directory, capture output, and manage timeouts and process cleanup. They do not
apply the session filesystem/network sandbox policy.

Setting the working directory is not filesystem confinement. Sanitizing the
child environment and cleaning up the process tree are useful protections,
but neither provides filesystem or network isolation.

A shared `sandboxed_runner_command` already exists for tools that execute
workspace code outside Shell. Task gates and the test runner use it; these two
interpreter runners do not.

### 3. Execution permission and sandbox elevation are different decisions

This is not a claim that the tools execute without approval, nor a proposal to
forbid an explicitly authorized out-of-workspace operation.

The intended distinction is:

- Ordinary approval permits the planned call while retaining its effective
  restrictions.
- Explicit approval of a wider policy permits that call under the approved
  wider policy.

The engine already distinguishes these decisions. The interpreter runners do
not consume the policy, and the explicit `sandbox_permissions` request path is
currently connected to Shell tools, not these interpreter tools.

The concrete confinement concern is a session with an available enforcing
sandbox and restrictive policy, without an explicit elevation grant, where the
interpreter still takes the direct local launch path.

The ordinary approval descriptions also call these executions a "local
execution sandbox", even though their launch paths do not provide that
isolation. This wording should agree with the actual capability.

## Why evaluate removal before extending the tools?

Keeping both entrypoints requires maintaining tool definitions and discovery,
approval and elevation handling, interpreter selection, environment setup,
process lifetime management, documentation, and tests.

The current inconsistency shows the cost of parallel execution paths. If Shell
satisfies the real product requirements, adding more interpreter-specific
permission handling may increase maintenance rather than address unnecessary
duplication.

The decision should therefore be whether these entrypoints need to exist,
before deciding how to extend them.

## Maintainer decision requested

### Preferred direction: remove duplicate entrypoints and use Shell

If no essential independent value is identified:

- Remove the model-visible `code_execution` and `js_execution` entrypoints and
  their dedicated dispatch/execution branches.
- Run Python and Node.js through the existing Shell execution tools.
- Reuse Shell's existing approval, explicit elevation, and execution handling.
- Update affected catalog entries, permission references, documentation, and
  obsolete tests together.

Inspect actual consumers before removal. Inline Python/REPL admission depends
on `code_execution` being offered in the tool catalog. Deleting the name alone
could unintentionally disable a capability maintainers still want to retain.
Adjust that dependency deliberately if the capability remains. Decide RLM/REPL
retention separately; do not mechanically delete it with these two tools.

### Alternative: retain only with demonstrated independent value

If maintainers identify a necessary product capability or real external
integration requirement that Shell cannot reasonably replace, record that
reason before implementing a repair.

Then:

- Pass the effective per-call context to both interpreter runners.
- Reuse the existing permission-aware launcher instead of adding another
  sandbox implementation.
- If dedicated interpreter calls need elevation, extend the existing explicit
  authorization path rather than treating ordinary approval as a wider grant.
- Apply an approved wider policy only to the corresponding call.
- Correct approval wording and verify script delivery, output, timeout,
  cancellation, and confinement behavior.

No new interpreter-specific elevation flow is proposed before this decision.

## Scope and limitations

- An informed, explicitly approved sandbox-external operation is not itself
  evidence of unauthorized execution.
- Selecting `workspace-write` alone does not prove OS-level enforcement is
  available. The current Windows local command path advertises no enforcing OS
  sandbox; Linux also has no local wrapper without usable opt-in Bubblewrap.
- Consolidation does not create missing platform isolation. The shared local
  runner currently refuses unsupported read-only execution and sessions using
  an external sandbox backend, while retaining the existing unenforced
  workspace-write behavior where no local wrapper is available.
- This RFC does not redesign all plugin, MCP, RLM, or other subprocess paths.
- No tool removal, permission change, or new elevation mechanism is implemented
  by this document.

## Evidence and verification status

Source inspection confirms the separate local interpreter launch paths, the
missing context at interpreter dispatch, the existing shared launcher, and the
mismatch in approval wording.

Relevant source locations:

- `crates/tui/src/core/engine/tool_execution.rs`: interpreter dispatch.
- `crates/tui/src/core/engine/tool_catalog.rs`: Python runner and tool catalog.
- `crates/tui/src/tools/js_execution.rs`: Node.js runner.
- `crates/tui/src/core/engine/tool_preparation.rs`: approval descriptions.
- `crates/tui/src/core/engine/turn_loop.rs`: policy approval and REPL admission.
- `crates/tui/src/tools/shell.rs`: `sandboxed_runner_command`.
- `docs/AUTHORIZATION_ORDER.md` and `docs/SANDBOX.md`: authorization semantics
  and platform enforcement limits.

No runtime confinement reproduction on an enforcing platform has been
performed for this proposal. Source inspection is not a claim of demonstrated
unauthorized execution, and this RFC is not a completed fix.

## Decision gate

Maintainers should first decide whether to remove these duplicate entrypoints
and consolidate on Shell. Proceed with shared-permission integration only if
maintainers establish that the dedicated tools have essential independent
value.
