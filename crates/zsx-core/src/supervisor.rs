//! Call-scoped daemonless execute supervisor (Wave 10).
//!
//! The supervisor runs one [`ZerokernelExecuteRequest`] through the existing
//! restricted interpreter with a **fresh bounded runtime per call**, then
//! verifies quiescence and destruction before returning. The session (the
//! supervisor's adapters, roots, and identity) survives across calls; the
//! interpreter does not.
//!
//! # Two production profiles, one protocol
//!
//! - [`SupervisorProfile::Embedded`] — the runtime (`ZsxConnector` +
//!   [`Host`] + interpreter) is created and dropped **on the calling
//!   thread** for every call. No socket, listener, daemon, worker process,
//!   or idle event loop exists; the per-call connector dispatcher threads
//!   are joined before the call returns.
//! - [`SupervisorProfile::OneShot`] — the supervisor spawns **one sandboxed
//!   child** (the same executable's `kernel` subcommand by default), hands
//!   it the canonical request over a stdio pipe, waits for the canonical
//!   response, then kills and reaps the exact process tree on every terminal
//!   path (success, syntax error, JS exception, deadline, cancellation, and
//!   worker crash). The child runs the embedded profile inside itself.
//!
//! Both profiles return the identical [`ZerokernelExecuteResponse`] envelope
//! and keep the native direct path (`zsx exec`, MCP `zero_execute`) intact as
//! the fallback.
//!
//! # Forbidden shapes
//!
//! No listener, daemon, per-session resident worker, idle pool, background
//! poller, detached task, or event loop is created. The only threads are
//! call-scoped (per-call connector dispatchers; per-call child stdio
//! threads) and are joined on every path before the call returns. The
//! one-shot child is parent-death bound and lives in its own process group,
//! so a crashed harness cannot orphan it.
//!
//! # Terminal-outcome laws (W10-T3/T4/T6/T11)
//!
//! - **Quiescent commit** (T3): a returned response implies zero surviving
//!   cell work — on failure the embedded profile cancels the request signal
//!   and waits for dispatch idle before dropping the runtime; on success the
//!   interpreter only settles after every dispatch it admitted completed.
//! - **Zero resident executor** (T4): [`Supervisor::live_executors`] and
//!   [`Supervisor::live_children`] return zero after every call; the counters
//!   are decremented by RAII guards on every path, including panics.
//! - **No-orphan process** (T11): the one-shot child is killed (SIGTERM,
//!   bounded grace, SIGKILL to its pinned group) and reaped on every
//!   terminal path; [`Supervisor::child_spawn_count`] is the audit
//!   instrument.
//! - **Transactional failure** (T6): every non-completed terminal returns a
//!   `Failed` or `DecisionRequired` response whose root evidence proves the
//!   injected roots unchanged. The protocol grants no write authority and
//!   the supervisor installs no approval grants, so the approval-required
//!   mutation `fs.write` is refused at the connector boundary before any
//!   adapter call ("approval grant rejected: Missing"). All other capability
//!   policy — `fs.edit`/`fs.transact` journaled mutations, shell, index,
//!   remember — is exactly the canonical adapter policy, unchanged from
//!   native.
//! - **K0 preflight boundary** (zerostack-pvwg): every call first runs the
//!   capability broker ([`crate::preflight::broker`]), which parses the
//!   cell, resolves and normalizes capability mentions against the existing
//!   V6 registration, injects the operational context into the receipt, and
//!   validates rooted manifests and explicit read-grant coverage. It never
//!   rewrites the program; semantic ambiguity returns a typed
//!   `DecisionRequired`, structural refusals fail before any execution (and
//!   before any one-shot child spawn), and the original program is what the
//!   interpreter runs.
//!
//! # Honest accounting notes
//!
//! - The restricted interpreter has no separate CPU clock; `cpu_ms_used` in
//!   the ledger reports the wall upper bound (CPU time can never exceed
//!   wall), never a fabricated zero or a clamped value.
//! - The zerokernel response schema has no error field; a terminal failure
//!   rides the `preflight` report (`ok = false`, `errors` = the typed
//!   detail). `preflight.ok = true` therefore means the call completed
//!   (`Completed` or `DecisionRequired`).
//! - `wall_ms_used` reports actual measured wall and may marginally exceed
//!   the budget when a deadline enforcement granularity lands late; the
//!   ledger is honest bookkeeping, never clamped to the budget.
//! - The caller-supplied cancellation flag is shared with the runtime and
//!   adapters; a failed call leaves it set (per-call flags are expected).

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use zero_abi::raw_worker::EngineIdentity;
use zero_abi::zerokernel::{
    ExactHandles, FiniteBudget, KernelResourceLedger, PreflightReport, ReturnPolicy,
    RootEvidence, RootSnapshot, ZerokernelExecuteRequest, ZerokernelExecuteResponse,
};
use zero_codemode::{
    CancellationSignal, Connector, ExecutionMetrics, Host, HostError, HostLimits,
    MAX_INFLIGHT_CONNECTOR_CALLS, finalize_visible_error,
};
use zero_process::VerifiedChild;

use crate::adapter::DomainAdapter;
use crate::connector::{AggregateExecutionContext, ZsxConnector, registration};
use crate::preflight::BrokerOutcome;

/// Subcommand the one-shot child runs in the same executable
/// (`<current_exe> kernel`). The `zsx` binary implements it.
pub const KERNEL_CHILD_COMMAND: &str = "kernel";

/// Environment names the supervisor passes to the one-shot child, reusing
/// the raw-worker env contract so the child-side store and session identity
/// resolve exactly like every other process-backed path.
pub const STORE_ROOT_ENV: &str = "ZEROSTACK_STORE_ROOT";
pub const SESSION_ID_ENV: &str = "ZEROSTACK_SESSION_ID";

/// Hard ceiling for the canonical request the child accepts on stdin. The
/// protocol caps the program at 64 KiB; 256 KiB covers JSON escaping and
/// every other request field with slack, and anything larger fails closed.
pub const KERNEL_CHILD_MAX_REQUEST_BYTES: usize = 256 * 1024;

