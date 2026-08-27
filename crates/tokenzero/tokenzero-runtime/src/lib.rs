#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokenzero_core::shell_display_command_from_argv_for_platform;
use zero_process::VerifiedChild;

/// Zero-process owner-session binding for every TokenZero engine shell child.
/// Every signal (timeout, IO grace, background teardown, raw-worker cancel)
/// goes through the hub-owned `VerifiedChild` tree handle under this binding;
/// no TokenZero code ever signals a numeric pid or process group.
pub const PROCESS_OWNER_SESSION: &str = "tokenzero-engine";
/// Worker generation for engine shell children. The engine never respawns a
/// shell stem, so the binding generation is constant for the process.
pub const PROCESS_GENERATION: u64 = 0;
/// Bounded graceful window for shell tree teardown: SIGTERM to the exact
/// owned group, the full window for graceful exit, then SIGKILL escalation
/// and a group-gone proof.
pub const SHELL_TEARDOWN_GRACE: Duration = Duration::from_millis(250);

pub const DEFAULT_SHELL_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_SHELL_SPILL_BYTES: usize = 1024 * 1024;
/// Hard ceiling for in-memory stream capture. Oversize env/policy values fail
/// at run rather than allocating unbounded preview buffers.
pub const MAX_SHELL_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("empty command")]
    EmptyCommand,
    #[error("spawned command {0} pipe is unavailable")]
    MissingPipe(&'static str),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    Argv,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub execution_mode: ExecutionMode,
    pub argv: Vec<String>,
    pub shell: Option<String>,
    pub shell_arg: Option<String>,
    pub cwd: Option<String>,
    pub platform: String,
    pub explicit_binary: bool,
    pub alias_dependency: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AllocatorPressureRelief {
    pub attempted: bool,
    pub reclaimed_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamCapture {
    pub bytes_seen: usize,
    pub captured_bytes: usize,
    pub truncated: bool,
    /// Whether the in-memory captured bytes were valid UTF-8 without replacement.
    /// Defaults false for older records, so absence never authorizes exact text recovery.
    #[serde(default)]
    pub captured_utf8_lossless: bool,
    /// SHA-256 over every byte read from the stream, including bytes beyond
    /// the in-memory preview. Absence never authorizes exact recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_stream_sha256: Option<String>,
    pub spill_path: Option<String>,
    pub spill_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutputPolicy {
    pub per_stream_capture_bytes: usize,
    pub spill_threshold_bytes: usize,
    pub spill_dir: Option<PathBuf>,
}

impl Default for RunOutputPolicy {
    fn default() -> Self {
        let env_usize = |key, default| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        Self {
            per_stream_capture_bytes: env_usize(
                "TOKENZERO_SHELL_CAPTURE_BYTES",
                DEFAULT_SHELL_CAPTURE_BYTES,
            ),
            spill_threshold_bytes: env_usize(
                "TOKENZERO_SHELL_SPILL_BYTES",
                DEFAULT_SHELL_SPILL_BYTES,
            ),
            spill_dir: std::env::var_os("TOKENZERO_SHELL_SPILL_DIR").map(PathBuf::from),
        }
        .normalized()
    }
}

impl RunOutputPolicy {
    pub fn normalized(mut self) -> Self {
        if self.per_stream_capture_bytes == 0 {
            self.per_stream_capture_bytes = DEFAULT_SHELL_CAPTURE_BYTES;
        }
        if self.spill_threshold_bytes == 0 {
            self.spill_threshold_bytes = DEFAULT_SHELL_SPILL_BYTES;
        }
        self.spill_threshold_bytes = self
            .spill_threshold_bytes
            .min(self.per_stream_capture_bytes);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub ok: bool,
    pub command: String,
    pub argv: Vec<String>,
    pub execution_mode: ExecutionMode,
    pub alias_dependency: bool,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_capture: StreamCapture,
    pub stderr_capture: StreamCapture,
    pub capture_limit_bytes: usize,
    pub spill_threshold_bytes: usize,
    pub allocator_pressure_relief: AllocatorPressureRelief,
    pub timed_out: bool,
    /// Main child exited; group terminated after IO grace (not a timeout).
    #[serde(default)]
    pub io_grace_expired: bool,
    pub duration_ms: u128,
}

pub fn current_platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "posix" }
}

pub fn plan_command(
    argv: &[String],
    cwd: Option<&Path>,
    explicit_shell: bool,
) -> Result<RuntimePlan, RuntimeError> {
    plan_command_for_platform(argv, cwd, explicit_shell, current_platform())
}

pub fn plan_command_for_platform(
    argv: &[String],
    cwd: Option<&Path>,
    explicit_shell: bool,
    platform: &str,
) -> Result<RuntimePlan, RuntimeError> {
    if argv.is_empty() || argv.iter().all(String::is_empty) {
        return Err(RuntimeError::EmptyCommand);
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err(invalid_runtime_input(
            "command arguments must not contain NUL",
        ));
    }
    let make = |execution_mode, argv, shell, shell_arg, explicit_binary| RuntimePlan {
        execution_mode,
        argv,
        shell,
        shell_arg,
        cwd: cwd.map(|p| p.display().to_string()),
        platform: platform.into(),
        explicit_binary,
        alias_dependency: false,
    };
    let windows = matches!(platform, "windows" | "cmd" | "powershell" | "pwsh");
    let first = argv.first();
    let powershell = windows
        && !first.is_some_and(|v| is_windows_shell_host(v))
        && looks_like_powershell_syntax(&argv.join(" "));
    let needs_shell = explicit_shell
        || (argv.len() == 1 && contains_platform_shell_syntax(&argv[0], platform))
        || argv_has_shell_operator_tokens(argv)
        || powershell
        || (windows && first.is_some_and(|v| is_windows_shell_builtin(v)))
        || (!windows && first.is_some_and(|v| is_posix_shell_builtin(v)));
    if !needs_shell {
        return Ok(make(ExecutionMode::Argv, argv.to_vec(), None, None, true));
    }
    let (host, arg, syntax, prefix): (&str, &str, &str, &[&str]) = match (windows, powershell) {
        (true, true) => (
            "powershell",
            "-Command",
            "powershell",
            &["powershell", "-NoProfile", "-Command"],
        ),
        (true, false) => ("cmd", "/C", "cmd", &["cmd", "/C"]),
        // Pipefail prevents a successful final stage from masking an earlier failure.
        (false, _) => (
            "/bin/bash",
            "-c",
            "posix",
            &["/bin/bash", "-o", "pipefail", "-c"],
        ),
    };
    let mut shell_argv = prefix.iter().map(|s| (*s).into()).collect::<Vec<_>>();
    shell_argv.push(shell_command_string_from_argv(argv, syntax));
    Ok(make(
        ExecutionMode::Shell,
        shell_argv,
        Some(host.into()),
        Some(arg.into()),
        false,
    ))
}

fn is_posix_shell_builtin(program: &str) -> bool {
    matches!(
        program,
        "." | "alias"
            | "bg"
            | "break"
            | "cd"
            | "command"
            | "continue"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "fg"
            | "getopts"
            | "hash"
            | "jobs"
            | "read"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "source"
            | "times"
            | "trap"
            | "type"
            | "typeset"
            | "ulimit"
            | "umask"
            | "unalias"
            | "unset"
            | "wait"
    )
}

fn shell_command_string_from_argv(argv: &[String], shell_platform: &str) -> String {
    if argv.len() == 1 {
        return argv[0].clone();
    }
    argv.iter()
        .map(|arg| {
            if is_shell_operator_token(arg) {
                arg.clone()
            } else {
                quote_for(shell_platform, std::slice::from_ref(arg))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn run_command(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
) -> Result<RunResult, RuntimeError> {
    run_command_with_policy(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        RunOutputPolicy::default(),
    )
}

pub fn run_command_with_policy(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
) -> Result<RunResult, RuntimeError> {
    run_command_with_policy_observer(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        output_policy,
        |_, _, _| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_command_with_policy_observer<F>(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
    observer: F,
) -> Result<RunResult, RuntimeError>
where
    F: FnMut(Option<u32>, Option<u32>, &'static str),
{
    run_command_with_policy_observers(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        output_policy,
        observer,
        |_, _| {},
    )
}

/// Like [`run_command_with_policy_observer`] but additionally hands the exact
/// hub-owned tree handle to `on_child` at spawn, before any wait. Callers
/// that must cancel the child later (raw-worker cancel, background job
/// teardown) retain a `Clone` of the [`VerifiedChild`]; signaling goes
/// through that handle under [`PROCESS_OWNER_SESSION`]/[`PROCESS_GENERATION`],
/// never through a numeric pid or process group.
#[allow(clippy::too_many_arguments)]
pub fn run_command_with_policy_observer_with_child<F, H>(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
    observer: F,
    on_child: H,
) -> Result<RunResult, RuntimeError>
where
    F: FnMut(Option<u32>, Option<u32>, &'static str),
    H: FnOnce(&VerifiedChild),
{
    run_command_with_policy_observers_with_child(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        output_policy,
        observer,
        |_, _| {},
        on_child,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_command_with_policy_observers<F, G>(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
    observer: F,
    stream_observer: G,
) -> Result<RunResult, RuntimeError>
where
    F: FnMut(Option<u32>, Option<u32>, &'static str),
    G: Fn(&'static str, &[u8]) + Send + Sync + 'static,
{
    run_command_with_policy_observers_with_child(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        output_policy,
        observer,
        stream_observer,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_command_with_policy_observers_with_child<F, G, H>(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
    observer: F,
    stream_observer: G,
    on_child: H,
) -> Result<RunResult, RuntimeError>
where
    F: FnMut(Option<u32>, Option<u32>, &'static str),
    G: Fn(&'static str, &[u8]) + Send + Sync + 'static,
    H: FnOnce(&VerifiedChild),
{
    run_command_with_policy_observers_with_child_and_cancel(
        argv,
        cwd,
        env_overrides,
        stdin,
        timeout,
        explicit_shell,
        output_policy,
        observer,
        stream_observer,
        on_child,
        || false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_command_with_policy_observers_with_child_and_cancel<F, G, H, C>(
    argv: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
    stdin: Option<&str>,
    timeout: Duration,
    explicit_shell: bool,
    output_policy: RunOutputPolicy,
    mut observer: F,
    stream_observer: G,
    on_child: H,
    is_cancelled: C,
) -> Result<RunResult, RuntimeError>
where
    F: FnMut(Option<u32>, Option<u32>, &'static str),
    G: Fn(&'static str, &[u8]) + Send + Sync + 'static,
    H: FnOnce(&VerifiedChild),
    C: Fn() -> bool,
{
    let output_policy = output_policy.normalized();
    // `Instant + Duration::MAX` cannot be represented. Collapsing that to
    // `Instant::now()` would spawn then immediately timeout; fail loud first.
    if Instant::now().checked_add(timeout).is_none() {
        return Err(invalid_runtime_input(
            "timeout duration overflows Instant",
        ));
    }
    if output_policy.per_stream_capture_bytes > MAX_SHELL_CAPTURE_BYTES {
        return Err(invalid_runtime_input(format!(
            "per_stream_capture_bytes {} exceeds hard max {MAX_SHELL_CAPTURE_BYTES}",
            output_policy.per_stream_capture_bytes
        )));
    }
    if let Some(dir) = output_policy.spill_dir.as_deref()
        && unexpanded_tilde_path(dir)
    {
        return Err(invalid_runtime_input(format!(
            "unexpanded ~ spill path: {}",
            dir.display()
        )));
    }
    if let Some(cwd) = cwd
        && unexpanded_tilde_path(cwd)
    {
        return Err(invalid_runtime_input(format!(
            "unexpanded ~ cwd: {}",
            cwd.display()
        )));
    }
    if let Some(env) = env_overrides {
        for (key, value) in env {
            validate_env_pair(key, value)?;
        }
    }
    let plan = plan_command(argv, cwd, explicit_shell)?;
    let command_display = match plan.execution_mode {
        ExecutionMode::Shell => plan.argv.last().cloned().unwrap_or_else(|| argv.join(" ")),
        ExecutionMode::Argv => {
            shell_display_command_from_argv_for_platform(&plan.argv, &plan.platform)
        }
    };
    let result_command = command_display.clone();
    let result_argv = plan.argv.clone();
    let (result_mode, result_alias_dependency) = (plan.execution_mode, plan.alias_dependency);
    // Always echo the effective cwd. When the caller omits cwd the child inherits
    // process cwd — report that path instead of leaving telemetry null.
    let effective_cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let result_cwd = effective_cwd
        .as_ref()
        .map(|path| path.display().to_string());
    let capture_limit_bytes = output_policy.per_stream_capture_bytes;
    let spill_threshold_bytes = output_policy.spill_threshold_bytes;
    let start = Instant::now();
    let (program, rest) = plan.argv.split_first().ok_or(RuntimeError::EmptyCommand)?;
    let mut command = match plan.execution_mode {
        ExecutionMode::Argv => {
            command_for_argv(program, rest, effective_cwd.as_deref(), env_overrides)
        }
        ExecutionMode::Shell => {
            let mut cmd = Command::new(program);
            cmd.args(rest);
            cmd
        }
    };
    if let Some(cwd) = effective_cwd.as_deref() {
        command.current_dir(cwd);
    }
    // Caller-selected commands only; explicit env overrides applied after scrub.
    scrub_inherited_orchestration_env(&mut command);
    if let Some(env) = env_overrides {
        command.envs(env);
    }
    if plan.execution_mode == ExecutionMode::Shell {
        scrub_shell_injection_env(&mut command);
    }
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Hub-owned tree spawn: Unix isolates the child in its own process group
    // at exec; Windows assigns it to a kill-on-close job before it runs. The
    // exact tree handle is the only teardown authority from here on.
    let (verified, pipes) =
        VerifiedChild::spawn_tree_with_pipes(command, PROCESS_OWNER_SESSION, PROCESS_GENERATION)
            .map_err(RuntimeError::Io)?;
    let stdout = match required_child_pipe(pipes.stdout, "stdout") {
        Ok(stdout) => stdout,
        Err(error) => {
            terminate_child_after_setup_error(&verified);
            return Err(error);
        }
    };
    let stderr = match required_child_pipe(pipes.stderr, "stderr") {
        Ok(stderr) => stderr,
        Err(error) => {
            terminate_child_after_setup_error(&verified);
            return Err(error);
        }
    };
    let child_stdin = if stdin.is_some() {
        match required_child_pipe(pipes.stdin, "stdin") {
            Ok(stdin) => Some(stdin),
            Err(error) => {
                terminate_child_after_setup_error(&verified);
                return Err(error);
            }
        }
    } else {
        None
    };
    on_child(&verified);
    observer(
        Some(verified.child_id()),
        verified_tree_pgid(&verified),
        "running",
    );
    let stdout_policy = output_policy.clone();
    let stderr_policy = output_policy.clone();
    let stream_observer = Arc::new(stream_observer);
    let stdout_observer = Arc::clone(&stream_observer);
    let stderr_observer = Arc::clone(&stream_observer);
    let stdout_reader = spawn_io_worker("stdout reader", move || {
        capture_reader_with_observer(stdout, "stdout", stdout_policy, move |chunk| {
            stdout_observer("stdout", chunk);
        })
    });
    let stderr_reader = spawn_io_worker("stderr reader", move || {
        capture_reader_with_observer(stderr, "stderr", stderr_policy, move |chunk| {
            stderr_observer("stderr", chunk);
        })
    });
    // Stdin writes can block; keep them off the wait_timeout path.
    let stdin_writer = spawn_stdin_writer(stdin, child_stdin);
    let mut force_timed_out = false;
    let mut force_cancelled = false;
    let mut settlement_error = None;
    let wait_deadline = deadline_from(Instant::now(), timeout);
    let status = loop {
        if let Some(status) = verified.terminal_status() {
            break Some(status);
        }
        if is_cancelled() {
            force_cancelled = true;
            match terminate_verified_child(&verified) {
                Ok(status) => break Some(status),
                Err(error) => {
                    settlement_error = Some(error);
                    break None;
                }
            }
        }
        let now = Instant::now();
        if now >= wait_deadline {
            force_timed_out = true;
            match terminate_verified_child(&verified) {
                Ok(status) => break Some(status),
                Err(error) => {
                    settlement_error = Some(error);
                    break None;
                }
            }
        }
        let poll = wait_deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(25));
        if !verified.wait_for_exit(poll) {
            continue;
        }
        if let Some(status) = verified.terminal_status() {
            break Some(status);
        }
        match verified.wait(
            PROCESS_OWNER_SESSION,
            PROCESS_GENERATION,
            Duration::ZERO,
            SHELL_TEARDOWN_GRACE,
        ) {
            Ok(status) => break Some(status),
            Err(error) => {
                if let Some(status) = verified.terminal_status() {
                    break Some(status);
                }
                settlement_error = Some(RuntimeError::Io(identity_error_io(error)));
                break None;
            }
        }
    };
    let process_io = collect_process_io(
        stdin_writer,
        stdout_reader,
        stderr_reader,
        force_timed_out || force_cancelled,
        start,
        timeout,
        &verified,
        !(force_timed_out || force_cancelled),
    )?;
    if let Some(error) = settlement_error {
        return Err(error);
    }
    if force_cancelled {
        observer(None, None, "cancelled_killed");
        return Err(RuntimeError::Cancelled);
    }
    let status = status
        .ok_or_else(|| RuntimeError::Io(identity_error_io(zero_process::IdentityError::Missing)))?;
    let timed_out = force_timed_out || process_io.timed_out;
    observer(
        None,
        None,
        if timed_out {
            "timed_out_killed"
        } else {
            "completed"
        },
    );
    let allocator_pressure_relief = allocator_pressure_relief_after_large_capture(
        &process_io.stdout.capture,
        &process_io.stderr.capture,
    );
    Ok(RunResult {
        ok: !timed_out && status.success(),
        command: result_command,
        argv: result_argv,
        execution_mode: result_mode,
        alias_dependency: result_alias_dependency,
        cwd: result_cwd,
        exit_code: status.code(),
        stdout: process_io.stdout.text,
        stderr: process_io.stderr.text,
        stdout_capture: process_io.stdout.capture,
        stderr_capture: process_io.stderr.capture,
        capture_limit_bytes,
        spill_threshold_bytes,
        allocator_pressure_relief,
        timed_out,
        io_grace_expired: process_io.io_grace_expired,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn terminate_verified_child(
    verified: &VerifiedChild,
) -> Result<std::process::ExitStatus, RuntimeError> {
    if let Err(error) = signal_tree(verified)
        && verified.terminal_status().is_none()
    {
        return Err(RuntimeError::Io(error));
    }
    if let Err(error) = verified.revoke()
        && verified.terminal_status().is_none()
    {
        return Err(RuntimeError::Io(identity_error_io(error)));
    }
    verified
        .terminal_status()
        .ok_or_else(|| RuntimeError::Io(identity_error_io(zero_process::IdentityError::Missing)))
}

fn allocator_pressure_relief_after_large_capture(
    stdout: &StreamCapture,
    stderr: &StreamCapture,
) -> AllocatorPressureRelief {
    if [stdout, stderr]
        .iter()
        .any(|capture| capture.truncated || capture.spill_path.is_some())
    {
        platform_allocator_pressure_relief()
    } else {
        AllocatorPressureRelief::default()
    }
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "macOS allocator pressure relief requires a tiny FFI shim"
)]
fn platform_allocator_pressure_relief() -> AllocatorPressureRelief {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }
    let reclaimed = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    AllocatorPressureRelief {
        attempted: true,
        reclaimed_bytes: Some(reclaimed),
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_allocator_pressure_relief() -> AllocatorPressureRelief {
    AllocatorPressureRelief::default()
}

#[derive(Debug)]
struct CapturedStream {
    text: String,
    capture: StreamCapture,
}

struct ProcessIo {
    stdout: CapturedStream,
    stderr: CapturedStream,
    timed_out: bool,
    io_grace_expired: bool,
}

struct IoWorker<T> {
    name: &'static str,
    receiver: Receiver<std::io::Result<T>>,
}

type IoPoolJob = Box<dyn FnOnce() + Send>;

struct IoPoolState {
    jobs: VecDeque<IoPoolJob>,
}

struct IoPool {
    state: Mutex<IoPoolState>,
    woke: Condvar,
    spawned: AtomicUsize,
}

fn io_pool_size() -> usize {
    // stdin + stdout + stderr readers can run at once per exec.
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_mul(3))
        .unwrap_or(12)
        .clamp(6, 48)
}

fn io_pool() -> &'static IoPool {
    static POOL: OnceLock<&'static IoPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let size = io_pool_size();
        let pool: &'static IoPool = Box::leak(Box::new(IoPool {
            state: Mutex::new(IoPoolState {
                jobs: VecDeque::new(),
            }),
            woke: Condvar::new(),
            spawned: AtomicUsize::new(0),
        }));
        for index in 0..size {
            thread::Builder::new()
                .name(format!("tokenzero-io-{index}"))
                .spawn(move || io_pool_worker(pool))
                .unwrap_or_else(|err| panic!("tokenzero-runtime io pool thread: {err}"));
            pool.spawned.fetch_add(1, Ordering::Relaxed);
        }
        pool
    })
}

fn io_pool_worker(pool: &'static IoPool) {
    loop {
        let job = {
            let mut guard = pool.state.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(job) = guard.jobs.pop_front() {
                    break job;
                }
                guard = pool.woke.wait(guard).unwrap_or_else(|p| p.into_inner());
            }
        };
        let _ = catch_unwind(AssertUnwindSafe(job));
    }
}

fn submit_io_job(job: IoPoolJob) {
    let pool = io_pool();
    let mut guard = pool.state.lock().unwrap_or_else(|p| p.into_inner());
    guard.jobs.push_back(job);
    drop(guard);
    pool.woke.notify_one();
}

#[cfg(test)]
fn io_pool_spawned_threads() -> usize {
    io_pool().spawned.load(Ordering::Relaxed)
}

fn spawn_io_worker<T: Send + 'static>(
    name: &'static str,
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> IoWorker<T> {
    let (sender, receiver) = mpsc::channel();
    submit_io_job(Box::new(move || {
        let _ = sender.send(work());
    }));
    IoWorker { name, receiver }
}

fn required_child_pipe<T>(pipe: Option<T>, name: &'static str) -> Result<T, RuntimeError> {
    pipe.ok_or(RuntimeError::MissingPipe(name))
}

fn terminate_child_after_setup_error(verified: &VerifiedChild) {
    let _ = signal_tree(verified);
    let _ = verified.revoke();
}

/// Bounded graceful teardown of the exact owned shell tree. After this
/// returns, the root is reaped and the group is proven gone.
fn signal_tree(verified: &VerifiedChild) -> io::Result<zero_process::SignalOutcome> {
    verified
        .signal_graceful_for(
            PROCESS_OWNER_SESSION,
            PROCESS_GENERATION,
            SHELL_TEARDOWN_GRACE,
        )
        .map_err(identity_error_io)
}

fn identity_error_io(error: zero_process::IdentityError) -> io::Error {
    match error {
        zero_process::IdentityError::Io(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

/// Evidence-only process group id of the spawned tree: on Unix the
/// hub-owned spawn places the root in its own process group, so the group id
/// equals the root pid. Observers receive this value for accounting and
/// evidence; no TokenZero code ever signals it.
fn verified_tree_pgid(verified: &VerifiedChild) -> Option<u32> {
    #[cfg(unix)]
    {
        Some(verified.child_id())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn spawn_stdin_writer(input: Option<&str>, stdin: Option<ChildStdin>) -> Option<IoWorker<()>> {
    input.zip(stdin).map(|(input, mut stdin)| {
        let input = input.as_bytes().to_vec();
        spawn_io_worker("stdin writer", move || stdin.write_all(&input))
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_process_io(
    mut stdin: Option<IoWorker<()>>,
    mut stdout: IoWorker<CapturedStream>,
    mut stderr: IoWorker<CapturedStream>,
    tolerate_write_error: bool,
    start: Instant,
    timeout: Duration,
    verified: &VerifiedChild,
    child_exited: bool,
) -> Result<ProcessIo, RuntimeError> {
    let deadline = if child_exited {
        deadline_from(Instant::now(), CHILD_EXITED_IO_GRACE).min(deadline_from(start, timeout))
    } else {
        deadline_from(start, timeout)
    };
    let mut stdin_result = poll_stdin(stdin.as_mut(), deadline)?;
    let mut stdout_result = poll_worker(&mut stdout, deadline)?;
    let mut stderr_result = poll_worker(&mut stderr, deadline)?;
    let incomplete = stdin_result.is_none() || stdout_result.is_none() || stderr_result.is_none();
    let timed_out = incomplete && !child_exited;
    let io_grace_expired = incomplete && child_exited;
    if incomplete {
        // Bounded re-signal of the exact owned tree (already swept by the
        // wait/teardown above, so this is purely defensive): it closes
        // inherited pipe writers so blocked readers can finish.
        let _ = signal_tree(verified);
        let cleanup = deadline_from(Instant::now(), PROCESS_IO_SHUTDOWN_GRACE);
        stdin_result = stdin_result.or(poll_stdin(stdin.as_mut(), cleanup)?);
        stdout_result = stdout_result.or(poll_worker(&mut stdout, cleanup)?);
        stderr_result = stderr_result.or(poll_worker(&mut stderr, cleanup)?);
    }
    // Never return while leaving live reader JoinHandles detached: if cleanup
    // still left a worker blocked, terminate again and join with a final grace.
    stdin_result = ensure_worker_joined(stdin.as_mut(), stdin_result, verified)?;
    stdout_result = ensure_worker_joined(Some(&mut stdout), stdout_result, verified)?;
    stderr_result = ensure_worker_joined(Some(&mut stderr), stderr_result, verified)?;
    let stdin_result = stdin_result.ok_or_else(|| worker_timeout("shell stdin writer"))?;
    if !tolerate_write_error && !timed_out && !io_grace_expired {
        stdin_result?;
    }
    Ok(ProcessIo {
        stdout: stdout_result.ok_or_else(|| worker_timeout("shell stdout reader"))??,
        stderr: stderr_result.ok_or_else(|| worker_timeout("shell stderr reader"))??,
        timed_out,
        io_grace_expired,
    })
}

/// After process-tree terminate, require the worker to finish and be joined so
/// inherited-pipe readers are never detached on the timeout error path.
fn ensure_worker_joined<T>(
    worker: Option<&mut IoWorker<T>>,
    result: Option<std::io::Result<T>>,
    verified: &VerifiedChild,
) -> Result<Option<std::io::Result<T>>, RuntimeError> {
    if result.is_some() {
        return Ok(result);
    }
    let Some(worker) = worker else {
        return Ok(None);
    };
    let _ = signal_tree(verified);
    let final_grace = deadline_from(Instant::now(), PROCESS_IO_JOIN_GRACE);
    let recovered = poll_worker(worker, final_grace)?;
    if recovered.is_some() {
        return Ok(recovered);
    }
    // Last resort: the pooled worker stays on the pipe until terminate closes
    // it. Do not spawn a joiner thread; that was the old spawn-per-exec leak.
    let _ = worker;
    Ok(None)
}

fn poll_stdin(
    worker: Option<&mut IoWorker<()>>,
    deadline: Instant,
) -> Result<Option<std::io::Result<()>>, RuntimeError> {
    worker.map_or(Ok(Some(Ok(()))), |worker| poll_worker(worker, deadline))
}

fn poll_worker<T>(
    worker: &mut IoWorker<T>,
    deadline: Instant,
) -> Result<Option<std::io::Result<T>>, RuntimeError> {
    match worker
        .receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        Ok(result) => Ok(Some(result)),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Io(std::io::Error::other(
            format!("{} exited without reporting a result", worker.name),
        ))),
    }
}

fn deadline_from(start: Instant, timeout: Duration) -> Instant {
    start.checked_add(timeout).unwrap_or_else(|| {
        // Unrepresentable timeout must not become "already expired".
        Instant::now()
            .checked_add(Duration::from_secs(24 * 60 * 60))
            .unwrap_or_else(Instant::now)
    })
}

const PROCESS_IO_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const CHILD_EXITED_IO_GRACE: Duration = Duration::from_millis(250);
/// Final join grace after a second terminate when inherited pipes still block readers.
const PROCESS_IO_JOIN_GRACE: Duration = Duration::from_millis(500);

fn worker_timeout(name: &str) -> RuntimeError {
    RuntimeError::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("{name} did not close after process timeout cleanup"),
    ))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn capture_reader_with_observer<R: Read, F: FnMut(&[u8])>(
    mut reader: R,
    stream_name: &str,
    policy: RunOutputPolicy,
    mut observer: F,
) -> std::io::Result<CapturedStream> {
    let policy = policy.normalized();
    let mut captured = Vec::with_capacity(policy.per_stream_capture_bytes.min(64 * 1024));
    let mut bytes_seen = 0usize;
    let mut full_stream_hasher = Sha256::new();
    let mut spill = SpillWriter::new(stream_name, policy.spill_dir.as_deref());
    let mut buf = [0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let chunk = &buf[..read];
        full_stream_hasher.update(chunk);
        observer(chunk);
        bytes_seen = bytes_seen.saturating_add(read);
        let captured_before = captured.len();
        if captured.len() < policy.per_stream_capture_bytes {
            let available = policy.per_stream_capture_bytes - captured.len();
            captured.extend_from_slice(&chunk[..read.min(available)]);
        }
        if bytes_seen > policy.spill_threshold_bytes {
            spill.write(chunk, captured_before, &captured)?;
        }
    }
    spill.retain = true;
    Ok(CapturedStream {
        text: String::from_utf8_lossy(&captured).into_owned(),
        capture: StreamCapture {
            bytes_seen,
            captured_bytes: captured.len(),
            truncated: bytes_seen > captured.len(),
            captured_utf8_lossless: std::str::from_utf8(&captured).is_ok(),
            full_stream_sha256: Some(lowercase_hex(&full_stream_hasher.finalize())),
            spill_path: spill.path.as_ref().map(|path| path.display().to_string()),
            spill_bytes: spill.bytes_written,
        },
    })
}

#[derive(Debug)]
struct SpillWriter {
    stream_name: String,
    dir: Option<PathBuf>,
    file: Option<File>,
    path: Option<PathBuf>,
    bytes_written: usize,
    retain: bool,
}

impl SpillWriter {
    fn new(stream_name: &str, dir: Option<&Path>) -> Self {
        Self {
            stream_name: stream_name.to_string(),
            dir: dir.map(Path::to_path_buf),
            file: None,
            path: None,
            bytes_written: 0,
            retain: false,
        }
    }

    fn write(
        &mut self,
        chunk: &[u8],
        captured_before: usize,
        captured: &[u8],
    ) -> std::io::Result<()> {
        if self.file.is_none() {
            let root = self
                .dir
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("tokenzero-spills"));
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = root.join(format!(
                "tokenzero-{}-{stamp}-{}.log",
                std::process::id(),
                self.stream_name
            ));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = File::create(&path)?;
            self.path = Some(path);
            self.file = Some(file);
            self.file
                .as_mut()
                .expect("spill file initialized")
                .write_all(&captured[..captured_before])?;
            self.bytes_written = self.bytes_written.saturating_add(captured_before);
        }
        self.file
            .as_mut()
            .expect("spill file initialized")
            .write_all(chunk)?;
        self.bytes_written = self.bytes_written.saturating_add(chunk.len());
        Ok(())
    }
}

impl Drop for SpillWriter {
    fn drop(&mut self) {
        self.file.take();
        if !self.retain {
            if let Some(path) = self.path.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Age after which a spill file is reclaimable (session path pointers expire).
pub const DEFAULT_SPILL_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Post-age-pass byte ceiling; oldest spills reclaimed first.
pub const DEFAULT_SPILL_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Metadata-work ceiling: at most this many directory entries are visited per prune.
pub const DEFAULT_SPILL_MAX_SCAN_ENTRIES: usize = 4096;
/// Wall deadline for a prune pass; mid-scan work aborts once the deadline elapses.
pub const DEFAULT_SPILL_PRUNE_DEADLINE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpillPruneReport {
    pub dir: String,
    pub dry_run: bool,
    pub scanned_files: usize,
    pub removed_files: usize,
    pub removed_bytes: u64,
    pub kept_files: usize,
    pub kept_bytes: u64,
    pub failed_removals: usize,
    /// Cap applied to directory enumeration (metadata-work bound).
    pub scan_budget: usize,
    /// True when enumeration stopped because `scan_budget` was exhausted.
    pub scan_truncated: bool,
    /// True when enumeration stopped because the prune deadline elapsed.
    pub deadline_elapsed: bool,
}

/// Prune with default scan budget and wall deadline.
pub fn prune_spill_dir(
    dir: &Path,
    max_age: Duration,
    max_total_bytes: u64,
    dry_run: bool,
) -> SpillPruneReport {
    prune_spill_dir_bounded(
        dir,
        max_age,
        max_total_bytes,
        dry_run,
        DEFAULT_SPILL_MAX_SCAN_ENTRIES,
        Some(Instant::now() + DEFAULT_SPILL_PRUNE_DEADLINE),
    )
}

/// Prune spill files with an explicit scanned-entry budget and optional deadline.
///
/// Storage-byte policy (`max_total_bytes`) is separate from metadata-work policy
/// (`max_scan_entries` / `deadline`): the latter bounds queue size and sort work
/// even when every visited file is a zero-byte fresh spill.
pub fn prune_spill_dir_bounded(
    dir: &Path,
    max_age: Duration,
    max_total_bytes: u64,
    dry_run: bool,
    max_scan_entries: usize,
    deadline: Option<Instant>,
) -> SpillPruneReport {
    let mut report = SpillPruneReport {
        dir: dir.display().to_string(),
        dry_run,
        scan_budget: max_scan_entries,
        ..Default::default()
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return report;
    };
    let now = SystemTime::now();
    let mut fresh = Vec::new();
    let mut visited = 0usize;
    for entry in entries.flatten().take(max_scan_entries) {
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            report.deadline_elapsed = true;
            break;
        }
        visited += 1;
        let path = entry.path();
        let valid_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("tokenzero-") && name.ends_with(".log"));
        let Ok(meta) = entry.metadata() else { continue };
        if !valid_name || !meta.is_file() {
            continue;
        }
        report.scanned_files += 1;
        let modified = meta.modified().unwrap_or(now);
        if now.duration_since(modified).is_ok_and(|age| age > max_age) {
            remove_spill_file(&path, meta.len(), dry_run, &mut report);
        } else {
            fresh.push((modified, meta.len(), path));
        }
    }
    report.scan_truncated = !report.deadline_elapsed && visited >= max_scan_entries;
    // Queue is already capped by the scan budget; sort work is O(B log B), not O(N log N).
    fresh.sort_by_key(|item| item.0);
    let mut bytes = fresh.iter().map(|item| item.1).sum::<u64>();
    let split = fresh
        .iter()
        .take_while(|item| {
            if bytes <= max_total_bytes {
                return false;
            }
            remove_spill_file(&item.2, item.1, dry_run, &mut report);
            bytes = bytes.saturating_sub(item.1);
            true
        })
        .count();
    for item in &fresh[split..] {
        report.kept_files += 1;
        report.kept_bytes += item.1;
    }
    report
}

fn remove_spill_file(path: &Path, len: u64, dry_run: bool, report: &mut SpillPruneReport) {
    if dry_run || fs::remove_file(path).is_ok() {
        report.removed_files += 1;
        report.removed_bytes += len;
    } else {
        report.failed_removals += 1;
    }
}

const ORCHESTRATION_ENV_PREFIXES: [&str; 4] = ["TOKENZERO_", "ZEROSTACK_", "FSZERO_", "GRAPHZERO_"];

fn scrub_inherited_orchestration_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|name| {
            ORCHESTRATION_ENV_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
                || is_shell_injection_env_key(name)
        }) {
            command.env_remove(key);
        }
    }
}

fn scrub_shell_injection_env(command: &mut Command) {
    // bash -c sources BASH_ENV/ENV even non-interactively; PS4 interpolates
    // under xtrace. Strip after overrides so neither inheritance nor --env
    // can inject a startup script into the wrapper shell.
    for key in ["BASH_ENV", "ENV", "BASHOPTS", "SHELLOPTS", "PS4"] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|name| name.starts_with("BASH_FUNC_"))
        {
            command.env_remove(key);
        }
    }
}

fn command_for_argv(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    env_overrides: Option<&BTreeMap<String, String>>,
) -> Command {
    #[cfg(windows)]
    {
        let resolved = resolve_windows_program(program, cwd, env_overrides);
        if resolved
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        {
            let mut cmd = Command::new("cmd");
            cmd.arg("/D").arg("/S").arg("/C").arg(
                std::iter::once("call".to_string())
                    .chain(std::iter::once(quote_windows_cmd(
                        &resolved.display().to_string(),
                    )))
                    .chain(args.iter().map(|arg| quote_windows_cmd(arg)))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            return cmd;
        }
        let mut cmd = Command::new(resolved);
        cmd.args(args);
        cmd
    }
    #[cfg(not(windows))]
    {
        let _ = (cwd, env_overrides);
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    }
}

#[cfg(windows)]
fn resolve_windows_program(
    program: &str,
    cwd: Option<&Path>,
    env: Option<&BTreeMap<String, String>>,
) -> PathBuf {
    let raw = Path::new(program);
    let find = |path: &Path| {
        windows_program_candidates(path, env)
            .into_iter()
            .find(|candidate| candidate.exists())
    };
    if program.contains('\\') || program.contains('/') || raw.is_absolute() {
        return find(raw).unwrap_or_else(|| raw.into());
    }
    let mut dirs = cwd.map(Path::to_path_buf).into_iter().collect::<Vec<_>>();
    if dirs.is_empty() {
        dirs.extend(std::env::current_dir());
    }
    if let Some(path) = env_value(env, "PATH").or_else(|| std::env::var("PATH").ok()) {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .find_map(|dir| find(&dir.join(program)))
        .unwrap_or_else(|| raw.into())
}

#[cfg(windows)]
fn windows_program_candidates(path: &Path, env: Option<&BTreeMap<String, String>>) -> Vec<PathBuf> {
    if path.extension().is_some() {
        return vec![path.into()];
    }
    let mut candidates = env_value(env, "PATHEXT")
        .or_else(|| std::env::var("PATHEXT").ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .map(str::trim)
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .map(|ext| {
            let mut name = path.as_os_str().to_os_string();
            name.push(ext.to_ascii_lowercase());
            PathBuf::from(name)
        })
        .collect::<Vec<_>>();
    candidates.push(path.into());
    candidates
}

#[cfg(windows)]
fn env_value(env: Option<&BTreeMap<String, String>>, key: &str) -> Option<String> {
    env?.iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
}

// Shell split/quote/platform helpers live in tokenzero_core (single source of truth).
pub use tokenzero_core::{
    argv_has_shell_operator_tokens, contains_platform_shell_syntax, contains_shell_syntax,
    is_shell_operator_token, is_windows_shell_builtin, is_windows_shell_host,
    looks_like_powershell_syntax, quote_for, quote_posix, quote_powershell, quote_windows_cmd,
    split_command_string, split_command_string_for_platform,
};

pub fn env_map(pairs: &[String]) -> Result<BTreeMap<String, String>, RuntimeError> {
    let mut out = BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(invalid_runtime_input("--env requires KEY=VALUE"));
        };
        validate_env_pair(key, value)?;
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn is_shell_injection_env_key(key: &str) -> bool {
    matches!(key, "BASH_ENV" | "BASHOPTS" | "SHELLOPTS") || key.starts_with("BASH_FUNC_")
}

fn validate_env_pair(key: &str, value: &str) -> Result<(), RuntimeError> {
    if key.is_empty() {
        return Err(invalid_runtime_input(
            "environment variable name must be non-empty",
        ));
    }
    if key.contains('\0') || value.contains('\0') {
        return Err(invalid_runtime_input(
            "environment variable names and values must not contain NUL",
        ));
    }
    if key.contains('=') || key.contains('\n') || key.contains('\r') {
        return Err(invalid_runtime_input(
            "environment variable names must not contain '=', CR, or LF",
        ));
    }
    if is_shell_injection_env_key(key) {
        return Err(invalid_runtime_input(format!(
            "environment variable {key} is not allowed"
        )));
    }
    Ok(())
}

fn unexpanded_tilde_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::Normal(component)) if component == "~"
    )
}

fn invalid_runtime_input(msg: impl Into<String>) -> RuntimeError {
    RuntimeError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        msg.into(),
    ))
}

