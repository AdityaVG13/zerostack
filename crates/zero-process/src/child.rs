//! Verified child identity for every process-signal cancellation site.
//!
//! # Why this exists
//!
//! A bare numeric `kill(pid, ...)` is a TOCTOU hazard: the pid can be recycled
//! between reading a pid file and signaling, so a cancellation can terminate an
//! unrelated replacement process. The canonical start identity primitive comes
//! from this hub-owned crate ([`ProcessIdentity`]): pid plus a native start identity captured at spawn.
//!
//! # Exactness model
//!
//! A `ProcessIdentity::is_live()` check followed by a numeric `kill(pid, ...)`
//! is still TOCTOU (the process can exit and the pid be recycled between the
//! two calls). This module therefore never combines a detached identity check
//! with a numeric kill:
//!
//! - **Same-process owned child** ([`VerifiedChild::capture`]): the owned,
//!   unreaped [`std::process::Child`] pins the pid on Unix (a pid cannot be
//!   recycled while an unreaped child owns it) and holds a real OS handle on
//!   Windows. Signaling through the owned handle is exact by construction;
//!   identity capture is defense-in-depth against state bugs.
//! - **Process tree** ([`VerifiedChild::spawn_tree`]): Unix spawns the root in
//!   its own process group (`process_group(0)`), so the root and every
//!   descendant that inherits the group form one exact tree signaled with a
//!   negative pid. Windows creates a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assigns the exact owned child
//!   handle to it immediately after spawn, so every descendant dies with the
//!   job. Neither path ever authorizes a numeric PID from a detached identity
//!   record followed by `kill`.
//! - **Owner death** ([`VerifiedChild::spawn_tree`]): Linux binds
//!   `PR_SET_PDEATHSIG(SIGKILL)` to the spawning owner on every tree root, so
//!   a tree built by construction (each level spawn-tree spawned) collapses on
//!   owner death: the root dies with its owner, and each descendant dies with
//!   its parent. Darwin binds a forked kqueue watcher that SIGKILLs the tree
//!   group instead (macOS has no PDEATHSIG). A Linux root whose owner died
//!   before its pre-exec binding refuses to exec rather than run unbound
//!   (zerostack-5bei).
//! - **Detached process** (warm daemon stem): the primary teardown path is an
//!   authenticated `Shutdown` RPC over the owned Unix socket. Escalation when
//!   the RPC is unavailable uses a Linux `pidfd` (kernel-pinned to the captured
//!   process) via `pidfd_send_signal`. On platforms without an exact signal
//!   handle (macOS, other), escalation **fails closed** rather than sending a
//!   PID-only signal.
//! - **Status liveness** binds the identity record (`is_live` against the
//!   captured start identity); it never probes with `kill(pid, 0)`.
//!
//! # Fail-closed rule
//!
//! When the identity record is missing, unparseable, stale, or the platform
//! cannot capture proof, every signaling entry point returns a typed
//! [`IdentityError`] and no signal is delivered.

use std::fmt;
use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ProcessIdentity;
#[cfg(windows)]
use crate::identity::Handle;
use crate::resource::{ProcessResourcePolicy, ResourceReceipt, configure_command};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenThread, ResumeThread, THREAD_SUSPEND_RESUME, WaitForSingleObject,
};

/// File name of the daemon identity record next to `stem.pid`.
pub const IDENTITY_FILE_NAME: &str = "stem.identity";

/// Bounded grace for fail-closed cleanup when spawn-tree job assignment or
/// thread resume fails: the exact child must be killed and reaped within this
/// window or the failure is loud.
const SPAWN_FAILED_CLEANUP_GRACE: Duration = Duration::from_secs(5);

/// Typed fail-closed error for every signaling entry point.
#[derive(Debug)]
pub enum IdentityError {
    /// The platform cannot capture a start identity or open an exact signal
    /// handle; cancellation is rejected instead of pretending PID-reuse
    /// protection.
    Unsupported,
    /// The identity record is absent, unparseable, or the bound process is
    /// gone (pidfd_open / capture returned not-found).
    Missing,
    /// A live process at the pid is not the captured process (start identity
    /// mismatch or PID reuse).
    IdentityChanged,
    /// The caller's expected owner session does not match the binding.
    OwnerMismatch { expected: String, actual: String },
    /// The caller's expected worker generation does not match the binding.
    GenerationMismatch { expected: u64, actual: u64 },
    /// Signal requested after the child was revoked.
    Revoked,
    /// Signal requested after the child was reaped.
    AlreadyReaped,
    /// Revocation was requested before the owned child exited.
    StillRunning,
    /// Underlying I/O failure.
    Io(io::Error),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(
                f,
                "verified process identity unsupported on this platform; cancellation failed closed"
            ),
            Self::Missing => write!(f, "verified child identity record missing or process gone"),
            Self::IdentityChanged => {
                write!(
                    f,
                    "process start identity changed; refusing to signal a replacement"
                )
            }
            Self::OwnerMismatch { expected, actual } => write!(
                f,
                "owner session mismatch: expected {expected:?}, bound {actual:?}"
            ),
            Self::GenerationMismatch { expected, actual } => write!(
                f,
                "worker generation mismatch: expected {expected}, bound {actual}"
            ),
            Self::Revoked => write!(f, "child identity already revoked"),
            Self::AlreadyReaped => write!(f, "child already reaped"),
            Self::StillRunning => write!(f, "child identity cannot be revoked before reap"),
            Self::Io(error) => write!(f, "verified child identity I/O: {error}"),
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for IdentityError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Outcome of a bounded graceful termination sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalOutcome {
    /// The child exited between capture and the first signal.
    ExitedBeforeSignal,
    /// The child exited after the graceful signal (SIGTERM on Unix).
    TerminatedGracefully,
    /// The child ignored the graceful signal and was escalated to SIGKILL.
    EscalatedToKill,
}

/// Persisted binding of a process to its captured start identity plus the
/// owner session and worker generation that accepted its work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildBinding {
    pub pid: u32,
    /// Native start identity when the platform can capture one; `None` means
    /// the owned unreaped handle (never a detached pid) is the exactness
    /// proof.
    pub start_key: Option<String>,
    /// Owner session that accepted this child's work (empty when detached).
    pub owner_session: String,
    /// Worker generation this child belongs to; bumped on every respawn.
    pub generation: u64,
}