/// Hard ceiling for the canonical response the parent accepts from the
/// child. The host result budget is 1 MiB; 2 MiB covers the spill envelope
/// and ledger with slack, and anything larger fails closed as a crash path.
pub const KERNEL_CHILD_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Bound on the child stderr tail retained for crash diagnostics.
pub const KERNEL_CHILD_STDERR_CAPTURE_BYTES: usize = 64 * 1024;

/// Extra wall the parent grants the child beyond the request budget: the
/// child must create its own runtime before the plan's deadline starts.
pub const ONESHOT_SETTLE_GRACE: Duration = Duration::from_secs(2);

/// How long the parent waits for the child to exit after a valid response.
pub const ONESHOT_EXIT_SETTLE: Duration = Duration::from_secs(2);

/// SIGTERM-then-SIGKILL window for the exact one-shot process tree,
/// matching the raw-worker escalation window.
pub const ONESHOT_KILL_GRACE: Duration = Duration::from_millis(250);

/// Bounded wait for in-flight adapter dispatches to settle after a failed
/// embedded execution (quiescence before the runtime is dropped).
pub const SUPERVISOR_IDLE_WAIT: Duration = Duration::from_secs(15);

/// Wait-loop granularity for the one-shot child: the loop re-checks the
/// cancellation flag and the deadline at least this often.
pub const ONESHOT_CANCEL_POLL: Duration = Duration::from_millis(25);

/// One-call executor stack budget, matching the connector host.
const SUPERVISOR_STACK_BYTES: usize = 1024 * 1024;

/// One-call interpreter instruction budget, matching the connector host.
const SUPERVISOR_INSTRUCTION_BUDGET: u64 = 10_000_000;

/// One-call microtask ceiling, matching the connector host.
const SUPERVISOR_MICROTASK_CEILING: usize = 1_024;

/// One-call plan byte ceiling; the protocol already caps programs at 64 KiB.
const SUPERVISOR_MAX_PLAN_BYTES: usize = 256 * 1024;

/// One-call host JSON budget; the visible result budget comes from the
/// request's return policy.
const SUPERVISOR_MAX_JSON_BYTES: usize = 1024 * 1024;

/// Disposable-executor profile for one call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorProfile {
    /// Fresh bounded runtime on the calling thread, dropped before return.
    Embedded,
    /// One sandboxed child per call, killed and reaped on every terminal
    /// path. Never retained between calls.
    OneShot,
}

/// Launch spec of the one-shot kernel child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneShotChild {
    program: PathBuf,
    args: Vec<String>,
}

impl OneShotChild {
    /// Explicit child program and arguments. The child must read one
    /// canonical [`ZerokernelExecuteRequest`] from stdin, write one
    /// canonical [`ZerokernelExecuteResponse`] line to stdout, and exit.
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, SupervisorError> {
        let program = program.into();
        if program.as_os_str().is_empty() {
            return Err(SupervisorError::Internal(
                "one-shot child program must not be empty".into(),
            ));
        }
        Ok(Self {
            program,
            args: args.into_iter().map(Into::into).collect(),
        })
    }

    /// Default child: the current executable with the `kernel` subcommand.
    pub fn current_exe() -> Result<Self, SupervisorError> {
        let exe = std::env::current_exe().map_err(|error| {
            SupervisorError::Internal(format!(
                "cannot locate the current executable for the one-shot child: {error}"
            ))
        })?;
        Self::new(exe, [KERNEL_CHILD_COMMAND])
    }
}

/// Typed supervisor failure. These are caller errors — the request is not a
/// valid protocol message for this supervisor. Protocol-level terminal
/// outcomes (syntax error, exception, deadline, cancellation, crash, failed
/// preflight) are returned as `Failed` responses in the protocol envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SupervisorError {
    /// The request failed structural validation and was refused before any
    /// execution began.
    InvalidRequest(String),
    /// The request names a session different from this supervisor's session.
    SessionMismatch { expected: String, actual: String },
    /// A request root does not bind to this supervisor's roots.
    RootMismatch(String),
    /// The per-call runtime could not be constructed.
    Runtime(String),
    /// The one-shot child could not be spawned or its pipes were missing.
    Spawn(String),
    /// Internal invariant failure.
    Internal(String),
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(detail) => write!(f, "invalid execute request: {detail}"),
            Self::SessionMismatch { expected, actual } => write!(
                f,
                "request session {actual:?} does not match supervisor session {expected:?}"
            ),
            Self::RootMismatch(detail) => write!(f, "request root mismatch: {detail}"),
            Self::Runtime(detail) => write!(f, "runtime unavailable: {detail}"),
            Self::Spawn(detail) => write!(f, "cannot spawn one-shot child: {detail}"),
            Self::Internal(detail) => write!(f, "internal supervisor error: {detail}"),
        }
    }
}

impl std::error::Error for SupervisorError {}

/// RAII live-executor accounting: one in-flight call (either profile).
struct LiveExecutorGuard<'a> {
    counter: &'a AtomicU64,
}

impl<'a> LiveExecutorGuard<'a> {
    fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for LiveExecutorGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// RAII live-child accounting for the one-shot profile.
struct LiveChildGuard<'a> {
    counter: &'a AtomicU64,
}

impl<'a> LiveChildGuard<'a> {
    fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for LiveChildGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Call-scoped daemonless execute supervisor (Wave 10).
///
/// The session shell — adapters, roots, and session identity — is built once
/// and survives across calls; every call creates a fresh bounded runtime and
/// destroys it before returning. See the [module docs](self) for the
/// terminal-outcome laws and honest accounting notes.
pub struct Supervisor {
    profile: SupervisorProfile,
    root: PathBuf,
    state_root: PathBuf,
    session_id: String,
    adapters: BTreeMap<EngineIdentity, Arc<dyn DomainAdapter>>,
    child: OneShotChild,
    request_sequence: AtomicU64,
    live_executors: AtomicU64,
    live_children: AtomicU64,
    child_spawns: AtomicU64,
}

impl Supervisor {
    /// Start building a supervisor rooted at `root` (canonicalized on build).
    pub fn builder(root: impl Into<PathBuf>) -> SupervisorBuilder {
        SupervisorBuilder::new(root.into())
    }

