//! Single-process aggregate session authority, canonical in zsx-core.
//!
//! The session owns generation, bounded admission, approval replay protection,
//! replacement, and shutdown. Its executor calls registered [`DomainAdapter`]
//! implementations in-process. Cancellation is per request: each execution
//! runs under its own token, `cancel_request` stops one request while the
//! session stays accepting, and whole-session termination remains available
//! through [`ZsxSessionCancellation::cancel`]. Durable mutation attempt
//! journals created at connector dispatch can be reconciled through
//! [`ZsxSession::reconcile_request`] or [`ZsxSession::reconcile_all_attempts`]
//! without ever calling an adapter. Every harness embeds this native session.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use zero_abi::EffectClass;
use zero_codemode::CancellationSignal;
use zero_codemode::{ExecutionMetrics, Host, HostError};

use crate::adapter::{AdapterBinding, DomainAdapter};
use crate::connector::{
    AggregateExecutionContext, MAX_SESSION_APPROVAL_GRANTS, MAX_SESSION_APPROVAL_LIFETIME_MS,
    MAX_SESSION_CONSUMED_APPROVALS, SessionApprovalGrantV1, ZsxAttemptJournalStatus, ZsxConnector,
    attempts_root_for, now_ms, reconcile_all_attempts, reconcile_request_attempts, registration,
};
use crate::verdict::{VerdictLoopEnvelope, VerdictLoopResult};

#[derive(Clone, Copy, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReplacementReason {
    SessionStart,
    BeforeSwitch,
    BeforeFork,
    WorkerRevisionChange,
    Manual,
}

impl SessionReplacementReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::BeforeSwitch => "before_switch",
            Self::BeforeFork => "before_fork",
            Self::WorkerRevisionChange => "worker_revision_change",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionReplacementReceipt {
    pub previous_generation: u64,
    pub generation: u64,
    pub reason: SessionReplacementReason,
}

pub const SESSION_EXECUTION_QUEUE_CAPACITY: usize = 8;
/// Canonical model-visible JSON byte budget. This avoids a durable spill for
/// a tiny read plus its execution receipt while retaining a hard inline cap;
/// tokenizer-specific certification remains a separate TokenZero boundary.
const SESSION_VISIBLE_RESULT_BYTES: usize = 4 * 1024;
pub const SESSION_REPLACEMENT_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
pub const SESSION_EXECUTOR_START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZsxSessionFailureCode {
    InvalidGeneration,
    StaleGeneration,
    DuplicateRequestId,
    InvalidApproval,
    ApprovalReplay,
    Backpressure,
    ReplacementInProgress,
    Terminating,
    GenerationExhausted,
    BackendUnavailable,
    MethodNotFound,
    SurfaceNotFound,
    BackendExecution,
    VerdictRejected,
    /// The request was cancelled through its per-request token before it
    /// settled. The session itself remains accepting.
    Cancelled,
    Internal,
}

impl ZsxSessionFailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidGeneration => "invalid_generation",
            Self::StaleGeneration => "stale_generation",
            Self::DuplicateRequestId => "duplicate_request_id",
            Self::InvalidApproval => "invalid_approval",
            Self::ApprovalReplay => "approval_replay",
            Self::Backpressure => "backpressure",
            Self::ReplacementInProgress => "replacement_in_progress",
            Self::Terminating => "session_terminating",
            Self::GenerationExhausted => "generation_exhausted",
            Self::BackendUnavailable => "backend_unavailable",
            Self::MethodNotFound => "method_not_found",
            Self::SurfaceNotFound => "surface_not_found",
            Self::BackendExecution => "backend_execution",
            Self::VerdictRejected => "verdict_rejected",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

