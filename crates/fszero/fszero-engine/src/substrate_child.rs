//! Supervised substrate child process with durable stderr capture.
//!
//! Parent-side contract for FSZero CodeMode/MCP children (and any process the
//! hub treats as a substrate):
//!
//! 1. **Startup failure is loud** — spawn / immediate death yields
//!    [`SubstrateDown`] with program path, exit code, and stderr artifact path.
//! 2. **Stderr on disk** — child stderr is teed into a known file under
//!    `stderr_dir` (last [`STDERR_RING_BYTES`] kept in memory for the error body).
//! 3. **Mid-session death** — [`SupervisedChild::ensure_alive`] / [`poll_death`]
//!    return a structured error immediately (no hang).
//!
//! Hub-exclusive wiring of `substrate_down` onto the ZeroStack router is out of
//! this crate; this module is the FSZero-owned observation contract.

use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Keep the last 16 KiB of child stderr in memory (and on disk as the full
/// stream written so far, truncated only by ring for the inline error body).
pub const STDERR_RING_BYTES: usize = 16 * 1024;

/// Drain piped child stdout+stderr into owned strings (best-effort).
pub fn read_piped_stdio(child: &mut std::process::Child) -> (String, String) {
    use std::io::Read;
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

/// Structured loud failure when a substrate child cannot start or dies.
#[derive(Debug, Clone)]
pub struct SubstrateDown {
    pub kind: &'static str,
    pub program: PathBuf,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_path: PathBuf,
    pub stderr_bytes: usize,
    pub stderr_tail: String,
    pub message: String,
}

impl SubstrateDown {
    pub const KIND: &'static str = "substrate_down";

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind, "program": self.program.display().to_string(), "exit_code": self.exit_code, "signal": self.signal,
            "stderr_path": self.stderr_path.display().to_string(), "stderr_bytes": self.stderr_bytes, "stderr_tail": self.stderr_tail,
            "message": self.message, "retryable": false,
        })
    }

    pub fn to_json_string(&self) -> String {
        self.to_json().to_string()
    }
}

/// Configuration for spawning a supervised substrate child.
#[derive(Debug, Clone)]
pub struct SubstrateChildConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub stderr_dir: PathBuf,
    pub env: Vec<(String, String)>,
    pub clear_env_keys: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub startup_probe: Duration,
}

impl SubstrateChildConfig {
    pub fn new(program: impl Into<PathBuf>, stderr_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            stderr_dir: stderr_dir.into(),
            env: Vec::new(),
            clear_env_keys: Vec::new(),
            current_dir: None,
            startup_probe: Duration::from_millis(50),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.push((key.into(), val.into()));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn startup_probe(mut self, d: Duration) -> Self {
        self.startup_probe = d;
        self
    }
}

struct StderrCapture {
    path: PathBuf,
    ring: Arc<Mutex<Vec<u8>>>,
    total: Arc<Mutex<usize>>,
    join: Option<JoinHandle<()>>,
}

/// Live supervised child with durable stderr artifact.
pub struct SupervisedChild {
    program: PathBuf,
    child: Child,
    stderr: StderrCapture,
    dead: Option<SubstrateDown>,
}

impl SupervisedChild {
    /// Spawn the child, tee stderr to disk, and probe for immediate crash.
    pub fn spawn(config: SubstrateChildConfig) -> Result<Self, SubstrateDown> {
        fs::create_dir_all(&config.stderr_dir).map_err(|e| {
            spawn_io_failure(
                &config.program,
                config.stderr_dir.join("unwritable.stderr"),
                format!("cannot create stderr_dir: {e}"),
            )
        })?;

        let stderr_path = next_stderr_path(&config.stderr_dir);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stderr_path)
            .map_err(|e| {
                spawn_io_failure(
                    &config.program,
                    stderr_path.clone(),
                    format!("cannot open stderr artifact: {e}"),
                )
            })?;