    /// The profile this supervisor runs every call under.
    pub fn profile(&self) -> SupervisorProfile {
        self.profile
    }

    /// Canonicalized workspace root the supervisor is bound to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonicalized session state root the supervisor is bound to.
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Session identity the supervisor is bound to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Executor calls currently in flight (embedded runtimes plus one-shot
    /// children). Zero after every settled call.
    pub fn live_executors(&self) -> u64 {
        self.live_executors.load(Ordering::Acquire)
    }

    /// One-shot children currently live. Zero after every settled call.
    pub fn live_children(&self) -> u64 {
        self.live_children.load(Ordering::Acquire)
    }

    /// Total one-shot children spawned since this supervisor was built. The
    /// process-tree audit instrument: after every call,
    /// `live_children() == 0` while `child_spawn_count()` counts completed
    /// spawns, so a leftover child would surface as a non-zero live count.
    pub fn child_spawn_count(&self) -> u64 {
        self.child_spawns.load(Ordering::Acquire)
    }

    /// Execute one request under the supervisor's profile with no external
    /// cancellation.
    pub fn execute(
        &self,
        request: ZerokernelExecuteRequest,
    ) -> Result<ZerokernelExecuteResponse, SupervisorError> {
        self.execute_cancellable(request, Arc::new(AtomicBool::new(false)))
    }

    /// Execute one request under the supervisor's profile.
    ///
    /// `cancel` is a shared per-call flag: the interpreter, the connector
    /// admission, and every adapter observe it (embedded), and the one-shot
    /// wait loop polls it and kills the child when it flips. A failed call
    /// leaves the flag set, so callers must use a per-call flag.
    pub fn execute_cancellable(
        &self,
        request: ZerokernelExecuteRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<ZerokernelExecuteResponse, SupervisorError> {
        request.validate().map_err(|error| {
            SupervisorError::InvalidRequest(error.to_string())
        })?;
        self.bind_session(&request)?;
        self.bind_roots(&request)?;
        let snapshot = self.root_snapshot(&request);
        let preflight = self.preflight(&request, &snapshot);
        if !preflight.ok {
            return Ok(failed_response(
                preflight,
                zeroed_ledger(),
                unchanged_evidence(&snapshot),
                "preflight failed",
            ));
        }
        // K0 capability broker (zerostack-pvwg): parse / resolve /
        // normalize / inject / validate. The broker never rewrites the
        // program; it either proves the plan structurally sound (receipt
        // evidence merged into the preflight report), returns a typed
        // DecisionRequired for semantic ambiguity, or refuses before any
        // execution — one call either way, and a refused one-shot request
        // never spawns its child.
        let mut preflight = preflight;
        match crate::preflight::broker(&request, &self.root, &self.session_id) {
            BrokerOutcome::Proceed(receipt) => {
                preflight.warnings.extend(receipt.warning_lines());
            }
            BrokerOutcome::DecisionRequired(decision) => {
                return Ok(ZerokernelExecuteResponse::decision_required(
                    ExactHandles::default(),
                    preflight,
                    zeroed_ledger(),
                    unchanged_evidence(&snapshot),
                    decision,
                )
                .map_err(|error| {
                    SupervisorError::Internal(format!(
                        "cannot build broker decision response: {error}"
                    ))
                })?);
            }
            BrokerOutcome::Refused(detail) => {
                return Ok(failed_response(
                    preflight,
                    zeroed_ledger(),
                    unchanged_evidence(&snapshot),
                    &detail,
                ));
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(failed_response(
                preflight,
                zeroed_ledger(),
                unchanged_evidence(&snapshot),
                "execution cancelled before start",
            ));
        }
        let _executor_guard = LiveExecutorGuard::new(&self.live_executors);
        match self.profile {
            SupervisorProfile::Embedded => {
                self.run_embedded(&request, &cancel, preflight, snapshot)
            }
            SupervisorProfile::OneShot => {
                self.run_oneshot(&request, &cancel, preflight, snapshot)
            }
        }
    }

    /// The request's session handle must name this supervisor's session.
    fn bind_session(&self, request: &ZerokernelExecuteRequest) -> Result<(), SupervisorError> {
        if let Some(actual) = &request.session
            && actual != &self.session_id
        {
            return Err(SupervisorError::SessionMismatch {
                expected: self.session_id.clone(),
                actual: actual.clone(),
            });
        }
        Ok(())
    }

    /// Identity binding of the request's roots to this supervisor. Paths
    /// that do not exist cannot be identity-checked; they flow into the
    /// preflight report instead (the injected root failed its check).
    fn bind_roots(&self, request: &ZerokernelExecuteRequest) -> Result<(), SupervisorError> {
        let project = Path::new(&request.roots.project_root);
        if let Ok(resolved) = project.canonicalize()
            && resolved != self.root
        {
            return Err(SupervisorError::RootMismatch(format!(
                "project_root {} resolves to {} but this supervisor is bound to {}",
                request.roots.project_root,
                resolved.display(),
                self.root.display()
            )));
        }
        if let Some(workspace) = &request.roots.workspace_root
            && let Ok(resolved) = Path::new(workspace).canonicalize()
            && resolved != self.root
        {
            return Err(SupervisorError::RootMismatch(format!(
                "workspace_root {} resolves to {} but this supervisor is bound to {}",
                workspace,
                resolved.display(),
                self.root.display()
            )));
        }
        if let Some(session_root) = &request.roots.expected_session_root
            && let Ok(resolved) = Path::new(session_root).canonicalize()
            && resolved != self.state_root
        {
            return Err(SupervisorError::RootMismatch(format!(
                "expected_session_root {} resolves to {} but this supervisor's session state root is {}",
                session_root,
                resolved.display(),
                self.state_root.display()
            )));
        }
        Ok(())
    }

    /// The root snapshot both sides of the unchanged root evidence. The
    /// read-only protocol carries no successor root.
    fn root_snapshot(&self, request: &ZerokernelExecuteRequest) -> RootSnapshot {
        let root_text = self.root.to_string_lossy().into_owned();
        let state_text = self.state_root.to_string_lossy().into_owned();
        RootSnapshot {
            workspace_root: Some(
                request
                    .roots
                    .workspace_root
                    .clone()
                    .unwrap_or_else(|| root_text.clone()),
            ),
            project_root: request.roots.project_root.clone(),
            session_root: Some(
                request
                    .roots
                    .expected_session_root
                    .clone()
                    .unwrap_or(state_text),
            ),
        }
    }

    /// Read-only preflight: every injected root must exist with the expected
    /// shape. Failures are reported in the response envelope, never thrown.
    fn preflight(
        &self,
        request: &ZerokernelExecuteRequest,
        snapshot: &RootSnapshot,
    ) -> PreflightReport {
        let mut checked_roots = Vec::new();
        let warnings = Vec::new();
        let mut errors = Vec::new();
        let check_directory = |label: &str, value: &str, checked: &mut Vec<String>, errs: &mut Vec<String>| {
            checked.push(value.to_owned());
            if !is_directory(Path::new(value)) {
                errs.push(format!("{label} {value} is not an existing directory"));
            }
        };
        check_directory(
            "project_root",
            &snapshot.project_root,
            &mut checked_roots,
            &mut errors,
        );
        if let Some(workspace) = &snapshot.workspace_root {
            check_directory(
                "workspace_root",
                workspace,
                &mut checked_roots,
                &mut errors,
            );
        }
        if let Some(session_root) = &snapshot.session_root {
            check_directory(
                "expected_session_root",
                session_root,
                &mut checked_roots,
                &mut errors,
            );
        }
        if let Some(request_root) = &request.roots.request_root {
            checked_roots.push(request_root.clone());
            if !path_exists(Path::new(request_root)) {
                errors.push(format!("request_root {request_root} does not exist"));
            }
        }
        if let Some(manifest) = &request.roots.capability_manifest_root {
            checked_roots.push(manifest.clone());
            if !path_exists(Path::new(manifest)) {
                errors.push(format!(
                    "capability_manifest_root {manifest} does not exist"
                ));
            }
        }
        PreflightReport {
            ok: errors.is_empty(),
            checked_roots,
            warnings,
            errors,
        }
    }

    /// Embedded profile: create the bounded runtime on the calling thread,
    /// run the plan, cancel + wait for dispatch idle on failure, then drop
    /// the runtime (the per-call connector joins its dispatcher threads) so
    /// the call returns quiescent on every path.
    fn run_embedded(
        &self,
        request: &ZerokernelExecuteRequest,
        cancel: &Arc<AtomicBool>,
        preflight: PreflightReport,
        snapshot: RootSnapshot,
    ) -> Result<ZerokernelExecuteResponse, SupervisorError> {
        let generation: u64 = 1;
        let request_id = self.request_sequence.fetch_add(1, Ordering::Relaxed);
        let connector = Rc::new(
            ZsxConnector::new_with_state_root(
                self.root.clone(),
                self.state_root.clone(),
                self.session_id.clone(),
                self.adapters.clone(),
            )
            .map_err(|error| {
                SupervisorError::Runtime(format!("cannot create connector: {error}"))
            })?,
        );
        let limits = host_limits_for_budget(&request.budget)?;
        let mut host = Host::new(limits, registration())
            .map_err(|error| SupervisorError::Runtime(format!("cannot create host: {error}")))?;
        host = host
            .with_visible_result_budget(visible_result_budget(&request.return_policy))
            .map_err(|error| {
                SupervisorError::Runtime(format!("cannot set result budget: {error}"))
            })?;
        if self.state_root != self.root {
            host = host.with_result_spill(self.state_root.clone());
        }
        let signal = CancellationSignal::from_atomic(Arc::clone(cancel));
        connector
            .set_execution_context(AggregateExecutionContext {
                generation,
                request_id,
            })
            .map_err(|err| SupervisorError::Runtime(err.to_string()))?;
        connector.set_request_cancellation(signal);
        let outcome = host.execute_measured_with_cancel_timeout_context(
            &request.program,
            Rc::clone(&connector) as Rc<dyn Connector>,
            Arc::clone(cancel),
            Duration::from_millis(request.budget.wall_ms),
            generation,
            request_id,
        );
        if outcome.result.is_err() {
            // Stop any in-flight adapter work through the shared flag, then
            // wait until every admitted dispatch settled: the runtime must
            // not be dropped with work still running (W10-T3).
            cancel.store(true, Ordering::Release);
            connector
                .wait_for_dispatch_idle(SUPERVISOR_IDLE_WAIT)
                .map_err(|err| SupervisorError::Runtime(err.to_string()))?;
        }
        connector.clear_request_cancellation();
        connector.clear_execution_context();
        drop(connector);
        self.project_outcome(preflight, snapshot, outcome.metrics, outcome.result)
    }

    /// One-shot profile: spawn exactly one sandboxed child, feed it the
    /// canonical request, wait for the canonical response under the budget
    /// deadline, and kill + reap the exact process tree on every terminal
    /// path (W10-T4/T11).
    fn run_oneshot(
        &self,
        request: &ZerokernelExecuteRequest,
        cancel: &Arc<AtomicBool>,
        preflight: PreflightReport,
        snapshot: RootSnapshot,
    ) -> Result<ZerokernelExecuteResponse, SupervisorError> {
        let generation: u64 = 1;
        let started = Instant::now();
        let mut command = Command::new(&self.child.program);
        command
            .args(&self.child.args)
            .env(STORE_ROOT_ENV, &self.state_root)
            .env(SESSION_ID_ENV, &self.session_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, pipes) =
            VerifiedChild::spawn_tree_with_pipes(command, &self.session_id, generation)
                .map_err(|error| SupervisorError::Spawn(error.to_string()))?;
        crate::record_process_spawn();
        self.child_spawns.fetch_add(1, Ordering::AcqRel);
        let _child_guard = LiveChildGuard::new(&self.live_children);

        let Some(stdin) = pipes.stdin else {
            kill_and_reap(&child, &self.session_id, generation);
            return Err(SupervisorError::Spawn(
                "kernel child stdin unavailable".into(),
            ));
        };
        let Some(stdout) = pipes.stdout else {
            kill_and_reap(&child, &self.session_id, generation);
            return Err(SupervisorError::Spawn(
                "kernel child stdout unavailable".into(),
            ));
        };
        let Some(stderr) = pipes.stderr else {
            kill_and_reap(&child, &self.session_id, generation);
            return Err(SupervisorError::Spawn(
                "kernel child stderr unavailable".into(),
            ));
        };

        // Call-scoped stdio threads: bounded stdin writer (EOF after the
        // request), bounded stdout reader (one response line to EOF), and
        // bounded stderr capture for crash diagnostics. All three are joined
        // after the child is settled, never detached.
        let request_bytes = request.canonical_bytes();
        let (writer_done_tx, writer_done_rx) = mpsc::sync_channel(1);
        let writer = spawn_thread("zsx-kernel-stdin", move || {
            let result = (|| -> std::io::Result<()> {
                let mut stdin = stdin;
                stdin.write_all(&request_bytes)?;
                drop(stdin);
                Ok(())
            })();
            let _ = writer_done_tx.send(result);
        })
        .map_err(|err| SupervisorError::Internal(err.to_string()))?;

        let (output_tx, output_rx) = mpsc::sync_channel(1);
        let reader = spawn_thread("zsx-kernel-stdout", move || {
            let mut stdout = stdout;
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buffer.extend_from_slice(&chunk[..n]);
                        if buffer.len() > KERNEL_CHILD_MAX_RESPONSE_BYTES {
                            let _ = output_tx.send(Err(KernelIoError::Bounds(
                                buffer.len(),
                                KERNEL_CHILD_MAX_RESPONSE_BYTES,
                            )));
                            return;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = output_tx.send(Err(KernelIoError::Io(error)));
                        return;
                    }
                }
            }
            let _ = output_tx.send(Ok(buffer));
        })
        .map_err(|err| SupervisorError::Internal(err.to_string()))?;

        let stderr_state = Arc::new((Mutex::new(StderrState::default()), Condvar::new()));
        let stderr_thread = spawn_thread("zsx-kernel-stderr", {
            let stderr_state = Arc::clone(&stderr_state);
            move || {
                let mut stderr = stderr;
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            let (lock, ready) = &*stderr_state;
                            if let Ok(mut state) = lock.lock() {
                                state.total = state.total.saturating_add(n as u64);
                                if state.bytes.len() < KERNEL_CHILD_STDERR_CAPTURE_BYTES {
                                    let room = KERNEL_CHILD_STDERR_CAPTURE_BYTES
                                        .saturating_sub(state.bytes.len());
                                    let take = room.min(n);
                                    state.bytes.extend_from_slice(&chunk[..take]);
                                    if take < n {
                                        state.truncated = true;
                                    }
                                } else {
                                    state.truncated = true;
                                }
                            }
                            ready.notify_all();
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let (lock, ready) = &*stderr_state;
                if let Ok(mut state) = lock.lock() {
                    state.complete = true;
                }
                ready.notify_all();
            }
        })
        .map_err(|err| SupervisorError::Internal(err.to_string()))?;

        // Wait for the response line under the bounded deadline, polling the
        // cancellation flag. Every exit from this loop leads to kill (when
        // needed) + reap before any thread is joined.
        let wait_deadline = Instant::now()
            .checked_add(Duration::from_millis(request.budget.wall_ms))
            .and_then(|deadline| deadline.checked_add(ONESHOT_SETTLE_GRACE))
            .unwrap_or_else(Instant::now);
        let outcome = loop {
            if cancel.load(Ordering::Acquire) {
                break KernelChildOutcome::Cancelled;
            }
            let remaining = wait_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break KernelChildOutcome::Deadline;
            }
            match output_rx.recv_timeout(remaining.min(ONESHOT_CANCEL_POLL)) {
                Ok(Ok(bytes)) => {
                    if bytes.is_empty() {
                        // EOF with no bytes at all: the child exited without
                        // writing a response — the worker-crash terminal.
                        break KernelChildOutcome::OutputGone;
                    }
                    match ZerokernelExecuteResponse::from_canonical_bytes(&bytes) {
                        Ok(response) => break KernelChildOutcome::Response(response),
                        Err(error) => break KernelChildOutcome::Malformed(error.to_string()),
                    }
                }
                Ok(Err(error)) => break KernelChildOutcome::Transport(error.to_string()),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break KernelChildOutcome::OutputGone,
            }
        };

        let ledger = KernelResourceLedger {
            wall_ms_used: nanos_to_ms_ceil(started.elapsed().as_nanos()),
            cpu_ms_used: nanos_to_ms_ceil(started.elapsed().as_nanos()),
            calls_made: 0,
            bytes_out: 0,
        };
        let mut warning: Option<String> = None;
        let response = match outcome {
            KernelChildOutcome::Response(response) => {
                // The child consumed the full request before responding, so
                // the writer is already done. The child should exit by
                // itself; if it lingers, kill the exact tree and reap.
                match child.wait(
                    &self.session_id,
                    generation,
                    ONESHOT_EXIT_SETTLE,
                    ONESHOT_KILL_GRACE.min(Duration::from_millis(100)),
                ) {
                    Ok(_) => {}
                    Err(_) => {
                        kill_and_reap(&child, &self.session_id, generation);
                        warning = Some(
                            "kernel child did not exit after responding; killed and reaped"
                                .to_owned(),
                        );
                    }
                }
                let _ = writer_done_rx.recv_timeout(Duration::from_millis(500));
                let mut response = response;
                if let Some(warning) = warning {
                    response.preflight.warnings.push(warning);
                }
                Ok(response)
            }
            KernelChildOutcome::Cancelled => {
                kill_and_reap(&child, &self.session_id, generation);
                Ok(failed_response(
                    preflight,
                    ledger,
                    unchanged_evidence(&snapshot),
                    "execution cancelled",
                ))
            }
            KernelChildOutcome::Deadline => {
                kill_and_reap(&child, &self.session_id, generation);
                Ok(failed_response(
                    preflight,
                    ledger,
                    unchanged_evidence(&snapshot),
                    &format!(
                        "wall-clock deadline exceeded ({}ms budget + {}ms settle grace)",
                        request.budget.wall_ms,
                        ONESHOT_SETTLE_GRACE.as_millis()
                    ),
                ))
            }
            KernelChildOutcome::Malformed(detail) => {
                kill_and_reap(&child, &self.session_id, generation);
                Ok(failed_response(
                    preflight,
                    ledger,
                    unchanged_evidence(&snapshot),
                    &format!("kernel child returned a malformed response: {detail}"),
                ))
            }
            KernelChildOutcome::Transport(detail) => {
                kill_and_reap(&child, &self.session_id, generation);
                Ok(failed_response(
                    preflight,
                    ledger,
                    unchanged_evidence(&snapshot),
                    &format!("kernel child output failure: {detail}"),
                ))
            }
            KernelChildOutcome::OutputGone => {
                // The child exited (or died) without a response line: reap
                // and report the captured status and stderr tail.
                let status = settle_exited_child(&child, &self.session_id, generation);
                let stderr_tail = stderr_tail(&stderr_state, Duration::from_millis(100));
                let status_text = match status {
                    Some(status) => status.to_string(),
                    None => "unknown".to_owned(),
                };
                Ok(failed_response(
                    preflight,
                    ledger,
                    unchanged_evidence(&snapshot),
                    &format!(
                        "kernel child exited without a response (status: {status_text}): {stderr_tail}"
                    ),
                ))
            }
        };

        // The child is dead (reaped) on every path above, so every pipe read
        // end is closed and all three call-scoped threads exit promptly.
        let _ = writer.join();
        let _ = reader.join();
        let _ = stderr_thread.join();
        response
    }

