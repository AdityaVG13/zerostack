//! Shared test scaffolding for ZeroStack and the three engine repos
//! (FSZero, GraphZero, TokenZero).
//!
//! Everything here is TEST-ONLY by convention: no production crate may
//! depend on this package. Helpers must stay domain-free - engine-specific
//! fixtures belong in each engine's own test-support crate.
//!
//! Contents mirror the helpers previously duplicated across the four repos:
//! - [`NoopCancel`] and [`test_invocation`]: engine invocation construction
//! - [`TempWorkspace`]: hermetic workspace root with a `.zerostack` store
//! - [`run_with_timeout`]: bounded subprocess execution with captured output
//! - [`assert_hex_prefix`]: digest assertion helper

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use zero_abi::{CancellationProbe, EngineCallContext, EngineInvocation, KernelBudget};

/// Cancellation probe that never fires. The stand-in every engine's tests
/// previously rolled by hand.
#[derive(Debug, Default)]
pub struct NoopCancel;

impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Standard test budget: generous wall/CPU, small memory and output caps.
pub fn test_budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 20_000,
        cpu_ms: 20_000,
        memory_bytes: 64 * 1024 * 1024,
        call_limit: 256,
        task_limit: 8,
        output_byte_limit: 64 * 1024,
    }
}

/// Build an [`EngineInvocation`] bound to `root` with a [`NoopCancel`] probe
/// and the standard test budget.
pub fn test_invocation(root: &Path, session: &str, cell: &str) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: session.to_owned(),
            cell_id: cell.to_owned(),
            trace_id: format!("{session}-{cell}"),
            deadline_unix_ms: u64::MAX,
            budget: test_budget(),
        },
        cancellation: std::sync::Arc::new(NoopCancel),
    }
}

/// Hermetic workspace: a tempdir root plus the `.zerostack` store directory
/// engines expect to find beside it. Removed on drop.
pub struct TempWorkspace {
    root: tempfile::TempDir,
}

impl TempWorkspace {
    pub fn new(tag: &str) -> std::io::Result<Self> {
        let root = tempfile::Builder::new().prefix(tag).tempdir()?;
        std::fs::create_dir_all(root.path().join(".zerostack"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn store(&self) -> PathBuf {
        self.root.path().join(".zerostack")
    }
}

/// Output of a bounded subprocess run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run `argv` in `cwd`, capturing output, killing the process at `timeout`.
/// Never panics; spawn failures surface as status 127 with stderr populated.
pub fn run_with_timeout(
    argv: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> std::io::Result<BoundedOutput> {
    let mut command = Command::new(argv.first().copied().unwrap_or("true"));
    command
        .args(argv.iter().skip(1))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let started = std::time::Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let output = child.wait_with_output()?;
            return Ok(BoundedOutput {
                status: status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            return Ok(BoundedOutput {
                status: -1,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: true,
            });
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Assert `actual` starts with `expected_prefix` (digest comparison helper
/// that produces readable failures for long hex strings).
pub fn assert_hex_prefix(actual: &str, expected_prefix: &str) {
    assert!(
        actual.starts_with(expected_prefix),
        "digest mismatch: got {actual}, expected prefix {expected_prefix}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_workspace_creates_store_dir() {
        let workspace = TempWorkspace::new("tss-store").unwrap();
        assert!(workspace.store().is_dir());
        assert!(workspace.root().join(".zerostack").is_dir());
    }

    #[test]
    fn test_invocation_carries_roots_and_session() {
        let workspace = TempWorkspace::new("tss-invocation").unwrap();
        let invocation = test_invocation(workspace.root(), "sess", "cell-7");
        assert_eq!(invocation.context.session_id, "sess");
        assert_eq!(invocation.context.cell_id, "cell-7");
        assert!(!invocation.cancellation.is_cancelled());
    }

    #[test]
    fn bounded_runner_captures_output_and_detects_timeout() {
        let workspace = TempWorkspace::new("tss-bounded").unwrap();
        let ok = run_with_timeout(
            &["printf", "captured"],
            workspace.root(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(ok.status, 0);
        assert_eq!(ok.stdout, "captured");
        assert!(!ok.timed_out);

        let slow = run_with_timeout(
            &["sleep", "5"],
            workspace.root(),
            Duration::from_millis(120),
        )
        .unwrap();
        assert!(slow.timed_out, "sleep 5 must hit the 120ms bound");
    }
}
