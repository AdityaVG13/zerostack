use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use zero_abi::{
    EngineError, EngineErrorKind, EngineInvocation, ProjectionRequest, ShellOptions, ShellResult,
    TokenEngine,
};
use zero_process::{IdentityError, VerifiedChild};

const SHELL_GRACE: Duration = Duration::from_millis(100);
const SHELL_POLL: Duration = Duration::from_millis(10);
const MIN_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_VISIBLE_SHELL_BYTES: u32 = 64 * 1024;

#[derive(Clone, Debug)]
pub enum ShellCommand {
    Script(String),
    Argv(Vec<String>),
}

struct LiveProcessGuard(Arc<AtomicU64>);
impl LiveProcessGuard {
    fn acquire(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}
impl Drop for LiveProcessGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn run_shell(
    invocation: &EngineInvocation,
    tokens: &dyn TokenEngine,
    live_processes: Arc<AtomicU64>,
    command: ShellCommand,
    options: ShellOptions,
) -> Result<ShellResult, EngineError> {
    if invocation.cancellation.is_cancelled() {
        return Err(cancelled());
    }
    let mut process = build_command(command)?;
    let cwd = resolve_cwd(&invocation.context.project_root, options.cwd.as_deref())?;
    process
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &options.env {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "shell environment contains an invalid name or value",
                false,
            ));
        }
        process.env(name, value);
    }
    let timeout_ms = options
        .timeout_ms
        .unwrap_or(invocation.context.budget.wall_ms)
        .min(invocation.context.budget.wall_ms)
        .max(1);
    let capture_limit = usize::try_from(invocation.context.budget.memory_bytes / 4)
        .unwrap_or(MAX_CAPTURE_BYTES)
        .clamp(MIN_CAPTURE_BYTES, MAX_CAPTURE_BYTES);
    let (child, mut pipes) =
        VerifiedChild::spawn_tree_with_pipes(process, &invocation.context.session_id, 0)
            .map_err(|error| shell_io("spawn", error))?;
    let _live = LiveProcessGuard::acquire(live_processes);
    if let Some(input) = options.stdin.as_deref()
        && let Some(mut stdin) = pipes.stdin.take()
    {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| shell_io("stdin", error))?;
    }
    drop(pipes.stdin.take());
    let stdout = pipes
        .stdout
        .take()
        .ok_or_else(|| shell_io("stdout", "pipe missing"))?;
    let stderr = pipes
        .stderr
        .take()
        .ok_or_else(|| shell_io("stderr", "pipe missing"))?;
    let stdout_reader = thread::Builder::new()
        .name("zero-kernel-shell-stdout".into())
        .spawn(move || read_bounded(stdout, capture_limit))
        .map_err(|error| shell_io("stdout reader", error))?;
    let stderr_reader = thread::Builder::new()
        .name("zero-kernel-shell-stderr".into())
        .spawn(move || read_bounded(stderr, capture_limit))
        .map_err(|error| shell_io("stderr reader", error))?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if invocation.cancellation.is_cancelled() {
            terminate(&child, &invocation.context.session_id)?;
            let _ = join_reader(stdout_reader);
            let _ = join_reader(stderr_reader);
            return Err(cancelled());
        }
        if child.wait_for_exit(SHELL_POLL) {
            break;
        }
        if Instant::now() >= deadline {
            terminate(&child, &invocation.context.session_id)?;
            let _ = join_reader(stdout_reader);
            let _ = join_reader(stderr_reader);
            return Err(EngineError::new(
                EngineErrorKind::Deadline,
                "shell deadline exceeded; exact child tree was terminated",
                true,
            ));
        }
    }
    let status = child
        .wait(
            &invocation.context.session_id,
            0,
            Duration::ZERO,
            SHELL_GRACE,
        )
        .map_err(identity_error)?;
    let (stdout, stdout_overflow) = join_reader(stdout_reader)?;
    let (stderr, stderr_overflow) = join_reader(stderr_reader)?;
    // The process can exit between the loop's wait and its next cancellation
    // check. Do not publish that raced exit as a successful shell result.
    if invocation.cancellation.is_cancelled() {
        return Err(cancelled());
    }
    if stdout_overflow || stderr_overflow {
        return Err(EngineError::new(
            EngineErrorKind::Budget,
            format!("shell output exceeded {capture_limit} bytes per stream"),
            false,
        ));
    }
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let code = status.code().unwrap_or(-1);
    let exact_bytes = serde_json::to_vec(&json!({
        "status": code,
        "stdout": stdout,
        "stderr": stderr,
    }))
    .map_err(|error| shell_io("serialize result", error))?;
    let visible_limit = options
        .max_visible_bytes
        .unwrap_or(MAX_VISIBLE_SHELL_BYTES)
        .min(invocation.context.budget.output_byte_limit)
        .min(MAX_VISIBLE_SHELL_BYTES)
        .max(512);
    let projected = tokens.project(
        invocation,
        ProjectionRequest {
            bytes: exact_bytes,
            visible_byte_limit: visible_limit,
            media_type: "application/json".into(),
        },
    )?;
    if let Some(exact) = projected.exact {
        let stream_limit = (visible_limit as usize / 2).max(256);
        return Ok(ShellResult {
            status: code,
            stdout: stream_preview(&stdout, stream_limit),
            stderr: stream_preview(&stderr, stream_limit),
            exact: Some(exact),
            accounting: projected.accounting,
        });
    }
    let accounting = tokens.measure(invocation, format!("{stdout}{stderr}").as_bytes())?;
    Ok(ShellResult {
        status: code,
        stdout,
        stderr,
        exact: None,
        accounting,
    })
}