    /// Map the measured embedded outcome onto the protocol envelope.
    fn project_outcome(
        &self,
        preflight: PreflightReport,
        snapshot: RootSnapshot,
        metrics: ExecutionMetrics,
        result: Result<serde_json::Value, HostError>,
    ) -> Result<ZerokernelExecuteResponse, SupervisorError> {
        let ledger = KernelResourceLedger {
            wall_ms_used: nanos_to_ms_ceil(u128::from(metrics.wall_time_ns)),
            // The restricted interpreter has no separate CPU clock; wall is
            // the honest upper bound (CPU time can never exceed wall).
            cpu_ms_used: nanos_to_ms_ceil(u128::from(metrics.wall_time_ns)),
            calls_made: metrics
                .connector_dispatches
                .min(u64::from(u32::MAX)) as u32,
            bytes_out: 0,
        };
        let evidence = unchanged_evidence(&snapshot);
        match result {
            Ok(value) => {
                let bytes_out = serde_json::to_string(&value)
                    .map(|encoded| encoded.len())
                    .unwrap_or(0)
                    .min(u32::MAX as usize) as u32;
                ZerokernelExecuteResponse::completed(
                    ExactHandles::default(),
                    preflight,
                    KernelResourceLedger { bytes_out, ..ledger },
                    evidence,
                    value,
                )
                .map_err(|error| {
                    SupervisorError::Internal(format!(
                        "cannot build completed response: {error}"
                    ))
                })
            }
            Err(HostError::DecisionRequired(payload)) => {
                ZerokernelExecuteResponse::decision_required(
                    ExactHandles::default(),
                    preflight,
                    ledger,
                    evidence,
                    payload,
                )
                .map_err(|error| {
                    SupervisorError::Internal(format!(
                        "cannot build decision response: {error}"
                    ))
                })
            }
            Err(error) => Ok(failed_response(
                preflight,
                ledger,
                evidence,
                &finalize_visible_error(&format!("execution failed: {error}")),
            )),
        }
    }
}

/// Builder for a [`Supervisor`].
pub struct SupervisorBuilder {
    root: PathBuf,
    state_root: Option<PathBuf>,
    session_id: Option<String>,
    profile: SupervisorProfile,
    child: Option<OneShotChild>,
    fszero: Option<Arc<dyn DomainAdapter>>,
    graphzero: Option<Arc<dyn DomainAdapter>>,
    tokenzero: Option<Arc<dyn DomainAdapter>>,
}

static SESSION_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn default_session_id(root: &Path) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SESSION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root_digest = zero_abi::sha256(root.to_string_lossy().as_bytes());
    format!(
        "zsx-kernel-{:x}-{}-{timestamp:x}-{sequence:x}",
        std::process::id(),
        hex_prefix(&root_digest)
    )
}

