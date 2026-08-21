use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::Value;
use zero_abi::{
    AsgrepMode, AsgrepOptions, CompressionRequest, CompressionResult, EFFECT_RESULT_SCHEMA,
    EXPAND_RESULT_SCHEMA, EffectChangeKind, EffectChangeRequest, EffectRequest, EffectResult,
    EffectTargetResult, EffectVerificationResult, EngineCallContext, EngineError, EngineErrorKind,
    EngineInvocation, ExpandOptions, ExpandResult, FileEffectKind, FileEffectReceipt,
    FileEffectRequest, FileEngine, FileReadRequest, FileSnapshot, KernelBudget, KernelContext,
    KernelLedger, LookupOptions, ProjectionRequest, ProjectionResult, ReadOptions,
    SNAP_WORKSPACE_SCHEMA, ShellOptions, ShellResult, SnapAccounting, SnapByteRange, SnapNewline,
    SnapRecovery, SnapRequest, SnapResult, SnapSelection, SnapSelectionRequest, SnapSource,
    SnapStructuralEvidence, SnapTargetRequest, SnapView, SnapViewMode, StateEvidence,
    StructuralEngine, StructuralQuery, TokenAccounting, TokenEngine, ZERO_KERNEL_PROTOCOL,
    ZeroHandle, ZeroKernelEvent, ZeroKernelOutcome, ZeroKernelRequest, ZeroKernelResponse,
    ZeroOperationTrace,
};
use zero_store::{EventLog, ZeroCas};