impl ChildBinding {
    /// Capture a start identity for `pid` (fail closed when the platform
    /// cannot provide one). Used by the daemon stem, which is detached by
    /// design and therefore has no owned handle.
    pub fn capture_pid(pid: u32, owner_session: &str, generation: u64) -> io::Result<Self> {
        let identity = ProcessIdentity::capture(pid).map_err(|error| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!("capture start identity for pid {pid}: {error}"),
            )
        })?;
        Ok(Self {
            pid,
            start_key: Some(identity.start_key),
            owner_session: owner_session.to_string(),
            generation,
        })
    }

    pub fn identity(&self) -> Option<ProcessIdentity> {
        self.start_key.clone().map(|start_key| ProcessIdentity {
            pid: self.pid,
            start_key,
        })
    }

    /// Identity-bound liveness. This is the only liveness check used for
    /// status: it never probes with `kill(pid, 0)`.
    pub fn is_live(&self) -> bool {
        match self.identity() {
            Some(identity) => matches!(identity.is_live(), Ok(true)),
            None => false,
        }
    }

    /// Verify the binding matches the caller's owner session and generation.
    pub fn verify_owner(&self, owner_session: &str, generation: u64) -> Result<(), IdentityError> {
        if self.owner_session != owner_session {
            return Err(IdentityError::OwnerMismatch {
                expected: owner_session.to_string(),
                actual: self.owner_session.clone(),
            });
        }
        if self.generation != generation {
            return Err(IdentityError::GenerationMismatch {
                expected: generation,
                actual: self.generation,
            });
        }
        Ok(())
    }

    pub fn encode(&self) -> String {
        let start = self.start_key.as_deref().unwrap_or("-");
        // Sanitize line breaks so the record stays one line.
        let owner = self.owner_session.replace(['\t', '\r', '\n'], " ");
        format!("{}:{}\t{}\t{}", self.pid, start, owner, self.generation)
    }

    pub fn decode(text: &str) -> Result<Self, IdentityError> {
        let mut parts = text.trim().split('\t');
        let head = parts.next().ok_or(IdentityError::Missing)?;
        let owner = parts.next().ok_or(IdentityError::Missing)?;
        let generation = parts.next().ok_or(IdentityError::Missing)?;
        if parts.next().is_some() {
            return Err(IdentityError::Missing);
        }
        let (pid, start_key) = head.split_once(':').ok_or(IdentityError::Missing)?;
        let pid = pid.parse::<u32>().map_err(|_| IdentityError::Missing)?;
        if pid == 0 {
            return Err(IdentityError::Missing);
        }
        let start_key = (start_key != "-")
            .then(|| start_key.to_string())
            .filter(|key| !key.is_empty());
        let generation = generation
            .parse::<u64>()
            .map_err(|_| IdentityError::Missing)?;
        Ok(Self {
            pid,
            start_key,
            owner_session: owner.to_string(),
            generation,
        })
    }
}