fn hex_prefix(digest: &[u8; 32]) -> String {
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl SupervisorBuilder {
    fn new(root: PathBuf) -> Self {
        Self {
            state_root: None,
            session_id: None,
            profile: SupervisorProfile::Embedded,
            child: None,
            fszero: None,
            graphzero: None,
            tokenzero: None,
            root,
        }
    }

    /// Place mutable session, engine, CAS, journal, and spill state below an
    /// explicit root while keeping repository operations authorized to
    /// `root`.
    pub fn with_state_root(mut self, state_root: impl Into<PathBuf>) -> Self {
        self.state_root = Some(state_root.into());
        self
    }

    /// Override the session identity surfaced in traces, ref ownership, and
    /// request session binding. Defaults to a process-unique kernel session.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Select the disposable-executor profile (default: embedded).
    pub fn with_profile(mut self, profile: SupervisorProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Override the one-shot child launch spec (default: current executable
    /// with the `kernel` subcommand).
    pub fn with_one_shot_child(mut self, child: OneShotChild) -> Self {
        self.child = Some(child);
        self
    }

    /// Register the FSZero domain adapter (embedded profile).
    pub fn fszero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.fszero = Some(adapter);
        self
    }

    /// Register the GraphZero domain adapter (embedded profile).
    pub fn graphzero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.graphzero = Some(adapter);
        self
    }

    /// Register the TokenZero domain adapter (embedded profile).
    pub fn tokenzero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.tokenzero = Some(adapter);
        self
    }

    /// Build the supervisor. The embedded profile requires all three domain
    /// adapters, each declaring the engine of its slot; the one-shot profile
    /// runs the plan in the child and needs no adapters here.
    pub fn build(self) -> Result<Supervisor, SupervisorError> {
        let root = self.root.canonicalize().map_err(|error| {
            SupervisorError::Internal(format!(
                "cannot resolve supervisor root {}: {error}",
                self.root.display()
            ))
        })?;
        let state_root = self.state_root.unwrap_or_else(|| root.clone());
        std::fs::create_dir_all(&state_root).map_err(|error| {
            SupervisorError::Internal(format!(
                "cannot create supervisor state root {}: {error}",
                state_root.display()
            ))
        })?;
        let state_root = state_root.canonicalize().map_err(|error| {
            SupervisorError::Internal(format!(
                "cannot resolve supervisor state root {}: {error}",
                state_root.display()
            ))
        })?;
        let session_id =
            self.session_id
                .unwrap_or_else(|| default_session_id(&root));
        let child = match self.child {
            Some(child) => child,
            None => OneShotChild::current_exe()?,
        };
        let mut adapters = BTreeMap::new();
        if self.profile == SupervisorProfile::Embedded {
            let slots = [
                (EngineIdentity::FsZero, self.fszero),
                (EngineIdentity::GraphZero, self.graphzero),
                (EngineIdentity::TokenZero, self.tokenzero),
            ];
            for (engine, adapter) in slots {
                let Some(adapter) = adapter else {
                    return Err(SupervisorError::Internal(format!(
                        "embedded supervisor requires the {} domain adapter",
                        engine.as_str()
                    )));
                };
                if adapter.engine() != engine {
                    return Err(SupervisorError::Internal(format!(
                        "registered adapter engine {} does not match {} slot",
                        adapter.engine().as_str(),
                        engine.as_str()
                    )));
                }
                adapter.binding().validate().map_err(|error| {
                    SupervisorError::Internal(format!(
                        "invalid {} adapter binding: {error}",
                        engine.as_str()
                    ))
                })?;
                adapters.insert(engine, adapter);
            }
        }
        Ok(Supervisor {
            profile: self.profile,
            root,
            state_root,
            session_id,
            adapters,
            child,
            request_sequence: AtomicU64::new(1),
            live_executors: AtomicU64::new(0),
            live_children: AtomicU64::new(0),
            child_spawns: AtomicU64::new(0),
        })
    }

    /// Build over the three real engine adapters (FSZero, GraphZero,
    /// TokenZero) constructed from this builder's root and session identity.
    /// The embedded profile refuses a degraded FSZero durable store; the
    /// one-shot profile builds no adapters (the child constructs its own).
    #[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
    pub fn build_canonical(self) -> Result<Supervisor, SupervisorError> {
        if self.profile == SupervisorProfile::OneShot {
            return self.build();
        }
        let root = self.root.clone();
        let state_root = self.state_root.clone().unwrap_or_else(|| root.clone());
        // One session identity for the adapters AND the supervisor: the
        // connector verifies adapter ownership against its own session id,
        // so a mismatched default would fail every dispatch.
        let session_id = self
            .session_id
            .clone()
            .unwrap_or_else(|| default_session_id(&root));
        let fszero = Arc::new(if state_root == root {
            crate::fszero::FsZeroAdapter::new(&root, session_id.as_str())
        } else {
            crate::fszero::FsZeroAdapter::new_with_state_root(
                &root,
                &state_root,
                session_id.as_str(),
            )
        });
        if fszero.degraded() {
            return Err(SupervisorError::Internal(
                "FSZero durable store unavailable; refusing silent in-memory fallback".into(),
            ));
        }
        let graphzero = Arc::new(if state_root == root {
            crate::graphzero::GraphZeroAdapter::new(&root, session_id.as_str())
        } else {
            crate::graphzero::GraphZeroAdapter::new_with_state_root(
                &root,
                &state_root,
                session_id.as_str(),
            )
        });
        let tokenzero = Arc::new(
            (if state_root == root {
                crate::tokenzero::TokenZeroAdapter::new(&root, session_id.as_str())
            } else {
                crate::tokenzero::TokenZeroAdapter::new_with_state_root(
                    &root,
                    &state_root,
                    session_id.as_str(),
                )
            })
            .map_err(|error| {
                SupervisorError::Internal(format!("cannot construct TokenZero adapter: {error}"))
            })?,
        );
        self.with_session_id(session_id)
            .fszero(fszero)
            .graphzero(graphzero)
            .tokenzero(tokenzero)
            .build()
    }
}