fn backend_failure_code(error: &HostError) -> ZsxSessionFailureCode {
    match error {
        HostError::VerdictRejected(_) => ZsxSessionFailureCode::VerdictRejected,
        HostError::MethodNotFound(_) => ZsxSessionFailureCode::MethodNotFound,
        HostError::SurfaceNotFound(_) => ZsxSessionFailureCode::SurfaceNotFound,
        _ => ZsxSessionFailureCode::BackendExecution,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZsxSessionError {
    pub code: ZsxSessionFailureCode,
    pub generation: u64,
    pub request_id: Option<u64>,
    pub detail: String,
    pub retry_after_ms: Option<u64>,
}

impl ZsxSessionError {
    fn new(
        code: ZsxSessionFailureCode,
        generation: u64,
        request_id: Option<u64>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            generation,
            request_id,
            detail: detail.into(),
            retry_after_ms: None,
        }
    }

    fn backpressure(generation: u64, request_id: u64) -> Self {
        Self {
            code: ZsxSessionFailureCode::Backpressure,
            generation,
            request_id: Some(request_id),
            detail: format!(
                "session execution queue is full (capacity {})",
                SESSION_EXECUTION_QUEUE_CAPACITY
            ),
            retry_after_ms: Some(1),
        }
    }
}

impl std::fmt::Display for ZsxSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for ZsxSessionError {}

#[derive(Debug)]
pub struct ZsxExecutionResult {
    pub generation: u64,
    pub request_id: u64,
    pub value: Value,
    pub metrics: ZsxExecutionMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ZsxExecutionMetrics {
    pub host: ExecutionMetrics,
    pub engine_wall_ns: [u64; 3],
    pub engine_dispatches: [u64; 3],
    pub engine_wall_ns_sum: u64,
    /// Host wall not attributed to measured adapter calls. Parallel adapter
    /// intervals can overlap, so this is a conservative lower bound.
    pub runtime_overhead_lower_bound_ns: u64,
}

#[derive(Debug)]
struct ZsxSessionState {
    generation: u64,
    accepting: bool,
    replacing: bool,
    terminating: bool,
    shutdown_sent: bool,
    worker_stopped: bool,
    seen_request_ids: BTreeSet<u64>,
    active_request_ids: BTreeSet<u64>,
    root: String,
    state_root: String,
    consumed_approval_ids: BTreeSet<String>,
}

enum ZsxCommand {
    Execute {
        generation: u64,
        request_id: u64,
        source: String,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
        verdict_envelope: Option<VerdictLoopEnvelope>,
        reply:
            SyncSender<Result<(Value, ZsxExecutionMetrics, Option<VerdictLoopResult>), HostError>>,
    },
    Replace {
        generation: u64,
        reply: SyncSender<Result<(), String>>,
    },
    Shutdown {
        reply: SyncSender<Result<(), String>>,
    },
}

/// One-generation executor over the in-process connector.
pub(crate) struct ZsxExecutor {
    host: Host,
    connector: std::rc::Rc<ZsxConnector>,
    /// Shared with the session facade: the token of the request currently
    /// executing, plus cancellation requests that arrived before their
    /// request started.
    cancellation_slot: Arc<Mutex<ActiveCancellationSlot>>,
}

/// One per-request cancellation signal. `host` and `worker` share a single
/// atomic flag: the host runtime cancels through the `Arc<AtomicBool>` and
/// every connector dispatch and adapter call observes the `CancellationSignal`.
#[derive(Clone)]
struct ZsxSessionCancellationSignal {
    host: Arc<AtomicBool>,
    worker: CancellationSignal,
}

impl ZsxSessionCancellationSignal {
    fn cancel(&self) {
        self.host.store(true, Ordering::Release);
        self.worker.cancel();
    }

    fn fresh() -> Self {
        let worker = CancellationSignal::new();
        Self {
            host: worker.as_atomic(),
            worker,
        }
    }
}

/// Cancellation state shared between the session facade and its executor
/// worker thread. Requests execute one at a time, so at most one token is
/// active; `cancelled_requests` records `cancel_request` calls that landed
/// before their request started so those requests fail without dispatching.
#[derive(Default)]
struct ActiveCancellationSlot {
    active: Option<ActiveRequestCancellation>,
    cancelled_requests: BTreeSet<(u64, u64)>,
}

#[derive(Clone)]
struct ActiveRequestCancellation {
    generation: u64,
    request_id: u64,
    signal: ZsxSessionCancellationSignal,
}

impl ZsxExecutor {
    fn new(
        root: PathBuf,
        state_root: PathBuf,
        session_id: String,
        adapters: std::collections::BTreeMap<
            zero_abi::raw_worker::EngineIdentity,
            Arc<dyn DomainAdapter>,
        >,
        cancellation_slot: Arc<Mutex<ActiveCancellationSlot>>,
    ) -> Result<Self, HostError> {
        let root = root.canonicalize().map_err(|error| {
            HostError::Connector(format!("cannot resolve authorized session root: {error}"))
        })?;
        std::fs::create_dir_all(&state_root).map_err(|error| {
            HostError::Connector(format!("cannot prepare session result store: {error}"))
        })?;
        let state_root = state_root.canonicalize().map_err(|error| {
            HostError::Connector(format!("cannot resolve session state root: {error}"))
        })?;
        let connector = std::rc::Rc::new(if state_root == root {
            ZsxConnector::new(root, session_id, adapters)?
        } else {
            ZsxConnector::new_with_state_root(root, state_root.clone(), session_id, adapters)?
        });
        let limits = crate::connector::host_limits()?;
        let host = Host::new(limits, registration())?
            .with_visible_result_budget(SESSION_VISIBLE_RESULT_BYTES)?
            .with_result_spill(state_root);
        Ok(Self {
            host,
            connector,
            cancellation_slot,
        })
    }

    fn execute_with_context(
        &self,
        generation: u64,
        request_id: u64,
        source: &str,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
        verdict_envelope: Option<VerdictLoopEnvelope>,
    ) -> Result<(Value, ZsxExecutionMetrics, Option<VerdictLoopResult>), HostError> {
        // A cancel_request that arrived before this request started must
        // prevent any dispatch. The pending-cancellation check and the
        // installation of the fresh request token happen under one lock so a
        // concurrent cancel_request cannot slip between them.
        let signal = ZsxSessionCancellationSignal::fresh();
        {
            let mut slot = self
                .cancellation_slot
                .lock()
                .map_err(|_| HostError::Connector("cancellation slot poisoned".into()))?;
            if slot.cancelled_requests.contains(&(generation, request_id)) {
                return Err(HostError::Connector(
                    "request cancelled before start".into(),
                ));
            }
            slot.active = Some(ActiveRequestCancellation {
                generation,
                request_id,
                signal: signal.clone(),
            });
        }
        let clear_active = || {
            if let Ok(mut slot) = self.cancellation_slot.lock() {
                slot.active = None;
            }
        };
        if let Err(error) = self.connector.install_approvals(approval_grants) {
            clear_active();
            return Err(error);
        }
        if let Err(error) = self
            .connector
            .set_execution_context(AggregateExecutionContext {
                generation,
                request_id,
            })
        {
            self.connector.clear_approvals();
            clear_active();
            return Err(error);
        }
        self.connector
            .set_request_cancellation(signal.worker.clone());
        if let Err(error) = self.connector.install_verdict_meter(verdict_envelope) {
            self.connector.clear_request_cancellation();
            self.connector.clear_execution_context();
            self.connector.clear_approvals();
            clear_active();
            return Err(error);
        }
        self.connector.reset_dispatch_metrics();
        let outcome = self.host.execute_measured_with_cancel_timeout_context(
            source,
            self.connector.clone(),
            signal.host.clone(),
            timeout,
            generation,
            request_id,
        );
        let mut result = outcome.result;
        let host_metrics = outcome.metrics;
        if result.is_err() {
            signal.cancel();
            if let Err(tail_error) = self
                .connector
                .wait_for_dispatch_idle(Duration::from_secs(5))
            {
                result = Err(tail_error);
            }
        }
        let dispatch = self.connector.dispatch_metrics();
        let engine_wall_ns_sum = dispatch
            .wall_ns
            .iter()
            .copied()
            .fold(0_u64, u64::saturating_add);
        let metrics = ZsxExecutionMetrics {
            runtime_overhead_lower_bound_ns: host_metrics
                .wall_time_ns
                .saturating_sub(engine_wall_ns_sum),
            host: host_metrics,
            engine_wall_ns: dispatch.wall_ns,
            engine_dispatches: dispatch.dispatches,
            engine_wall_ns_sum,
        };
        self.connector.clear_request_cancellation();
        self.connector.clear_execution_context();
        self.connector.clear_approvals();
        clear_active();
        let verdict = match result.as_ref() {
            Ok(value) => match self.connector.finish_verdict_meter(value) {
                Ok(verdict) => verdict,
                Err(error) => return Err(error),
            },
            Err(_) => {
                self.connector.clear_verdict_meter();
                None
            }
        };
        result.map(|value| (value, metrics, verdict))
    }

    fn publish_reachability(&self) -> Result<(), HostError> {
        self.connector.publish_reachability()
    }
}

static SESSION_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn default_session_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SESSION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("zsx-{:x}-{timestamp:x}-{sequence:x}", std::process::id())
}

/// Builder for a single-process ZSX session with exactly the three domain
/// adapters: FSZero, GraphZero, and TokenZero.
///
/// ```ignore
/// use std::sync::Arc;
/// use zsx_core::{DomainAdapter, ZsxSession};
///
/// let session = ZsxSession::builder("/repo")
///     .fszero(Arc::new(fszero_adapter))
///     .graphzero(Arc::new(graphzero_adapter))
///     .tokenzero(Arc::new(tokenzero_adapter))
///     .build()?;
/// ```
pub struct ZsxBuilder {
    root: PathBuf,
    state_root: PathBuf,
    session_id: String,
    fszero: Option<Arc<dyn DomainAdapter>>,
    graphzero: Option<Arc<dyn DomainAdapter>>,
    tokenzero: Option<Arc<dyn DomainAdapter>>,
}

impl ZsxBuilder {
    fn new(root: PathBuf) -> Self {
        Self {
            state_root: root.clone(),
            root,
            session_id: default_session_id(),
            fszero: None,
            graphzero: None,
            tokenzero: None,
        }
    }

    /// Override the session identity surfaced in traces and ref ownership.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// Place mutable session, engine, CAS, journal, and spill state below an
    /// explicit root while keeping repository operations authorized to `root`.
    pub fn with_state_root(mut self, state_root: impl Into<PathBuf>) -> Self {
        self.state_root = state_root.into();
        self
    }

    /// Register the FSZero domain adapter.
    pub fn fszero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.fszero = Some(adapter);
        self
    }

    /// Register the GraphZero domain adapter.
    pub fn graphzero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.graphzero = Some(adapter);
        self
    }

    /// Register the TokenZero domain adapter.
    pub fn tokenzero(mut self, adapter: Arc<dyn DomainAdapter>) -> Self {
        self.tokenzero = Some(adapter);
        self
    }

    /// Build the session; requires all three adapters registered, each
    /// declaring the engine of its slot.
    pub fn build(self) -> Result<ZsxSession, ZsxSessionError> {
        let mut adapters = std::collections::BTreeMap::new();
        let slots = [
            (zero_abi::raw_worker::EngineIdentity::FsZero, self.fszero),
            (
                zero_abi::raw_worker::EngineIdentity::GraphZero,
                self.graphzero,
            ),
            (
                zero_abi::raw_worker::EngineIdentity::TokenZero,
                self.tokenzero,
            ),
        ];
        for (engine, adapter) in slots {
            let Some(adapter) = adapter else {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    0,
                    None,
                    format!("missing {} domain adapter", engine.as_str()),
                ));
            };
            if adapter.engine() != engine {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    0,
                    None,
                    format!(
                        "registered adapter engine {} does not match {} slot",
                        adapter.engine().as_str(),
                        engine.as_str()
                    ),
                ));
            }
            let binding: AdapterBinding = adapter.binding();
            binding.validate().map_err(|error| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    0,
                    None,
                    error.to_string(),
                )
            })?;
            adapters.insert(engine, adapter);
        }
        ZsxSession::new_authorized_with_adapters(
            1,
            self.root,
            self.state_root,
            self.session_id,
            adapters,
        )
    }

    /// Build the canonical session over exactly the three real adapters
    /// (FSZero, GraphZero, TokenZero) constructed from this builder's root
    /// and session id, with no custom or fixture adapters. `FsZeroAdapter`
    /// and `GraphZeroAdapter` constructors are infallible; a `TokenZeroAdapter`
    /// binding failure surfaces as
    /// [`ZsxSessionFailureCode::BackendUnavailable`].
    #[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
    pub fn build_canonical(self) -> Result<ZsxSession, ZsxSessionError> {
        let root = self.root.clone();
        let state_root = self.state_root.clone();
        let session_id = self.session_id.clone();
        let fszero = Arc::new(if state_root == root {
            crate::fszero::FsZeroAdapter::new(&root, session_id.as_str())
        } else {
            crate::fszero::FsZeroAdapter::new_with_state_root(
                &root,
                &state_root,
                session_id.as_str(),
            )
        });
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
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    0,
                    None,
                    format!("cannot construct TokenZero adapter: {error}"),
                )
            })?,
        );
        self.fszero(fszero)
            .graphzero(graphzero)
            .tokenzero(tokenzero)
            .build()
    }
}

