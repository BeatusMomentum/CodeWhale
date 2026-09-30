//! One extension-host process: launch plan, spawn, handshake, channel, exit.
//!
//! When the process exits, every in-flight call fails with a typed error.
//! The existing Rust manager owns heartbeat, crash budget, generation changes,
//! and receipt-checked replay; this channel never replays a tool call.
//!
//! **OS sandbox.** Where Codewhale's default command sandbox is available
//! (Seatbelt on macOS; bubblewrap stays opt-in for shell commands and is not
//! used here) the host runs under a workspace-write profile rooted at
//! `$CODEWHALE_HOME/extension-host/data`: no direct network, writes only
//! there and in the temp dirs, and **no reads** of the Codewhale homes
//! (everything but the bundle, its data dir and plugin code), the Codex and
//! DSH credential homes, and the credential-store default deny-list
//! (`sandbox::read_guard`). Other user-readable files stay readable —
//! including `.env` files, whose filename rule has no Seatbelt subpath form —
//! and Mach services are not restricted, so this is defense-in-depth, not a
//! containment boundary. Elsewhere (Linux, Windows) the host runs unsandboxed
//! with the user's permissions, and `/plugin` says so.
//!
//! **Runtime.** Node (the default) or Bun (opt-in: `runtime = "bun"`, or
//! `"auto"`, which prefers a supported Bun), chosen once per manager
//! (`[extension_host] runtime`). Each gets its own flags ([`runtime_args`])
//! and environment ([`runtime_env`]); Bun silently ignores Node's heap and
//! `__proto__` flags. The host itself takes in-process native code away from
//! plugins (`extension-host/src/runtime.ts`: `bun:ffi`, `Bun.FFI`, SQLite
//! extension loading, Worker threads, ShadowRealm, `process.dlopen`) and
//! refuses to start if a lock does not hold. That lockdown covers the entry
//! points found so far (Bun 1.4, Node 22 and 26), not every one a runtime
//! may add.
//!
//! **Memory cap** ([`MemoryEnforcement`]). What was measured where: the
//! macOS mechanism on macOS 26.1 arm64 (2026-09-30, Bun 1.4.0 and Node);
//! the Linux thresholds in [`HOST_MEMORY_CAP`] in a Linux container
//! (2026-09-29). Hosted CI runs the Rust memory-cap test on Linux, macOS and
//! Windows with Node only; no Bun host has been run on Linux or Windows.
//! * Linux: `RLIMIT_DATA`, set in the child before exec (clamped to an
//!   inherited hard limit that is already lower); an allocation past the cap
//!   fails. Plugin child processes inherit it.
//! * Windows: the Job Object's per-process limit; an allocation past the cap
//!   fails. It applies to each process in the job, plugin children included.
//! * macOS: `setrlimit(RLIMIT_AS/RLIMIT_DATA)` below the current mapping size
//!   fails with `EINVAL`, and `memorystatus_control` needs privilege. A fatal
//!   jetsam limit set as a `posix_spawn` attribute works unprivileged, but a
//!   later `exec` clears it, so it cannot be set on `sandbox-exec`. The Bun
//!   host therefore re-executes itself in place with the limit before any
//!   plugin loads, and reports it in `host/hello`; past the cap the kernel
//!   SIGKILLs it. Plugin child processes are not covered. A Bun host that
//!   cannot apply the requested limit is refused before initialization. Node
//!   has no FFI to do the same, so a Node host is checked at each heartbeat
//!   instead: enforced only as often as
//!   the heartbeat runs. Node also keeps `--max-old-space-size=256`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::dependencies::{HostRuntime, HostRuntimeKind};

use super::protocol::{
    self, CoreRequest, HostLimits, HostMessage, HostNotification, HostRequest, InitializeParams,
    RegisterResult, error_code,
};

/// Budget for spawn → `host/hello` → `host/initialize` → `host/ready`.
///
/// A warm start takes well under 100 ms, but the first start of a freshly
/// materialized bundle pays for a cold `node` launch, and on Windows for an
/// on-access antivirus scan of both. Windows CI under full test load missed
/// the former 5 s budget with the host silent on stderr (four runs on
/// 2026-09-29) while the same tests normally finish in about 1 s.
/// A miss is sticky: the host is marked failed until the next session, so
/// a too-tight budget disables every extension for that session. The
/// handshake runs in the background, off the first-prompt path, so a wider
/// budget costs nothing when the host is healthy; 30 s matches the MCP stdio
/// handshake (`codewhale_mcp::stdio_client::HANDSHAKE_TIMEOUT`).
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
pub const ACTIVATE_DEADLINE: Duration = Duration::from_secs(5);
pub const DISPOSE_DEADLINE: Duration = Duration::from_secs(2);
/// Grace between `$/cancel` and resolving a call as cancelled on this side.
pub const CANCEL_GRACE: Duration = Duration::from_millis(500);
const STDERR_TAIL_BYTES: usize = 8 * 1024;
const OUTBOUND_QUEUE: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostCallError {
    #[error("{message} (extension error {code})")]
    Rpc { code: i64, message: String },
    #[error("extension host exited: {0}")]
    Exited(String),
    #[error("cancelled: {0}")]
    Cancelled(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("extension host channel is full")]
    Busy,
}