/// True when the Unix stream's peer runs as the same effective user as this
/// process (authenticated shutdown gate over the owned daemon socket).
#[cfg(unix)]
pub fn peer_is_same_user(stream: &std::os::unix::net::UnixStream) -> bool {
    match crate::peer_euid(stream) {
        Ok(peer) => peer == crate::current_euid(),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub fn peer_is_same_user(_stream: &()) -> bool {
    false
}

/// The three stdio pipes of a spawn-tree child, all owned by the caller.
#[derive(Debug)]
pub struct ChildPipes {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

struct VerifiedChildInner {
    child: Mutex<Option<Child>>,
    binding: ChildBinding,
    /// Tree ownership established at spawn. `None` means single-process
    /// capture: the owned unreaped [`Child`] is the exactness proof.
    #[cfg(unix)]
    group_pgid: Mutex<Option<i32>>,
    /// Owned Windows Job Object when spawned as a tree; `None` otherwise.
    #[cfg(windows)]
    job: Mutex<Option<JobHandle>>,
    revoked: AtomicBool,
    /// Set only after a Unix tree teardown successfully swept the whole group
    /// while the root still pinned the numeric PGID. Gates [`revoke`] (Unix
    /// trees) and the abandonment [`Drop`]: reaping or signaling before settle
    /// would release the PGID pin or strand descendants. Single-process
    /// capture and Windows never read it.
    #[cfg(unix)]
    settled: AtomicBool,
    /// Exit status observed at reap time (std caches it in the owned
    /// [`Child`]); captured by [`VerifiedChild::revoke`] and readable through
    /// [`VerifiedChild::terminal_status`].
    exit_status: Mutex<Option<ExitStatus>>,
}

/// A same-process owned child whose identity was captured at spawn.
///
/// Signaling goes through the owned, unreaped [`Child`] handle, which is exact
/// by construction (the pid cannot be recycled while the child is unreaped on
/// Unix; Windows keeps a real process handle). `Clone` shares one underlying
/// handle so duplicate/concurrent cancel and reap settle exactly once.
#[derive(Clone)]
pub struct VerifiedChild(Arc<VerifiedChildInner>);

impl VerifiedChild {
    /// Capture identity at spawn (before work is accepted). Start-identity
    /// capture is best-effort: on platforms that cannot provide one, the owned
    /// unreaped handle remains the exactness proof.
    ///
    /// Single-process ownership: signaling stays on the exact owned child
    /// handle. Use [`Self::spawn_tree`] when descendants must die with the
    /// root.
    pub fn capture(child: Child, owner_session: &str, generation: u64) -> Self {
        let pid = child.id();
        let start_key = ProcessIdentity::capture(pid)
            .ok()
            .map(|identity| identity.start_key);
        Self(Arc::new(VerifiedChildInner {
            child: Mutex::new(Some(child)),
            binding: ChildBinding {
                pid,
                start_key,
                owner_session: owner_session.to_string(),
                generation,
            },
            #[cfg(unix)]
            group_pgid: Mutex::new(None),
            #[cfg(windows)]
            job: Mutex::new(None),
            revoked: AtomicBool::new(false),
            #[cfg(unix)]
            settled: AtomicBool::new(false),
            exit_status: Mutex::new(None),
        }))
    }

    /// Spawn `command` as the root of an isolated process tree and capture its
    /// identity before any work is accepted.
    ///
    /// - Unix: the child is spawned in its own process group
    ///   (`process_group(0)`), so the root and every descendant that inherits
    ///   the group can be signaled as one exact tree. The caller's process
    ///   group is never touched.
    /// - Windows: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is
    ///   created and the exact owned child handle is assigned to it right
    ///   after spawn, before this function returns. Descendants inherit job
    ///   membership, so they die with the job.
    /// - If job creation/assignment fails, the exact owned child is terminated
    ///   and reaped and the error is returned (fail closed; nothing is left
    ///   behind).
    ///
    /// The returned pipes are the child's stdin/stdout; the owned child stays
    /// intact inside `Self`.
    pub fn spawn_tree(
        command: Command,
        owner_session: &str,
        generation: u64,
    ) -> io::Result<(Self, Option<ChildStdin>, Option<ChildStdout>)> {
        let (owned, pipes) = Self::spawn_tree_with_pipes(command, owner_session, generation)?;
        Ok((owned, pipes.stdin, pipes.stdout))
    }

    /// Like [`Self::spawn_tree`] but returns stdin, stdout, and stderr so the
    /// caller can own all three raw-worker pipes.
    pub fn spawn_tree_with_pipes(
        command: Command,
        owner_session: &str,
        generation: u64,
    ) -> io::Result<(Self, ChildPipes)> {
        Self::spawn_tree_with_pipes_inner(command, owner_session, generation, None)
    }

    /// Spawn a tree under a validated native resource policy and return the
    /// truthful platform enforcement receipt.
    pub fn spawn_tree_with_pipes_and_policy(
        mut command: Command,
        owner_session: &str,
        generation: u64,
        policy: ProcessResourcePolicy,
    ) -> io::Result<(Self, ChildPipes, ResourceReceipt)> {
        let resource_receipt = configure_command(&mut command, policy)?;
        let (child, pipes) =
            Self::spawn_tree_with_pipes_inner(command, owner_session, generation, Some(policy))?;
        Ok((child, pipes, resource_receipt))
    }

    fn spawn_tree_with_pipes_inner(
        mut command: Command,
        owner_session: &str,
        generation: u64,
        _policy: Option<ProcessResourcePolicy>,
    ) -> io::Result<(Self, ChildPipes)> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Isolate the child in its own process group before exec: the
            // group id equals the child's pid and is pinned by the unreaped
            // owned Child until reap.
            command.process_group(0);
            // Linux: PR_SET_PDEATHSIG, fail-closed if prctl returns nonzero
            // (xk4c). The hook also proves the owner that forked us is still
            // alive (zerostack-5bei): a child reparented to init or a
            // subreaper before exec would carry a binding that can never fire.
            // Darwin: kqueue watcher (hgni). macOS has no PDEATHSIG; the owner
            // path remains that watcher plus kill_and_reap.
            #[cfg(target_os = "linux")]
            {
                let expected_parent = std::process::id() as libc::pid_t;
                unsafe {
                    command.pre_exec(move || linux_bind_pdeathsig(expected_parent));
                }
            }
            #[cfg(target_os = "macos")]
            {
                unsafe {
                    command.pre_exec(darwin_bind_tree_to_owner);
                }
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Suspend the primary thread before exec so the root cannot spawn
            // descendants until job assignment completed (closes the
            // pre-assignment escape race). The thread is resumed before this
            // function returns; a suspended root is never handed back.
            command.creation_flags(0x0000_0004); // CREATE_SUSPENDED
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let start_key = ProcessIdentity::capture(pid)
            .ok()
            .map(|identity| identity.start_key);
        #[cfg(windows)]
        let job = match JobHandle::assign(&child, _policy) {
            Ok(job) => {
                // Resume the primary thread now that the exact child handle is
                // inside the kill-on-close job. Never return while suspended.
                // The thread id comes from a Toolhelp snapshot (stable API);
                // `main_thread_handle` is still unstable on nightly.
                if let Err(error) = resume_primary_thread(pid) {
                    // Fail closed: kill and reap the exact root, then drop the
                    // job so KILL_ON_JOB_CLOSE sweeps any job member.
                    let _ = child.kill();
                    wait_child_exit(&mut child, SPAWN_FAILED_CLEANUP_GRACE)?;
                    drop(job);
                    return Err(error);
                }
                Some(job)
            }
            Err(error) => {
                // Fail closed: terminate and reap the exact owned child. The
                // child is still suspended, so it cannot have spawned anything.
                let _ = child.kill();
                wait_child_exit(&mut child, SPAWN_FAILED_CLEANUP_GRACE)?;
                return Err(error);
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Ok((
            Self(Arc::new(VerifiedChildInner {
                child: Mutex::new(Some(child)),
                binding: ChildBinding {
                    pid,
                    start_key,
                    owner_session: owner_session.to_string(),
                    generation,
                },
                #[cfg(unix)]
                group_pgid: Mutex::new(Some(pid as i32)),
                #[cfg(windows)]
                job: Mutex::new(job),
                revoked: AtomicBool::new(false),
                #[cfg(unix)]
                settled: AtomicBool::new(false),
                exit_status: Mutex::new(None),
            })),
            ChildPipes {
                stdin,
                stdout,
                stderr,
            },
        ))
    }

    pub fn binding(&self) -> &ChildBinding {
        &self.0.binding
    }

    pub fn is_revoked(&self) -> bool {
        self.0.revoked.load(Ordering::SeqCst)
    }

    pub fn child_id(&self) -> u32 {
        self.binding().pid
    }

    /// Full precondition check before any signal: not revoked, not reaped, the
    /// owned child has not exited, and (when captured) the native start
    /// identity is still live. Exited-anytime is fail-closed: once the owned
    /// handle no longer pins a live process, no signal entry point proceeds.
    ///
    /// Tree mode observes the root's exit **without reaping** (Unix `waitid`
    /// with `WNOWAIT`) so the root keeps pinning the numeric PGID for any later
    /// group signal; single-process capture may reap, since it owns no group.
    pub fn verify(&self) -> Result<(), IdentityError> {
        if self.is_revoked() {
            return Err(IdentityError::Revoked);
        }
        // Unix tree: observe root exit without reaping so the root keeps
        // pinning the numeric PGID (never `try_wait` here).
        #[cfg(unix)]
        if self.is_tree() {
            let guard = self
                .0
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(child) = guard.as_ref() else {
                return Err(IdentityError::AlreadyReaped);
            };
            if child_exited_no_reap(child)? {
                return Err(IdentityError::IdentityChanged);
            }
            drop(guard);
            return self.verify_identity_live();
        }
        let mut guard = self
            .0
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(child) = guard.as_mut() else {
            return Err(IdentityError::AlreadyReaped);
        };
        if child.try_wait()?.is_some() {
            return Err(IdentityError::IdentityChanged);
        }
        drop(guard);
        self.verify_identity_live()
    }

    /// Start-identity liveness gate shared by every verify path. Never probes
    /// with `kill(pid, 0)`; binds to the captured identity.
    fn verify_identity_live(&self) -> Result<(), IdentityError> {
        if let Some(identity) = self.binding().identity() {
            match identity.is_live() {
                Ok(true) => Ok(()),
                Ok(false) => Err(IdentityError::IdentityChanged),
                Err(_) => Err(IdentityError::Unsupported),
            }
        } else {
            Ok(())
        }
    }

    /// Verify plus an owner-session and worker-generation binding check.
    pub fn verify_for(&self, owner_session: &str, generation: u64) -> Result<(), IdentityError> {
        self.verify()?;
        self.binding().verify_owner(owner_session, generation)
    }

    /// Bounded graceful termination, gated on the caller's expected owner
    /// session and worker generation: the check is part of the signal action
    /// itself, so no caller can signal without binding both.
    ///
    /// Tree mode terminates the exact owned tree -- the Unix process group or
    /// the Windows Job Object -- never the caller's group. Single-process mode
    /// signals through the owned, unreaped child handle. No path authorizes a
    /// numeric PID from a detached identity record.
    #[allow(
        clippy::needless_return,
        reason = "each cfg-selected platform block is the function tail"
    )]
    pub fn signal_graceful_for(
        &self,
        expected_owner: &str,
        expected_generation: u64,
        grace: Duration,
    ) -> Result<SignalOutcome, IdentityError> {
        self.verify_signal_preconditions(expected_owner, expected_generation)?;
        let mut guard = self
            .0
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(child) = guard.as_mut() else {
            return Err(IdentityError::AlreadyReaped);
        };
        #[cfg(unix)]
        {
            let pgid = *self
                .0
                .group_pgid
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return match pgid {
                Some(pgid) => {
                    let outcome = terminate_tree_child(child, pgid, grace)?;
                    // Tree teardown swept the group while the root still pinned
                    // the PGID; mark settled so revoke may reap the root.
                    self.0.settled.store(true, Ordering::SeqCst);
                    Ok(outcome)
                }
                None => terminate_owned_child(child, grace),
            }
            .map_err(IdentityError::from);
        }
        #[cfg(windows)]
        {
            let mut job = self
                .0
                .job
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return match job.is_some() {
                true => terminate_tree_job(child, &mut job, grace),
                false => terminate_owned_child(child, grace),
            }
            .map_err(IdentityError::from);
        }
        #[cfg(not(any(unix, windows)))]
        {
            terminate_owned_child(child, grace).map_err(IdentityError::from)
        }
    }

    /// True when this `VerifiedChild` owns an exact tree primitive (Unix
    /// process group / Windows Job Object) rather than a bare single process.
    #[cfg(unix)]
    fn is_tree(&self) -> bool {
        self.0
            .group_pgid
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    #[cfg(windows)]
    fn is_tree(&self) -> bool {
        self.0
            .job
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    #[cfg(not(any(unix, windows)))]
    fn is_tree(&self) -> bool {
        false
    }

    /// Signal preconditions. Mandatory for every mode: the immutable
    /// owner-session and worker-generation binding, not revoked, and the owned
    /// child slot still present. Single-process capture additionally requires
    /// a live unreaped root/start identity -- a bare PID must never be
    /// signaled. Tree ownership (Unix group / Windows job) remains authorized
    /// for cleanup even if the root already exited, because the exact tree
    /// primitive is still owned: sweeping it cannot touch an unrelated
    /// process. This is not permission to signal a detached PID.
    fn verify_signal_preconditions(
        &self,
        expected_owner: &str,
        expected_generation: u64,
    ) -> Result<(), IdentityError> {
        if self.is_revoked() {
            return Err(IdentityError::Revoked);
        }
        self.binding()
            .verify_owner(expected_owner, expected_generation)?;
        if self
            .0
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
        {
            return Err(IdentityError::AlreadyReaped);
        }
        if !self.is_tree() {
            // Single-process capture: strict verification (live unreaped root
            // and start identity) before any signal.
            self.verify()?;
        }
        Ok(())
    }

    /// Revoke exactly once after the child is reaped. Duplicate or concurrent
    /// revokes are harmless. A premature revoke fails immediately instead of
    /// waiting on a live child without a bound; callers must complete bounded
    /// teardown first.
    ///
    /// Unix trees additionally require a successful tree teardown (the `settled`
    /// flag) before reaping: reaping the root early would release the numeric
    /// PGID pin and could strand descendants. A revoke on an unsettled Unix
    /// tree returns [`IdentityError::StillRunning`] **without** reaping or
    /// signaling, so the root keeps pinning the PGID. Windows keeps its
    /// kill-on-close job semantics.
    pub fn revoke(&self) -> Result<(), IdentityError> {
        if self.is_revoked() {
            return Ok(());
        }
        // Unix tree: refuse to reap the root before the group was swept.
        // Reaping would release the PGID pin (M1) and the descendants that
        // survive it would be stranded (M2). Never touch the child slot here.
        #[cfg(unix)]
        if self.is_tree() && !self.0.settled.load(Ordering::SeqCst) {
            return Err(IdentityError::StillRunning);
        }
        let mut guard = self
            .0
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(child) = guard.as_mut() {
            match child.try_wait()? {
                Some(status) => {
                    *self
                        .0
                        .exit_status
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(status);
                }
                None => return Err(IdentityError::StillRunning),
            }
        }
        guard.take();
        self.0.revoked.store(true, Ordering::SeqCst);
        // Windows: closing the job handle now fires JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        // and sweeps any job member that escaped the root's termination.
        #[cfg(windows)]
        {
            let mut job = self
                .0
                .job
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *job = None;
        }
        Ok(())
    }

    /// Poll whether the owned child has exited. Tree mode observes without
    /// reaping (Unix `waitid` with `WNOWAIT`) so the root keeps pinning the
    /// numeric PGID; single-process capture may reap. Used by tests and by
    /// callers that must wait for a tree root to exit before teardown without
    /// releasing the PGID pin.
    pub fn poll_exited(&self) -> bool {
        let mut guard = self
            .0
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(child) = guard.as_mut() else {
            return false;
        };
        #[cfg(unix)]
        if self.is_tree() {
            // Never reap a Unix tree root here: the PGID must stay pinned.
            return child_exited_no_reap(child).unwrap_or(false);
        }
        child.try_wait().ok().flatten().is_some()
    }
    /// Wait up to `timeout` for the exact owned root to exit without reaping
    /// it. Windows blocks on the retained process handle; Unix preserves the
    /// waitable root pin while checking within the bound.
    pub fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            loop {
                if self.terminal_status().is_some() {
                    return true;
                }
                let now = Instant::now();
                let wait = deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(10));
                let milliseconds = wait.as_millis().min(u32::MAX as u128) as u32;
                let exited = {
                    let guard = self
                        .0
                        .child
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let Some(child) = guard.as_ref() else {
                        drop(guard);
                        return self.terminal_status().is_some();
                    };
                    // SAFETY: the std Child owns this exact process handle for
                    // this short wait while the mutex guard prevents reaping.
                    (unsafe { WaitForSingleObject(child.as_raw_handle(), milliseconds) })
                        == WAIT_OBJECT_0
                };
                if exited {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
            }
        }
        #[cfg(not(windows))]
        {
            loop {
                // A concurrent exact-handle signal may reap and revoke between
                // polls. Recheck its recorded status every iteration; an empty
                // child slot alone is not proof because it may also be missing.
                if self.terminal_status().is_some() {
                    return true;
                }
                if self.poll_exited() {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// Exit status captured when the owned child was reaped by
    /// [`Self::revoke`] (or settled teardown followed by revoke).
    pub fn terminal_status(&self) -> Option<ExitStatus> {
        *self
            .0
            .exit_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait up to `timeout` for the owned root to exit, then settle the exact
    /// tree within `grace` and reap it. A live root at the deadline returns
    /// [`IdentityError::StillRunning`] without signaling or releasing the
    /// owned tree primitive.
    ///
    /// - Tree mode: after the root exits, the group (Unix) or job (Windows)
    ///   is swept so no descendant survives, then the root is reaped.
    /// - Single-process capture: reaps directly.
    ///
    /// The returned status is also available via [`Self::terminal_status`].
    pub fn wait(
        &self,
        expected_owner: &str,
        expected_generation: u64,
        timeout: Duration,
        grace: Duration,
    ) -> Result<ExitStatus, IdentityError> {
        if !self.wait_for_exit(timeout) {
            return Err(IdentityError::StillRunning);
        }
        if self.is_tree() {
            // The root has exited; sweeping the still-owned tree primitive
            // cannot touch an unrelated process (the job/group is ours).
            self.signal_graceful_for(expected_owner, expected_generation, grace)?;
        }
        self.revoke()?;
        self.terminal_status().ok_or(IdentityError::Missing)
    }
}

/// Set `PR_SET_PDEATHSIG` to `signal`. Nonzero prctl fails the spawn hook
/// so Command does not exec without the parent-death bit.
#[cfg(target_os = "linux")]
fn linux_set_pdeathsig(signal: libc::c_int) -> io::Result<()> {
    // SAFETY: prctl here only sets or queries the parent-death signal.
    let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal) };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Linux pre-exec owner-death binding for a spawn-tree root.
///
/// Binds `PR_SET_PDEATHSIG(SIGKILL)` to the process that forked us, verifies
/// the bit stuck (xk4c), and refuses exec when `expected_parent` no longer
/// names that process. A reparented child -- owner died between fork and this
/// hook -- would hold a dead binding (the signal only fires on a future
/// parent death), silently orphaning the whole tree. `getppid() == 1` alone
/// cannot detect the orphan: init is not the only reparent target, any
/// ancestor subreaper (systemd, session sidecar) adopts it. Fail closed.
#[cfg(target_os = "linux")]
fn linux_bind_pdeathsig(expected_parent: libc::pid_t) -> io::Result<()> {
    linux_set_pdeathsig(libc::SIGKILL)?;
    let mut got: libc::c_int = 0;
    // SAFETY: PR_GET_PDEATHSIG writes one int; fail closed if the bit did not
    // stick so Command cannot exec an unbound child.
    let rc = unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut got as *mut libc::c_int) };
    if rc != 0 || got != libc::SIGKILL {
        return Err(io::Error::other(
            "PR_SET_PDEATHSIG did not stick; refusing exec",
        ));
    }
    // SAFETY: getppid is async-signal-safe; a parent mismatch means the owner
    // died between fork and this hook, so the child must not exec unbound.
    if unsafe { libc::getppid() } != expected_parent {
        unsafe { libc::_exit(1) };
    }
    Ok(())
}

/// Darwin kqueue handshake: the tree root may Ok() only after the watcher
/// writes this ready byte (EVFILT_PROC registered). Any other read is
/// fail-closed (zerostack-sq32). Compiled on Linux tests so the rule is
/// not Darwin-only dead code.
#[cfg(any(test, target_os = "macos"))]
fn darwin_kqueue_registration_ready(n: isize, byte: u8) -> bool {
    n == 1 && byte == 1
}

/// Darwin owner-death: fork a kqueue watcher that SIGKILLs this process
/// group when the spawning owner exits. The watcher leaves the tree group
/// so it is not a member of the kill target. libc-only after fork.
#[cfg(target_os = "macos")]
fn darwin_bind_tree_to_owner() -> io::Result<()> {
    // SAFETY: pre_exec is async-signal-safe. getppid/getpid/fork/setpgid
    // pipe/read/write/close are AS-safe. The watcher process never returns
    // into Rust. The tree root does not Ok() until the watcher has
    // registered EVFILT_PROC (zerostack-sq32).
    unsafe {
        let owner = libc::getppid();
        let tree_root = libc::getpid();
        if owner <= 1 {
            libc::_exit(1);
        }
        let mut fds = [0i32; 2];
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);
        match libc::fork() {
            -1 => {
                libc::close(read_fd);
                libc::close(write_fd);
                Err(io::Error::last_os_error())
            }
            0 => {
                libc::close(read_fd);
                let _ = libc::setpgid(0, 0);
                darwin_watch_owner_then_kill(owner, tree_root, write_fd);
            }
            _ => {
                libc::close(write_fd);
                let mut byte: libc::c_uchar = 0;
                let n = libc::read(
                    read_fd,
                    &mut byte as *mut libc::c_uchar as *mut libc::c_void,
                    1,
                );
                libc::close(read_fd);
                if darwin_kqueue_registration_ready(n, byte) {
                    Ok(())
                } else {
                    Err(io::Error::other(
                        "Darwin kqueue watcher failed to register EVFILT_PROC; refusing exec",
                    ))
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn darwin_watch_owner_then_kill(
    owner: libc::pid_t,
    tree_root: libc::pid_t,
    ready_fd: libc::c_int,
) -> ! {
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            libc::close(ready_fd);
            libc::_exit(1);
        }
        let changes = [
            libc::kevent {
                ident: owner as usize,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            },
            libc::kevent {
                ident: tree_root as usize,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            },
        ];
        if libc::kevent(
            kq,
            changes.as_ptr(),
            2,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        ) < 0
        {
            libc::close(ready_fd);
            libc::_exit(1);
        }
        let ready: libc::c_uchar = 1;
        let _ = libc::write(ready_fd, &ready as *const libc::c_uchar as *const libc::c_void, 1);
        libc::close(ready_fd);
        let mut event = libc::kevent {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        loop {
            let rc = libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null());
            if rc < 0 {
                if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                libc::_exit(1);
            }
            if rc == 0 {
                continue;
            }
            if event.ident == owner as usize {
                let _ = libc::kill(-tree_root, libc::SIGKILL);
            }
            libc::_exit(0);
        }
    }
}

/// Last-owner abandonment cleanup. A Unix tree that was never torn down
/// would otherwise strand its descendants: the owned `Child` drops without
/// killing anything (std does not kill on drop). If an unsettled Unix tree
/// still owns a child, send SIGKILL to its pinned group (root still unreaped,
/// so the PGID cannot have been recycled) then reap best-effort. Single-process
/// capture and detached daemon bindings are unaffected (no group, no signal).
/// Windows relies on the kill-on-close job handle dropped as a field below.
impl Drop for VerifiedChildInner {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let is_tree = self
                .group_pgid
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some();
            if is_tree
                && !self.settled.load(Ordering::SeqCst)
                && !self.revoked.load(Ordering::SeqCst)
            {
                let child_pin_ok = self
                    .child
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .map(|child| child_exited_no_reap(child).is_ok())
                    .unwrap_or(false);
                // Only signal the numeric group when the waitable root pin is
                // provably still ours; otherwise the PGID may be recycled and a
                // stale signal would risk an unrelated tree.
                if child_pin_ok {
                    let pgid = *self
                        .group_pgid
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(pgid) = pgid {
                        // SAFETY: the waitable root pin is proved, so the numeric
                        // PGID is still pinned to our tree.
                        let _ = unsafe { libc::kill(-pgid, libc::SIGKILL) };
                        // Bounded best-effort root reap: never strand a zombie.
                        if let Some(child) = self
                            .child
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .as_mut()
                        {
                            let _ = wait_child_exit(child, SPAWN_FAILED_CLEANUP_GRACE);
                        }
                        // Bounded non-signaling group-gone sweep: every group
                        // signal is already done, so only `kill(-pgid, 0)` (no
                        // signal delivered) proves descendants died.
                        let _ = wait_for_group_gone(pgid, SPAWN_FAILED_CLEANUP_GRACE);
                    }
                }
            }
        }
        // Let the remaining fields (owned `Child`, Windows job handle) drop with
        // their inherent semantics. No further signal is needed.
    }
}

/// SIGTERM, bounded wait, then SIGKILL escalation, through an owned child.
#[cfg(unix)]
fn terminate_owned_child(child: &mut Child, grace: Duration) -> io::Result<SignalOutcome> {
    let pid = child.id();
    // SAFETY: `pid` is the unreaped Child's id; an unreaped child pins the
    // pid so it cannot be recycled. SIGTERM is delivered only to that process.
    if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            wait_child_exit(child, grace)?;
            return Ok(SignalOutcome::ExitedBeforeSignal);
        }
        return Err(error);
    }
    let deadline = Instant::now() + grace;
    loop {
        if let Some(_status) = child.try_wait()? {
            return Ok(SignalOutcome::TerminatedGracefully);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: still our unreaped child after the grace window; SIGKILL
    // targets the same pinned pid.
    if unsafe { libc::kill(pid as i32, libc::SIGKILL) } == -1 {
        return Err(io::Error::last_os_error());
    }
    wait_child_exit(child, grace)?;
    Ok(SignalOutcome::EscalatedToKill)
}

#[cfg(windows)]
fn terminate_owned_child(child: &mut Child, grace: Duration) -> io::Result<SignalOutcome> {
    // Windows `Child::kill` uses the real process handle (exact).
    child.kill()?;
    wait_child_exit(child, grace)?;
    Ok(SignalOutcome::EscalatedToKill)
}

#[cfg(not(any(unix, windows)))]
fn terminate_owned_child(child: &mut Child, grace: Duration) -> io::Result<SignalOutcome> {
    child.kill()?;
    wait_child_exit(child, grace)?;
    Ok(SignalOutcome::EscalatedToKill)
}

// ---------------------------------------------------------------------------
// Process-tree ownership
// ---------------------------------------------------------------------------

/// Bounded wait for the owned child to exit (reaping it), polling `try_wait`
/// until `grace` elapses. A child that still runs at the deadline is a loud
/// timeout error; the owned [`Child`] and its tree primitive are retained and
/// never silently revoked or dropped, so the caller can report or escalate.
fn wait_child_exit(child: &mut Child, grace: Duration) -> io::Result<()> {
    let deadline = Instant::now() + grace;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "owned child still running after bounded wait",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Owned Windows Job Object handle. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is
/// set at creation, so closing the last handle terminates every process still
/// assigned to the job.
#[cfg(windows)]
struct JobHandle(HANDLE);

#[cfg(windows)]
impl JobHandle {
    /// Create a kill-on-close job and assign the exact owned child handle to
    /// it. On any failure the job is closed and the error returned; the caller
    /// terminates and reaps the child (fail closed).
    fn assign(child: &Child, policy: Option<ProcessResourcePolicy>) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        let job = Self::create(policy)?;
        // SAFETY: as_raw_handle returns the exact owned process handle of the
        // unreaped child; the pid cannot be reused while any handle is open.
        let handle = child.as_raw_handle();
        let rc = unsafe { AssignProcessToJobObject(job.0, handle) };
        if rc == 0 {
            let error = io::Error::last_os_error();
            drop(job);
            return Err(error);
        }
        Ok(job)
    }

    /// Create a bounded kill-on-close job, owning the handle from creation.
    fn create(policy: Option<ProcessResourcePolicy>) -> io::Result<Self> {
        // SAFETY: null is valid for both the optional descriptor and name.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(policy) = policy {
            info.BasicLimitInformation.LimitFlags |=
                JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_JOB_TIME;
            info.BasicLimitInformation.PerJobUserTimeLimit =
                policy.cpu_seconds.saturating_mul(10_000_000) as i64;
            info.JobMemoryLimit = policy.active_tree_rss_bytes as usize;
        }
        // SAFETY: `job` is a valid CreateJobObjectW handle; `info` is a fully
        // initialized extended-limit carrier of the matching size.
        let rc = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if rc == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: closing our own job handle.
            unsafe {
                CloseHandle(job);
            }
            return Err(error);
        }
        Ok(Self(job))
    }

    /// Terminate every process currently in the job.
    fn terminate(&self) -> io::Result<()> {
        // SAFETY: `self.0` is a valid job handle created by `assign`.
        let rc = unsafe { TerminateJobObject(self.0, 1) };
        if rc == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

// SAFETY: the raw handle is only touched while holding the child mutex
// (terminate) or during exclusive `revoke`/`Drop`; `JobHandle` is not Clone and
// the owning `Arc` is its sole owner, so the handle is closed exactly once.
#[cfg(windows)]
unsafe impl Send for JobHandle {} // ubs:ignore — FFI wrapper, invariants: unique RAII owner of a job HANDLE
#[cfg(windows)]
unsafe impl Sync for JobHandle {} // ubs:ignore — FFI wrapper, invariants: unique RAII owner of a job HANDLE

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is our own job handle; Drop runs exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Terminate the owned Windows job (every member dies), then wait and reap the
/// root through the owned handle. `job_slot` is the tree's job field: on any
/// path where the job must no longer be held, it is taken so the
/// KILL_ON_JOB_CLOSE sweep terminates every remaining descendant (never leave
/// descendants silently).
#[cfg(windows)]
fn terminate_tree_job(
    child: &mut Child,
    job_slot: &mut Option<JobHandle>,
    grace: Duration,
) -> io::Result<SignalOutcome> {
    if child.try_wait()?.is_some() {
        // Root already exited (reaped by try_wait); sweep any job member that
        // survives it by closing the job (KILL_ON_JOB_CLOSE). No wait needed.
        *job_slot = None;
        return Ok(SignalOutcome::ExitedBeforeSignal);
    }
    let Some(job) = job_slot.as_ref() else {
        // The job was already taken (revoked or previous sweep); fall back to
        // the exact owned handle so teardown still settles.
        child.kill()?;
        wait_child_exit(child, grace)?;
        return Ok(SignalOutcome::EscalatedToKill);
    };
    match job.terminate() {
        Ok(()) => {
            wait_child_exit(child, grace)?;
            Ok(SignalOutcome::EscalatedToKill)
        }
        Err(error) => {
            // Close the job now: KILL_ON_JOB_CLOSE terminates every remaining
            // member (descendants included), then bounded-wait the root. The
            // bounded wait is the stronger failure if it also times out.
            *job_slot = None;
            if let Err(wait_error) = wait_child_exit(child, grace) {
                return Err(wait_error);
            }
            Err(error)
        }
    }
}

/// SIGTERM to the whole owned process group (while the root is unreaped and
/// still pins the PGID), the **full** bounded grace window so descendants get
/// their graceful window even if the root exits early, then a final SIGKILL to
/// the same exact pinned group. Only after every group signal is complete is
/// the root reaped (`wait_child_exit`), after which a non-signaling
/// `kill(-pgid, 0)` sweep confirms the descendants died. The root's numeric
/// PGID is pinned through every nonzero group signal, so a recycled group id
/// can never be hit.
///
/// Before any group signal, the waitable root pin is proved once via
/// [`child_exited_no_reap`]; if that pin is lost (`ECHILD`) no group signal is
/// sent (a recycled PGID must never be signaled). The final SIGKILL is issued
/// while the root is still unreaped; its ESRCH/EPERM is recorded (not fatal)
/// because a SIGTERM-drained group may leave only unsignalable reparented
/// zombies -- the non-signaling post-reap poll is the real proof. The outcome
/// is `ExitedBeforeSignal` if the group was already gone, `TerminatedGracefully`
/// if SIGKILL found no live member (the group drained on SIGTERM), and
/// `EscalatedToKill` if SIGKILL delivered to live survivors.
#[cfg(unix)]
fn terminate_tree_child(
    child: &mut Child,
    pgid: i32,
    grace: Duration,
) -> io::Result<SignalOutcome> {
    debug_assert!(pgid > 1, "process group must be our own spawned child's");
    let group = -pgid;
    // 0. Prove the waitable root pin still exists before any numeric group
    //    signal: the PGID is only provably ours while the root is waitable. A
    //    running root or a WNOWAIT-retained zombie are both safe (pin holds).
    //    On any error (ECHILD = pin lost) send no group signal and fail loud.
    let root_already_exited = child_exited_no_reap(child)?;
    // 1. SIGTERM to the exact group while the root is unreaped (pins the PGID).
    // SAFETY: `pgid` is our own spawned tree's group id; a negative pid signals
    // the whole group.
    let already_gone = if unsafe { libc::kill(group, libc::SIGTERM) } == -1 {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => true,
            // A group whose only members are zombies (root exited, no live
            // descendant) reports EPERM on macOS: no live member received the
            // signal. With the pin proving the root already exited, the group
            // is drained; reaping the zombie root is then exact.
            Some(libc::EPERM) if root_already_exited => true,
            _ => return Err(error),
        }
    } else {
        false
    };
    // 2. Bounded grace: keep the root unreaped (observe without reaping) so the
    //    PGID pin survives every later group signal. The full grace interval
    //    is preserved so descendants receive their graceful window even if the
    //    root exits early; only a lost pin (ECHILD) aborts loud.
    if !already_gone {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            // Surface a lost pin loud; never break early on root exit.
            let _ = child_exited_no_reap(child)?;
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // 3. Final SIGKILL to the same pinned group. The root is still unreaped, so
    //    the numeric PGID cannot have been recycled. Record ESRCH/EPERM instead
    //    of returning: a SIGTERM-drained group may leave only unsignalable
    //    members (EPERM) or be fully gone (ESRCH), and the
    //    non-signaling poll below is the real proof. Other errors are real
    //    failures. No signal is sent after this point.
    // SAFETY: as above; the root still pins the PGID.
    let mut sigkill_err: Option<io::Error> = None;
    if unsafe { libc::kill(group, libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) | Some(libc::EPERM) => sigkill_err = Some(error),
            _ => return Err(error),
        }
    }
    // 4. Every group signal is complete: reap the root now. Releasing the pin
    //    here is safe because no later group signal is issued.
    wait_child_exit(child, grace)?;
    // 5. Non-signaling group-gone polling. `kill(-pgid, 0)` delivers no signal;
    //    it only probes whether the group still has members, so it cannot harm
    //    a group that recycled the id after reap. Descendants received SIGKILL
    //    and must die; a group that survives is a loud error (root reaped,
    //    artifact retained, never silent success).
    match wait_for_group_gone(pgid, grace) {
        Ok(()) => {
            if already_gone {
                Ok(SignalOutcome::ExitedBeforeSignal)
            } else if sigkill_err.is_some() {
                // SIGKILL found no live member to kill: the group drained on
                // SIGTERM (graceful).
                Ok(SignalOutcome::TerminatedGracefully)
            } else {
                Ok(SignalOutcome::EscalatedToKill)
            }
        }
        Err(timeout) => {
            // Group did not drain: surface the original SIGKILL error (ESRCH or
            // EPERM) when present, otherwise the bounded-timeout error.
            Err(sigkill_err.unwrap_or(timeout))
        }
    }
}

/// Observe whether the owned child is still waitable (running or a reaped
/// zombie is fine) **without reaping it**, using `waitid(P_PID, ...,
/// WEXITED|WNOHANG|WNOWAIT)`. Returns `Ok(false)` while running, `Ok(true)`
/// once it has exited (zombie retained by `WNOWAIT`), or an error if the
/// waitable root pin is lost (`ECHILD`: already reaped elsewhere). The pin is
/// the proof that the numeric PGID is still ours; losing it is a loud error,
/// never `Ok(true)`, so no later numeric group signal targets a recycled id.
#[cfg(unix)]
fn child_exited_no_reap(child: &Child) -> io::Result<bool> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() }; // ubs:ignore — FFI wrapper, invariants: siginfo_t is a C union; waitid requires a zeroed out-param and writes every field later read via si_pid()
    // SAFETY: `P_PID` targets only our owned child's pid; `WNOWAIT` observes
    // without reaping; `WNOHANG` returns immediately when no state changed.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc == 0 {
        // POSIX specifies `si_pid == 0` when WNOHANG found no waitable status.
        // `si_signo` is not a portable discriminator here (Darwin leaves it
        // zero even when the exited child is reported).
        // SAFETY: waitid returned 0, so `info` holds a kernel-written siginfo_t;
        // POSIX defines si_pid == 0 for WNOHANG with no waitable status.
        Ok(unsafe { info.si_pid() } != 0)
    } else {
        // ECHILD means the waitable root pin is already gone: the numeric
        // PGID is no longer provably ours, so the caller must send no group
        // signal. Fail loud rather than pretending the child exited.
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn group_is_gone(pgid: i32) -> bool {
    // SAFETY: signal 0 only probes group existence; it sends nothing.
    let rc = unsafe { libc::kill(-pgid, 0) };
    if rc == 0 {
        return false;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Bounded sweep: the whole tree (root + descendants) must be gone. The SIGKILL
/// already went out; a group that still exists after the grace window is a loud
/// timeout error, never silent success.
#[cfg(unix)]
fn wait_for_group_gone(pgid: i32, grace: Duration) -> io::Result<()> {
    if group_is_gone(pgid) {
        return Ok(());
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if group_is_gone(pgid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("process tree (pgid {pgid}) still alive after SIGKILL"),
    ))
}

/// Exact escalation for a detached process (warm daemon stem) when the
/// authenticated socket RPC is unavailable.
///
/// Linux: opens a `pidfd` pinned to the process, verifies the captured start
/// identity against the process the fd names, then uses `pidfd_send_signal`
/// (kernel-pinned, never a numeric pid). Every other platform fails closed.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn escalate_detached(
    binding: &ChildBinding,
    grace: Duration,
) -> Result<SignalOutcome, IdentityError> {
    let expected = binding.identity().ok_or(IdentityError::Unsupported)?;
    if !binding.is_live() {
        return Err(IdentityError::Missing);
    }
    // SAFETY: pidfd_open returns a fresh fd (or -1); the pid is a u32 that
    // passed the `> 1` guard inside capture, and flags are 0.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, expected.pid as libc::pid_t, 0) }
        as libc::c_int;
    if fd < 0 {
        return Err(IdentityError::from(io::Error::last_os_error()));
    }
    let finish = |fd: libc::c_int, result: Result<SignalOutcome, IdentityError>| {
        // SAFETY: fd is uniquely owned by this function.
        unsafe {
            libc::close(fd);
        }
        result
    };
    // The pidfd is pinned to the process currently at that pid; re-verify the
    // captured start identity against it before signaling anything.
    if let Err(error) = verify_pidfd_identity(fd, &expected) {
        return finish(fd, Err(error));
    }
    // SAFETY: `fd` is a pidfd uniquely owned here and identity-verified
    // against `expected`; SIGTERM is sent through the kernel-pinned fd.
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd,
            libc::SIGTERM,
            std::ptr::null::<libc::c_void>(),
            0,
        )
    } != 0
    {
        let error = IdentityError::from(io::Error::last_os_error());
        return finish(fd, Err(error));
    }
    let deadline = Instant::now() + grace;
    loop {
        if pidfd_exited(fd) {
            return finish(fd, Ok(SignalOutcome::TerminatedGracefully));
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Escalate only through the still-pinned pidfd, after re-verifying.
    if let Err(error) = verify_pidfd_identity(fd, &expected) {
        return finish(fd, Err(error));
    }
    // SAFETY: same pinned pidfd after re-verify; SIGKILL still cannot target
    // a recycled pid.
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd,
            libc::SIGKILL,
            std::ptr::null::<libc::c_void>(),
            0,
        )
    } != 0
    {
        let error = IdentityError::from(io::Error::last_os_error());
        return finish(fd, Err(error));
    }
    let escalation_deadline = Instant::now() + grace;
    while Instant::now() < escalation_deadline {
        if pidfd_exited(fd) {
            return finish(fd, Ok(SignalOutcome::EscalatedToKill));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    finish(fd, Err(IdentityError::IdentityChanged))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_pidfd_identity(
    _fd: libc::c_int,
    expected: &ProcessIdentity,
) -> Result<(), IdentityError> {
    // The public API returns a plain `ProcessIdentity` and errors with
    // NotFound when the process is gone.
    match ProcessIdentity::capture(expected.pid) {
        Ok(current) if current == *expected => Ok(()),
        Ok(_) => Err(IdentityError::IdentityChanged),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(IdentityError::Missing),
        Err(_) => Err(IdentityError::Unsupported),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn pidfd_exited(fd: libc::c_int) -> bool {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll_fd points to one initialized pollfd; timeout 0 polls once.
    let rc = unsafe { libc::poll(&mut poll_fd, 1, 0) };
    rc > 0
}

/// Detached escalation fails closed on platforms without an exact signal
/// handle (macOS included). No PID-only signal is ever sent.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn escalate_detached(
    _binding: &ChildBinding,
    _grace: Duration,
) -> Result<SignalOutcome, IdentityError> {
    Err(IdentityError::Unsupported)
}

/// Resume the primary thread of a `CREATE_SUSPENDED` child by looking up its
/// thread id through a Toolhelp snapshot (stable API; `main_thread_handle` is
/// still unstable). The snapshot and the thread handle are both RAII/closed on
/// every path.
#[cfg(windows)]
fn resume_primary_thread(pid: u32) -> io::Result<()> {
    // SAFETY: TH32CS_SNAPTHREAD with pid 0 snapshots all threads; Handle owns
    // the returned snapshot and rejects NULL/INVALID_HANDLE_VALUE.
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        cntUsage: 0,
        th32ThreadID: 0,
        th32OwnerProcessID: 0,
        tpBasePri: 0,
        tpDeltaPri: 0,
        dwFlags: 0,
    };
    // SAFETY: snapshot is valid and entry is initialized with dwSize.
    if unsafe { Thread32First(snapshot.raw(), &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread id belongs to our CREATE_SUSPENDED child;
            // Handle owns the fresh THREAD_SUSPEND_RESUME handle.
            let thread =
                Handle::new(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: the child has not executed since CREATE_SUSPENDED, so
            // this is its sole primary thread and its suspend count is one.
            if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        // SAFETY: iterating our valid snapshot; zero means no more entries.
        if unsafe { Thread32Next(snapshot.raw(), &mut entry) } == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "spawned child primary thread not found",
            ));
        }
    }
}