/// Single-process aggregate session authority.
pub struct ZsxSession {
    state: Arc<Mutex<ZsxSessionState>>,
    commands: SyncSender<ZsxCommand>,
    cancellation: Arc<Mutex<ActiveCancellationSlot>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct ZsxSessionCancellation {
    state: Arc<Mutex<ZsxSessionState>>,
    cancellation: Arc<Mutex<ActiveCancellationSlot>>,
}

impl ZsxSessionCancellation {
    /// Terminate the whole session: stop accepting, advance the generation,
    /// and cancel whatever request is currently executing.
    pub fn cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.accepting = false;
            state.terminating = true;
            if let Some(next) = state.generation.checked_add(1) {
                state.generation = next;
                state.seen_request_ids.clear();
            }
        }
        if let Ok(mut slot) = self.cancellation.lock() {
            slot.cancelled_requests.clear();
        }
        cancel_backend(&self.cancellation);
    }

    /// Cancel one request without terminating the session.
    ///
    /// An in-flight request's token is cancelled immediately, so no further
    /// dispatch of that request is admitted and its adapters observe the
    /// cancellation. A request that has been admitted but has not started is
    /// marked cancelled and fails without dispatching when it starts. A later
    /// request in the same generation runs under a fresh token and is
    /// unaffected.
    ///
    /// Returns `true` when an in-flight request was actively cancelled, and
    /// `false` when the cancellation was recorded for a request that had not
    /// started yet (or the session state was unavailable).
    pub fn cancel_request(&self, generation: u64, request_id: u64) -> bool {
        let mut actively_cancelled = false;
        if let Ok(mut slot) = self.cancellation.lock() {
            if let Some(active) = slot.active.as_ref()
                && active.generation == generation
                && active.request_id == request_id
            {
                active.signal.cancel();
                actively_cancelled = true;
            }
            slot.cancelled_requests.insert((generation, request_id));
        }
        actively_cancelled
    }
}