/// One terminal outcome of the one-shot child wait loop.
enum KernelChildOutcome {
    Response(ZerokernelExecuteResponse),
    Cancelled,
    Deadline,
    Malformed(String),
    Transport(String),
    OutputGone,
}

/// Bounded reader/transport failure.
enum KernelIoError {
    Bounds(usize, usize),
    Io(std::io::Error),
}

impl std::fmt::Display for KernelIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bounds(actual, maximum) => {
                write!(f, "child output exceeded bound: {actual} > {maximum} bytes")
            }
            Self::Io(error) => write!(f, "child output io error: {error}"),
        }
    }
}

/// Bounded stderr capture shared with the capture thread.
#[derive(Default)]
struct StderrState {
    bytes: Vec<u8>,
    total: u64,
    complete: bool,
    truncated: bool,
}

/// SIGTERM (bounded grace) then SIGKILL to the exact tree, then reap,
/// mirroring the raw-worker escalation path. Never a numeric pid: the owned,
/// unreaped child pins the group id until revoke.
fn kill_and_reap(child: &VerifiedChild, owner: &str, generation: u64) {
    let _ = child.signal_graceful_for(owner, generation, ONESHOT_KILL_GRACE);
    let _ = child.revoke();
}

/// The child exited on its own: sweep the still-owned tree, reap the root,
/// and return its status. `None` only when the platform teardown failed.
fn settle_exited_child(
    child: &VerifiedChild,
    owner: &str,
    generation: u64,
) -> Option<ExitStatus> {
    let grace = ONESHOT_KILL_GRACE.min(Duration::from_millis(100));
    match child.wait(owner, generation, Duration::from_millis(100), grace) {
        Ok(status) => Some(status),
        Err(_) => {
            kill_and_reap(child, owner, generation);
            child.terminal_status()
        }
    }
}