/// How the host is started: the argv (wrapped by the OS sandbox when one is
/// available), its working directory, and which sandbox applies.
#[derive(Debug, Clone)]
pub(crate) struct HostLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// `seatbelt` / `bwrap`, or `None` when the host runs unsandboxed.
    pub sandbox: Option<String>,
    /// Environment the sandbox wrapper adds (`CODEWHALE_SANDBOX`, …).
    pub sandbox_env: Vec<(String, String)>,
    /// The runtime inside the wrapper; `host/hello` must report the same one.
    pub runtime: HostRuntime,
    /// Environment the runtime needs ([`runtime_env`]).
    pub runtime_env: Vec<(String, String)>,
    /// Bytes; see the module docs for how each platform enforces it.
    pub memory_cap: u64,
    /// How the cap is meant to be enforced; a macOS Bun host confirms its
    /// jetsam limit in `host/hello` or initialization is refused.
    pub memory: MemoryEnforcement,
}

/// Environment variable carrying the jetsam limit a macOS Bun host applies to
/// itself, in MiB (`extension-host/src/runtime.ts`, `applyMemoryLimit`).
pub(crate) const MEMORY_LIMIT_REQUEST_ENV: &str = "CODEWHALE_HOST_MEMORY_LIMIT_MIB";

/// How the host's memory cap is enforced (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryEnforcement {
    /// Linux: kernel `RLIMIT_DATA`.
    Rlimit,
    /// Windows: the Job Object's per-process memory limit.
    JobObject,
    /// macOS + Bun: a fatal jetsam limit the host applied to itself.
    Jetsam,
    /// macOS otherwise: resident size checked at each heartbeat.
    Heartbeat,
    /// No cap on this platform.
    Unenforced,
}

impl MemoryEnforcement {
    /// What this platform does for `kind`, before the host has confirmed it.
    #[must_use]
    pub fn planned(kind: HostRuntimeKind) -> Self {
        if cfg!(target_os = "linux") {
            Self::Rlimit
        } else if cfg!(windows) {
            Self::JobObject
        } else if cfg!(target_os = "macos") {
            match kind {
                HostRuntimeKind::Bun => Self::Jetsam,
                HostRuntimeKind::Node => Self::Heartbeat,
            }
        } else {
            Self::Unenforced
        }
    }

    /// One line for `/plugin` and doctor.
    #[must_use]
    pub fn describe(self, cap: u64) -> String {
        let mib = cap / (1024 * 1024);
        match self {
            Self::Rlimit => format!(
                "memory cap {mib} MiB (kernel RLIMIT_DATA; plugin child processes inherit it)"
            ),
            Self::JobObject => format!(
                "memory cap {mib} MiB (Job Object per-process limit; plugin child processes included)"
            ),
            Self::Jetsam => format!(
                "memory cap {mib} MiB (kernel jetsam limit; the host is killed past it; plugin child processes are not covered)"
            ),
            Self::Heartbeat => format!(
                "memory cap {mib} MiB (resident size checked at each heartbeat; on macOS only the Bun host gets a kernel limit)"
            ),
            Self::Unenforced => "no memory cap on this platform".to_string(),
        }
    }
}

/// The default memory cap. Measured once in a Linux container (2026-09-29),
/// not in CI: under `RLIMIT_DATA` Node 24 aborts when it creates the host's
/// watchdog Worker at 512 MiB, and Bun 1.4 aborts at startup at 256 MiB.
/// Both started and ran at 1 GiB and failed an allocation past it. An idle
/// host used 34–67 MB resident.
pub const HOST_MEMORY_CAP: u64 = 1 << 30;

/// Runtime flags, before the bundle path. Keep in sync with
/// `extension-host/test/harness.mjs` (`HOST_ARGS`).
///
/// - Node: a 256 MB old-space cap, `__proto__` throws, no native addons,
///   and the [`crate::dependencies::NODE_NATIVE_CODE_FLAGS`] this Node
///   accepts (`node:sqlite`, and `node:ffi` where it exists).
/// - Bun ignores all of those except `--no-addons`. Instead it gets
///   `--no-install`, because Bun otherwise fetches a missing package from npm
///   while plugin code is running. It also gets `--no-env-file` and
///   `--config=<null device>`, because Bun otherwise loads `.env` and
///   `bunfig.toml` (which can preload code) from the working directory, and
///   that directory is the host's writable data dir. `bun:ffi` and the rest
///   are locked in the host itself (`src/runtime.ts`).
#[must_use]
pub(crate) fn runtime_args(runtime: &HostRuntime) -> Vec<String> {
    base_runtime_args(runtime.kind)
        .iter()
        .chain(&runtime.native_code_flags)
        .map(|arg| (*arg).to_string())
        .collect()
}

/// Environment for the runtime. Keep in sync with `HOST_ENV` in
/// `extension-host/test/harness.mjs`.
///
/// - Both: `NODE_OPTIONS` is blanked. The child-environment allowlist passes
///   the user's value through (shell tools need it), and a `--require` or
///   `--import` preload there would run before the host's native-code
///   lockdown. Node honours it; Bun 1.4.0 ignored a `--require` in it when
///   checked (2026-09-30), and it is blanked for Bun too in case a later
///   Bun does not. `BUN_OPTIONS`, which Bun does honour, is not in that
///   allowlist.
/// - Bun: no ShadowRealm, engine-wide (a realm imports a fresh `bun:ffi`;
///   `node:vm` contexts would otherwise hand the constructor out). The host
///   refuses to start without it.
#[must_use]
pub(crate) fn runtime_env(kind: HostRuntimeKind) -> Vec<(String, String)> {
    let mut env = vec![("NODE_OPTIONS".to_string(), String::new())];
    if kind == HostRuntimeKind::Bun {
        env.push(("BUN_JSC_useShadowRealm".to_string(), "0".to_string()));
    }
    env
}

