use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::Value;
use zero_abi::{
    AsgrepMode, AsgrepOptions, CapsuleEventRoots, CapsulePublication, CapsuleRoots,
    EFFECT_RESULT_SCHEMA, EXPAND_RESULT_SCHEMA, EffectChangeKind, EffectChangeRequest,
    EffectRequest, EffectResult, EffectTargetResult, EffectVerificationResult, EngineCallContext,
    EngineError, EngineErrorKind, EngineInvocation, ExpandOptions, ExpandResult, FileEffectKind,
    FileEffectReceipt, FileEffectRequest, FileEngine, FileReadRequest, FileSnapshot, KernelBudget,
    KernelContext, KernelLedger, LookupOptions, ProjectionRequest, ProviderUsageObservation,
    ReadOptions, SNAP_WORKSPACE_SCHEMA, SafetyVerdict, ShellOptions, ShellResult, SnapAccounting,
    SnapByteRange, SnapNewline, SnapRecovery, SnapRequest, SnapResult, SnapSelection,
    SnapSelectionRequest, SnapSource, SnapStructuralEvidence, SnapTargetRequest, SnapView,
    SnapViewMode, SpeculationBinding, StateEvidence, StructuralEngine, StructuralQuery,
    TaskLensCompilerImpact, TaskLensError, TaskLensRequest, TaskLensResult, TokenEngine,
    TurnMetadata, TurnRecord, WorkCapsule, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent,
    ZeroKernelOutcome, ZeroKernelRequest, ZeroKernelResponse, ZeroOperationTrace, canonical_json,
    sha256_hex,
};
use zero_store::{EventLog, ProviderUsagePublication, ZeroCas};

use crate::preparation::{CellPreparation, PreparedCell};
use crate::shell::{ShellCommand, run_shell};
use crate::state::{StateError, StateSnapshot, StateStore};
use crate::transaction::{
    PendingFileContent, Transaction, TransactionCoordinator, TransactionError,
};

#[derive(Clone, Debug)]
pub struct AtomicCancellation(Arc<AtomicBool>);

impl AtomicCancellation {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Default for AtomicCancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl zero_abi::CancellationProbe for AtomicCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn atomic_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.0))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("engine: {0}")]
    Engine(#[from] EngineError),
    #[error("state: {0}")]
    State(#[from] StateError),
    #[error("transaction: {0}")]
    Transaction(#[from] TransactionError),
    #[error("event: {0}")]
    Event(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("result serialization: {0}")]
    Serialization(String),
}

#[derive(Default)]
struct AsyncRecord {
    calls: u32,
    bytes_read: u64,
    bytes_visible: u64,
    handles: Vec<ZeroHandle>,
}

#[derive(Clone)]
pub(crate) struct DirectCallContext {
    files: Arc<dyn FileEngine>,
    structural: Arc<dyn StructuralEngine>,
    tokens: Arc<dyn TokenEngine>,
    cas: ZeroCas,
    invocation: EngineInvocation,
    records: Arc<Mutex<AsyncRecord>>,
    live_tasks: Arc<AtomicU64>,
    live_processes: Arc<AtomicU64>,
    frame_tasks: Arc<AtomicU64>,
    frame_processes: Arc<AtomicU64>,
}

struct LiveTaskGuard {
    global: Arc<AtomicU64>,
    frame: Arc<AtomicU64>,
}

impl LiveTaskGuard {
    fn acquire(global: Arc<AtomicU64>, frame: Arc<AtomicU64>) -> Self {
        global.fetch_add(1, Ordering::AcqRel);
        frame.fetch_add(1, Ordering::AcqRel);
        Self { global, frame }
    }
}

impl Drop for LiveTaskGuard {
    fn drop(&mut self) {
        self.frame.fetch_sub(1, Ordering::AcqRel);
        self.global.fetch_sub(1, Ordering::AcqRel);
    }
}

struct FrameProcessGuard(Arc<AtomicU64>);

impl FrameProcessGuard {
    fn acquire(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for FrameProcessGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl DirectCallContext {
    pub(crate) fn project_root(&self) -> &std::path::Path {
        &self.invocation.context.project_root
    }

    fn normalize_explicit_external_read(&self, path: PathBuf) -> Result<PathBuf, HostError> {
        if path.is_absolute()
            || !path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Ok(path);
        }
        std::fs::canonicalize(self.project_root().join(&path)).map_err(|error| {
            HostError::InvalidRequest(format!(
                "resolve explicit external read {}: {error}",
                path.display()
            ))
        })
    }

    pub fn read(&self, path: PathBuf, options: ReadOptions) -> Result<String, HostError> {
        let request_path = path;
        let path = self.normalize_explicit_external_read(request_path.clone())?;
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path: path.clone(),
                options,
            },
        )?;
        let value = match snapshot.inline_utf8 {
            Some(text) if !text.as_bytes().contains(&0) => text,
            _ => labeled_outline(&request_path, &snapshot),
        };
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_read = record.bytes_read.saturating_add(snapshot.byte_len);
        if !record.handles.contains(&snapshot.content) {
            record.handles.push(snapshot.content);
        }
        Ok(value)
    }

    pub fn lookup(&self, root: PathBuf, options: LookupOptions) -> Result<Vec<PathBuf>, HostError> {
        let root = self.normalize_explicit_external_read(root)?;
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let paths = self.files.lookup(&self.invocation, root, options)?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        Ok(paths)
    }

    pub fn asgrep(
        &self,
        query: String,
        options: AsgrepOptions,
    ) -> Result<zero_abi::StructuralResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let result = self
            .structural
            .query(&self.invocation, StructuralQuery { query, options })?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        for handle in result
            .hits
            .iter()
            .filter_map(|hit| hit.evidence.clone())
            .chain(result.continuation.clone())
        {
            if !record.handles.contains(&handle) {
                record.handles.push(handle);
            }
        }
        Ok(result)
    }

    pub fn task_lens(&self, request: TaskLensRequest) -> Result<TaskLensResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let result = match self.structural.task_lens(&self.invocation, request.clone()) {
            // An engine without task-lens support degrades to a canonical
            // Unknown verdict, never a Safe-looking or error result.
            Err(error) if error.kind == EngineErrorKind::Unsupported => {
                task_lens_unknown("task_lens_unsupported")
            }
            Err(error) => return Err(HostError::from(error)),
            Ok(result) => match result.validate(&request) {
                Ok(()) => result,
                // An invalid would-be Safe must never surface as authority:
                // degrade to Unknown with the canonical contract reason.
                Err(error) if result.verdict == SafetyVerdict::Safe => {
                    task_lens_unknown(&task_lens_reason(&error))
                }
                Err(error) => {
                    return Err(HostError::InvalidRequest(format!(
                        "task lens result failed validation: {error}"
                    )));
                }
            },
        };
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        for handle in task_lens_handles(&result) {
            if !record.handles.contains(&handle) {
                record.handles.push(handle);
            }
        }
        Ok(result)
    }

    pub fn expand(
        &self,
        handle: &ZeroHandle,
        options: ExpandOptions,
    ) -> Result<ExpandResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let result = expand_handle(&self.cas, &*self.tokens, &self.invocation, handle, options)?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_read = record
            .bytes_read
            .saturating_add(result.byte_end.saturating_sub(result.byte_start));
        if !record.handles.contains(handle) {
            record.handles.push(handle.clone());
        }
        Ok(result)
    }

    pub fn shell(
        &self,
        command: ShellCommand,
        options: ShellOptions,
    ) -> Result<ShellResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let _process = FrameProcessGuard::acquire(Arc::clone(&self.frame_processes));
        let result = run_shell(
            &self.invocation,
            &*self.tokens,
            Arc::clone(&self.live_processes),
            command,
            options,
        )?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_visible = record
            .bytes_visible
            .saturating_add((result.stdout.len() + result.stderr.len()) as u64);
        if let Some(handle) = result.exact.clone()
            && !record.handles.contains(&handle)
        {
            record.handles.push(handle);
        }
        Ok(result)
    }
}

pub struct ZeroKernel {
    context: KernelContext,
    budget: KernelBudget,
    files: Arc<dyn FileEngine>,
    structural: Arc<dyn StructuralEngine>,
    tokens: Arc<dyn TokenEngine>,
    cas: ZeroCas,
    events: EventLog,
    state: StateStore,
    transactions: TransactionCoordinator,
    next_cell: AtomicU64,
    live_frames: Arc<AtomicU64>,
    live_tasks: Arc<AtomicU64>,
    live_processes: Arc<AtomicU64>,
}

impl std::fmt::Debug for ZeroKernel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZeroKernel")
            .field("context", &self.context)
            .field("budget", &self.budget)
            .field("live_frames", &self.live_frames())
            .field("live_tasks", &self.live_tasks())
            .field("live_processes", &self.live_processes())
            .finish_non_exhaustive()
    }
}