use crate::shell::{ShellCommand, run_shell};
use crate::state::{StateError, StateSnapshot, StateStore};
use crate::transaction::{Transaction, TransactionCoordinator, TransactionError};

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
    output_byte_limit: u32,
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
    pub(crate) fn output_byte_limit(&self) -> u32 {
        self.output_byte_limit
    }

    pub fn read(&self, path: PathBuf, options: ReadOptions) -> Result<String, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let snapshot = self
            .files
            .read(&self.invocation, FileReadRequest { path, options })?;
        let value = match snapshot.inline_utf8 {
            Some(text) => text,
            None => format!(
                "{}\nexact: {}",
                snapshot
                    .outline
                    .unwrap_or_else(|| format!("{} bytes", snapshot.byte_len)),
                snapshot.content
            ),
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

    pub fn measure(&self, bytes: Vec<u8>) -> Result<TokenAccounting, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let result = self.tokens.measure(&self.invocation, &bytes)?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_read = record.bytes_read.saturating_add(bytes.len() as u64);
        Ok(result)
    }

    pub fn project(&self, request: ProjectionRequest) -> Result<ProjectionResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let input_bytes = request.bytes.len() as u64;
        let result = self.tokens.project(&self.invocation, request)?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_read = record.bytes_read.saturating_add(input_bytes);
        record.bytes_visible = record
            .bytes_visible
            .saturating_add(result.visible.len() as u64);
        if let Some(handle) = result.exact.clone()
            && !record.handles.contains(&handle)
        {
            record.handles.push(handle);
        }
        Ok(result)
    }

    pub fn compress(&self, request: CompressionRequest) -> Result<CompressionResult, HostError> {
        let _task =
            LiveTaskGuard::acquire(Arc::clone(&self.live_tasks), Arc::clone(&self.frame_tasks));
        let input_bytes = request.bytes.len() as u64;
        let result = self.tokens.compress(&self.invocation, request)?;
        let mut record = self.records.lock();
        record.calls = record.calls.saturating_add(1);
        record.bytes_read = record.bytes_read.saturating_add(input_bytes);
        record.bytes_visible = record
            .bytes_visible
            .saturating_add(result.visible.len() as u64);
        if !record.handles.contains(&result.exact) {
            record.handles.push(result.exact.clone());
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
        self.live_frames.fetch_add(1, Ordering::AcqRel);
        Ok(Cell {
            source: request.source,
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
            settled: false,
        })
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
}

pub struct Cell {
    source: String,
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
    settled: bool,
}

impl Cell {
    pub fn cancellation(&self) -> AtomicCancellation {
        self.cancellation.clone()
    }

    pub fn read(
        &mut self,
        path: impl Into<PathBuf>,
        options: ReadOptions,
    ) -> Result<String, HostError> {
        let snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path: path.into(),
                options,
            },
        )?;
        self.ledger.calls = self.ledger.calls.saturating_add(1);
        self.ledger.bytes_read = self.ledger.bytes_read.saturating_add(snapshot.byte_len);
        self.handles.push(snapshot.content.clone());
        if let Some(text) = snapshot.inline_utf8 {
            return Ok(text);
        }
        Ok(format!(
            "{}\nexact: {}",
            snapshot
                .outline
                .unwrap_or_else(|| format!("{} bytes", snapshot.byte_len)),
            snapshot.content
        ))
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
                "z.snap mutation-grade discovery requires cardinality exactly_one".into(),
            ));
        }

        let (path, structural, derived_lines) = match target {
            SnapTargetRequest::Path { path } => {
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
                    },
                )?;
                let path = result
                    .hits
                    .first()
                    .map(|hit| hit.path.clone())
                    .ok_or_else(|| {
                        HostError::Engine(EngineError::new(
                            EngineErrorKind::NotFound,
                            "z.snap search found no target",
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
                    "z.snap full view requires UTF-8 source",
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

    pub fn effect(&mut self, request: EffectRequest) -> Result<EffectResult, HostError> {
        request.validate().map_err(HostError::InvalidRequest)?;
        if self.transaction.is_some() {
            return Err(HostError::InvalidRequest(
                "z.effect cannot follow z.write, z.edit, z.remove, or z.transact in one cell; express all mutations in z.effect or start a separate ZeroKernel call"
                    .into(),
            ));
        }
        if request.targets.is_empty() || request.changes.is_empty() {
            return Err(HostError::InvalidRequest(
                "z.effect requires at least one target and one change".into(),
            ));
        }
        if request.verify.parse {
            return Err(HostError::InvalidRequest(
                "verification_unavailable: z.effect parse verification requires a confined child image"
                    .into(),
            ));
        }
        if !request.verify.changed_targets_only {
            return Err(HostError::InvalidRequest(
                "z.effect requires verify.changedTargetsOnly=true".into(),
            ));
        }
        if request.verify.command.is_some() {
            return Err(HostError::InvalidRequest(
                "verification_unavailable: z.effect commands require child-image confinement and exact delta verification"
                    .into(),
            ));
        }

        let mut planned = BTreeMap::new();
        for (name, target) in request.targets {
            if name.is_empty() {
                return Err(HostError::InvalidRequest(
                    "z.effect target names must not be empty".into(),
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
                        "z.effect target {name:?} has unknown expectation {other:?}"
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
                    "z.effect change names unknown target {:?}",
                    change.target
                ))
            })?;
            plan_effect_change(target, change)?;
        }
        if planned.values().any(|target| !target.changed) {
            return Err(HostError::InvalidRequest(
                "every z.effect target must receive at least one change".into(),
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
            changed_files: results.len() as u32,
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

    pub fn begin_transaction(&mut self) -> Result<(), HostError> {
        if self.transaction.is_some() {
            return Ok(());
        }
        self.transaction = Some(
            self.transaction_coordinator
                .begin(self.invocation.clone())?,
        );
        Ok(())
    }

    pub fn apply_file_effect(
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

    pub fn write(
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

    pub fn edit(
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

    pub fn remove(
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

    pub fn commit_transaction(&mut self) -> Result<Vec<FileEffectReceipt>, HostError> {
        let transaction = self.transaction.take().ok_or_else(|| {
            HostError::Transaction(TransactionError::Store("no active transaction".into()))
        })?;
        transaction.commit().map_err(Into::into)
    }

    pub fn rollback_transaction(&mut self) -> Result<(), HostError> {
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
            output_byte_limit: self.budget.output_byte_limit,
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

    pub fn record_operations(&mut self, operations: Vec<ZeroOperationTrace>, truncated: bool) {
        self.operations = operations;
        self.operations_truncated = truncated;
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
        let snapshot = self.files.read(
            &self.invocation,
            FileReadRequest {
                path: path.into(),
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

    pub fn fail(mut self, mut error: EngineError) -> Result<ZeroKernelResponse, HostError> {
        self.merge_async_records();
        if let Some(transaction) = self.transaction.take()
            && let Err(rollback) = transaction.rollback()
        {
            error = EngineError::new(
                EngineErrorKind::Corrupt,
                format!("{}; rollback: {rollback}", error.detail),
                false,
            );
        }
        let outcome = if error.kind == EngineErrorKind::Cancelled {
            ZeroKernelOutcome::Cancelled
        } else {
            ZeroKernelOutcome::Failed
        };
        let visible = error.detail.clone();
        let visible_digest = blake3::hash(visible.as_bytes()).to_hex().to_string();
        let event = ZeroKernelEvent {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            session_id: self.context.session_id.clone(),
            cell_id: self.invocation.context.cell_id.clone(),
            source_digest: source_digest(&self.source),
            contract_digest: self.context.contract_digest.clone(),
            policy_digest: source_digest(b"direct-z"),
            state_root_before: self.state.root.as_ref().map(ToString::to_string),
            state_root_after: self.state.root.as_ref().map(ToString::to_string),
            input_handles: Vec::new(),
            output_handles: self.handles.clone(),
            outcome: outcome.clone(),
            ledger: self.ledger.clone(),
            model_visible_digest: visible_digest,
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
                before: self.state.root.as_ref().map(ToString::to_string),
                after: self.state.root.as_ref().map(ToString::to_string),
                unchanged: true,
            },
            ledger: self.ledger.clone(),
        };
        response
            .validate()
            .map_err(|validation| HostError::Serialization(validation.to_string()))?;
        Ok(response)
    }

    pub fn finish(mut self, value: Value) -> Result<ZeroKernelResponse, HostError> {
        self.merge_async_records();
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
        let visible_bytes = projection.visible.as_bytes();
        let visible_digest = blake3::hash(visible_bytes).to_hex().to_string();
        let event = ZeroKernelEvent {
            protocol: ZERO_KERNEL_PROTOCOL.into(),
            session_id: self.context.session_id.clone(),
            cell_id: self.invocation.context.cell_id.clone(),
            source_digest: source_digest(&self.source),
            contract_digest: self.context.contract_digest.clone(),
            policy_digest: source_digest(b"direct-z"),
            state_root_before: before.as_ref().map(ToString::to_string),
            state_root_after: after.as_ref().map(ToString::to_string),
            input_handles: Vec::new(),
            output_handles: self.handles.clone(),
            outcome: ZeroKernelOutcome::Completed,
            ledger: self.ledger.clone(),
            model_visible_digest: visible_digest,
        };
        let publication = match self.events.publish(&event, visible_bytes) {
            Ok(publication) => publication,
            Err(error) => {
                let restoration = self.restore_effects(&committed_effects);
                if after != before {
                    self.state_store
                        .compare_and_set_root(after.as_ref(), before.as_ref())?;
                }
                if let Err(restoration) = restoration {
                    return Err(HostError::Event(format!(
                        "{error}; committed effect restoration failed: {restoration}"
                    )));
                }
                return Err(HostError::Event(error.to_string()));
            }
        };
        self.settled = true;
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
                before: before.as_ref().map(ToString::to_string),
                after: after.as_ref().map(ToString::to_string),
                unchanged: before == after,
            },
            ledger: self.ledger.clone(),
        };
        response
            .validate()
            .map_err(|error| HostError::Serialization(error.to_string()))?;
        Ok(response)
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
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
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
            "z.effect cannot apply another change after replace_file or remove_file".into(),
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
            let content = change.content.ok_or_else(|| {
                HostError::InvalidRequest("insert_before/insert_after requires content".into())
            })?;
            let mut text = target_text(target)?;
            let (start, end) = exactly_one_span(&text, &anchor, "insert anchor")?;
            let offset = if matches!(change.kind, EffectChangeKind::InsertBefore) {
                start
            } else {
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
        return Err(HostError::Engine(EngineError::new(
            EngineErrorKind::InvalidInput,
            "z.snap structural result is incomplete",
            false,
        )));
    }
    if result.hits.len() != 1 {
        let detail = if result.hits.is_empty() {
            "z.snap structural target was not found".into()
        } else {
            format!(
                "z.snap structural target is ambiguous: {} candidates",
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
        .ok_or_else(|| HostError::InvalidRequest("z.snap hit has no exact line span".into()))?;
    let source = hit
        .source
        .clone()
        .ok_or_else(|| HostError::InvalidRequest("z.snap hit has no exact source handle".into()))?;
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
            "z.snap selection requires exactly one of lines, bytes, symbol, or exactText".into(),
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
                "z.snap byte selection is outside the source".into(),
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
                "z.snap exactText must not be empty".into(),
            ));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| HostError::InvalidRequest("exactText requires UTF-8 source".into()))?;
        let mut matches = text.match_indices(exact);
        let first = matches
            .next()
            .ok_or_else(|| HostError::InvalidRequest("z.snap exactText was not found".into()))?;
        if matches.next().is_some() {
            return Err(HostError::InvalidRequest(
                "z.snap exactText is ambiguous".into(),
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
            "z.snap symbol selection requires structural discovery evidence".into(),
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