/// Wait briefly for the stderr capture to complete after the child settled,
/// then return the tail as lossy UTF-8.
fn stderr_tail(state: &Arc<(Mutex<StderrState>, Condvar)>, wait: Duration) -> String {
    let (lock, ready) = &**state;
    let mut guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let deadline = Instant::now().checked_add(wait);
    while !guard.complete {
        let Some(deadline) = deadline else { break };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match ready.wait_timeout(guard, remaining) {
            Ok((next, result)) => {
                guard = next;
                if result.timed_out() {
                    break;
                }
            }
            Err(poisoned) => {
                guard = poisoned.into_inner().0;
                break;
            }
        }
    }
    let tail_start = guard.bytes.len().saturating_sub(2048);
    let tail = String::from_utf8_lossy(&guard.bytes[tail_start..]).into_owned();
    let truncated = guard.truncated || tail_start > 0;
    if truncated {
        format!("[stderr truncated, {} bytes observed] {tail}", guard.total)
    } else {
        tail
    }
}

fn spawn_thread(
    name: &'static str,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.into()).spawn(body)
}

/// Build the bounded host limits for one call from the request's finite
/// budget. Every budget field is protocol-bounded before this runs.
fn host_limits_for_budget(budget: &FiniteBudget) -> Result<HostLimits, SupervisorError> {
    HostLimits::new(
        budget.memory_bytes as usize,
        SUPERVISOR_STACK_BYTES,
        Duration::from_millis(budget.wall_ms),
        SUPERVISOR_INSTRUCTION_BUDGET,
        SUPERVISOR_MICROTASK_CEILING,
        MAX_INFLIGHT_CONNECTOR_CALLS,
        SUPERVISOR_MAX_PLAN_BYTES,
        SUPERVISOR_MAX_JSON_BYTES,
    )
    .map_err(|error| SupervisorError::Internal(format!("invalid host limits: {error}")))
}