fn base_runtime_args(kind: HostRuntimeKind) -> &'static [&'static str] {
    match kind {
        HostRuntimeKind::Node => &[
            "--max-old-space-size=256",
            "--disable-proto=throw",
            "--no-addons",
        ],
        #[cfg(windows)]
        HostRuntimeKind::Bun => &[
            "--no-install",
            "--no-env-file",
            "--config=NUL",
            "--no-addons",
        ],
        #[cfg(not(windows))]
        HostRuntimeKind::Bun => &[
            "--no-install",
            "--no-env-file",
            "--config=/dev/null",
            "--no-addons",
        ],
    }
}

/// Top-level entries of a Codewhale home the host may read: its own bundle
/// and data (`extension-host`), and plugin code (the staged snapshots live
/// under `plugins/.runtime`). Everything else in a Codewhale home — secrets,
/// tokens, config and its backups, sessions, state, tool outputs, history —
/// is denied.
const HOST_READABLE_HOME_ENTRIES: &[&str] = &["extension-host", "plugins", "builtin-plugins"];

/// Codewhale-home entries denied by name even before they exist, so a store
/// created after the host started is still covered. Existing entries are
/// denied by enumeration (`host_denied_read_paths`).
const HOST_DENIED_HOME_ENTRIES: &[&str] = &[
    "secrets",
    "credentials",
    "tokens",
    "state",
    "state.db",
    "sessions",
    "session-archives",
    "session_index.jsonl",
    "tool_outputs",
    "composer_history.txt",
    "composer_history.jsonl",
    "remote-control",
    "integrations",
    "audit.log",
    "logs",
    "memory",
    "mcp.json",
    "mcp.json.bak",
    "config.toml.bak",
    "settings.toml",
];

/// Paths the host process must never read, even though the sandbox otherwise
/// grants full-disk read: the curated credential-store defaults; every entry
/// of Codewhale's homes (the runtime home, the ambient `~/.codewhale`, and the
/// legacy `~/.deepseek`) except [`HOST_READABLE_HOME_ENTRIES`]; and the Codex
/// and DSH homes whose credential files Codewhale itself reads. Blocking.
pub(crate) fn host_denied_read_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = crate::sandbox::read_guard::ReadDenylist::build(true, &[], &[]).subtree_paths();
    let mut push = |path: PathBuf| {
        if !paths.contains(&path) {
            paths.push(path);
        }
    };
    let user_home = codewhale_paths::user_home();
    let mut roots = vec![home.to_path_buf()];
    roots.extend(codewhale_config::codewhale_home().ok());
    if let Some(user) = &user_home {
        roots.push(user.join(codewhale_config::CODEWHALE_APP_DIR));
        roots.push(user.join(".deepseek"));
    }
    for root in roots {
        let mut names: Vec<std::ffi::OsString> = HOST_DENIED_HOME_ENTRIES
            .iter()
            .chain(std::iter::once(&codewhale_config::CONFIG_FILE_NAME))
            .map(std::ffi::OsString::from)
            .collect();
        if let Ok(entries) = std::fs::read_dir(&root) {
            names.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name()),
            );
        }
        // Seatbelt matches the kernel-resolved path, and a name that does not
        // exist yet cannot be canonicalized later, so deny it under both the
        // given and the resolved spelling of its (existing) root.
        let resolved = std::fs::canonicalize(&root).ok();
        for name in names {
            let readable = name
                .to_str()
                .is_some_and(|name| HOST_READABLE_HOME_ENTRIES.contains(&name));
            if !readable {
                if let Some(resolved) = &resolved {
                    push(resolved.join(&name));
                }
                push(root.join(name));
            }
        }
    }
    // Codex's home (ChatGPT OAuth tokens in `auth.json`) and the DSH home
    // (`.credentials.yaml`), wherever the environment points them.
    if let Some(codex_home) = crate::oauth::auth_file_path().parent() {
        push(codex_home.to_path_buf());
    }
    if let Some(user) = &user_home {
        push(user.join(".codex"));
        push(user.join(".dsh"));
    }
    if let Some(dsh_home) = codewhale_config::default_dsh_credentials_path().parent() {
        push(dsh_home.to_path_buf());
    }
    paths
}

