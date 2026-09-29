//! `js_execution` tool — execute model-provided JavaScript via a local
//! Node.js runtime, returning stdout / stderr / exit code as JSON.
//!
//! Mirrors the shape of `code_execution` (Python) so the model sees a
//! single consistent surface for "run this snippet locally and tell me
//! what it printed." The split into a dedicated module (rather than
//! living inline in `core::engine::tool_catalog` next to
//! `execute_code_execution_tool`) keeps the dependency-probe and
//! tempfile-spawn logic isolated for the test pin.
//!
//! Registration is gated by [`crate::dependencies::resolve_node`]:
//! when Node is missing the tool is simply not advertised, so the
//! model never sees a runtime it can't actually use. See
//! `core::engine::tool_catalog::ensure_advanced_tooling` for the
//! catalog-side dispatch.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::dependencies::ExternalTool;
use serde_json::{Value, json};

use crate::tools::spec::{ToolError, ToolResult, required_str};
use codewhale_models::Tool;

/// Tool name surfaced to the model. Held alongside `code_execution`
/// in the deferred-tool dispatcher.
pub const JS_EXECUTION_TOOL_NAME: &str = "js_execution";
/// Tool-type tag — uses the same `code_execution_*` family the
/// Anthropic message API expects so the wire shape stays stable
/// across the two interpreters.
const JS_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
const NODE_USE_ENV_PROXY: &str = "NODE_USE_ENV_PROXY";
const NODE_PROXY_PAIRS: &[(&str, &str)] =
    &[("HTTP_PROXY", "http_proxy"), ("HTTPS_PROXY", "https_proxy")];

fn first_non_empty_env_from(
    keys: &[&str],
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<OsString> {
    keys.iter()
        .filter_map(|key| env(key))
        .find(|value| !value.is_empty())
}

fn node_proxy_env_overrides_from(
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    let all_proxy = first_non_empty_env_from(&["ALL_PROXY", "all_proxy"], &env);
    let proxy_configured = all_proxy.is_some()
        || NODE_PROXY_PAIRS
            .iter()
            .any(|(upper, lower)| first_non_empty_env_from(&[upper, lower], &env).is_some());

    let mut overrides = Vec::new();
    if proxy_configured && first_non_empty_env_from(&[NODE_USE_ENV_PROXY], &env).is_none() {
        overrides.push((NODE_USE_ENV_PROXY, OsString::from("1")));
    }

    for (upper, lower) in NODE_PROXY_PAIRS {
        if first_non_empty_env_from(&[upper], &env).is_none()
            && let Some(value) =
                first_non_empty_env_from(&[lower], &env).or_else(|| all_proxy.clone())
        {
            overrides.push((*upper, value));
        }
    }

    if first_non_empty_env_from(&["NO_PROXY"], &env).is_none()
        && let Some(value) = first_non_empty_env_from(&["no_proxy"], &env)
    {
        overrides.push(("NO_PROXY", value));
    }

    overrides
}

fn node_proxy_env_overrides() -> Vec<(&'static str, OsString)> {
    node_proxy_env_overrides_from(|key| std::env::var_os(key))
}

fn apply_node_execution_env(cmd: &mut tokio::process::Command) {
    crate::child_env::apply_to_tokio_command(cmd, node_proxy_env_overrides());
}

/// Build the `Tool` definition the catalog should advertise when
/// Node.js is present on the host. Kept as a constructor (rather
/// than a `static`) so the input schema can stay declarative
/// without a `lazy_static!`-style indirection.
#[must_use]
pub fn js_execution_tool_definition() -> Tool {
    Tool {
        tool_type: Some(JS_EXECUTION_TOOL_TYPE.to_string()),
        name: JS_EXECUTION_TOOL_NAME.to_string(),
        description:
            "Execute JavaScript code with the local Node.js runtime in the workspace and return stdout/stderr/return_code as JSON."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "JavaScript source code to execute." }
            },
            "required": ["code"]
        }),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