/// The visible result budget for one call. The protocol measures the
/// preview in characters; the host budget is bytes, and interpreting
/// characters as bytes is the conservative direction for UTF-8 output.
fn visible_result_budget(policy: &ReturnPolicy) -> usize {
    policy.max_preview_chars as usize
}

fn zeroed_ledger() -> KernelResourceLedger {
    KernelResourceLedger {
        wall_ms_used: 0,
        cpu_ms_used: 0,
        calls_made: 0,
        bytes_out: 0,
    }
}

fn nanos_to_ms_ceil(nanos: u128) -> u64 {
    let ms = nanos.div_ceil(1_000_000);
    ms.min(u128::from(u64::MAX)) as u64
}

fn unchanged_evidence(snapshot: &RootSnapshot) -> RootEvidence {
    RootEvidence {
        before: snapshot.clone(),
        after: snapshot.clone(),
        unchanged: true,
        successor_root: None,
    }
}

/// Build a `Failed` response carrying the terminal detail in the preflight
/// report (the zerokernel schema has no separate error field). The root
/// evidence proves the injected roots unchanged.
fn failed_response(
    mut preflight: PreflightReport,
    ledger: KernelResourceLedger,
    evidence: RootEvidence,
    detail: &str,
) -> ZerokernelExecuteResponse {
    preflight.ok = false;
    preflight.errors.push(detail.to_owned());
    ZerokernelExecuteResponse::failed(ExactHandles::default(), preflight, ledger, evidence)
        .expect("a failed response with unchanged evidence is always valid")
}

fn is_directory(path: &Path) -> bool {
    path.is_dir()
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

/// One-shot kernel child entry point: read one canonical
/// [`ZerokernelExecuteRequest`] from stdin, run it through the embedded
/// profile, write one canonical [`ZerokernelExecuteResponse`] line to
/// stdout, and exit. Returns the process exit code; any failure before a
/// response can be produced exits nonzero with no stdout output (the parent
/// treats that as the worker-crash terminal path).
#[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
pub fn run_kernel_child() -> i32 {
    match run_kernel_child_inner() {
        Ok(()) => 0,
        Err(()) => 1,
    }
}

#[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
fn run_kernel_child_inner() -> Result<(), ()> {
    let store_root = std::env::var(STORE_ROOT_ENV).map_err(|_| ())?;
    let session_id = std::env::var(SESSION_ID_ENV).map_err(|_| ())?;
    let input = read_stdin_bounded(KERNEL_CHILD_MAX_REQUEST_BYTES).map_err(|_| ())?;
    let request = ZerokernelExecuteRequest::from_canonical_bytes(&input).map_err(|_| ())?;
    let supervisor = Supervisor::builder(request.roots.project_root.clone())
        .with_state_root(store_root)
        .with_session_id(session_id)
        .with_profile(SupervisorProfile::Embedded)
        .build_canonical()
        .map_err(|_| ())?;
    let response = supervisor.execute(request).map_err(|_| ())?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&response.canonical_bytes())
        .map_err(|_| ())?;
    stdout.write_all(b"\n").map_err(|_| ())?;
    stdout.flush().map_err(|_| ())
}

/// Read all of stdin to EOF, failing closed past `limit`.
fn read_stdin_bounded(limit: usize) -> std::io::Result<Vec<u8>> {
    let mut locked = std::io::stdin().lock();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = locked.read(&mut chunk)?;
        if n == 0 {
            return Ok(buffer);
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > limit {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("kernel request exceeds {limit} bytes"),
            ));
        }
    }
}