impl ZeroKernel {
    pub fn new(
        mut context: KernelContext,
        budget: KernelBudget,
        files: Arc<dyn FileEngine>,
        structural: Arc<dyn StructuralEngine>,
        tokens: Arc<dyn TokenEngine>,
        store_root: impl Into<PathBuf>,
    ) -> Result<Self, HostError> {
        context.workspace_root =
            std::fs::canonicalize(&context.workspace_root).map_err(|error| {
                HostError::InvalidRequest(format!("canonicalize workspace root: {error}"))
            })?;
        context.project_root = std::fs::canonicalize(&context.project_root).map_err(|error| {
            HostError::InvalidRequest(format!("canonicalize project root: {error}"))
        })?;
        context
            .validate()
            .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        budget
            .validate()
            .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        let store_root = store_root.into();
        let cas = ZeroCas::open(store_root.clone());
        let state = StateStore::open(store_root.clone(), &context.session_id);
        let transactions =
            TransactionCoordinator::new(store_root.clone(), cas.clone(), Arc::clone(&files));
        let events = EventLog::open(store_root);
        let event_sequence = events
            .records(&context.session_id)
            .map_err(|error| HostError::Event(error.to_string()))?
            .iter()
            .filter_map(|record| cell_sequence(&record.cell_id))
            .max()
            .unwrap_or(0);
        let next_cell =
            event_sequence.max(transactions.highest_cell_sequence(&context.session_id)?);
        Ok(Self {
            context,
            budget,
            files,
            structural,
            tokens,
            cas,
            events,
            state,
            transactions,
            next_cell: AtomicU64::new(next_cell),
            live_frames: Arc::new(AtomicU64::new(0)),
            live_tasks: Arc::new(AtomicU64::new(0)),
            live_processes: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn begin_cell(&self, source: &str) -> Result<Cell, HostError> {
        self.begin_cell_with_cancellation(source, AtomicCancellation::new())
    }

    pub fn begin_cell_with_cancellation(
        &self,
        source: &str,
        cancellation: AtomicCancellation,
    ) -> Result<Cell, HostError> {
        let request =
            ZeroKernelRequest::new(source.to_owned(), self.context.clone(), self.budget.clone())
                .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        self.begin_request(request, cancellation)
    }
    pub fn begin_request(
        &self,
        request: ZeroKernelRequest,
        cancellation: AtomicCancellation,
    ) -> Result<Cell, HostError> {
        self.begin_from_request(request, cancellation, None)
    }

    /// Shared launch core for ordinary and prepared cells.
    ///
    /// Ordinary cells derive canonical capsule roots from the current facts,
    /// draft a `WorkCapsule`, publish it through the FileEngine, and seal a
    /// `PreparedCell` carrying the exact capsule, publication, and binding.
    /// Prepared cells bind the caller-supplied coordinates instead: the
    /// publication must roundtrip through the FileEngine byte-for-byte and
    /// the current effective state/contract coordinates must match the
    /// sealed binding before any guest work launches. No capsule is drafted
    /// or published twice and no transient divergent capsule exists.
    pub(crate) fn begin_from_request(
        &self,
        request: ZeroKernelRequest,
        cancellation: AtomicCancellation,
        sealed: Option<&PreparedCell>,
    ) -> Result<Cell, HostError> {
        request
            .validate()
            .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        if request.context != self.context {
            return Err(HostError::InvalidRequest(
                "request context differs from initialized ZeroKernel context".into(),
            ));
        }
        let expected = request
            .context
            .expected_state_root
            .as_deref()
            .map(ZeroHandle::parse)
            .transpose()
            .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        let state = self.state.load(expected.as_ref())?;
        let cell_number = self.next_cell.fetch_add(1, Ordering::Relaxed) + 1;
        let cell_id = format!("cell-{cell_number}");
        let deadline_unix_ms = now_ms().saturating_add(request.budget.wall_ms);
        let invocation = EngineInvocation {
            context: EngineCallContext {
                workspace_root: self.context.workspace_root.clone(),
                project_root: self.context.project_root.clone(),
                session_id: self.context.session_id.clone(),
                cell_id: cell_id.clone(),
                trace_id: format!("{}-{cell_id}", self.context.session_id),
                deadline_unix_ms,
                budget: request.budget.clone(),
            },
            cancellation: Arc::new(cancellation.clone()),
        };
        self.transactions.reconcile(&invocation)?;
        let prepared = match sealed {
            Some(sealed) => {
                // The finalized source is bound into the capsule task root:
                // an internal caller must never launch sealed coordinates
                // under a different source. Reject before any state,
                // binding, or recovery check.
                if request.source != sealed.source() {
                    return Err(HostError::InvalidRequest(
                        "prepared cell launch source differs from the sealed source".into(),
                    ));
                }
                let current_state_root = effective_state_root(&state);
                let current_contract_root = contract_root(&self.context);
                if sealed.binding().state_root != current_state_root
                    || sealed.binding().contract_root != current_contract_root
                {
                    return Err(HostError::InvalidRequest(
                        "prepared cell state or contract binding drifted".into(),
                    ));
                }
                if sealed.capsule().roots.execution != execution_root(&request.budget)
                    || sealed.capsule().provider_usage_budget
                        != u64::from(request.budget.call_limit)
                    || sealed.capsule().complete_work_budget != request.budget.cpu_ms
                {
                    return Err(HostError::InvalidRequest(
                        "prepared cell budget binding drifted".into(),
                    ));
                }
                let recovered = self
                    .files
                    .get_capsule(&invocation, sealed.publication())
                    .map_err(HostError::Engine)?;
                if &recovered != sealed.capsule() {
                    return Err(HostError::InvalidRequest(
                        "prepared cell capsule is not recoverable from its publication".into(),
                    ));
                }
                sealed.clone()
            }
            None => {
                // The capsule budgets are explicit units mapped from the
                // kernel budget: provider_usage_budget binds the
                // provider-visible dispatch envelope (call_limit) and
                // complete_work_budget binds the compute envelope (cpu_ms).
                // The epoch is this cell's session sequence. Nothing is
                // silently zeroed; request validation already guarantees
                // every budget dimension is positive.
                let capsule = WorkCapsule::draft(
                    capsule_roots(&self.context, &request.budget, &request.source)?,
                    cell_number,
                    u64::from(request.budget.call_limit),
                    request.budget.cpu_ms,
                )
                .map_err(|detail| HostError::InvalidRequest(detail))?;
                let publication = self
                    .files
                    .put_capsule(&invocation, &capsule)
                    .map_err(HostError::Engine)?;
                let binding = SpeculationBinding {
                    capsule_root: publication.capsule_root.clone(),
                    state_root: effective_state_root(&state),
                    contract_root: contract_root(&self.context),
                    epoch: capsule.epoch,
                };
                let mut preparation = CellPreparation::new();
                preparation
                    .feed(&request.source)
                    .map_err(|detail| HostError::InvalidRequest(detail))?;
                preparation
                    .finish(binding, capsule, publication)
                    .map_err(|detail| HostError::InvalidRequest(detail))?
            }
        };
        self.live_frames.fetch_add(1, Ordering::AcqRel);
        Ok(Cell {
            source: request.source,
            prepared,
            context: request.context,
            budget: request.budget,
            invocation,
            cancellation,
            files: Arc::clone(&self.files),
            structural: Arc::clone(&self.structural),
            tokens: Arc::clone(&self.tokens),
            cas: self.cas.clone(),
            events: self.events.clone(),
            state_store: self.state.clone(),
            transaction_coordinator: self.transactions.clone(),
            state,
            state_dirty: false,
            transaction: None,
            handles: Vec::new(),
            ledger: KernelLedger::default(),
            operations: Vec::new(),
            operations_truncated: false,
            live_frames: Arc::clone(&self.live_frames),
            live_tasks: Arc::clone(&self.live_tasks),
            live_processes: Arc::clone(&self.live_processes),
            frame_tasks: Arc::new(AtomicU64::new(0)),
            frame_processes: Arc::new(AtomicU64::new(0)),
            async_records: Arc::new(Mutex::new(AsyncRecord::default())),
            turn_metadata: request.turn.unwrap_or_else(TurnMetadata::native),
            settled: false,
        })
    }

    pub(crate) fn request_context(&self) -> &KernelContext {
        &self.context
    }

    pub(crate) fn request_budget(&self) -> &KernelBudget {
        &self.budget
    }

    pub fn live_frames(&self) -> u64 {
        self.live_frames.load(Ordering::Acquire)
    }

    pub fn live_tasks(&self) -> u64 {
        self.live_tasks.load(Ordering::Acquire)
    }

    pub fn live_processes(&self) -> u64 {
        self.live_processes.load(Ordering::Acquire)
    }

    pub fn cas(&self) -> &ZeroCas {
        &self.cas
    }

    pub fn record_provider_usage(
        &self,
        kernel_event_handle: &ZeroHandle,
        observation: ProviderUsageObservation,
    ) -> Result<ProviderUsagePublication, HostError> {
        self.events
            .publish_provider_usage(&self.context.session_id, kernel_event_handle, observation)
            .map_err(|error| HostError::Event(error.to_string()))
    }

    /// One-call prepare, validate, execute, and atomically commit.
    ///
    /// Binds the prepared source digest, WorkCapsule roots (including policy
    /// via the contract coordinate), state-before root, and the exact effect
    /// receipt in a single host call. Cancellation is enforced before any
    /// commit and receipt/binding drift is rejected. The returned typed
    /// response's `state` and `effects` refer to the same committed effect;
    /// validation/cancellation failures leave state unchanged. No placeholder
    /// receipt and no second effect authority exists outside this path.
    pub fn execute_atomic_effect(
        &self,
        source: &str,
        request: zero_abi::EffectRequest,
        cancellation: AtomicCancellation,
    ) -> Result<ZeroKernelResponse, HostError> {
        request
            .validate()
            .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
        if cancellation.is_cancelled() {
            let cell = self.begin_cell_with_cancellation(source, cancellation.clone())?;
            return cell.fail(EngineError::new(
                EngineErrorKind::Cancelled,
                "cancelled before validation",
                false,
            ));
        }
        let mut cell = self.begin_cell_with_cancellation(source, cancellation.clone())?;
        if cancellation.is_cancelled() || cell.invocation.cancellation.is_cancelled() {
            return cell.fail(EngineError::new(
                EngineErrorKind::Cancelled,
                "cancelled before effect",
                false,
            ));
        }
        let effect_result = match cell.effect(request) {
            Ok(result) => result,
            Err(HostError::Engine(error))
            | Err(HostError::Transaction(TransactionError::Engine(error))) => {
                return cell.fail(error);
            }
            Err(HostError::Transaction(TransactionError::RecoveryRequired(details))) => {
                return cell.fail(EngineError::new(
                    EngineErrorKind::Corrupt,
                    format!("transaction recovery required: {}", details.join("; ")),
                    false,
                ));
            }
            Err(HostError::Transaction(TransactionError::Store(detail))) => {
                return cell.fail(EngineError::new(EngineErrorKind::Corrupt, detail, false));
            }
            Err(HostError::InvalidRequest(detail)) => {
                return cell.fail(EngineError::new(
                    EngineErrorKind::InvalidInput,
                    detail,
                    false,
                ));
            }
            Err(error) => {
                return cell.fail(EngineError::new(
                    EngineErrorKind::Internal,
                    error.to_string(),
                    false,
                ));
            }
        };
        // Enforce cancellation before commit (no commit after cancellation).
        // Call fail with the transaction still present so fail owns rollback.
        if cancellation.is_cancelled() || cell.invocation.cancellation.is_cancelled() {
            return cell.fail(EngineError::new(
                EngineErrorKind::Cancelled,
                "cancelled before commit",
                false,
            ));
        }
        // The cell's finish will enforce binding drift and receipt drift atomically
        // and return a typed response where state and receipt are the same committed effect.
        let value = serde_json::to_value(&effect_result)
            .map_err(|error| HostError::Serialization(error.to_string()))?;
        cell.finish(value)
    }

    /// Convenience wrapper using a fresh cancellation token.
    pub fn execute_atomic_effect_simple(
        &self,
        source: &str,
        request: zero_abi::EffectRequest,
    ) -> Result<ZeroKernelResponse, HostError> {
        self.execute_atomic_effect(source, request, AtomicCancellation::new())
    }
}

pub struct Cell {
    source: String,
    prepared: PreparedCell,
    context: KernelContext,
    budget: KernelBudget,
    invocation: EngineInvocation,
    cancellation: AtomicCancellation,
    files: Arc<dyn FileEngine>,
    structural: Arc<dyn StructuralEngine>,
    tokens: Arc<dyn TokenEngine>,
    cas: ZeroCas,
    events: EventLog,
    state_store: StateStore,
    transaction_coordinator: TransactionCoordinator,
    state: StateSnapshot,
    state_dirty: bool,
    transaction: Option<Transaction>,
    handles: Vec<ZeroHandle>,
    ledger: KernelLedger,
    operations: Vec<ZeroOperationTrace>,
    operations_truncated: bool,
    live_frames: Arc<AtomicU64>,
    live_tasks: Arc<AtomicU64>,
    live_processes: Arc<AtomicU64>,
    frame_tasks: Arc<AtomicU64>,
    frame_processes: Arc<AtomicU64>,
    async_records: Arc<Mutex<AsyncRecord>>,
    turn_metadata: TurnMetadata,
    settled: bool,
}

impl Cell {
    pub fn cancellation(&self) -> AtomicCancellation {
        self.cancellation.clone()
    }
    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// The exact capsule root this cell runs under: the canonical root of
    /// the Draft work capsule sealed at launch. Every guest operation trace
    /// and every terminal event binds to this root.
    pub fn capsule_root(&self) -> &str {
        &self.prepared.publication().capsule_root
    }

    /// The capsule publication through which the sealed capsule was (or, for
    /// prepared launches, was originally) persisted.
    pub fn publication(&self) -> &CapsulePublication {
        self.prepared.publication()
    }

    /// The speculation binding sealed for this cell: capsule, effective state
    /// root, contract root, and epoch.
    pub fn binding(&self) -> &SpeculationBinding {
        self.prepared.binding()
    }

    /// The exact Draft work capsule this cell launches under.
    pub fn capsule(&self) -> &WorkCapsule {
        self.prepared.capsule()
    }
    pub(crate) fn has_active_transaction(&self) -> bool {
        self.transaction.is_some()
    }

    pub fn read(
        &mut self,
        path: impl Into<PathBuf>,
        options: ReadOptions,
    ) -> Result<String, HostError> {
        let path = path.into();
        let candidate = if path.is_absolute() {
            path.clone()
        } else {
            self.context.project_root.join(&path)
        };
        if let Some(pending) = self.pending_file_bytes(&path)? {
            let bytes = Self::project_read_bytes(pending, &options)?;
            self.ledger.calls = self.ledger.calls.saturating_add(1);
            self.ledger.bytes_read = self
                .ledger
                .bytes_read
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            if let Ok(text) = String::from_utf8(bytes.clone())
                && !text.as_bytes().contains(&0)
            {
                return Ok(text);
            }
        }
        if candidate.is_dir() {
            if options.range.is_some() || options.max_bytes.is_some() {
                return Err(HostError::InvalidRequest(
                    "directory reads do not accept file range or byte-limit options".into(),
                ));
            }
            let paths = self.lookup(
                path.clone(),
                LookupOptions {
                    filter: None,
                    limit: None,
                    recursive: options.recursive,
                },
            )?;
            if options.offset.is_some() || options.limit.is_some() {
                let start = usize::try_from(options.offset.unwrap_or(0)).unwrap_or(usize::MAX);
                let limit = usize::try_from(options.limit.unwrap_or(100)).unwrap_or(usize::MAX);
                let start = start.min(paths.len());
                let end = start.saturating_add(limit).min(paths.len());
                let next = (end < paths.len()).then_some(end as u32);
                return serde_json::to_string(&serde_json::json!({
                    "entries": &paths[start..end],
                    "next": next,
                    "complete": next.is_none(),
                }))
                .map_err(|error| HostError::Serialization(error.to_string()));
            }
            return serde_json::to_string(&paths)
                .map_err(|error| HostError::Serialization(error.to_string()));
        }
        let mut snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path: path.clone(),
                options,
            },
        )?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_read = self.ledger.bytes_read.saturating_add(snapshot.byte_len);
        self.handles.push(snapshot.content.clone());
        match snapshot.inline_utf8.take() {
            Some(text) if !text.as_bytes().contains(&0) => return Ok(text),
            Some(text) => snapshot.inline_utf8 = Some(text),
            None => {}
        }
        Ok(labeled_outline(&path, &snapshot))
    }

    pub fn snap(&mut self, request: SnapRequest) -> Result<SnapResult, HostError> {
        request.validate().map_err(HostError::InvalidRequest)?;
        let SnapRequest {
            target,
            cardinality,
            selection,
            view,
        } = request;
        if cardinality
            .as_deref()
            .is_some_and(|value| value != "exactly_one")
        {
            return Err(HostError::InvalidRequest(
                "structured z.read discovery requires cardinality exactly_one".into(),
            ));
        }

        let (path, structural, derived_lines) = match target {
            SnapTargetRequest::Path { path } => {
                // Directory targets use the simple z.read(path) form.
                let candidate = if path.is_absolute() {
                    path.clone()
                } else {
                    self.invocation.context.project_root.join(&path)
                };
                if candidate.is_dir() {
                    return Err(HostError::InvalidRequest(format!(
                        "structured z.read requires a file but {} is a directory; use z.read({:?}) to list entries",
                        path.display(),
                        path.display()
                    )));
                }
                if let Some(symbol) = selection
                    .as_ref()
                    .and_then(|selection| selection.symbol.as_ref())
                {
                    let result = self.asgrep(
                        symbol.clone(),
                        AsgrepOptions {
                            mode: AsgrepMode::Definition,
                            path: Some(path.clone()),
                            language: None,
                            source: None,
                            sink: None,
                            limit: Some(2),
                            budget_tokens: None,
                        },
                    )?;
                    let (structural, lines) = exact_structural_hit(result)?;
                    (path, Some(structural), Some(lines))
                } else {
                    (path, None, None)
                }
            }
            SnapTargetRequest::Search { search } => {
                let result = self.asgrep(
                    search.query,
                    AsgrepOptions {
                        mode: search.mode.unwrap_or(AsgrepMode::Natural),
                        path: search.under,
                        language: search.language,
                        source: None,
                        sink: None,
                        limit: Some(2),
                        budget_tokens: None,
                    },
                )?;
                let path = result
                    .hits
                    .first()
                    .map(|hit| hit.path.clone())
                    .ok_or_else(|| {
                        HostError::Engine(EngineError::new(
                            EngineErrorKind::NotFound,
                            "structured z.read search found no target",
                            false,
                        ))
                    })?;
                let (structural, lines) = exact_structural_hit(result)?;
                (path, Some(structural), Some(lines))
            }
        };

        let snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path: path.clone(),
                options: ReadOptions::default(),
            },
        )?;
        let mapped = self.cas.map(&snapshot.content).map_err(cas_host_error)?;
        let bytes = mapped.bytes();
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_read = self.ledger.bytes_read.saturating_add(snapshot.byte_len);
        push_handle(&mut self.handles, snapshot.content.clone());
        if structural
            .as_ref()
            .is_some_and(|evidence| evidence.source != snapshot.content)
        {
            return Err(HostError::Engine(EngineError::new(
                EngineErrorKind::Conflict,
                "stale structural source: GraphZero evidence does not match the FSZero snapshot",
                true,
            )));
        }