/// Wall-clock budget for one `js_execution` call. Real scripts call slow
/// APIs, wait on local services, and legitimately run for minutes, so the
/// budget is deliberately generous — and when it does fire, the interpreter
/// is killed rather than left running orphaned.
fn js_execution_timeout() -> Duration {
    if cfg!(test) {
        // Short enough that the timeout-kill test finishes quickly, long
        // enough that the happy-path tests never approach it.
        Duration::from_secs(5)
    } else {
        Duration::from_secs(600)
    }
}

/// Kill the interpreter after its timeout fired. Node scripts may spawn
/// their own children, so on Unix the kill names the whole process group
/// created by `process_group(0)` at spawn — the same containment
/// `codex_model_cache` and the extension host give their bounded children —
/// then the immediate child is killed; the caller reaps it with `wait()`
/// so no window is left where the tool has failed but Node still runs.
fn kill_js_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) {
        // SAFETY: kill(2) dereferences no pointers; the negative pid names
        // the process group this child leads.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Read one piped child stream to EOF, the per-pipe half of
/// `Child::wait_with_output`. Runs as its own task so the wait is not
/// blocked by a full pipe buffer.
async fn drain_to_end(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut pipe = match pipe {
        Some(pipe) => pipe,
        None => return Ok(Vec::new()),
    };
    use tokio::io::AsyncReadExt as _;
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Run the model-provided JavaScript and return the captured
/// stdout / stderr / return_code payload. Mirrors
/// `execute_code_execution_tool` — same tempfile pattern, same error
/// shape — so the surfaces stay interchangeable from the model's point of
/// view. The wall-clock budget is 600 seconds, and on timeout the child
/// process group is killed instead of being left running orphaned.
///
/// Tempfile lives only for the duration of this execution; `Drop`
/// removes it. We use the `.js` extension so any source-map /
/// shebang / encoding-sniffer logic in the interpreter behaves
/// normally.
pub async fn execute_js_execution_tool(
    input: &Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // Resolve the Node runtime via ExternalTool. If it's absent now
    // tokio_command() returns None and we fail fast with a clear message.

    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("js_execution.js");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = crate::dependencies::Node::tokio_command().ok_or_else(|| {
        ToolError::execution_failed("js_execution: Node.js runtime became unavailable".to_string())
    })?;
    // Recent Node releases use this startup env to make fetch/http(s) honor
    // standard proxy variables; older runtimes ignore it and keep prior behavior.
    apply_node_execution_env(&mut cmd);
    cmd.arg(&script_path).current_dir(workspace);

    // #3273: Node's built-in `fetch` (undici) ignores HTTP(S)_PROXY env vars
    // unless `NODE_USE_ENV_PROXY` is set (Node >= 24). This child already
    // inherits CodeWhale's proxy environment, so enabling the flag lets
    // `js_execution`'s `fetch()` reach the network through the same proxy/VPN
    // as the rest of the app and honor `NO_PROXY`. Only default it on when the
    // user hasn't chosen a value, so an explicit opt-out (`NODE_USE_ENV_PROXY=0`)
    // still wins. No-op on Node < 24, which ignores the unknown variable.
    if std::env::var_os("NODE_USE_ENV_PROXY").is_none() {
        cmd.env("NODE_USE_ENV_PROXY", "1");
    }

    // Stdio wiring equivalent to `Command::output()` (stdin / stdout /
    // stderr), but the child is spawned under our ownership so the timeout
    // path can kill it: dropping the wait future alone would leave Node
    // running detached. The child leads its own process group on Unix so
    // the timeout kill reaches scripts that spawned their own children,
    // and `kill_on_drop` remains the backstop when the whole tool future
    // is dropped (turn interrupt).
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::execution_failed(format!("js_execution: {e}")))?;

    // Drain stdout/stderr concurrently with the wait — what
    // `Command::output()` does internally — so a script blocked on a full
    // pipe buffer still exits and the timeout stays a real kill rather
    // than a deadlock.
    let mut stdout_task = tokio::spawn(drain_to_end(child.stdout.take()));
    let mut stderr_task = tokio::spawn(drain_to_end(child.stderr.take()));

    let budget = js_execution_timeout();
    let waited = tokio::time::timeout(budget, async {
        let status = child
            .wait()
            .await
            .map_err(|e| ToolError::execution_failed(format!("js_execution: {e}")))?;
        let (stdout, stderr) = match tokio::try_join!(&mut stdout_task, &mut stderr_task) {
            Ok((Ok(stdout), Ok(stderr))) => (stdout, stderr),
            Ok(_) => {
                return Err(ToolError::execution_failed(
                    "js_execution: failed to capture node output".to_string(),
                ));
            }
            Err(e) => return Err(ToolError::execution_failed(format!("js_execution: {e}"))),
        };
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await;

    let output = match waited {
        Ok(result) => result?,
        Err(_elapsed) => {
            // Kill before aborting the drain tasks: the group kill is what
            // frees the pipes, and the aborts only stop our readers. Reap
            // the child explicitly so the kill cannot leave a zombie behind
            // for the lazy orphan reaper (a wait() on a SIGKILLed child
            // returns promptly and never blocks on the pipes).
            kill_js_child(&mut child);
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ToolError::Timeout {
                seconds: budget.as_secs(),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use std::ffi::OsString;
    use tempfile::tempdir;

    /// Skip helper — `js_execution` is a no-op on hosts without Node.
    /// The tool simply isn't advertised in that case, so happy-path
    /// tests don't fail; they just don't exercise the spawn path.
    fn node_present() -> bool {
        crate::dependencies::resolve_node().is_some()
    }

    fn proxy_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find_map(|(name, value)| (*name == key).then(|| OsString::from(value)))
        }
    }

    #[test]
    fn tool_definition_advertises_js_execution_name_and_required_code_field() {
        let tool = js_execution_tool_definition();
        assert_eq!(tool.name, JS_EXECUTION_TOOL_NAME);
        assert_eq!(tool.tool_type.as_deref(), Some(JS_EXECUTION_TOOL_TYPE));
        assert!(tool.description.contains("local Node.js runtime"));
        assert!(!tool.description.contains("sandbox"));
        let required = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("schema must declare a `required` array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("code")),
            "input_schema must require `code`",
        );
    }

    #[test]
    fn node_proxy_overrides_enable_env_proxy_when_proxy_env_is_present() {
        let overrides =
            node_proxy_env_overrides_from(proxy_env(&[("HTTPS_PROXY", "http://127.0.0.1:20499")]));

        assert_eq!(
            overrides,
            vec![(NODE_USE_ENV_PROXY, OsString::from("1"))],
            "uppercase proxy vars are inherited by the child; only Node's env-proxy flag is needed"
        );
    }

    #[test]
    fn node_proxy_overrides_mirror_lowercase_proxy_vars() {
        let overrides = node_proxy_env_overrides_from(proxy_env(&[
            ("https_proxy", "http://127.0.0.1:20499"),
            ("no_proxy", "localhost"),
        ]));

        assert_eq!(
            overrides,
            vec![
                (NODE_USE_ENV_PROXY, OsString::from("1")),
                ("HTTPS_PROXY", OsString::from("http://127.0.0.1:20499")),
                ("NO_PROXY", OsString::from("localhost")),
            ]
        );
    }

    #[tokio::test]
    async fn execute_js_runs_node_and_returns_stdout_payload() {
        if !node_present() {
            // Catalog-build skips the tool entirely on hosts without
            // Node — match that behaviour in the test rather than
            // failing the suite for users without Node installed.
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "process.stdout.write('hello from node')" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(result.success, "successful node run must report success");
        assert!(
            result.content.contains("hello from node"),
            "stdout payload must surface the printed text; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_surfaces_runtime_error_with_nonzero_exit() {
        if !node_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "throw new Error('intentional fail')" }),
            tmp.path(),
        )
        .await
        .expect("execute should not Err — runtime errors land in stderr/exit code");
        assert!(
            !result.success,
            "non-zero exit must report success=false in the result payload"
        );
        assert!(
            result.content.contains("intentional fail"),
            "stderr payload must surface the error message; got {}",
            result.content
        );
    }

    // The env lock must stay held across the await so no other env-mutating test
    // races the process env while the child node run reads it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn execute_js_does_not_inherit_parent_secret_env() {
        if !node_present() {
            return;
        }
        let _env_lock = lock_test_env();
        let _secret = EnvVarGuard::set("CODEWHALE_JS_SECRET_LEAK_TEST", "secret-value");
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({
                "code": "process.stdout.write(process.env.CODEWHALE_JS_SECRET_LEAK_TEST || 'missing')"
            }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(
            result.success,
            "node run should succeed: {}",
            result.content
        );
        assert!(
            result.content.contains("missing"),
            "sanitized child env must not expose parent secrets; got {}",
            result.content
        );
        assert!(
            !result.content.contains("secret-value"),
            "secret value must not appear in js_execution output"
        );
    }

    #[tokio::test]
    async fn execute_js_enables_env_proxy_so_fetch_honors_proxy_vars() {
        if !node_present() {
            return;
        }
        // The tool defers to an explicit caller choice; only assert the
        // default-on behavior when the surrounding env hasn't set it.
        if std::env::var_os("NODE_USE_ENV_PROXY").is_some() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let result = execute_js_execution_tool(
            &json!({ "code": "process.stdout.write(String(process.env.NODE_USE_ENV_PROXY))" }),
            tmp.path(),
        )
        .await
        .expect("execute");
        assert!(
            result.content.contains("\"stdout\":\"1\""),
            "#3273: js_execution must default NODE_USE_ENV_PROXY=1 so Node's fetch \
             routes through HTTP(S)_PROXY; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn execute_js_rejects_input_without_code_field() {
        let tmp = tempdir().expect("tempdir");
        let err = execute_js_execution_tool(&json!({}), tmp.path())
            .await
            .expect_err("missing `code` must reject before any node spawn");
        let msg = err.to_string();
        assert!(
            msg.contains("code"),
            "error must name the missing `code` field; got {msg}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_the_node_child_instead_of_orphaning_it() {
        if !node_present() {
            eprintln!("skipping: node not present");
            return;
        }
        let workspace = tempdir().expect("workspace tempdir");
        let pid_file = workspace.path().join("child_pid");
        let code = format!(
            "const fs = require('fs'); \
             fs.writeFileSync({}, String(process.pid)); \
             setTimeout(() => {{}}, 60000);",
            serde_json::json!(pid_file.to_string_lossy())
        );

        let err = execute_js_execution_tool(&serde_json::json!({ "code": code }), workspace.path())
            .await
            .expect_err("a 60s sleep must hit the execution timeout");
        assert!(
            matches!(err, ToolError::Timeout { .. }),
            "expected a timeout error; got {err:?}"
        );

        // The child reported its pid before sleeping; the timeout must have
        // killed it (and the tool reaped it), not left it running.
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("child must have written its pid")
            .trim()
            .parse()
            .expect("pid file must contain an integer");
        let mut attempts = 0;
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                attempts < 50,
                "node child {pid} is still alive after the timeout kill"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
            attempts += 1;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_returns_promptly_even_when_a_grandchild_holds_the_pipes() {
        if !node_present() {
            eprintln!("skipping: node not present");
            return;
        }
        let workspace = tempdir().expect("workspace tempdir");
        // The child spawns a grandchild that inherits stdout/stderr (so the
        // pipe write ends outlive the child) and then blocks far past the
        // execution timeout. The budget bounds the whole call: the timeout
        // fires and the process-group kill takes the grandchild down with
        // the interpreter instead of the call hanging on pipe EOF.
        let code = "const { spawn } = require('child_process'); \
                    const g = spawn('sleep', ['30'], { stdio: ['ignore', 'inherit', 'inherit'] }); \
                    console.log('grandchild ' + g.pid); \
                    setTimeout(() => {}, 60000);";

        let started = std::time::Instant::now();
        let err = execute_js_execution_tool(&serde_json::json!({ "code": code }), workspace.path())
            .await
            .expect_err("a 60s sleep must hit the execution timeout");
        let elapsed = started.elapsed();
        assert!(
            matches!(err, ToolError::Timeout { .. }),
            "expected a timeout error; got {err:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(15),
            "timeout must return promptly even with a grandchild holding the pipes; took {elapsed:?}"
        );
    }
}