impl ZsxSession {
    /// Start building a session rooted at `root` (canonicalized on build).
    pub fn builder(root: impl Into<PathBuf>) -> ZsxBuilder {
        ZsxBuilder::new(root.into())
    }

    pub(crate) fn new_authorized_with_adapters(
        initial_generation: u64,
        root: PathBuf,
        state_root: PathBuf,
        session_id: String,
        adapters: std::collections::BTreeMap<
            zero_abi::raw_worker::EngineIdentity,
            Arc<dyn DomainAdapter>,
        >,
    ) -> Result<Self, ZsxSessionError> {
        if initial_generation == 0 {
            return Err(ZsxSessionError::new(
                ZsxSessionFailureCode::InvalidGeneration,
                initial_generation,
                None,
                "initial generation must be nonzero",
            ));
        }
        if session_id.is_empty() {
            return Err(ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                "missing explicit ZeroStack session identity",
            ));
        }
        let root = root.canonicalize().map_err(|error| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                format!("cannot resolve authorized session root: {error}"),
            )
        })?;
        std::fs::create_dir_all(&state_root).map_err(|error| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                format!("cannot create session state root: {error}"),
            )
        })?;
        let state_root = state_root.canonicalize().map_err(|error| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                format!("cannot resolve session state root: {error}"),
            )
        })?;
        let root_text = root.to_string_lossy().into_owned();
        let state_root_text = state_root.to_string_lossy().into_owned();
        let (commands, receiver) = mpsc::sync_channel(SESSION_EXECUTION_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cancellation = Arc::new(Mutex::new(ActiveCancellationSlot::default()));
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = thread::Builder::new()
            .name("zsx-session-executor".into())
            .spawn(move || {
                session_worker(
                    initial_generation,
                    root,
                    state_root,
                    session_id,
                    adapters,
                    receiver,
                    worker_cancellation,
                    ready_tx,
                )
            })
            .map_err(|error| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    format!("failed to spawn session executor: {error}"),
                )
            })?;
        match ready_rx.recv_timeout(SESSION_EXECUTOR_START_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(detail)) => {
                let _ = worker.join();
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    detail,
                ));
            }
            Err(error) => {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    format!(
                        "session executor did not start within {}ms: {error}",
                        SESSION_EXECUTOR_START_TIMEOUT.as_millis()
                    ),
                ));
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(ZsxSessionState {
                generation: initial_generation,
                accepting: true,
                replacing: false,
                terminating: false,
                shutdown_sent: false,
                worker_stopped: false,
                seen_request_ids: BTreeSet::new(),
                active_request_ids: BTreeSet::new(),
                root: root_text,
                state_root: state_root_text,
                consumed_approval_ids: BTreeSet::new(),
            })),
            commands,
            cancellation,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn generation(&self) -> Result<u64, ZsxSessionError> {
        self.state
            .lock()
            .map(|state| state.generation)
            .map_err(|_| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::Internal,
                    0,
                    None,
                    "session lifecycle state is poisoned",
                )
            })
    }

    pub fn cancellation(&self) -> ZsxSessionCancellation {
        ZsxSessionCancellation {
            state: Arc::clone(&self.state),
            cancellation: Arc::clone(&self.cancellation),
        }
    }

    pub fn execute(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
    ) -> Result<ZsxExecutionResult, ZsxSessionError> {
        self.execute_internal(generation, request_id, source, timeout, Vec::new(), None)
            .map(|(result, _)| result)
    }

    pub fn execute_with_approvals(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
    ) -> Result<ZsxExecutionResult, ZsxSessionError> {
        self.execute_internal(
            generation,
            request_id,
            source,
            timeout,
            approval_grants,
            None,
        )
        .map(|(result, _)| result)
    }

    /// Execute one bounded server-side verdict loop. The plan may compose and
    /// poll ordinary typed capabilities, but its only public value is the
    /// exact string `pass` or `fail`; accounting is returned separately.
    pub fn execute_verdict_loop(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
        envelope: VerdictLoopEnvelope,
    ) -> Result<VerdictLoopResult, ZsxSessionError> {
        let (_, verdict) = self.execute_internal(
            generation,
            request_id,
            source,
            timeout,
            Vec::new(),
            Some(envelope),
        )?;
        verdict.ok_or_else(|| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendExecution,
                generation,
                Some(request_id),
                "verdict loop completed without a receipt",
            )
        })
    }

    fn execute_internal(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
        verdict_envelope: Option<VerdictLoopEnvelope>,
    ) -> Result<(ZsxExecutionResult, Option<VerdictLoopResult>), ZsxSessionError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let approval_ids = {
            let mut state = self.lock_state(Some(request_id))?;
            if generation != state.generation {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::StaleGeneration,
                    state.generation,
                    Some(request_id),
                    format!(
                        "request generation {generation} does not match active generation {}",
                        state.generation
                    ),
                ));
            }
            if state.terminating || !state.accepting {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::Terminating,
                    state.generation,
                    Some(request_id),
                    "session is not accepting execution",
                ));
            }
            if state.seen_request_ids.contains(&request_id) {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::DuplicateRequestId,
                    state.generation,
                    Some(request_id),
                    "request id was already admitted in this generation",
                ));
            }
            let approval_ids =
                validate_session_approvals(&state, generation, request_id, &approval_grants)?;
            state.seen_request_ids.insert(request_id);
            state.active_request_ids.insert(request_id);
            state
                .consumed_approval_ids
                .extend(approval_ids.iter().cloned());
            approval_ids
        };
        match self.commands.try_send(ZsxCommand::Execute {
            generation,
            request_id,
            source: source.into(),
            timeout,
            approval_grants,
            verdict_envelope,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.release_unadmitted(generation, request_id, &approval_ids);
                return Err(ZsxSessionError::backpressure(generation, request_id));
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_unadmitted(generation, request_id, &approval_ids);
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    generation,
                    Some(request_id),
                    "session executor is unavailable",
                ));
            }
        }
        let backend_result = reply_rx.recv().map_err(|error| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                generation,
                Some(request_id),
                format!("session executor dropped the result: {error}"),
            )
        });
        let (current, terminating) = {
            let mut state = self.lock_state(Some(request_id))?;
            state.active_request_ids.remove(&request_id);
            (state.generation, state.terminating)
        };
        if current != generation || terminating {
            return Err(ZsxSessionError::new(
                ZsxSessionFailureCode::StaleGeneration,
                current,
                Some(request_id),
                "execution settled after its generation was replaced",
            ));
        }
        let request_cancelled = {
            let mut slot = self.cancellation.lock().map_err(|_| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::Internal,
                    generation,
                    Some(request_id),
                    "cancellation state is poisoned",
                )
            })?;
            slot.cancelled_requests.remove(&(generation, request_id))
        };
        let (value, metrics, verdict) = match backend_result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                let code = if request_cancelled {
                    ZsxSessionFailureCode::Cancelled
                } else {
                    backend_failure_code(&error)
                };
                Err(ZsxSessionError::new(
                    code,
                    generation,
                    Some(request_id),
                    error.to_string(),
                ))
            }
            Err(error) => Err(ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                generation,
                Some(request_id),
                format!("session executor dropped the result: {error}"),
            )),
        }?;
        Ok((
            ZsxExecutionResult {
                generation,
                request_id,
                value,
                metrics,
            },
            verdict,
        ))
    }

    /// Reconcile the durable mutation attempt journals of one request.
    ///
    /// This is the manual recovery/read API for native addon resume: it maps
    /// every journal of the request through the zero-store recovery law
    /// (`recover_attempt_v1`) and returns the terminal status of each. A
    /// Prepared journal is classified `SafeToRetry` (dispatch never crossed),
    /// a DispatchCrossed journal without authoritative evidence is classified
    /// `Indeterminate`, and terminal journals are returned unchanged. This
    /// never calls an adapter and never redispatchable: recovery cannot write
    /// a DispatchCrossed entry, so no recovered attempt can be replayed.
    pub fn reconcile_request(
        &self,
        generation: u64,
        request_id: u64,
    ) -> Result<Vec<ZsxAttemptJournalStatus>, ZsxSessionError> {
        let root = {
            let state = self.lock_state(Some(request_id))?;
            if generation != state.generation {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::StaleGeneration,
                    state.generation,
                    Some(request_id),
                    format!(
                        "request generation {generation} does not match active generation {}",
                        state.generation
                    ),
                ));
            }
            state.state_root.clone()
        };
        let attempts_root = attempts_root_for(Path::new(&root));
        reconcile_request_attempts(&attempts_root, generation, request_id).map_err(|detail| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::Internal,
                generation,
                Some(request_id),
                detail,
            )
        })
    }

    /// Reconcile every durable mutation attempt visible under this session's
    /// store root. This is the native harness manual-resume API after a process
    /// interruption. It never calls an adapter and never redispatches effects.
    pub fn reconcile_all_attempts(&self) -> Result<Vec<ZsxAttemptJournalStatus>, ZsxSessionError> {
        let (root, generation) = {
            let state = self.lock_state(None)?;
            (state.state_root.clone(), state.generation)
        };
        let attempts_root = attempts_root_for(Path::new(&root));
        reconcile_all_attempts(&attempts_root).map_err(|detail| {
            ZsxSessionError::new(ZsxSessionFailureCode::Internal, generation, None, detail)
        })
    }

    pub fn replace(
        &self,
        expected_generation: u64,
        reason: SessionReplacementReason,
    ) -> Result<SessionReplacementReceipt, ZsxSessionError> {
        let next_generation = {
            let mut state = self.lock_state(None)?;
            if expected_generation != state.generation {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::StaleGeneration,
                    state.generation,
                    None,
                    format!(
                        "replacement generation {expected_generation} does not match active generation {}",
                        state.generation
                    ),
                ));
            }
            if state.terminating {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::Terminating,
                    state.generation,
                    None,
                    "session is terminating",
                ));
            }
            if state.replacing {
                return Err(ZsxSessionError::new(
                    ZsxSessionFailureCode::ReplacementInProgress,
                    state.generation,
                    None,
                    "another replacement is already in progress",
                ));
            }
            let next = state.generation.checked_add(1).ok_or_else(|| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::GenerationExhausted,
                    state.generation,
                    None,
                    "session generation cannot advance",
                )
            })?;
            state.accepting = false;
            state.replacing = true;
            state.generation = next;
            state.seen_request_ids.clear();
            next
        };
        cancel_backend(&self.cancellation);
        let control_started = Instant::now();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        if let Err(detail) = send_control_with_deadline(
            &self.commands,
            ZsxCommand::Replace {
                generation: next_generation,
                reply: reply_tx,
            },
            SESSION_REPLACEMENT_SETTLE_TIMEOUT,
        ) {
            self.finish_failed_replacement(next_generation);
            return Err(ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                next_generation,
                None,
                detail,
            ));
        }
        let settle_remaining =
            SESSION_REPLACEMENT_SETTLE_TIMEOUT.saturating_sub(control_started.elapsed());
        let replacement = reply_rx.recv_timeout(settle_remaining).map_err(|error| {
            self.finish_failed_replacement(next_generation);
            ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                next_generation,
                None,
                format!(
                    "session replacement did not settle within {}ms: {error}",
                    SESSION_REPLACEMENT_SETTLE_TIMEOUT.as_millis()
                ),
            )
        })?;
        let mut state = self.lock_state(None)?;
        state.replacing = false;
        match replacement {
            Ok(()) if state.generation == next_generation && !state.terminating => {
                state.accepting = true;
                Ok(SessionReplacementReceipt {
                    previous_generation: expected_generation,
                    generation: next_generation,
                    reason,
                })
            }
            Ok(()) => Err(ZsxSessionError::new(
                ZsxSessionFailureCode::StaleGeneration,
                state.generation,
                None,
                "replacement completed after lifecycle state advanced",
            )),
            Err(detail) => Err(ZsxSessionError::new(
                ZsxSessionFailureCode::BackendUnavailable,
                state.generation,
                None,
                detail,
            )),
        }
    }

    pub fn shutdown(&self) -> Result<u64, ZsxSessionError> {
        let (generation, should_send) = {
            let mut state = self.lock_state(None)?;
            if state.worker_stopped || state.shutdown_sent {
                return Ok(state.generation);
            }
            state.accepting = false;
            state.terminating = true;
            state.replacing = false;
            state.shutdown_sent = true;
            (state.generation, true)
        };
        cancel_backend(&self.cancellation);
        if should_send {
            let control_started = Instant::now();
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            send_control_with_deadline(
                &self.commands,
                ZsxCommand::Shutdown { reply: reply_tx },
                SESSION_REPLACEMENT_SETTLE_TIMEOUT,
            )
            .map_err(|detail| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    detail,
                )
            })?;
            let settle_remaining =
                SESSION_REPLACEMENT_SETTLE_TIMEOUT.saturating_sub(control_started.elapsed());
            let closure = reply_rx.recv_timeout(settle_remaining).map_err(|error| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    format!(
                        "session shutdown did not settle within {}ms: {error}",
                        SESSION_REPLACEMENT_SETTLE_TIMEOUT.as_millis()
                    ),
                )
            })?;
            closure.map_err(|detail| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    detail,
                )
            })?;
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(handle) = worker.take()
        {
            handle.join().map_err(|_| {
                ZsxSessionError::new(
                    ZsxSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    "session executor panicked during shutdown",
                )
            })?;
        }
        if let Ok(mut state) = self.state.lock() {
            state.worker_stopped = true;
        }
        Ok(generation)
    }

    fn lock_state(
        &self,
        request_id: Option<u64>,
    ) -> Result<std::sync::MutexGuard<'_, ZsxSessionState>, ZsxSessionError> {
        self.state.lock().map_err(|_| {
            ZsxSessionError::new(
                ZsxSessionFailureCode::Internal,
                0,
                request_id,
                "session lifecycle state is poisoned",
            )
        })
    }

    fn release_unadmitted(&self, generation: u64, request_id: u64, approval_ids: &[String]) {
        if let Ok(mut state) = self.state.lock() {
            state.active_request_ids.remove(&request_id);
            if state.generation == generation {
                state.seen_request_ids.remove(&request_id);
            }
            for approval_id in approval_ids {
                state.consumed_approval_ids.remove(approval_id);
            }
        }
        if let Ok(mut slot) = self.cancellation.lock() {
            slot.cancelled_requests.remove(&(generation, request_id));
        }
    }

    fn finish_failed_replacement(&self, generation: u64) {
        if let Ok(mut state) = self.state.lock()
            && state.generation == generation
        {
            state.replacing = false;
            state.accepting = false;
        }
    }
}