        let selected = snap_selection(
            &self.cas,
            &snapshot.content,
            bytes,
            selection.as_ref(),
            derived_lines,
        )?;
        let source_accounting = self.tokens.measure(&self.invocation, bytes)?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        let utf8 = std::str::from_utf8(&bytes).ok();
        let source = SnapSource {
            exact: snapshot.content.clone(),
            content_digest: snapshot.content.digest().to_owned(),
            byte_length: snapshot.byte_len,
            line_count: utf8.map(source_line_count),
            encoding: if utf8.is_some() { "utf8" } else { "binary" }.into(),
            newline: utf8.map_or(SnapNewline::None, |_| source_newline(bytes)),
            bom: bytes.starts_with(&[0xef, 0xbb, 0xbf]),
            mode: snapshot.mode,
            modified_unix_ns: snapshot.modified_unix_ns.to_string(),
        };
        let visible_limit = self.budget.output_byte_limit.saturating_div(2).max(256);

        let (view_text, full_file_visible, visible_source_bytes, accounting) = if matches!(
            view.mode,
            SnapViewMode::Full
        ) {
            let text = utf8.ok_or_else(|| {
                HostError::Engine(EngineError::new(
                    EngineErrorKind::InvalidInput,
                    "z.read full view requires UTF-8 source",
                    false,
                ))
            })?;
            if text.len() > visible_limit as usize {
                return Err(HostError::Engine(EngineError::new(
                    EngineErrorKind::Budget,
                    {
                        let recovery = store_recovery_manifest(
                            &self.cas,
                            &path,
                            &source,
                            &selected,
                            &structural,
                            None,
                        )?;
                        push_handle(&mut self.handles, recovery.clone());
                        format!(
                            "full_view_unavailable: {} source bytes exceed the {}-byte result envelope; exact={} recovery={}",
                            bytes.len(),
                            visible_limit,
                            snapshot.content,
                            recovery
                        )
                    },
                    false,
                )));
            }
            (
                Some(text.to_owned()),
                true,
                bytes.len() as u64,
                source_accounting.clone(),
            )
        } else if let (Some(selection), Some(_)) = (selected.as_ref(), utf8) {
            let start = selection.byte_start as usize;
            let end = selection.byte_end as usize;
            let projection = self.tokens.project(
                &self.invocation,
                ProjectionRequest {
                    bytes: bytes[start..end].to_vec(),
                    visible_byte_limit: visible_limit,
                    media_type: "text/plain; charset=utf-8".into(),
                },
            )?;
            self.ledger.calls = self.ledger.calls.saturating_add(1);
            if let Some(handle) = projection.exact.clone() {
                push_handle(&mut self.handles, handle);
            }
            let visible_source_bytes = projection.visible_source_bytes;
            let mut accounting = projection.accounting;
            accounting.billed = source_accounting.billed;
            (
                Some(projection.visible),
                false,
                visible_source_bytes,
                accounting,
            )
        } else if utf8.is_some() {
            let projection = self.tokens.project(
                &self.invocation,
                ProjectionRequest {
                    bytes: bytes.to_vec(),
                    visible_byte_limit: visible_limit,
                    media_type: "text/plain; charset=utf-8".into(),
                },
            )?;
            self.ledger.calls = self.ledger.calls.saturating_add(1);
            if let Some(handle) = projection.exact.clone() {
                push_handle(&mut self.handles, handle);
            }
            let full = projection.exact.is_none();
            let visible_source_bytes = projection.visible_source_bytes;
            (
                Some(projection.visible),
                full,
                visible_source_bytes,
                projection.accounting,
            )
        } else {
            let mut accounting = source_accounting.clone();
            accounting.visible = 0;
            (None, false, 0, accounting)
        };