        let mut cmd = Command::new(&config.program);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &config.current_dir {
            cmd.current_dir(dir);
        }
        for key in &config.clear_env_keys {
            cmd.env_remove(key);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        // Advertise the artifact path so operators / hub can find it without
        // parsing parent errors.
        cmd.env("FSZERO_CHILD_STDERR_PATH", &stderr_path);

        crate::runtime_metrics::record_process_start();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Ok(mut f) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&stderr_path)
                {
                    let _ = writeln!(f, "spawn failed for {}: {e}", config.program.display());
                }
                return Err(SubstrateDown {
                    kind: SubstrateDown::KIND,
                    program: config.program.clone(),
                    exit_code: None,
                    signal: None,
                    stderr_path: stderr_path.clone(),
                    stderr_bytes: fs::metadata(&stderr_path)
                        .map(|m| m.len() as usize)
                        .unwrap_or(0),
                    stderr_tail: format!("spawn failed: {e}"),
                    message: format!(
                        "substrate_down: failed to spawn {} ({e}); stderr={}",
                        config.program.display(),
                        stderr_path.display()
                    ),
                });
            }
        };

        let pipe = child.stderr.take().ok_or_else(|| {
            spawn_io_failure(
                &config.program,
                stderr_path.clone(),
                "child has no stderr pipe".into(),
            )
        })?;

        let ring = Arc::new(Mutex::new(Vec::with_capacity(4096)));
        let total = Arc::new(Mutex::new(0usize));
        let join = Some(spawn_stderr_pump(
            pipe,
            file,
            Arc::clone(&ring),
            Arc::clone(&total),
        ));

        let mut supervised = Self {
            program: config.program.clone(),
            child,
            stderr: StderrCapture {
                path: stderr_path,
                ring,
                total,
                join,
            },
            dead: None,
        };

        // Startup probe: if the child dies immediately, surface exit + stderr.
        if !config.startup_probe.is_zero() {
            thread::sleep(config.startup_probe);
            if let Some(down) = supervised.poll_death() {
                return Err(down);
            }
        }

        Ok(supervised)
    }

    pub fn stderr_path(&self) -> &Path {
        &self.stderr.path
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn stdin(&mut self) -> Option<&mut std::process::ChildStdin> {
        self.child.stdin.as_mut()
    }

    pub fn stdout(&mut self) -> Option<&mut std::process::ChildStdout> {
        self.child.stdout.as_mut()
    }

    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    #[inline]
    fn record_dead(&mut self, down: SubstrateDown) -> SubstrateDown {
        self.dead = Some(down.clone());
        down
    }

    /// Non-blocking death check. Returns structured down report once.
    pub fn poll_death(&mut self) -> Option<SubstrateDown> {
        if self.dead.is_some() {
            return self.dead.clone();
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let down = self.finish_with_status(status);
                Some(self.record_dead(down))
            }
            Ok(None) => None,
            Err(e) => {
                let down = self.finish_with_message(format!("try_wait failed: {e}"), None, None);
                Some(self.record_dead(down))
            }
        }
    }

    /// Hard fail if the child is no longer running. Never hangs.
    pub fn ensure_alive(&mut self) -> Result<(), SubstrateDown> {
        match self.poll_death() {
            Some(down) => Err(down),
            None => Ok(()),
        }
    }

    /// Block until the child exits, then return the structured report.
    pub fn wait_dead(&mut self) -> SubstrateDown {
        if let Some(down) = &self.dead {
            return down.clone();
        }
        match self.child.wait() {
            Ok(status) => {
                let down = self.finish_with_status(status);
                self.record_dead(down)
            }
            Err(e) => {
                let down = self.finish_with_message(format!("wait failed: {e}"), None, None);
                self.record_dead(down)
            }
        }
    }

    /// Kill the child (SIGKILL / TerminateProcess). Used by chaos tests.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    fn finish_with_status(&mut self, status: ExitStatus) -> SubstrateDown {
        let (code, signal) = exit_parts(status);
        let msg = match (code, signal) {
            (Some(c), _) => format!(
                "substrate_down: {} exited with code {c}; stderr={}",
                self.program.display(),
                self.stderr.path.display()
            ),
            (None, Some(s)) => format!(
                "substrate_down: {} killed by signal {s}; stderr={}",
                self.program.display(),
                self.stderr.path.display()
            ),
            _ => format!(
                "substrate_down: {} exited; stderr={}",
                self.program.display(),
                self.stderr.path.display()
            ),
        };
        self.finish_with_message(msg, code, signal)
    }

    fn finish_with_message(
        &mut self,
        message: String,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> SubstrateDown {
        // Join the pump so the ring + file include the final bytes.
        if let Some(handle) = self.stderr.join.take() {
            let _ = handle.join();
        }
        let (tail, bytes) = snapshot_ring(&self.stderr.ring, &self.stderr.total);
        SubstrateDown {
            kind: SubstrateDown::KIND,
            program: self.program.clone(),
            exit_code,
            signal,
            stderr_path: self.stderr.path.clone(),
            stderr_bytes: bytes,
            stderr_tail: tail,
            message,
        }
    }
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stderr.join.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_io_failure(program: &Path, stderr_path: PathBuf, detail: String) -> SubstrateDown {
    SubstrateDown {
        kind: SubstrateDown::KIND,
        program: program.to_path_buf(),
        exit_code: None,
        signal: None,
        stderr_path,
        stderr_bytes: 0,
        stderr_tail: detail.clone(),
        message: format!("substrate_down: {} ({detail})", program.display()),
    }
}

fn next_stderr_path(dir: &Path) -> PathBuf {
    dir.join(format!(
        "fszero-child-{}-{}-{}.stderr",
        std::process::id(),
        super::unix_epoch_nanos(),
        randomish()
    ))
}

fn randomish() -> u32 {
    Instant::now().elapsed().subsec_nanos() ^ std::process::id()
}

fn spawn_stderr_pump(
    mut pipe: std::process::ChildStderr,
    mut file: File,
    ring: Arc<Mutex<Vec<u8>>>,
    total: Arc<Mutex<usize>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    let _ = file.write_all(chunk);
                    let _ = file.flush();
                    if let Ok(mut ring) = ring.lock() {
                        ring.extend_from_slice(chunk);
                        if ring.len() > STDERR_RING_BYTES {
                            let excess = ring.len() - STDERR_RING_BYTES;
                            ring.drain(..excess);
                        }
                    }
                    if let Ok(mut t) = total.lock() {
                        *t = t.saturating_add(n);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn snapshot_ring(ring: &Arc<Mutex<Vec<u8>>>, total: &Arc<Mutex<usize>>) -> (String, usize) {
    let bytes = total.lock().map(|t| *t).unwrap_or(0);
    let tail = ring
        .lock()
        .map(|r| String::from_utf8_lossy(&r).into_owned())
        .unwrap_or_default();
    (tail, bytes)
}

#[cfg(unix)]
fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    use std::os::unix::process::ExitStatusExt;
    (status.code(), status.signal())
}

#[cfg(not(unix))]
fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    (status.code(), None)
}