impl Drop for ZsxSession {
    fn drop(&mut self) {
        self.cancellation().cancel();
        if let Ok(state) = self.state.lock()
            && (state.shutdown_sent || state.worker_stopped)
        {
            return;
        }
        let (reply, _) = mpsc::sync_channel(1);
        let _ = self.commands.try_send(ZsxCommand::Shutdown { reply });
    }
}

fn send_control_with_deadline(
    commands: &SyncSender<ZsxCommand>,
    mut command: ZsxCommand,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match commands.try_send(command) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err("session executor is unavailable".into());
            }
            Err(TrySendError::Full(returned)) => {
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "session control queue did not admit within {}ms",
                        timeout.as_millis()
                    ));
                }
                command = returned;
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn cancel_backend(cancellation: &Arc<Mutex<ActiveCancellationSlot>>) {
    if let Ok(slot) = cancellation.lock()
        && let Some(active) = slot.active.as_ref()
    {
        active.signal.cancel();
    }
}

fn session_worker(
    initial_generation: u64,
    root: PathBuf,
    state_root: PathBuf,
    session_id: String,
    adapters: std::collections::BTreeMap<
        zero_abi::raw_worker::EngineIdentity,
        Arc<dyn DomainAdapter>,
    >,
    commands: Receiver<ZsxCommand>,
    cancellation: Arc<Mutex<ActiveCancellationSlot>>,
    ready: SyncSender<Result<(), String>>,
) {
    let mut executor = match start_session_executor(
        initial_generation,
        &root,
        &state_root,
        &session_id,
        &adapters,
        Arc::clone(&cancellation),
    ) {
        Ok(executor) => {
            let _ = ready.send(Ok(()));
            Some(executor)
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while let Ok(command) = commands.recv() {
        match command {
            ZsxCommand::Execute {
                generation,
                request_id,
                source,
                timeout,
                approval_grants,
                verdict_envelope,
                reply,
            } => {
                let result = executor
                    .as_ref()
                    .ok_or_else(|| HostError::Runtime("session executor is unavailable".into()))
                    .and_then(|executor| {
                        executor.execute_with_context(
                            generation,
                            request_id,
                            &source,
                            timeout,
                            approval_grants,
                            verdict_envelope,
                        )
                    });
                let _ = reply.send(result);
            }
            ZsxCommand::Replace { generation, reply } => {
                if let Ok(mut slot) = cancellation.lock() {
                    *slot = ActiveCancellationSlot::default();
                }
                let closed = executor
                    .as_ref()
                    .ok_or_else(|| "session executor is unavailable".to_string())
                    .and_then(|executor| {
                        executor
                            .publish_reachability()
                            .map_err(|error| error.to_string())
                    });
                drop(executor.take());
                let result = closed.and_then(|()| {
                    start_session_executor(
                        generation,
                        &root,
                        &state_root,
                        &session_id,
                        &adapters,
                        Arc::clone(&cancellation),
                    )
                    .map(|next| {
                        executor = Some(next);
                    })
                });
                let _ = reply.send(result);
            }
            ZsxCommand::Shutdown { reply } => {
                if let Ok(mut slot) = cancellation.lock() {
                    *slot = ActiveCancellationSlot::default();
                }
                let result = executor
                    .as_ref()
                    .ok_or_else(|| "session executor is unavailable".to_string())
                    .and_then(|executor| {
                        executor
                            .publish_reachability()
                            .map_err(|error| error.to_string())
                    });
                drop(executor.take());
                let _ = reply.send(result);
                break;
            }
        }
    }
    if let Ok(mut slot) = cancellation.lock() {
        *slot = ActiveCancellationSlot::default();
    }
}

/// Starts one generation-bound in-process executor.
fn start_session_executor(
    generation: u64,
    root: &Path,
    state_root: &Path,
    session_id: &str,
    adapters: &std::collections::BTreeMap<
        zero_abi::raw_worker::EngineIdentity,
        Arc<dyn DomainAdapter>,
    >,
    cancellation_slot: Arc<Mutex<ActiveCancellationSlot>>,
) -> Result<ZsxExecutor, String> {
    let executor = ZsxExecutor::new(
        root.to_path_buf(),
        state_root.to_path_buf(),
        session_id.to_owned(),
        adapters.clone(),
        cancellation_slot,
    )
    .map_err(|error| error.to_string())?;
    executor
        .execute_with_context(
            generation,
            0,
            "return null",
            Duration::from_secs(1),
            Vec::new(),
            None,
        )
        .map(|_| ())
        .map_err(|error| format!("session prewarm failed: {error}"))?;
    Ok(executor)
}

fn validate_session_approvals(
    state: &ZsxSessionState,
    generation: u64,
    request_id: u64,
    grants: &[SessionApprovalGrantV1],
) -> Result<Vec<String>, ZsxSessionError> {
    let invalid = |detail: String| {
        ZsxSessionError::new(
            ZsxSessionFailureCode::InvalidApproval,
            state.generation,
            Some(request_id),
            detail,
        )
    };
    if grants.len() > MAX_SESSION_APPROVAL_GRANTS {
        return Err(invalid(format!(
            "approval grant count {} exceeds maximum {MAX_SESSION_APPROVAL_GRANTS}",
            grants.len()
        )));
    }
    if state
        .consumed_approval_ids
        .len()
        .saturating_add(grants.len())
        > MAX_SESSION_CONSUMED_APPROVALS
    {
        return Err(invalid(
            "session approval replay ledger capacity exhausted".into(),
        ));
    }
    let now = now_ms();
    let mut ids = BTreeSet::new();
    for grant in grants {
        let lower_hex = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if grant.schema != crate::connector::SESSION_APPROVAL_SCHEMA
            || grant.grant_id.is_empty()
            || grant.grant_id.len() > 128
            || grant.operation.is_empty()
            || grant.operation.len() > 256
            || !lower_hex(&grant.authority_digest)
            || !lower_hex(&grant.policy_digest)
            || grant.issued_at_unix_ms >= grant.expires_at_unix_ms
            || grant
                .expires_at_unix_ms
                .saturating_sub(grant.issued_at_unix_ms)
                > MAX_SESSION_APPROVAL_LIFETIME_MS
        {
            return Err(invalid(format!(
                "approval grant '{}' is malformed",
                grant.grant_id
            )));
        }
        if grant.root != state.root
            || grant.generation != generation
            || grant.request_id != request_id
        {
            return Err(invalid(format!(
                "approval grant '{}' binding mismatch",
                grant.grant_id
            )));
        }
        if grant.effect != EffectClass::ApprovalRequiredMutation {
            return Err(invalid(format!(
                "approval grant '{}' has wrong effect",
                grant.grant_id
            )));
        }
        if now < grant.issued_at_unix_ms || now >= grant.expires_at_unix_ms {
            return Err(invalid(format!(
                "approval grant '{}' is expired or not yet valid",
                grant.grant_id
            )));
        }
        if !ids.insert(grant.grant_id.clone())
            || state.consumed_approval_ids.contains(&grant.grant_id)
        {
            return Err(ZsxSessionError::new(
                ZsxSessionFailureCode::ApprovalReplay,
                state.generation,
                Some(request_id),
                format!("approval grant '{}' was already consumed", grant.grant_id),
            ));
        }
    }
    Ok(ids.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zero_abi::raw_worker::EngineIdentity;

    #[test]
    fn default_session_id_is_unique_per_builder() {
        let first = ZsxBuilder::new(PathBuf::from("/tmp/one"));
        let second = ZsxBuilder::new(PathBuf::from("/tmp/two"));
        assert_ne!(first.session_id, second.session_id);
    }

    #[test]
    fn session_approval_contract_is_bounded_and_replay_safe() {
        let root = "/tmp/approved-root";
        let now = now_ms();
        let grant = SessionApprovalGrantV1 {
            schema: crate::connector::SESSION_APPROVAL_SCHEMA.into(),
            grant_id: "grant-1".into(),
            engine: EngineIdentity::FsZero,
            root: root.into(),
            generation: 7,
            request_id: 9,
            operation: "fs.write".into(),
            effect: EffectClass::ApprovalRequiredMutation,
            authority_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            issued_at_unix_ms: now.saturating_sub(1),
            expires_at_unix_ms: now.saturating_add(1_000),
        };
        let mut state = ZsxSessionState {
            generation: 7,
            accepting: true,
            replacing: false,
            terminating: false,
            shutdown_sent: false,
            worker_stopped: false,
            seen_request_ids: BTreeSet::new(),
            active_request_ids: BTreeSet::new(),
            root: root.into(),
            state_root: root.into(),
            consumed_approval_ids: BTreeSet::new(),
        };
        let ids = validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
            .expect("valid approval");
        state.consumed_approval_ids.extend(ids);
        assert_eq!(
            validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::ApprovalReplay
        );

        let mut fresh_state = state;
        fresh_state.consumed_approval_ids.clear();
        let mut wrong_root = grant.clone();
        wrong_root.root = "/tmp/other-root".into();
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_root])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        let mut wrong_effect = grant.clone();
        wrong_effect.effect = EffectClass::ReadOnly;
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_effect])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        let mut expired = grant.clone();
        expired.issued_at_unix_ms = now.saturating_sub(2);
        expired.expires_at_unix_ms = now.saturating_sub(1);
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[expired])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[grant.clone(), grant.clone()])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::ApprovalReplay
        );
        assert_eq!(
            validate_session_approvals(
                &fresh_state,
                7,
                9,
                &vec![grant.clone(); MAX_SESSION_APPROVAL_GRANTS + 1],
            )
            .unwrap_err()
            .code,
            ZsxSessionFailureCode::InvalidApproval
        );
    }
}