/// Plan the host launch. Blocking (creates the data dir, canonicalizes the
/// deny-list); call from `spawn_blocking`.
pub(crate) fn plan_launch(
    runtime: &HostRuntime,
    bundle: &Path,
    home: &Path,
    memory_cap: u64,
) -> Result<HostLaunch, String> {
    use crate::sandbox::{CommandSpec, SandboxManager, SandboxPolicy, SandboxType};
    let data = home.join("extension-host").join("data");
    std::fs::create_dir_all(&data)
        .map_err(|error| format!("cannot create {}: {error}", data.display()))?;
    let mut args = runtime_args(runtime);
    args.push(bundle.to_string_lossy().into_owned());
    let unsandboxed = HostLaunch {
        program: runtime.path.clone(),
        args: args.clone(),
        cwd: data.clone(),
        sandbox: None,
        sandbox_env: Vec::new(),
        runtime: runtime.clone(),
        runtime_env: runtime_env(runtime.kind),
        memory_cap,
        memory: MemoryEnforcement::planned(runtime.kind),
    };
    if cfg!(windows) {
        // The Windows helper is process containment only; ProcessTree already
        // provides that, and it must not be reported as isolation.
        return Ok(unsandboxed);
    }
    let spec = CommandSpec::program(&runtime.path.to_string_lossy(), args, data, Duration::ZERO)
        .with_policy(SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        });
    let mut manager = SandboxManager::new();
    manager.set_denied_read_subpaths(host_denied_read_paths(home));
    let env = manager.prepare(&spec);
    if matches!(env.sandbox_type, SandboxType::None) {
        return Ok(unsandboxed);
    }
    let mut command = env.command.into_iter();
    let program = command
        .next()
        .ok_or("sandbox wrapper produced an empty command")?;
    Ok(HostLaunch {
        program: PathBuf::from(program),
        args: command.collect(),
        cwd: env.cwd,
        sandbox: Some(env.sandbox_type.to_string()),
        sandbox_env: env.env.into_iter().collect(),
        ..unsandboxed
    })
}