fn stream_preview(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    const MARKER: &str = "\n... output omitted ...\n";
    let available = limit.saturating_sub(MARKER.len());
    let mut head_end = available / 2;
    while !value.is_char_boundary(head_end) {
        head_end = head_end.saturating_sub(1);
    }
    let mut tail_start = value
        .len()
        .saturating_sub(available.saturating_sub(head_end));
    while tail_start < value.len() && !value.is_char_boundary(tail_start) {
        tail_start = tail_start.saturating_add(1);
    }
    format!("{}{}{}", &value[..head_end], MARKER, &value[tail_start..])
}

fn build_command(command: ShellCommand) -> Result<Command, EngineError> {
    match command {
        ShellCommand::Script(script) => {
            if script.trim().is_empty() {
                return Err(EngineError::new(
                    EngineErrorKind::InvalidInput,
                    "shell script must not be empty",
                    false,
                ));
            }
            #[cfg(unix)]
            let command = {
                let mut command = Command::new("/bin/sh");
                command.arg("-lc").arg(script);
                command
            };
            #[cfg(windows)]
            let command = {
                let mut command = Command::new("cmd.exe");
                command.arg("/D").arg("/S").arg("/C").arg(script);
                command
            };
            #[cfg(not(any(unix, windows)))]
            return Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "shell scripts are unsupported on this platform",
                false,
            ));
            Ok(command)
        }
        ShellCommand::Argv(argv) => {
            let (program, args) = argv.split_first().ok_or_else(|| {
                EngineError::new(
                    EngineErrorKind::InvalidInput,
                    "shell argv must not be empty",
                    false,
                )
            })?;
            if program.is_empty() {
                return Err(EngineError::new(
                    EngineErrorKind::InvalidInput,
                    "shell argv program must not be empty",
                    false,
                ));
            }
            let mut command = Command::new(program);
            command.args(args);
            Ok(command)
        }
    }
}

fn resolve_cwd(root: &Path, requested: Option<&Path>) -> Result<PathBuf, EngineError> {
    let path = match requested {
        None => root.to_path_buf(),
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => root.join(path),
    };
    let canonical = std::fs::canonicalize(&path).map_err(|error| shell_io("cwd", error))?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(EngineError::new(
            EngineErrorKind::OutsideWorkspace,
            "shell cwd must be an existing directory inside the project root",
            false,
        ));
    }
    Ok(canonical)
}

fn read_bounded(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::with_capacity(limit.min(64 * 1024));
    let mut overflow = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..count.min(remaining)]);
        overflow |= count > remaining;
    }
    (kept, overflow)
}

fn join_reader(
    reader: thread::JoinHandle<(Vec<u8>, bool)>,
) -> Result<(Vec<u8>, bool), EngineError> {
    reader
        .join()
        .map_err(|_| EngineError::new(EngineErrorKind::Internal, "shell reader panicked", false))
}

fn terminate(child: &VerifiedChild, owner: &str) -> Result<(), EngineError> {
    child
        .signal_graceful_for(owner, 0, SHELL_GRACE)
        .map_err(identity_error)?;
    child.revoke().map_err(identity_error)
}

fn identity_error(error: IdentityError) -> EngineError {
    EngineError::new(
        EngineErrorKind::Io,
        format!("verified shell child: {error}"),
        false,
    )
}

fn shell_io(stage: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Io,
        format!("shell {stage}: {error}"),
        false,
    )
}

fn cancelled() -> EngineError {
    EngineError::new(
        EngineErrorKind::Cancelled,
        "shell cancelled; exact child tree was terminated",
        false,
    )
}