        let visible_start = if full_file_visible {
            0
        } else {
            selected
                .as_ref()
                .map_or(0, |selection| selection.byte_start)
        };
        let visible_end = visible_start
            .saturating_add(visible_source_bytes)
            .min(source.byte_length);
        let visible_ranges = (visible_end > visible_start)
            .then_some(SnapByteRange {
                byte_start: visible_start,
                byte_end: visible_end,
            })
            .into_iter()
            .collect();
        let view = SnapView {
            mode: view.mode,
            visible_bytes: view_text.as_ref().map_or(0, |text| text.len() as u64),
            text: view_text,
            full_file_visible,
            omitted_bytes: source.byte_length.saturating_sub(visible_source_bytes),
            visible_ranges,
        };
        let omitted_tokens = source_accounting.billed.saturating_sub(accounting.visible);
        let accounting = SnapAccounting {
            tokenizer: accounting.tokenizer,
            certified: accounting.certified,
            source_tokens: source_accounting.billed,
            visible_tokens: accounting.visible,
            omitted_tokens,
            recovered_tokens: 0,
            saved_tokens_now: omitted_tokens,
            cached_tokens: accounting.cached,
        };
        let recovery_manifest = store_recovery_manifest(
            &self.cas,
            &path,
            &source,
            &selected,
            &structural,
            Some(&view),
        )?;
        push_handle(&mut self.handles, recovery_manifest.clone());
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Manifest<'a> {
            schema: &'static str,
            path: &'a PathBuf,
            source: &'a SnapSource,
            selection: &'a Option<SnapSelection>,
            view: &'a SnapView,
            accounting: &'a SnapAccounting,
            recovery: &'a ZeroHandle,
            structural: &'a Option<SnapStructuralEvidence>,
        }
        let manifest = serde_json::to_vec(&Manifest {
            schema: SNAP_WORKSPACE_SCHEMA,
            path: &path,
            source: &source,
            selection: &selected,
            view: &view,
            accounting: &accounting,
            recovery: &recovery_manifest,
            structural: &structural,
        })
        .map_err(|error| HostError::Serialization(error.to_string()))?;
        let snapshot_handle = self.cas.put(&manifest).map_err(cas_host_error)?;
        push_handle(&mut self.handles, snapshot_handle.clone());

        let result = SnapResult {
            schema: SNAP_WORKSPACE_SCHEMA.into(),
            snapshot: snapshot_handle.clone(),
            path,
            source: source.clone(),
            selection: selected,
            view,
            recovery: SnapRecovery {
                manifest: recovery_manifest,
                exact: source.exact,
                complete: true,
                recoverable_bytes: source.byte_length,
                unrecoverable_bytes: 0,
                retained: false,
                retention_policy: "call_output_handles".into(),
                selectors: if utf8.is_some() {
                    vec!["bytes".into(), "lines".into(), "next".into(), "all".into()]
                } else {
                    vec!["bytes".into(), "next".into(), "all".into()]
                },
                next: (!full_file_visible).then_some(0),
            },
            structural,
            accounting,
        };
        result.validate().map_err(HostError::Serialization)?;
        Ok(result)
    }

    pub(crate) fn effect(&mut self, request: EffectRequest) -> Result<EffectResult, HostError> {
        request.validate().map_err(HostError::InvalidRequest)?;
        if self.transaction.is_some() {
            return Err(HostError::InvalidRequest(
                "z.apply cannot follow z.edit in one cell; express all mutations in z.apply or start a separate ZeroKernel call"
                    .into(),
            ));
        }
        if request.targets.is_empty() || request.changes.is_empty() {
            return Err(HostError::InvalidRequest(
                "z.apply requires at least one target and one change".into(),
            ));
        }
        if request.verify.parse {
            return Err(HostError::InvalidRequest(
                "verification_unavailable: z.apply parse verification requires a confined child image"
                    .into(),
            ));
        }
        if !request.verify.changed_targets_only {
            return Err(HostError::InvalidRequest(
                "z.apply requires verify.changedTargetsOnly=true".into(),
            ));
        }
        if request.verify.command.is_some() {
            return Err(HostError::InvalidRequest(
                "verification_unavailable: z.apply commands require child-image confinement and exact delta verification"
                    .into(),
            ));
        }

        let mut planned = BTreeMap::new();
        for (name, target) in request.targets {
            if name.is_empty() {
                return Err(HostError::InvalidRequest(
                    "z.apply target names must not be empty".into(),
                ));
            }
            let expectation = target.expect.as_deref().unwrap_or("exists");
            let (before, postimage) = match expectation {
                "exists" => {
                    let snapshot = self.files.read(
                        &self.invocation,
                        FileReadRequest {
                            path: target.path.clone(),
                            options: ReadOptions::default(),
                        },
                    )?;
                    let bytes = self.cas.get(&snapshot.content).map_err(cas_host_error)?;
                    self.ledger.calls = self.ledger.calls.saturating_add(1);
                    self.ledger.bytes_read =
                        self.ledger.bytes_read.saturating_add(snapshot.byte_len);
                    push_handle(&mut self.handles, snapshot.content.clone());
                    (Some(snapshot), Some(bytes))
                }
                "absent" => (None, None),
                other => {
                    return Err(HostError::InvalidRequest(format!(
                        "z.apply target {name:?} has unknown expectation {other:?}"
                    )));
                }
            };
            planned.insert(
                name,
                PlannedFileEffect {
                    path: target.path,
                    before,
                    postimage,
                    changed: false,
                    sealed: false,
                    remove: false,
                },
            );
        }

        for change in request.changes {
            let target = planned.get_mut(&change.target).ok_or_else(|| {
                HostError::InvalidRequest(format!(
                    "z.apply change names unknown target {:?}",
                    change.target
                ))
            })?;
            plan_effect_change(target, change)?;
        }
        if planned.values().any(|target| !target.changed) {
            return Err(HostError::InvalidRequest(
                "every z.apply target must receive at least one change".into(),
            ));
        }

        self.begin_transaction()?;
        let mut results = Vec::with_capacity(planned.len());
        let apply = (|| -> Result<(), HostError> {
            for (name, target) in planned {
                let before = target
                    .before
                    .as_ref()
                    .map(|snapshot| snapshot.content.clone());
                let (kind, content, label) = if target.remove {
                    (FileEffectKind::Remove, None, "remove")
                } else if before.is_none() {
                    (FileEffectKind::Write, target.postimage, "create")
                } else {
                    (FileEffectKind::Edit, target.postimage, "edit")
                };
                if let Some(bytes) = content.as_ref() {
                    self.ledger.bytes_written =
                        self.ledger.bytes_written.saturating_add(bytes.len() as u64);
                }
                let receipt = self.apply_file_effect(FileEffectRequest {
                    kind,
                    path: target.path.clone(),
                    content,
                    patch: None,
                    expected_preimage: before.clone(),
                    expect_absent: before.is_none(),
                })?;
                results.push(EffectTargetResult {
                    name,
                    path: target.path,
                    kind: label.into(),
                    before: receipt.before,
                    after: receipt.after,
                    journal: receipt.journal,
                });
            }

            Ok(())
        })();

        if let Err(error) = apply {
            if let Err(rollback) = self.rollback_transaction() {
                return Err(rollback);
            }
            return Err(error);
        }
        let delta_bytes = serde_json::to_vec(&results)
            .map_err(|error| HostError::Serialization(error.to_string()))?;
        let delta = self.cas.put(&delta_bytes).map_err(cas_host_error)?;
        push_handle(&mut self.handles, delta.clone());
        let result = EffectResult {
            schema: EFFECT_RESULT_SCHEMA.into(),
            outcome: "staged".into(),
            delta,
            changed_files: u32::try_from(results.len())
                .map_err(|_| HostError::Serialization("effect target count exceeds u32".into()))?,
            targets: results,
            verification: EffectVerificationResult {
                parse: "not_requested".into(),
                command: "not_requested".into(),
                changed_targets_only: true,
            },
        };
        result.validate().map_err(HostError::Serialization)?;
        Ok(result)
    }

    pub fn lookup(
        &mut self,
        root: impl Into<PathBuf>,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, HostError> {
        let paths = self.files.lookup(&self.invocation, root.into(), options)?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        Ok(paths)
    }

    pub fn asgrep(
        &mut self,
        query: impl Into<String>,
        options: AsgrepOptions,
    ) -> Result<zero_abi::StructuralResult, HostError> {
        let result = self.structural.query(
            &self.invocation,
            StructuralQuery {
                query: query.into(),
                options,
            },
        )?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        for handle in result
            .hits
            .iter()
            .filter_map(|hit| hit.evidence.clone())
            .chain(result.continuation.clone())
        {
            if !self.handles.contains(&handle) {
                self.handles.push(handle);
            }
        }
        Ok(result)
    }

    pub fn expand(
        &mut self,
        handle: &ZeroHandle,
        options: ExpandOptions,
    ) -> Result<ExpandResult, HostError> {
        let result = expand_handle(&self.cas, &*self.tokens, &self.invocation, handle, options)?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_read = self
            .ledger
            .bytes_read
            .saturating_add(result.byte_end.saturating_sub(result.byte_start));
        if !self.handles.contains(handle) {
            self.handles.push(handle.clone());
        }
        Ok(result)
    }

    pub fn shell(
        &mut self,
        command: ShellCommand,
        options: ShellOptions,
    ) -> Result<ShellResult, HostError> {
        let _process = FrameProcessGuard::acquire(Arc::clone(&self.frame_processes));
        let result = run_shell(
            &self.invocation,
            &*self.tokens,
            Arc::clone(&self.live_processes),
            command,
            options,
        )?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        if let Some(handle) = result.exact.clone() {
            self.handles.push(handle);
        }
        Ok(result)
    }

    pub(crate) fn begin_transaction(&mut self) -> Result<(), HostError> {
        if self.transaction.is_some() {
            return Ok(());
        }
        self.transaction = Some(
            self.transaction_coordinator
                .begin(self.invocation.clone())?,
        );
        Ok(())
    }

    pub(crate) fn apply_file_effect(
        &mut self,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, HostError> {
        if self.transaction.is_none() {
            self.transaction = Some(
                self.transaction_coordinator
                    .begin(self.invocation.clone())?,
            );
        }
        let receipt = self
            .transaction
            .as_mut()
            .expect("transaction initialized")
            .apply(request)?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        if let Some(handle) = receipt.after.clone() {
            self.handles.push(handle);
        }
        self.handles.push(receipt.journal.clone());
        Ok(receipt)
    }

    pub(crate) fn create(
        &mut self,
        path: impl Into<PathBuf>,
        content: Vec<u8>,
    ) -> Result<FileEffectReceipt, HostError> {
        self.ledger.bytes_written = self
            .ledger
            .bytes_written
            .saturating_add(content.len() as u64);
        self.apply_file_effect(FileEffectRequest {
            kind: FileEffectKind::Write,
            path: path.into(),
            content: Some(content),
            patch: None,
            expected_preimage: None,
            expect_absent: true,
        })
    }

    pub(crate) fn write(
        &mut self,
        path: impl Into<PathBuf>,
        content: Vec<u8>,
        expected_preimage: Option<ZeroHandle>,
    ) -> Result<FileEffectReceipt, HostError> {
        self.ledger.bytes_written = self
            .ledger
            .bytes_written
            .saturating_add(content.len() as u64);
        self.apply_file_effect(FileEffectRequest {
            kind: FileEffectKind::Write,
            path: path.into(),
            content: Some(content),
            patch: None,
            expected_preimage,
            expect_absent: false,
        })
    }

    pub(crate) fn edit(
        &mut self,
        path: impl Into<PathBuf>,
        postimage: Vec<u8>,
        patch: Option<String>,
        expected_preimage: Option<ZeroHandle>,
    ) -> Result<FileEffectReceipt, HostError> {
        self.ledger.bytes_written = self
            .ledger
            .bytes_written
            .saturating_add(postimage.len() as u64);
        self.apply_file_effect(FileEffectRequest {
            kind: FileEffectKind::Edit,
            path: path.into(),
            content: Some(postimage),
            patch,
            expected_preimage,
            expect_absent: false,
        })
    }

    pub(crate) fn remove(
        &mut self,
        path: impl Into<PathBuf>,
        expected_preimage: Option<ZeroHandle>,
    ) -> Result<FileEffectReceipt, HostError> {
        self.apply_file_effect(FileEffectRequest {
            kind: FileEffectKind::Remove,
            path: path.into(),
            content: None,
            patch: None,
            expected_preimage,
            expect_absent: false,
        })
    }

    fn restore_effects(&self, receipts: &[FileEffectReceipt]) -> Result<(), HostError> {
        let mut failures = Vec::new();
        for receipt in receipts.iter().rev() {
            if let Err(error) = self.files.restore(&self.invocation, receipt) {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(HostError::Transaction(TransactionError::RecoveryRequired(
                failures,
            )))
        }
    }

    pub(crate) fn commit_transaction(&mut self) -> Result<Vec<FileEffectReceipt>, HostError> {
        let transaction = self.transaction.take().ok_or_else(|| {
            HostError::Transaction(TransactionError::Store("no active transaction".into()))
        })?;
        transaction.commit().map_err(Into::into)
    }

    pub(crate) fn rollback_transaction(&mut self) -> Result<(), HostError> {
        let transaction = self.transaction.take().ok_or_else(|| {
            HostError::Transaction(TransactionError::Store("no active transaction".into()))
        })?;
        transaction.rollback().map_err(Into::into)
    }

    pub(crate) fn wait_for_quiescence(&self, timeout: Duration) -> Result<(), HostError> {
        let deadline = Instant::now() + timeout;
        loop {
            let tasks = self.frame_tasks.load(Ordering::Acquire);
            let processes = self.frame_processes.load(Ordering::Acquire);
            if tasks == 0 && processes == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(HostError::InvalidRequest(format!(
                    "frame did not quiesce: tasks={tasks} processes={processes}"
                )));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub(crate) fn direct_context(&self) -> DirectCallContext {
        DirectCallContext {
            files: Arc::clone(&self.files),
            structural: Arc::clone(&self.structural),
            tokens: Arc::clone(&self.tokens),
            cas: self.cas.clone(),
            invocation: self.invocation.clone(),
            records: Arc::clone(&self.async_records),
            live_tasks: Arc::clone(&self.live_tasks),
            live_processes: Arc::clone(&self.live_processes),
            frame_tasks: Arc::clone(&self.frame_tasks),
            frame_processes: Arc::clone(&self.frame_processes),
        }
    }

    fn merge_async_records(&mut self) {
        let mut async_record = self.async_records.lock();
        self.ledger.calls = self.ledger.calls.saturating_add(async_record.calls);
        self.ledger.bytes_read = self
            .ledger
            .bytes_read
            .saturating_add(async_record.bytes_read);
        self.ledger.bytes_visible = self
            .ledger
            .bytes_visible
            .saturating_add(async_record.bytes_visible);
        for handle in async_record.handles.drain(..) {
            if !self.handles.contains(&handle) {
                self.handles.push(handle);
            }
        }
        async_record.calls = 0;
        async_record.bytes_read = 0;
        async_record.bytes_visible = 0;
    }

    /// Bind the guest dispatch trace to this cell. Every trace must carry the
    /// exact cell capsule root and a positive, strictly monotonic occurrence
    /// (and sequence); an empty or mismatched root, a nonpositive occurrence,
    /// or a monotonicity break is rejected outright — nothing is stamped or
    /// repaired after the guest recorded it. A rejected trace is never
    /// rendered into a response or terminal event.
    pub fn record_operations(
        &mut self,
        operations: Vec<ZeroOperationTrace>,
        truncated: bool,
    ) -> Result<(), HostError> {
        let capsule_root = self.capsule_root().to_owned();
        let mut previous_sequence = 0_u64;
        let mut previous_occurrence = 0_u64;
        for operation in &operations {
            if operation.capsule_root.is_empty() {
                return Err(HostError::InvalidRequest(format!(
                    "operation trace {} carries an empty capsule root",
                    operation.sequence
                )));
            }
            if operation.capsule_root != capsule_root {
                return Err(HostError::InvalidRequest(format!(
                    "operation trace {} capsule root {} differs from the cell capsule root {capsule_root}",
                    operation.sequence, operation.capsule_root
                )));
            }
            if operation.occurrence == 0 {
                return Err(HostError::InvalidRequest(format!(
                    "operation trace {} carries a nonpositive occurrence",
                    operation.sequence
                )));
            }
            if operation.sequence <= previous_sequence
                || operation.occurrence <= previous_occurrence
            {
                return Err(HostError::InvalidRequest(format!(
                    "operation trace {} sequence and occurrence must be strictly monotonic",
                    operation.sequence
                )));
            }
            previous_sequence = operation.sequence;
            previous_occurrence = operation.occurrence;
        }
        self.operations = operations;
        self.operations_truncated = truncated;
        Ok(())
    }

    pub fn record_runtime_metrics(&mut self, wall_ns: u64, calls: u64, peak_tasks: u64) {
        self.ledger.wall_ns = self.ledger.wall_ns.max(wall_ns);
        self.ledger.cpu_ns_upper_bound = self.ledger.cpu_ns_upper_bound.max(wall_ns);
        self.ledger.calls = self.ledger.calls.max(calls.min(u64::from(u32::MAX)) as u32);
        self.ledger.tasks = self
            .ledger
            .tasks
            .max(peak_tasks.min(u64::from(u32::MAX)) as u32);
    }

    pub fn context(&self) -> &KernelContext {
        &self.context
    }

    pub fn budget(&self) -> &KernelBudget {
        &self.budget
    }

    pub fn state_values(&self) -> BTreeMap<String, Value> {
        self.state.values.clone()
    }

    pub fn replace_state(&mut self, values: BTreeMap<String, Value>) {
        self.state_dirty |= self.state.values != values;
        self.state.values = values;
    }

    pub fn read_exact(&mut self, path: impl Into<PathBuf>) -> Result<Vec<u8>, HostError> {
        let path = path.into();
        if let Some(bytes) = self.pending_file_bytes(&path)? {
            self.ledger.calls = self.ledger.calls.saturating_add(1);
            self.ledger.bytes_read = self
                .ledger
                .bytes_read
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            return Ok(bytes);
        }
        let snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path,
                options: ReadOptions::default(),
            },
        )?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_read = self.ledger.bytes_read.saturating_add(snapshot.byte_len);
        if !self.handles.contains(&snapshot.content) {
            self.handles.push(snapshot.content.clone());
        }
        self.cas.get(&snapshot.content).map_err(|error| {
            HostError::Engine(EngineError::new(
                EngineErrorKind::Corrupt,
                error.to_string(),
                false,
            ))
        })
    }

    fn pending_file_bytes(&self, path: &Path) -> Result<Option<Vec<u8>>, HostError> {
        let Some(content) = self
            .transaction
            .as_ref()
            .and_then(|transaction| transaction.pending_content(path))
        else {
            return Ok(None);
        };
        match content {
            PendingFileContent::Present(bytes) => Ok(Some(bytes.to_vec())),
            PendingFileContent::Removed => Err(HostError::Engine(EngineError::new(
                EngineErrorKind::NotFound,
                format!("file removed by active transaction: {}", path.display()),
                false,
            ))),
            PendingFileContent::Unavailable => Ok(None),
        }
    }

    pub fn state_get(&self, key: &str) -> Option<&Value> {
        self.state.values.get(key)
    }

    pub fn state_set(&mut self, key: impl Into<String>, value: Value) {
        self.state.values.insert(key.into(), value);
        self.state_dirty = true;
    }

    pub fn state_has(&self, key: &str) -> bool {
        self.state.values.contains_key(key)
    }

    pub fn state_delete(&mut self, key: &str) -> bool {
        let removed = self.state.values.remove(key).is_some();
        self.state_dirty |= removed;
        removed
    }

    pub fn state_list(&self) -> Vec<String> {
        self.state.values.keys().cloned().collect()
    }

    fn dedup_handles(&mut self) {
        let mut seen = BTreeSet::new();
        self.handles
            .retain(|handle| seen.insert(handle.to_string()));
    }

    fn turn_record(&self, outcome: &ZeroKernelOutcome) -> Result<TurnRecord, HostError> {
        let sequence = cell_sequence(&self.invocation.context.cell_id).ok_or_else(|| {
            HostError::Serialization("cell id does not carry a positive sequence".into())
        })?;
        let ledger = serde_json::to_value(&self.ledger)
            .map_err(|error| HostError::Serialization(error.to_string()))?;
        let trace = serde_json::json!({
            "cellId": self.invocation.context.cell_id,
            "sourceDigest": source_digest(&self.source),
            "outcome": outcome,
            "operations": self.operations,
            "operationsTruncated": self.operations_truncated,
        });
        let record = TurnRecord {
            sequence,
            class: self.turn_metadata.class,
            operation_count: u32::try_from(self.operations.len()).unwrap_or(u32::MAX),
            retry_count: self.turn_metadata.retry_count,
            resource_ledger_root: sha256_hex(canonical_json(&ledger).as_bytes()),
            trace_root: sha256_hex(canonical_json(&trace).as_bytes()),
        };
        record.validate().map_err(HostError::Serialization)?;
        Ok(record)
    }
    pub fn fail(mut self, mut error: EngineError) -> Result<ZeroKernelResponse, HostError> {
        self.merge_async_records();
        self.dedup_handles();
        let rolled_back = self
            .transaction
            .as_ref()
            .map(|transaction| transaction.receipts())
            .unwrap_or_default();
        let rollback_state = match self.transaction.take() {
            Some(transaction) => match transaction.rollback() {
                Ok(()) => "rolled_back",
                Err(rollback) => {
                    error = EngineError::new(
                        EngineErrorKind::Corrupt,
                        format!("{}; rollback: {rollback}", error.detail),
                        false,
                    );
                    "rollback_failed"
                }
            },
            None => "no_effects",
        };
        let outcome = if error.kind == EngineErrorKind::Cancelled {
            ZeroKernelOutcome::Cancelled
        } else {
            ZeroKernelOutcome::Failed
        };
        let turn = self.turn_record(&outcome)?;
        let visible = error.detail.clone();
        let visible_digest = blake3::hash(visible.as_bytes()).to_hex().to_string();
        let capsule_object = self.publication().object.clone();
        let event_capsule = event_capsule_roots(
            self.publication(),
            serde_json::json!({
                "kind": "effects",
                "state": rollback_state,
                "receipts": rolled_back.iter().map(effect_fact).collect::<Vec<_>>(),
            }),
            occurrence_manifest(&self.operations),
        );
        let event = ZeroKernelEvent {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            session_id: self.context.session_id.clone(),
            cell_id: self.invocation.context.cell_id.clone(),
            source_digest: source_digest(&self.source),
            contract_digest: self.context.contract_digest.clone(),
            policy_digest: source_digest(b"direct-z"),
            state_root_before: self
                .state
                .root
                .as_ref()
                .map(|handle| handle.as_str().to_owned()),
            state_root_after: self
                .state
                .root
                .as_ref()
                .map(|handle| handle.as_str().to_owned()),
            input_handles: vec![capsule_object],
            output_handles: self.handles.clone(),
            outcome: outcome.clone(),
            ledger: self.ledger.clone(),
            model_visible_digest: visible_digest,
            turn: Some(turn.clone()),
            capsule: Some(event_capsule),
        };
        let publication = self
            .events
            .publish(&event, visible.as_bytes())
            .map_err(|publish| HostError::Event(publish.to_string()))?;
        self.settled = true;
        let response = ZeroKernelResponse {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            outcome,
            value: None,
            error: Some(error),
            operations: self.operations.clone(),
            operations_truncated: self.operations_truncated,
            handles: self.handles.clone(),
            event: publication.event,
            state: StateEvidence {
                before: self
                    .state
                    .root
                    .as_ref()
                    .map(|handle| handle.as_str().to_owned()),
                after: self
                    .state
                    .root
                    .as_ref()
                    .map(|handle| handle.as_str().to_owned()),
                unchanged: true,
            },
            ledger: self.ledger.clone(),
            turn: Some(turn),
            effects: Vec::new(),
        };
        response
            .validate()
            .map_err(|validation| HostError::Serialization(validation.to_string()))?;
        Ok(response)
    }

    fn validate_staged_receipts(&self) -> Result<(), EngineError> {
        let receipts = self
            .transaction
            .as_ref()
            .map(Transaction::receipts)
            .unwrap_or_default();
        for receipt in receipts {
            if receipt.journal.as_str().is_empty() {
                return Err(typed_error(
                    EngineErrorKind::Corrupt,
                    "effect receipt journal is placeholder",
                ));
            }
            let observed = self.files.read(
                &self.invocation,
                FileReadRequest {
                    path: receipt.path.clone(),
                    options: ReadOptions::default(),
                },
            );
            if receipt.kind == FileEffectKind::Remove {
                if receipt.after.is_some() {
                    return Err(typed_error(
                        EngineErrorKind::Corrupt,
                        "remove receipt must not carry an after handle",
                    ));
                }
                match observed {
                    Err(error) if error.kind == EngineErrorKind::NotFound => continue,
                    Ok(_) => {
                        return Err(typed_error(
                            EngineErrorKind::Corrupt,
                            "removed effect target still exists",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
            let expected = receipt.after.as_ref().ok_or_else(|| {
                typed_error(
                    EngineErrorKind::Corrupt,
                    "non-remove receipt is missing its after handle",
                )
            })?;
            let snapshot = observed?;
            if &snapshot.content != expected {
                return Err(typed_error(
                    EngineErrorKind::Corrupt,
                    "effect receipt after handle differs from FileEngine state",
                ));
            }
        }
        Ok(())
    }

    pub fn finish(mut self, value: Value) -> Result<ZeroKernelResponse, HostError> {
        self.merge_async_records();
        self.dedup_handles();
        // Enforce cancellation before any durable commit. A cancelled frame must
        // never commit state or effects; it rolls back and returns Cancelled with
        // state unchanged and no placeholder effect.
        if self.cancellation.is_cancelled() || self.invocation.cancellation.is_cancelled() {
            return self.fail(EngineError::new(
                EngineErrorKind::Cancelled,
                "cancelled before commit",
                false,
            ));
        }
        // Reject binding drift before commit: the effective state root and
        // contract coordinate must still match the sealed binding. Drift is a
        // hard failure with state unchanged, never a silent second authority.
        // Call fail with the transaction still present so fail owns rollback
        // and surfaces any rollback errors.
        let current_state_root = effective_state_root(&self.state);
        if self.binding().state_root != current_state_root
            || self.binding().contract_root != contract_root(&self.context)
        {
            return self.fail(EngineError::new(
                EngineErrorKind::Conflict,
                "binding drifted before commit",
                false,
            ));
        }
        let raw = match serde_json::to_vec(&value) {
            Ok(raw) => raw,
            Err(error) => {
                return self.fail(EngineError::new(
                    EngineErrorKind::Internal,
                    format!("result serialization: {error}"),
                    false,
                ));
            }
        };
        let projection = match self.tokens.project(
            &self.invocation,
            ProjectionRequest {
                bytes: raw,
                visible_byte_limit: self.budget.output_byte_limit,
                media_type: "application/json".into(),
            },
        ) {
            Ok(projection) => projection,
            Err(error) => return self.fail(error),
        };
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_visible = projection.visible.len() as u64;
        if let Some(handle) = projection.exact.clone() {
            self.handles.push(handle);
        }
        let turn = self.turn_record(&ZeroKernelOutcome::Completed)?;
        // Validate staged receipts against the FileEngine authority before
        // committing state or the transaction. Keep the transaction live so
        // fail() remains the single rollback path.
        if let Err(error) = self.validate_staged_receipts() {
            return self.fail(error);
        }
        let before = self.state.root.clone();
        let after = if self.state_dirty {
            match self.state_store.commit(before.as_ref(), &self.state.values) {
                Ok(root) => Some(root),
                Err(error) => {
                    return self.fail(EngineError::new(
                        EngineErrorKind::Corrupt,
                        format!("state commit: {error}"),
                        false,
                    ));
                }
            }
        } else {
            before.clone()
        };
        let committed_effects = match self.transaction.take() {
            Some(transaction) => match transaction.commit() {
                Ok(receipts) => receipts,
                Err(error) => {
                    if after != before {
                        self.state_store
                            .compare_and_set_root(after.as_ref(), before.as_ref())?;
                    }
                    return self.fail(EngineError::new(
                        EngineErrorKind::Corrupt,
                        format!("transaction commit: {error}"),
                        false,
                    ));
                }
            },
            None => Vec::new(),
        };
        self.dedup_handles();
        let capsule_object = self.publication().object.clone();
        let event_capsule = event_capsule_roots(
            self.publication(),
            serde_json::json!({
                "kind": "effects",
                "state": "committed",
                "receipts": committed_effects.iter().map(effect_fact).collect::<Vec<_>>(),
            }),
            occurrence_manifest(&self.operations),
        );
        let visible_bytes = projection.visible.as_bytes();
        let visible_digest = blake3::hash(visible_bytes).to_hex().to_string();
        let event = ZeroKernelEvent {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            session_id: self.context.session_id.clone(),
            cell_id: self.invocation.context.cell_id.clone(),
            source_digest: source_digest(&self.source),
            contract_digest: self.context.contract_digest.clone(),
            policy_digest: source_digest(b"direct-z"),
            state_root_before: before.as_ref().map(|handle| handle.as_str().to_owned()),
            state_root_after: after.as_ref().map(|handle| handle.as_str().to_owned()),
            input_handles: vec![capsule_object],
            output_handles: self.handles.clone(),
            outcome: ZeroKernelOutcome::Completed,
            ledger: self.ledger.clone(),
            model_visible_digest: visible_digest,
            turn: Some(turn.clone()),
            capsule: Some(event_capsule),
        };
        let publication = self
            .events
            .publish(&event, visible_bytes)
            .map_err(|error| {
                // The transaction and state are already durably committed. Reverting
                // bytes here would contradict the committed journal and could destroy
                // a later write. Surface the publication failure without inventing a
                // rollback after the commit authority has settled.
                HostError::Event(format!(
                    "terminal event publication failed after durable commit: {error}"
                ))
            })?;
        self.settled = true;
        // The typed response binds state and receipt to the same committed effect:
        // state.after is the exact root committed with these receipts, and effects
        // are the exact receipts committed in this transaction. No placeholder.
        let response = ZeroKernelResponse {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            outcome: ZeroKernelOutcome::Completed,
            value: Some(Value::String(projection.visible)),
            error: None,
            operations: self.operations.clone(),
            operations_truncated: self.operations_truncated,
            handles: self.handles.clone(),
            event: publication.event,
            state: StateEvidence {
                before: before.as_ref().map(|handle| handle.as_str().to_owned()),
                after: after.as_ref().map(|handle| handle.as_str().to_owned()),
                unchanged: before == after,
            },
            ledger: self.ledger.clone(),
            turn: Some(turn),
            effects: committed_effects.clone(),
        };
        response
            .validate()
            .map_err(|error| HostError::Serialization(error.to_string()))?;
        Ok(response)
    }
    fn project_read_bytes(mut bytes: Vec<u8>, options: &ReadOptions) -> Result<Vec<u8>, HostError> {
        if let Some(range) = options.range.as_deref() {
            let (start, end) = range.split_once(':').ok_or_else(|| {
                HostError::InvalidRequest(
                    "read range must be START:END with inclusive positive line numbers".into(),
                )
            })?;
            let start = start.parse::<usize>().ok().filter(|line| *line > 0);
            let end = end.parse::<usize>().ok().filter(|line| *line > 0);
            let (Some(start), Some(end)) = (start, end) else {
                return Err(HostError::InvalidRequest(
                    "read range must be START:END with inclusive positive line numbers".into(),
                ));
            };
            if end < start {
                return Err(HostError::InvalidRequest(
                    "read range end must be greater than or equal to start".into(),
                ));
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                HostError::InvalidRequest("line ranges require a UTF-8 file".into())
            })?;
            let lines = text.split_inclusive('\n').collect::<Vec<_>>();
            if start > lines.len() {
                return Err(HostError::InvalidRequest(format!(
                    "read range start {start} exceeds file line count {}",
                    lines.len()
                )));
            }
            bytes = lines[start - 1..end.min(lines.len())].concat().into_bytes();
        }
        if let Some(limit) = options.max_bytes {
            bytes.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        Ok(bytes)
    }
}

fn cell_sequence(cell_id: &str) -> Option<u64> {
    cell_id
        .strip_prefix("cell-")?
        .parse()
        .ok()
        .filter(|sequence| *sequence > 0)
}

impl Drop for Cell {
    fn drop(&mut self) {
        self.live_frames.fetch_sub(1, Ordering::AcqRel);
    }
}

fn source_digest(source: impl AsRef<[u8]>) -> String {
    blake3::hash(source.as_ref()).to_hex().to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Canonical coordinate root: SHA256 over the canonical JSON encoding of a
/// typed manifest. Coordinates that are not directly derivable content
/// (unmeasured, empty, or environmental facts) bind this way, so no root is
/// ever an all-zero or sentinel placeholder.
fn coordinate_root(manifest: serde_json::Value) -> String {
    sha256_hex(canonical_json(&manifest).as_bytes())
}

/// The effective state root a cell launches under: the loaded durable root
/// when state exists, otherwise the canonical manifest for an explicitly
/// empty state.
fn effective_state_root(state: &StateSnapshot) -> String {
    state
        .root
        .as_ref()
        .map(|handle| handle.digest().to_owned())
        .unwrap_or_else(|| coordinate_root(serde_json::json!({"kind": "state", "state": "empty"})))
}

/// The canonical contract coordinate: the direct contract digest bound into
/// a typed manifest so any digest form yields a canonical root.
fn contract_root(context: &KernelContext) -> String {
    coordinate_root(serde_json::json!({
        "kind": "contract",
        "digest": context.contract_digest,
    }))
}

/// Real current capsule coordinates for a fresh cell: the project and
/// protected scope bind the canonical workspace/project facts, task binds
/// the finalized source digest, policy binds the direct contract, execution
/// binds the complete enforced budget, and the explicitly empty or
/// unmeasured planes (obligations, evidence, verifier, ledger) bind typed
/// manifests.
fn capsule_roots(
    context: &KernelContext,
    budget: &KernelBudget,
    source: &str,
) -> Result<CapsuleRoots, HostError> {
    let project = coordinate_root(serde_json::json!({
        "kind": "project",
        "workspaceRoot": context.workspace_root.to_string_lossy(),
        "projectRoot": context.project_root.to_string_lossy(),
        "sessionId": &context.session_id,
    }));
    let protected_scope = coordinate_root(serde_json::json!({
        "kind": "protected_scope",
        "workspaceRoot": context.workspace_root.to_string_lossy(),
        "projectRoot": context.project_root.to_string_lossy(),
    }));
    let policy = coordinate_root(serde_json::json!({
        "kind": "policy",
        "direct": "direct-z",
        "contractDigest": &context.contract_digest,
    }));
    let execution = execution_root(budget);
    let fallback = coordinate_root(serde_json::json!({
        "kind": "fallback",
        "engine": "direct-z",
    }));
    Ok(CapsuleRoots {
        project,
        task: sha256_hex(source.as_bytes()),
        protected_scope,
        obligations: coordinate_root(serde_json::json!({
            "kind": "obligations",
            "state": "unmeasured",
        })),
        evidence: coordinate_root(serde_json::json!({
            "kind": "evidence",
            "state": "unmeasured",
        })),
        policy,
        execution,
        verifier: coordinate_root(serde_json::json!({
            "kind": "verifier",
            "state": "unmeasured",
        })),
        fallback,
        ledger: coordinate_root(serde_json::json!({
            "kind": "ledger",
            "state": "empty",
        })),
    })
}

fn execution_root(budget: &KernelBudget) -> String {
    coordinate_root(serde_json::json!({
        "kind": "execution",
        "budget": {
            "wallMs": budget.wall_ms,
            "cpuMs": budget.cpu_ms,
            "memoryBytes": budget.memory_bytes,
            "callLimit": budget.call_limit,
            "taskLimit": budget.task_limit,
            "outputByteLimit": budget.output_byte_limit,
        },
    }))
}

/// Canonical receipt facts for the terminal effect root: the actual
/// committed or rolled-back receipt coordinates, never a placeholder.
fn effect_fact(receipt: &FileEffectReceipt) -> serde_json::Value {
    serde_json::json!({
        "kind": match receipt.kind {
            FileEffectKind::Write => "write",
            FileEffectKind::Edit => "edit",
            FileEffectKind::Remove => "remove",
            FileEffectKind::Restore => "restore",
        },
        "path": receipt.path.to_string_lossy(),
        "before": receipt.before.as_ref().map(|handle| handle.as_str()),
        "after": receipt.after.as_ref().map(|handle| handle.as_str()),
        "journal": receipt.journal.as_str(),
    })
}

/// The terminal capsule tuple every new event carries: the sealed capsule
/// root and object, explicit unmeasured provider/cache/quality coordinates,
/// an explicit ordinary speculation coordinate, the actual effect facts, and
/// the actual operation trace vector.
fn event_capsule_roots(
    publication: &CapsulePublication,
    effect_manifest: serde_json::Value,
    occurrence_manifest: serde_json::Value,
) -> CapsuleEventRoots {
    CapsuleEventRoots {
        capsule_root: publication.capsule_root.clone(),
        capsule_object: publication.object.clone(),
        provider_root: coordinate_root(serde_json::json!({
            "kind": "provider_usage",
            "state": "unmeasured",
        })),
        cache_root: coordinate_root(serde_json::json!({
            "kind": "cache",
            "state": "unmeasured",
        })),
        speculation_root: coordinate_root(serde_json::json!({
            "kind": "speculation",
            "mode": "ordinary",
            "claims": 0,
        })),
        effect_root: coordinate_root(effect_manifest),
        quality_root: coordinate_root(serde_json::json!({
            "kind": "quality",
            "state": "unmeasured",
        })),
        occurrence_root: coordinate_root(occurrence_manifest),
    }
}

/// Canonical occurrence facts for the terminal occurrence root: the actual
/// trace vector as recorded by the guest, or an explicitly empty vector.
fn occurrence_manifest(operations: &[ZeroOperationTrace]) -> serde_json::Value {
    serde_json::json!({
        "kind": "occurrences",
        "trace": operations,
    })
}

struct PlannedFileEffect {
    path: PathBuf,
    before: Option<FileSnapshot>,
    postimage: Option<Vec<u8>>,
    changed: bool,
    sealed: bool,
    remove: bool,
}

fn plan_effect_change(
    target: &mut PlannedFileEffect,
    change: EffectChangeRequest,
) -> Result<(), HostError> {
    if target.sealed {
        return Err(HostError::InvalidRequest(
            "z.apply cannot apply another change after replace_file or remove_file".into(),
        ));
    }
    match change.kind {
        EffectChangeKind::CreateFile => {
            if target.before.is_some() || target.postimage.is_some() {
                return Err(HostError::InvalidRequest(
                    "create_file requires a target observed absent".into(),
                ));
            }
            target.postimage = Some(required_effect_content(change.content, "create_file")?);
        }
        EffectChangeKind::RemoveFile => {
            if target.before.is_none() || target.changed {
                return Err(HostError::InvalidRequest(
                    "remove_file requires one unchanged existing target".into(),
                ));
            }
            target.postimage = None;
            target.sealed = true;
            target.remove = true;
        }
        EffectChangeKind::ReplaceFile => {
            if target.postimage.is_none() {
                return Err(HostError::InvalidRequest(
                    "replace_file requires an existing or previously created target".into(),
                ));
            }
            if target.changed {
                return Err(HostError::InvalidRequest(
                    "replace_file must be the target's only change".into(),
                ));
            }
            target.postimage = Some(required_effect_content(change.content, "replace_file")?);
            target.sealed = true;
        }
        EffectChangeKind::ReplaceExact => {
            if change.expected_count != Some(1) {
                return Err(HostError::InvalidRequest(
                    "replace_exact requires explicit expectedCount 1".into(),
                ));
            }
            let old = change
                .old
                .ok_or_else(|| HostError::InvalidRequest("replace_exact requires old".into()))?;
            let replacement = change.replacement.ok_or_else(|| {
                HostError::InvalidRequest("replace_exact requires replacement".into())
            })?;
            let mut text = target_text(target)?;
            let (start, end) = exactly_one_span(&text, &old, "replace_exact")?;
            text.replace_range(start..end, &replacement);
            target.postimage = Some(text.into_bytes());
        }
        EffectChangeKind::InsertBefore | EffectChangeKind::InsertAfter => {
            let anchor = change
                .anchor
                .map(|anchor| anchor.exact_text)
                .ok_or_else(|| {
                    HostError::InvalidRequest(
                        "insert_before/insert_after requires anchor.exactText".into(),
                    )
                })?;
            let mut content = change.content.ok_or_else(|| {
                HostError::InvalidRequest("insert_before/insert_after requires content".into())
            })?;
            let mut text = target_text(target)?;
            let (start, end) = exactly_one_span(&text, &anchor, "insert anchor")?;
            let offset = if matches!(change.kind, EffectChangeKind::InsertBefore) {
                start
            } else {
                if text[end..].starts_with("\r\n")
                    && !content.starts_with('\r')
                    && !content.starts_with('\n')
                {
                    content.insert_str(0, "\r\n");
                } else if text[end..].starts_with('\n') && !content.starts_with('\n') {
                    content.insert(0, '\n');
                }
                end
            };
            text.insert_str(offset, &content);
            target.postimage = Some(text.into_bytes());
        }
    }
    target.changed = true;
    Ok(())
}

fn required_effect_content(content: Option<String>, kind: &str) -> Result<Vec<u8>, HostError> {
    content
        .map(String::into_bytes)
        .ok_or_else(|| HostError::InvalidRequest(format!("{kind} requires content")))
}
fn target_text(target: &mut PlannedFileEffect) -> Result<String, HostError> {
    String::from_utf8(
        target
            .postimage
            .take()
            .ok_or_else(|| HostError::InvalidRequest("effect target has no postimage".into()))?,
    )
    .map_err(|_| HostError::InvalidRequest("text effect requires UTF-8 source".into()))
}

fn exactly_one_span(text: &str, needle: &str, label: &str) -> Result<(usize, usize), HostError> {
    if needle.is_empty() {
        return Err(HostError::InvalidRequest(format!(
            "{label} must not be empty"
        )));
    }
    let mut matches = text.match_indices(needle);
    let first = matches
        .next()
        .ok_or_else(|| HostError::InvalidRequest(format!("{label} did not match")))?;
    if matches.next().is_some() {
        return Err(HostError::InvalidRequest(format!("{label} is ambiguous")));
    }
    Ok((first.0, first.0 + needle.len()))
}

fn exact_structural_hit(
    result: zero_abi::StructuralResult,
) -> Result<(SnapStructuralEvidence, (u32, u32)), HostError> {
    if !result.complete {
        let detail = result
            .diagnostic
            .as_deref()
            .unwrap_or("structural coverage is partial or stale; absence is not certified");
        return Err(HostError::Engine(EngineError::new(
            EngineErrorKind::InvalidInput,
            format!("structured z.read result is incomplete: {detail}"),
            false,
        )));
    }
    if result.hits.len() != 1 {
        let detail = if result.hits.is_empty() {
            "structured z.read target was not found".into()
        } else {
            let candidates = result
                .hits
                .iter()
                .take(8)
                .map(|hit| match (hit.line_start, hit.line_end) {
                    (Some(start), Some(end)) => {
                        format!("{}:{start}-{end}", hit.path.display())
                    }
                    _ => hit.path.display().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "structured z.read target is ambiguous: {} candidates ({candidates})",
                result.hits.len()
            )
        };
        return Err(HostError::Engine(EngineError::new(
            if result.hits.is_empty() {
                EngineErrorKind::NotFound
            } else {
                EngineErrorKind::Conflict
            },
            detail,
            false,
        )));
    }
    let hit = &result.hits[0];
    let lines = hit
        .line_start
        .zip(hit.line_end)
        .ok_or_else(|| HostError::InvalidRequest("z.read hit has no exact line span".into()))?;
    let source = hit
        .source
        .clone()
        .ok_or_else(|| HostError::InvalidRequest("z.read hit has no exact source handle".into()))?;
    let evidence = hit.evidence.clone();
    Ok((
        SnapStructuralEvidence {
            index_digest: result.index_digest,
            complete: true,
            source,
            evidence,
        },
        lines,
    ))
}

fn snap_selection(
    cas: &ZeroCas,
    source: &ZeroHandle,
    bytes: &[u8],
    request: Option<&SnapSelectionRequest>,
    derived_lines: Option<(u32, u32)>,
) -> Result<Option<SnapSelection>, HostError> {
    let Some(request) = request else {
        return derived_lines
            .map(|(start, end)| selection_from_expansion(cas, source, "lines", start, end, None))
            .transpose();
    };
    let count = usize::from(request.lines.is_some())
        + usize::from(request.bytes.is_some())
        + usize::from(request.symbol.is_some())
        + usize::from(request.exact_text.is_some());
    if count != 1 {
        return Err(HostError::InvalidRequest(
            "z.read selection requires exactly one of lines, bytes, symbol, or exactText".into(),
        ));
    }
    if let Some(lines) = &request.lines {
        return selection_from_expansion(cas, source, "lines", lines.start, lines.end, None)
            .map(Some);
    }
    if let Some(range) = &request.bytes {
        let start = usize::try_from(range.start)
            .map_err(|_| HostError::InvalidRequest("byte start does not fit platform".into()))?;
        let end = usize::try_from(range.end)
            .map_err(|_| HostError::InvalidRequest("byte end does not fit platform".into()))?;
        if start >= end || end > bytes.len() {
            return Err(HostError::InvalidRequest(
                "z.read byte selection is outside the source".into(),
            ));
        }
        return Ok(Some(SnapSelection {
            kind: "bytes".into(),
            line_start: None,
            line_end: None,
            byte_start: range.start,
            byte_end: range.end,
            selected_digest: blake3::hash(&bytes[start..end]).to_hex().to_string(),
        }));
    }
    if let Some(exact) = &request.exact_text {
        if exact.is_empty() {
            return Err(HostError::InvalidRequest(
                "z.read exactText must not be empty".into(),
            ));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| HostError::InvalidRequest("exactText requires UTF-8 source".into()))?;
        let mut matches = text.match_indices(exact);
        let first = matches
            .next()
            .ok_or_else(|| HostError::InvalidRequest("z.read exactText was not found".into()))?;
        if matches.next().is_some() {
            return Err(HostError::InvalidRequest(
                "z.read exactText is ambiguous".into(),
            ));
        }
        let end = first.0 + exact.len();
        return Ok(Some(SnapSelection {
            kind: "exact_text".into(),
            line_start: None,
            line_end: None,
            byte_start: first.0 as u64,
            byte_end: end as u64,
            selected_digest: blake3::hash(&bytes[first.0..end]).to_hex().to_string(),
        }));
    }
    let (start, end) = derived_lines.ok_or_else(|| {
        HostError::InvalidRequest(
            "z.read symbol selection requires structural discovery evidence".into(),
        )
    })?;
    selection_from_expansion(
        cas,
        source,
        request.symbol.as_deref().unwrap_or("symbol"),
        start,
        end,
        Some("symbol"),
    )
    .map(Some)
}

fn selection_from_expansion(
    cas: &ZeroCas,
    source: &ZeroHandle,
    kind: &str,
    line_start: u32,
    line_end: u32,
    kind_override: Option<&str>,
) -> Result<SnapSelection, HostError> {
    let expanded = cas
        .expand_with_range(
            source,
            &ExpandOptions {
                line_start: Some(line_start),
                line_end: Some(line_end),
                ..ExpandOptions::default()
            },
        )
        .map_err(cas_host_error)?;
    Ok(SnapSelection {
        kind: kind_override.unwrap_or(kind).into(),
        line_start: Some(line_start),
        line_end: Some(line_end),
        byte_start: expanded.byte_start,
        byte_end: expanded.byte_end,
        selected_digest: blake3::hash(&expanded.bytes).to_hex().to_string(),
    })
}

fn store_recovery_manifest(
    cas: &ZeroCas,
    path: &PathBuf,
    source: &SnapSource,
    selection: &Option<SnapSelection>,
    structural: &Option<SnapStructuralEvidence>,
    view: Option<&SnapView>,
) -> Result<ZeroHandle, HostError> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RecoveryManifest<'a> {
        schema: &'static str,
        path: &'a PathBuf,
        source: &'a SnapSource,
        selection: &'a Option<SnapSelection>,
        structural: &'a Option<SnapStructuralEvidence>,
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<&'a SnapView>,
    }
    let bytes = serde_json::to_vec(&RecoveryManifest {
        schema: "zerostack.snap.recovery",
        path,
        source,
        selection,
        structural,
        view,
    })
    .map_err(|error| HostError::Serialization(error.to_string()))?;
    cas.put(&bytes).map_err(cas_host_error)
}

/// Large reads return a structural outline plus an exact handle. The outline
/// MUST be impossible to mistake for file content (pc_821d2acdacfc): every
/// outline read carries a machine-greppable header naming the path, the true
/// byte length, and the exact-content handle for expansion.
fn labeled_outline(path: &std::path::Path, snapshot: &FileSnapshot) -> String {
    let outline = snapshot
        .outline
        .clone()
        .unwrap_or_else(|| format!("{} bytes", snapshot.byte_len));
    format!(
        "[ZeroStack READ OUTLINE - not file content | path={} | {} bytes total | exact={} | recover with z.read(\"{}\", selectors)]\n{}\n",
        path.display(),
        snapshot.byte_len,
        snapshot.content,
        snapshot.content,
        outline
    )
}

fn source_newline(source: &[u8]) -> SnapNewline {
    let mut lf = false;
    let mut crlf = false;
    let mut bare_cr = false;
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            b'\r' if source.get(index + 1) == Some(&b'\n') => {
                crlf = true;
                index += 2;
            }
            b'\r' => {
                bare_cr = true;
                index += 1;
            }
            b'\n' => {
                lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }
    match (lf, crlf, bare_cr) {
        (false, false, false) => SnapNewline::None,
        (true, false, false) => SnapNewline::Lf,
        (false, true, false) => SnapNewline::Crlf,
        _ => SnapNewline::Mixed,
    }
}

fn source_line_count(source: &str) -> u64 {
    if source.is_empty() {
        0
    } else {
        source.lines().count() as u64
    }
}

fn push_handle(handles: &mut Vec<ZeroHandle>, handle: ZeroHandle) {
    if !handles.contains(&handle) {
        handles.push(handle);
    }
}

/// The canonical fail-closed lens result: `Unknown` with one reason, no
/// locus, no impact closure, no evidence, and no index claim.
fn task_lens_unknown(reason: &str) -> TaskLensResult {
    TaskLensResult {
        verdict: SafetyVerdict::Unknown {
            reasons: vec![reason.to_owned()],
        },
        locus: None,
        impact: TaskLensCompilerImpact {
            complete: false,
            edge_roots: Vec::new(),
            reverse_roots: Vec::new(),
        },
        proof_support: Vec::new(),
        evidence_roots: Vec::new(),
        coverage: None,
        index_digest: String::new(),
        reasons: vec![reason.to_owned()],
    }
}

/// Canonical snake_case reason for a task-lens contract violation.
fn task_lens_reason(error: &TaskLensError) -> String {
    match error {
        TaskLensError::EmptyQuery => "empty_query".into(),
        TaskLensError::InvalidRequestedRoot(root) => {
            format!("invalid_requested_root:{root}")
        }
        TaskLensError::UnnormalizedReasons => "unnormalized_reasons".into(),
        TaskLensError::MissingLocus => "missing_locus".into(),
        TaskLensError::UnrootedLocus => "unrooted_locus".into(),
        TaskLensError::IncompleteImpact => "incomplete_impact".into(),
        TaskLensError::MissingProofSupport => "missing_proof_support".into(),
        TaskLensError::MissingCoverage => "missing_coverage".into(),
        TaskLensError::StaleCoverage => "stale_coverage".into(),
        TaskLensError::IncompleteCoverage => "incomplete_coverage".into(),
        TaskLensError::MissingEvidenceRoot(root) => {
            format!("missing_evidence_root:{root}")
        }
        TaskLensError::MalformedLocusRoot(root) => {
            format!("malformed_locus_root:{root}")
        }
        TaskLensError::MalformedImpactRoot(root) => {
            format!("malformed_impact_root:{root}")
        }
        TaskLensError::MalformedProofRoot(root) => {
            format!("malformed_proof_root:{root}")
        }
        TaskLensError::MalformedEvidenceRoot(root) => {
            format!("malformed_evidence_root:{root}")
        }
        TaskLensError::MalformedIndexDigest => "malformed_index_digest".into(),
        TaskLensError::SafeWithReasons => "safe_with_reasons".into(),
        TaskLensError::UnsafeWithoutReasons => "unsafe_without_reasons".into(),
        TaskLensError::ReasonMismatch => "reason_mismatch".into(),
    }
}

/// Every content handle a lens result binds: the locus anchors plus the
/// impact, proof, and evidence root sets.
fn task_lens_handles(result: &TaskLensResult) -> Vec<ZeroHandle> {
    let mut handles = Vec::new();
    if let Some(locus) = &result.locus {
        if let Some(handle) = &locus.evidence {
            handles.push(handle.clone());
        }
        if let Some(handle) = &locus.source {
            handles.push(handle.clone());
        }
    }
    handles.extend(result.evidence_roots.iter().cloned());
    handles.extend(result.proof_support.iter().cloned());
    handles.extend(result.impact.edge_roots.iter().cloned());
    handles.extend(result.impact.reverse_roots.iter().cloned());
    handles
}

fn cas_host_error(error: impl std::fmt::Display) -> HostError {
    HostError::Engine(EngineError::new(
        EngineErrorKind::Corrupt,
        error.to_string(),
        false,
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn expand_handle(
    cas: &ZeroCas,
    tokens: &dyn TokenEngine,
    invocation: &EngineInvocation,
    handle: &ZeroHandle,
    options: ExpandOptions,
) -> Result<ExpandResult, HostError> {
    let (bytes, byte_start, byte_end, byte_length) = match cas.expand_with_range(handle, &options) {
        Ok(expanded) => (
            expanded.bytes,
            expanded.byte_start,
            expanded.byte_end,
            expanded.byte_length,
        ),
        Err(_) => {
            let bytes = tokens.expand(invocation, handle, options)?;
            let length = bytes.len() as u64;
            (bytes, 0, length, length)
        }
    };
    let accounting = tokens.measure(invocation, &bytes)?;
    let exact_digest = blake3::hash(&bytes).to_hex().to_string();
    let (text, encoded_bytes, encoding) = match String::from_utf8(bytes) {
        Ok(text) => (Some(text), None, "utf8".to_owned()),
        Err(error) => (None, Some(hex_encode(error.as_bytes())), "hex".to_owned()),
    };
    let complete = byte_start == 0 && byte_end == byte_length;
    Ok(ExpandResult {
        schema: EXPAND_RESULT_SCHEMA.into(),
        source: handle.clone(),
        text,
        bytes: encoded_bytes,
        encoding,
        byte_start,
        byte_end,
        byte_length,
        exact_digest,
        complete,
        recovered_tokens: accounting.visible,
        accounting,
        next: (!complete && byte_end < byte_length).then_some(byte_end),
    })
}
pub fn typed_error(kind: EngineErrorKind, detail: impl Into<String>) -> EngineError {
    EngineError::new(kind, detail, false)
}

#[allow(dead_code)]
fn _empty_state() -> BTreeMap<String, Value> {
    BTreeMap::new()
}
#[cfg(test)]
mod capsule_launch_tests {
    use super::*;
    use zero_abi::{
        CertifyResult, CompressionRequest, CompressionResult, FileLease, ProjectionRequest,
        ProjectionResult, StructuralResult, TokenAccounting,
    };

    struct CapsuleOnlyLease;
    impl FileLease for CapsuleOnlyLease {}

    /// FileEngine whose only real surface is capsule storage; the sealed
    /// source check fires before any other engine interaction.
    struct CapsuleOnlyFiles(Mutex<BTreeMap<String, WorkCapsule>>);

    impl FileEngine for CapsuleOnlyFiles {
        fn lease(&self, _invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
            Ok(Box::new(CapsuleOnlyLease))
        }

        fn read(
            &self,
            _invocation: &EngineInvocation,
            _request: FileReadRequest,
        ) -> Result<FileSnapshot, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::NotFound,
                "no files",
                false,
            ))
        }

        fn lookup(
            &self,
            _invocation: &EngineInvocation,
            _root: PathBuf,
            _options: LookupOptions,
        ) -> Result<Vec<PathBuf>, EngineError> {
            Ok(Vec::new())
        }

        fn apply(
            &self,
            _invocation: &EngineInvocation,
            _request: FileEffectRequest,
        ) -> Result<FileEffectReceipt, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Internal,
                "no effects",
                false,
            ))
        }

        fn restore(
            &self,
            _invocation: &EngineInvocation,
            _receipt: &FileEffectReceipt,
        ) -> Result<(), EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Internal,
                "no restore",
                false,
            ))
        }

        fn reconcile(
            &self,
            _invocation: &EngineInvocation,
        ) -> Result<Vec<ZeroHandle>, EngineError> {
            Ok(Vec::new())
        }

        fn put_capsule(
            &self,
            _invocation: &EngineInvocation,
            capsule: &WorkCapsule,
        ) -> Result<CapsulePublication, EngineError> {
            let capsule_root = capsule
                .root()
                .map_err(|detail| EngineError::new(EngineErrorKind::InvalidInput, detail, false))?;
            let value = serde_json::to_value(capsule).map_err(|error| {
                EngineError::new(EngineErrorKind::InvalidInput, error.to_string(), false)
            })?;
            let object_digest = sha256_hex(canonical_json(&value).as_bytes());
            let object = ZeroHandle::from_digest(&object_digest).map_err(|error| {
                EngineError::new(EngineErrorKind::InvalidInput, error.to_string(), false)
            })?;
            self.0.lock().insert(object_digest, capsule.clone());
            Ok(CapsulePublication {
                capsule_root,
                object,
                created: true,
            })
        }

        fn get_capsule(
            &self,
            _invocation: &EngineInvocation,
            publication: &CapsulePublication,
        ) -> Result<WorkCapsule, EngineError> {
            let capsule = self
                .0
                .lock()
                .get(publication.object.digest())
                .cloned()
                .ok_or_else(|| {
                    EngineError::new(
                        EngineErrorKind::NotFound,
                        "capsule object is not published",
                        false,
                    )
                })?;
            let actual = capsule
                .root()
                .map_err(|detail| EngineError::new(EngineErrorKind::InvalidInput, detail, false))?;
            if actual != publication.capsule_root {
                return Err(EngineError::new(
                    EngineErrorKind::Corrupt,
                    "capsule root does not match its publication",
                    false,
                ));
            }
            Ok(capsule)
        }
    }

    struct NoGraph;
    impl StructuralEngine for NoGraph {
        fn query(
            &self,
            _invocation: &EngineInvocation,
            _query: StructuralQuery,
        ) -> Result<StructuralResult, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "no graph",
                false,
            ))
        }
    }

    struct NoTokens;
    impl TokenEngine for NoTokens {
        fn measure(
            &self,
            _invocation: &EngineInvocation,
            _bytes: &[u8],
        ) -> Result<TokenAccounting, EngineError> {
            Ok(TokenAccounting {
                tokenizer: "bytes".into(),
                billed: 0,
                visible: 0,
                cached: 0,
                certified: false,
            })
        }

        fn certify(
            &self,
            _invocation: &EngineInvocation,
            _bytes: &[u8],
            _claimed: &TokenAccounting,
        ) -> Result<CertifyResult, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "no certify",
                false,
            ))
        }

        fn project(
            &self,
            _invocation: &EngineInvocation,
            _request: ProjectionRequest,
        ) -> Result<ProjectionResult, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "no project",
                false,
            ))
        }

        fn compress(
            &self,
            _invocation: &EngineInvocation,
            _request: CompressionRequest,
        ) -> Result<CompressionResult, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "no compress",
                false,
            ))
        }

        fn expand(
            &self,
            _invocation: &EngineInvocation,
            _handle: &ZeroHandle,
            _options: ExpandOptions,
        ) -> Result<Vec<u8>, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Unsupported,
                "no expand",
                false,
            ))
        }
    }

    fn sealed_kernel_and_prepared(
        root: &tempfile::TempDir,
    ) -> (ZeroKernel, crate::PreparedCell, String) {
        let kernel = ZeroKernel::new(
            KernelContext {
                workspace_root: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                session_id: "capsule-launch".into(),
                expected_state_root: None,
                contract_digest: "contract".into(),
            },
            KernelBudget {
                wall_ms: 1_000,
                cpu_ms: 1_000,
                memory_bytes: 8 * 1024 * 1024,
                call_limit: 16,
                task_limit: 4,
                output_byte_limit: 16 * 1024,
            },
            Arc::new(CapsuleOnlyFiles(Mutex::new(BTreeMap::new()))),
            Arc::new(NoGraph),
            Arc::new(NoTokens),
            root.path().join(".zerostack"),
        )
        .unwrap();
        let sealed_source = "return 'sealed';".to_string();
        let probe = kernel.begin_cell(&sealed_source).unwrap();
        let mut preparation = CellPreparation::new();
        preparation.feed(&sealed_source).unwrap();
        let sealed = preparation
            .finish(
                probe.binding().clone(),
                probe.capsule().clone(),
                probe.publication().clone(),
            )
            .unwrap();
        drop(probe);
        (kernel, sealed, sealed_source)
    }

    #[test]
    fn begin_from_request_rejects_sealed_source_drift() {
        let root = tempfile::tempdir().unwrap();
        let (kernel, sealed, sealed_source) = sealed_kernel_and_prepared(&root);
        let drifted = ZeroKernelRequest::new(
            "return 'other';".into(),
            kernel.context.clone(),
            kernel.budget.clone(),
        )
        .unwrap();
        let error = kernel
            .begin_from_request(drifted, AtomicCancellation::new(), Some(&sealed))
            .expect_err("sealed launch must reject source drift");
        assert!(matches!(error, HostError::InvalidRequest(_)));
        assert_eq!(kernel.live_frames(), 0);
        assert_eq!(kernel.live_tasks(), 0);
        assert_eq!(kernel.live_processes(), 0);
        let valid = ZeroKernelRequest::new(
            sealed_source.into(),
            kernel.context.clone(),
            kernel.budget.clone(),
        )
        .unwrap();
        let cell = kernel
            .begin_from_request(valid, AtomicCancellation::new(), Some(&sealed))
            .expect("valid sealed launch must succeed after source rejection");
        assert_eq!(kernel.live_frames(), 1);
        drop(cell);
        assert_eq!(kernel.live_frames(), 0);
    }

    #[test]
    fn begin_from_request_rejects_sealed_budget_drift() {
        let root = tempfile::tempdir().unwrap();
        let (kernel, sealed, sealed_source) = sealed_kernel_and_prepared(&root);
        let mut changed_budget = kernel.budget.clone();
        changed_budget.cpu_ms += 1;
        let budget_drifted = ZeroKernelRequest::new(
            sealed_source.clone().into(),
            kernel.context.clone(),
            changed_budget,
        )
        .unwrap();
        let error = kernel
            .begin_from_request(budget_drifted, AtomicCancellation::new(), Some(&sealed))
            .expect_err("sealed launch must reject budget drift");
        assert!(matches!(error, HostError::InvalidRequest(_)));
        assert_eq!(kernel.live_frames(), 0);
        assert_eq!(kernel.live_tasks(), 0);
        assert_eq!(kernel.live_processes(), 0);
        let valid = ZeroKernelRequest::new(
            sealed_source.into(),
            kernel.context.clone(),
            kernel.budget.clone(),
        )
        .unwrap();
        let cell = kernel
            .begin_from_request(valid, AtomicCancellation::new(), Some(&sealed))
            .expect("valid sealed launch must succeed after budget rejection");
        assert_eq!(kernel.live_frames(), 1);
        drop(cell);
        assert_eq!(kernel.live_frames(), 0);
    }
}