/// The kernel-enforced memory cap on Linux: `RLIMIT_DATA`, applied in the
/// child between fork and exec, so only the host (and what it starts) is
/// limited. Soft and hard limit are both set, so plugin code cannot raise
/// it. An unprivileged process cannot raise its hard limit, so when the
/// inherited hard limit is already below `cap` the host gets that lower
/// limit instead of failing to spawn with `EPERM`.
///
/// Known limit: in that case `/plugin` and doctor still name the configured
/// cap, not the lower inherited one.
#[cfg(target_os = "linux")]
fn limit_child_memory(command: &mut tokio::process::Command, cap: u64) {
    // SAFETY: the closure runs in the forked child before exec and calls only
    // `getrlimit` and `setrlimit`, which are async-signal-safe; it allocates
    // nothing.
    unsafe {
        command.pre_exec(move || {
            let mut inherited = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_DATA, &raw mut inherited) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let cap = (cap as libc::rlim_t).min(inherited.rlim_max);
            let limit = libc::rlimit {
                rlim_cur: cap,
                rlim_max: cap,
            };
            if libc::setrlimit(libc::RLIMIT_DATA, &raw const limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn limit_child_memory(_command: &mut tokio::process::Command, _cap: u64) {}

/// Resident size of `pid` in bytes, for the macOS memory-cap check.
#[cfg(target_os = "macos")]
pub(crate) fn resident_bytes(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `info` is a correctly sized, writable `proc_taskinfo` buffer.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    // SAFETY: a full-size write initialized the struct.
    (written == size).then(|| unsafe { info.assume_init() }.pti_resident_size)
}

/// Platforms without a supervisor-side check (Linux uses `RLIMIT_DATA`,
/// Windows the Job Object).
#[cfg(not(target_os = "macos"))]
pub(crate) fn resident_bytes(_pid: u32) -> Option<u64> {
    None
}

/// Report the observed exit and configured cap. SIGKILL alone cannot identify
/// jetsam: an operator or another process can send the same signal.
fn exit_reason(
    status: std::process::ExitStatus,
    memory: Option<MemoryEnforcement>,
    cap: u64,
) -> String {
    let reason = format!("exited with {status}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if status.signal() == Some(libc::SIGKILL) && memory == Some(MemoryEnforcement::Jetsam) {
            return format!(
                "{reason}; configured kernel memory limit: {} MiB; SIGKILL cause unavailable",
                cap / (1024 * 1024)
            );
        }
    }
    #[cfg(not(unix))]
    let _ = (memory, cap);
    reason
}

/// Callbacks from the channel into the manager.
pub(crate) trait HostEvents: Send + Sync + 'static {
    fn register(&self, params: &protocol::RegisterParams) -> RegisterResult;
    fn unregister(&self, params: &protocol::UnregisterParams);
    fn faulted(&self, params: &protocol::FaultedParams);
    fn log(&self, params: &protocol::LogParams);
    fn exited(&self, host_generation: u64, reason: String, stderr_tail: String);
}

/// Receives one request's outcome.
pub(crate) type CallReceiver = oneshot::Receiver<Result<Value, HostCallError>>;

struct PendingCall {
    tx: oneshot::Sender<Result<Value, HostCallError>>,
    /// Plugin whose revocation cancels this call.
    owner: Option<String>,
    revoked: bool,
    heartbeat: bool,
}

#[derive(Default)]
struct Handshake {
    hello: Option<oneshot::Sender<protocol::HelloParams>>,
    ready: Option<oneshot::Sender<()>>,
}

pub(crate) struct HostProcess {
    pub pid: Option<u32>,
    /// The runtime the Rust side launched; `host/hello` must agree.
    pub runtime: HostRuntime,
    /// Runtime version as reported by the host in `host/hello`.
    pub runtime_version: std::sync::OnceLock<String>,
    pub memory_cap: u64,
    /// How the cap is enforced for this process, settled at the handshake.
    memory: Arc<std::sync::OnceLock<MemoryEnforcement>>,
    /// `seatbelt` / `bwrap`, or `None` when unsandboxed.
    pub sandbox: Option<String>,
    tree: Arc<crate::process_tree::ProcessTree>,
    outbound: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<HashMap<u64, PendingCall>>>,
    /// Admission checks and sealing hold `pending`; the manager also reads
    /// this flag to avoid activation while the exit callback is still pending.
    admission_closed: AtomicBool,
    next_id: AtomicU64,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    exited: tokio::sync::watch::Receiver<bool>,
    kill: mpsc::Sender<String>,
}

fn push_tail(tail: &Mutex<VecDeque<u8>>, bytes: &[u8]) {
    let mut tail = tail.lock().expect("stderr tail lock");
    tail.extend(bytes);
    while tail.len() > STDERR_TAIL_BYTES {
        tail.pop_front();
    }
}

fn tail_string(tail: &Mutex<VecDeque<u8>>) -> String {
    let tail = tail.lock().expect("stderr tail lock");
    let bytes: Vec<u8> = tail.iter().copied().collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl HostProcess {
    /// Spawn the host and complete the handshake. `expected_sha256` is the
    /// digest of the bundle this process materialized; the host's
    /// self-reported digest must match (a consistency check, not
    /// anti-substitution: the control is that Rust chooses what to exec).
    pub(crate) async fn spawn(
        generation: u64,
        launch: &HostLaunch,
        expected_sha256: &str,
        events: Arc<dyn HostEvents>,
    ) -> Result<Arc<Self>, String> {
        let mut command = tokio::process::Command::new(&launch.program);
        crate::utils::suppress_tokio_console_window(&mut command);
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Scrubbed environment: no credentials, no ambient proxy URLs.
        command.env_clear();
        let parent_pid = std::process::id().to_string();
        // On Unix the host leads its own process group (below), so it may kill
        // that group when the core goes away (stdin EOF, or a parent change
        // seen by its watchdog thread).
        let own_group = if cfg!(unix) { "1" } else { "0" };
        let memory_request = (launch.memory == MemoryEnforcement::Jetsam)
            .then(|| (launch.memory_cap / (1024 * 1024)).max(1).to_string());
        let overrides = launch
            .sandbox_env
            .iter()
            .chain(&launch.runtime_env)
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .chain(
                memory_request
                    .as_deref()
                    .map(|mib| (MEMORY_LIMIT_REQUEST_ENV, mib)),
            )
            .chain([
                ("CODEWHALE_HOST_PARENT_PID", parent_pid.as_str()),
                ("CODEWHALE_HOST_PROCESS_GROUP", own_group),
            ]);
        for (key, value) in
            crate::child_env::sanitized_plugin_mcp_env_from(std::env::vars_os(), overrides)
        {
            command.env(key, value);
        }
        #[cfg(unix)]
        command.process_group(0);
        limit_child_memory(&mut command, launch.memory_cap);

        // On Linux a failure to apply the memory cap in the child surfaces
        // here as a spawn error carrying only its errno, indistinguishable
        // from a failed exec, so the message names both.
        let mut child = command.spawn().map_err(|error| {
            if cfg!(target_os = "linux") {
                format!(
                    "failed to start {}, or to apply its {} MiB memory cap (RLIMIT_DATA) before exec: {error}",
                    launch.program.display(),
                    launch.memory_cap / (1024 * 1024)
                )
            } else {
                format!("failed to start {}: {error}", launch.program.display())
            }
        })?;
        let pid = child.id();
        let tree = match crate::process_tree::ProcessTree::attach_tokio(&child) {
            Ok(tree) => Arc::new(tree),
            Err(error) => {
                let _ = child.start_kill();
                return Err(format!("failed to contain the extension host: {error}"));
            }
        };
        #[cfg(windows)]
        if let Err(error) = tree.limit_process_memory(launch.memory_cap) {
            let _ = tree.kill();
            let _ = child.start_kill();
            return Err(format!(
                "failed to cap the extension host's memory: {error}"
            ));
        }
        let memory: Arc<std::sync::OnceLock<MemoryEnforcement>> = Arc::default();
        let stdin = child.stdin.take().ok_or("host stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("host stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("host stderr unavailable")?;

        let (outbound, mut outbound_rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_QUEUE);
        let pending: Arc<Mutex<HashMap<u64, PendingCall>>> = Arc::default();
        let stderr_tail: Arc<Mutex<VecDeque<u8>>> = Arc::default();
        let (exited_tx, exited_rx) = tokio::sync::watch::channel(false);
        let (hello_tx, hello_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let handshake = Arc::new(Mutex::new(Handshake {
            hello: Some(hello_tx),
            ready: Some(ready_tx),
        }));

        // Writer: the only task that touches stdin. Dropping every sender
        // closes stdin, which the host treats as "core is gone".
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(frame) = outbound_rx.recv().await {
                if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // stderr: a bounded tail for diagnostics, chunks into tracing. Read
        // in fixed-size chunks, never by line: a plugin writing endless
        // output without a newline must not grow this process's memory.
        {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut stderr = stderr;
                let mut chunk = vec![0_u8; 4096];
                while let Ok(read) = stderr.read(&mut chunk).await {
                    if read == 0 {
                        break;
                    }
                    push_tail(&tail, &chunk[..read]);
                    tracing::debug!(
                        target: "extension_host",
                        "host stderr: {}",
                        String::from_utf8_lossy(&chunk[..read]).trim_end()
                    );
                }
            });
        }

        // Reader: every frame is validated strictly; a framing or protocol
        // violation kills the host (a plugin wrote to the channel, or the
        // host is not ours).
        let (kill_tx, mut kill_rx) = mpsc::channel::<String>(1);
        {
            let pending = Arc::clone(&pending);
            let outbound = outbound.clone();
            let events = Arc::clone(&events);
            let handshake = Arc::clone(&handshake);
            let kill_tx = kill_tx.clone();
            tokio::spawn(async move {
                let mut stdout = stdout;
                loop {
                    let value = match protocol::read_frame(&mut stdout).await {
                        Ok(Some(value)) => value,
                        Ok(None) => break,
                        Err(error) => {
                            let _ = kill_tx.try_send(format!("channel framing violation: {error}"));
                            break;
                        }
                    };
                    let message = match protocol::parse_host_message(value) {
                        Ok(message) => message,
                        Err(error) => {
                            let _ = kill_tx.try_send(format!("protocol violation: {error}"));
                            break;
                        }
                    };
                    handle_host_message(message, &pending, &outbound, events.as_ref(), &handshake);
                }
            });
        }

        // Exit watcher: owns the child. On exit, fail everything and report.
        {
            let pending = Arc::clone(&pending);
            let tail = Arc::clone(&stderr_tail);
            let events = Arc::clone(&events);
            let tree = Arc::clone(&tree);
            let memory = Arc::clone(&memory);
            let memory_cap = launch.memory_cap;
            tokio::spawn(async move {
                let reason = tokio::select! {
                    biased;
                    status = child.wait() => match status {
                        Ok(status) => exit_reason(status, memory.get().copied(), memory_cap),
                        Err(error) => format!("wait failed: {error}"),
                    },
                    Some(reason) = kill_rx.recv() => {
                        let _ = tree.kill();
                        let _ = child.kill().await;
                        reason
                    }
                };
                // The leader is gone; take anything it left behind with it.
                let _ = tree.kill();
                let drained: Vec<PendingCall> = {
                    let mut pending = pending.lock().expect("pending lock");
                    // Publish exit under the admission lock before draining:
                    // waking a failed call must not admit another orphaned call.
                    let _ = exited_tx.send(true);
                    pending.drain().map(|(_, call)| call).collect()
                };
                for call in drained {
                    let _ = call.tx.send(Err(HostCallError::Exited(reason.clone())));
                }
                // Give the stderr task a moment to capture the last lines.
                tokio::time::sleep(Duration::from_millis(50)).await;
                events.exited(generation, reason, tail_string(&tail));
            });
        }

        let host = Arc::new(Self {
            pid,
            runtime: launch.runtime.clone(),
            runtime_version: std::sync::OnceLock::new(),
            memory_cap: launch.memory_cap,
            memory,
            sandbox: launch.sandbox.clone(),
            tree,
            outbound,
            pending,
            admission_closed: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            stderr_tail,
            exited: exited_rx,
            kill: kill_tx.clone(),
        });

        let handshake_result = tokio::time::timeout(HANDSHAKE_DEADLINE, async {
            let hello = hello_rx
                .await
                .map_err(|_| "host exited before host/hello".to_string())?;
            if hello.protocol.min > protocol::PROTOCOL_VERSION
                || hello.protocol.max < protocol::PROTOCOL_VERSION
            {
                return Err(format!(
                    "host speaks protocol {}..={}, core speaks {}",
                    hello.protocol.min,
                    hello.protocol.max,
                    protocol::PROTOCOL_VERSION
                ));
            }
            // Never a silent runtime switch: the host must be running on
            // the runtime this process chose and launched.
            if hello.runtime.name != launch.runtime.kind.name() {
                return Err(format!(
                    "host reports runtime {} but {} was launched",
                    hello.runtime.name,
                    launch.runtime.kind.name()
                ));
            }
            // Restarts reuse the pinned runtime without probing it again, so
            // the binary at that path can have been replaced since (an
            // upgrade mid-session). Its flags and lockdown were chosen for
            // the probed version; refuse rather than run an unprobed one.
            if !launch.runtime.reports_version(&hello.runtime.version) {
                return Err(format!(
                    "host reports {} {} but {} {} was probed at {}: the runtime binary changed mid-session; restart Codewhale to use the new version",
                    hello.runtime.name,
                    hello.runtime.version,
                    launch.runtime.kind.name(),
                    launch.runtime.version_string(),
                    launch.runtime.path.display()
                ));
            }
            if hello.bundle_sha256 != expected_sha256 {
                return Err(format!(
                    "host bundle digest {} does not match the materialized bundle {}",
                    hello.bundle_sha256, expected_sha256
                ));
            }
            let requested = memory_request
                .as_deref()
                .and_then(|mib| mib.parse::<u64>().ok());
            let memory = match (hello.memory_limit_mib, requested) {
                (Some(applied), Some(requested)) if applied == requested => {
                    MemoryEnforcement::Jetsam
                }
                (Some(applied), _) => {
                    return Err(format!(
                        "host reports a {applied} MiB memory limit the core did not ask for"
                    ));
                }
                // The mandatory kernel cap cannot degrade to a delayed RSS
                // observation. Refuse before plugin initialization and keep
                // the host's stderr explanation in the existing diagnosis.
                (None, Some(requested)) => {
                    return Err(format!(
                        "host did not apply the requested {requested} MiB kernel memory limit; initialization refused"
                    ));
                }
                (None, None) => launch.memory,
            };
            let _ = host.memory.set(memory);
            let initialize = CoreRequest::Initialize(InitializeParams {
                protocol: protocol::PROTOCOL_VERSION,
                limits: HostLimits {
                    max_frame: protocol::MAX_FRAME as u64,
                    max_inflight: protocol::MAX_INFLIGHT as u64,
                    dispose_deadline_ms: DISPOSE_DEADLINE.as_millis() as u64,
                    activate_deadline_ms: ACTIVATE_DEADLINE.as_millis() as u64,
                },
            });
            host.request(initialize, None)
                .await
                .map_err(|error| format!("host/initialize failed: {error}"))?;
            ready_rx
                .await
                .map_err(|_| "host exited before host/ready".to_string())?;
            Ok(hello.runtime.version)
        })
        .await;
        match handshake_result {
            Ok(Ok(runtime_version)) => {
                let _ = host.runtime_version.set(runtime_version);
                Ok(host)
            }
            Ok(Err(reason)) => {
                let _ = kill_tx.try_send(reason.clone());
                Err(format!(
                    "{reason}; stderr: {}",
                    tail_string(&host.stderr_tail)
                ))
            }
            Err(_) => {
                let reason = format!("handshake exceeded {HANDSHAKE_DEADLINE:?}");
                let _ = kill_tx.try_send(reason.clone());
                Err(format!(
                    "{reason}; stderr: {}",
                    tail_string(&host.stderr_tail)
                ))
            }
        }
    }

    /// How the memory cap is enforced for this process (planned until the
    /// handshake settles it).
    #[must_use]
    pub fn memory(&self) -> MemoryEnforcement {
        self.memory
            .get()
            .copied()
            .unwrap_or_else(|| MemoryEnforcement::planned(self.runtime.kind))
    }

    /// How many requests the core has sent this host (handshake included).
    #[cfg(test)]
    pub(crate) fn requests_started(&self) -> u64 {
        self.next_id.load(Ordering::Relaxed) - 1
    }

    #[must_use]
    pub fn has_exited(&self) -> bool {
        *self.exited.borrow()
    }

    pub(crate) fn terminate(&self, reason: String) {
        let _ = self.kill.try_send(reason);
    }

    pub(crate) fn is_retiring(&self) -> bool {
        self.admission_closed.load(Ordering::Acquire)
    }

    /// Seal admission and retire this process only if no non-heartbeat call
    /// is pending. The same lock guards admission in `start_request`.
    pub(crate) fn terminate_if_idle(&self, reason: &str) -> bool {
        let pending = self.pending.lock().expect("pending lock");
        if self.has_exited()
            || self.admission_closed.load(Ordering::Relaxed)
            || pending.values().any(|call| !call.heartbeat)
        {
            return false;
        }
        if self.kill.try_send(reason.to_string()).is_err() {
            return false;
        }
        self.admission_closed.store(true, Ordering::Release);
        true
    }

    pub(crate) fn stderr_tail(&self) -> String {
        tail_string(&self.stderr_tail)
    }

    fn send_frame(&self, value: &Value) -> Result<(), HostCallError> {
        let frame = protocol::encode_frame(value).map_err(|error| HostCallError::Rpc {
            code: error_code::INVALID_PARAMS,
            message: error.to_string(),
        })?;
        self.outbound.try_send(frame).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => HostCallError::Busy,
            mpsc::error::TrySendError::Closed(_) => {
                HostCallError::Exited("channel closed".to_string())
            }
        })
    }

    /// Send a request; the returned id can be cancelled with [`Self::cancel`].
    pub(crate) fn start_request(
        &self,
        request: CoreRequest,
        owner: Option<String>,
    ) -> Result<(u64, CallReceiver), HostCallError> {
        if self.has_exited() {
            return Err(HostCallError::Exited("already exited".to_string()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("pending lock");
            // Exit may have won after the fast check above. It publishes under
            // this same lock, so every admitted call is either live or drained.
            if self.has_exited() {
                return Err(HostCallError::Exited("already exited".to_string()));
            }
            if self.admission_closed.load(Ordering::Relaxed) {
                return Err(HostCallError::Exited("host is restarting".to_string()));
            }
            // Reserve one control request for the single heartbeat monitor,
            // so saturated tool calls cannot make a healthy host look hung.
            if pending.len() >= protocol::MAX_INFLIGHT && !matches!(request, CoreRequest::Ping) {
                return Err(HostCallError::Busy);
            }
            pending.insert(
                id,
                PendingCall {
                    tx,
                    owner,
                    revoked: false,
                    heartbeat: matches!(request, CoreRequest::Ping),
                },
            );
        }
        if let Err(error) = self.send_frame(&request.to_value(id)) {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(error);
        }
        Ok((id, rx))
    }

    pub(crate) async fn request(
        &self,
        request: CoreRequest,
        owner: Option<String>,
    ) -> Result<Value, HostCallError> {
        let (_, rx) = self.start_request(request, owner)?;
        rx.await
            .unwrap_or_else(|_| Err(HostCallError::Exited("channel closed".to_string())))
    }

    /// Request with a deadline; on expiry the call is cancelled host-side.
    pub(crate) async fn request_with_deadline(
        &self,
        request: CoreRequest,
        owner: Option<String>,
        deadline: Duration,
    ) -> Result<Value, HostCallError> {
        let (id, rx) = self.start_request(request, owner)?;
        match tokio::time::timeout(deadline, rx).await {
            Ok(result) => {
                result.unwrap_or_else(|_| Err(HostCallError::Exited("channel closed".to_string())))
            }
            Err(_) => {
                self.cancel(id);
                self.pending.lock().expect("pending lock").remove(&id);
                Err(HostCallError::Timeout(deadline))
            }
        }
    }

    /// Fire `$/cancel`. Best effort: a full or closed channel is fine, the
    /// caller resolves its side on its own schedule.
    pub(crate) fn cancel(&self, id: u64) {
        let _ = self.send_frame(&protocol::cancel_value(id));
    }

    /// Forget a request without cancelling it (its answer will be dropped).
    pub(crate) fn forget(&self, id: u64) {
        self.pending.lock().expect("pending lock").remove(&id);
    }

    /// Revocation: cancel every in-flight call owned by `plugin_id`; each
    /// resolves as cancelled when the host answers or after `CANCEL_GRACE`,
    /// whichever is first — revocation never waits on the host.
    pub(crate) fn revoke_calls_of(self: &Arc<Self>, plugin_id: &str) {
        let ids: Vec<u64> = {
            let mut pending = self.pending.lock().expect("pending lock");
            pending
                .iter_mut()
                .filter(|(_, call)| call.owner.as_deref() == Some(plugin_id))
                .map(|(id, call)| {
                    call.revoked = true;
                    *id
                })
                .collect()
        };
        for id in ids {
            self.cancel(id);
            let pending = Arc::clone(&self.pending);
            tokio::spawn(async move {
                tokio::time::sleep(CANCEL_GRACE).await;
                if let Some(call) = pending.lock().expect("pending lock").remove(&id) {
                    let _ = call.tx.send(Err(HostCallError::Cancelled(
                        "extension was revoked".to_string(),
                    )));
                }
            });
        }
    }

    /// Bounded shutdown: `host/shutdown` (2 s), close stdin, then kill the
    /// process group at 3 s total.
    #[cfg(test)]
    pub(crate) async fn shutdown(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.request(CoreRequest::Shutdown, None),
        )
        .await;
        let mut exited = self.exited.clone();
        let waited = tokio::time::timeout(Duration::from_secs(1), async {
            while !*exited.borrow() {
                if exited.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        if waited.is_err() {
            let _ = self.tree.kill();
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        // The exit watcher also holds the tree; kill explicitly so dropping
        // the last handle to a live host never leaves it running.
        if !self.has_exited() {
            let _ = self.tree.kill();
        }
    }
}

fn handle_host_message(
    message: HostMessage,
    pending: &Mutex<HashMap<u64, PendingCall>>,
    outbound: &mpsc::Sender<Vec<u8>>,
    events: &dyn HostEvents,
    handshake: &Mutex<Handshake>,
) {
    let send = |value: Value| {
        if let Ok(frame) = protocol::encode_frame(&value) {
            let _ = outbound.try_send(frame);
        }
    };
    match message {
        HostMessage::Response { id, outcome } => {
            let Some(call) = pending.lock().expect("pending lock").remove(&id) else {
                tracing::debug!(target: "extension_host", id, "dropping late host response");
                return;
            };
            let result = if call.revoked {
                Err(HostCallError::Cancelled(
                    "extension was revoked".to_string(),
                ))
            } else {
                match outcome {
                    Ok(value) => Ok(value),
                    Err(error) if error.code == error_code::CANCELLED => {
                        Err(HostCallError::Cancelled(error.message))
                    }
                    Err(error) => Err(HostCallError::Rpc {
                        code: error.code,
                        message: error.message,
                    }),
                }
            };
            let _ = call.tx.send(result);
        }
        HostMessage::Request { id, request } => match request {
            HostRequest::Register(params) => {
                let result = events.register(&params);
                send(protocol::response_ok(
                    id,
                    serde_json::to_value(result).unwrap_or_else(|_| json!({"refused": "internal"})),
                ));
            }
            HostRequest::Unregister(params) => {
                events.unregister(&params);
                send(protocol::response_ok(id, json!({})));
            }
        },
        HostMessage::Notification(notification) => match notification {
            HostNotification::Hello(hello) => {
                if let Some(tx) = handshake.lock().expect("handshake lock").hello.take() {
                    let _ = tx.send(hello);
                }
            }
            HostNotification::Ready => {
                if let Some(tx) = handshake.lock().expect("handshake lock").ready.take() {
                    let _ = tx.send(());
                }
            }
            HostNotification::Faulted(params) => events.faulted(&params),
            HostNotification::Log(log) => {
                events.log(&log);
                let plugin = log.plugin_id.as_deref().unwrap_or("host");
                match log.level.as_str() {
                    "error" => tracing::warn!(target: "extension_host", plugin, "{}", log.msg),
                    "warn" => tracing::info!(target: "extension_host", plugin, "{}", log.msg),
                    _ => tracing::debug!(target: "extension_host", plugin, "{}", log.msg),
                }
            }
            // Phase 1 has no host-originated requests to cancel.
            HostNotification::Cancel(_) => {}
        },
    }
}

/// Where the embedded bundle is written: `<root>/extension-host/<sha256>/`.
#[must_use]
pub fn bundle_dir(root: &Path, sha256: &str) -> PathBuf {
    root.join("extension-host").join(sha256)
}
