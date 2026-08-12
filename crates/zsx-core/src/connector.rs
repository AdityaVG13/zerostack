//! In-process aggregate connector over registered `DomainAdapter`s.
//!
//! This is the canonical ZSX dispatch path: capability calls lower to
//! canonical domain operations and are admitted into a bounded channel,
//! executed by a small fixed pool of dispatcher threads that call the
//! registered adapters directly in memory. No worker process is spawned and
//! no NDJSON frame crosses a pipe; every worker-side validation the process
//! path performed (approval grant consumption, typed frame validation, result
//! binding, ref reachability, telemetry validation) happens here instead.
//!
//! Cancellation is per request, never whole-session: the session installs one
//! request token per execution, every admitted dispatch carries a clone of it,
//! and both the host runtime and each adapter call observe the same flag. A
//! cancelled request's dispatches fail closed; a later request in the same
//! generation runs under a fresh token.
//!
//! Every mutation dispatch is journaled through the zero-store attempt
//! journal before adapter admission: the Prepared entry is durable before the
//! dispatch is admitted, DispatchCrossed is persisted immediately before the
//! adapter call, and an ambiguous adapter failure is resolved Indeterminate.
//! Recovery (`recover_attempt_v1`, exposed through the session reconciliation
//! API) maps a Prepared journal to SafeToRetry and never redispatchable; no
//! recovery path can call an adapter.
//!
//! No process-backed compatibility runtime remains after the native cutover.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::raw_worker::{ApprovalGrant as WorkerApprovalGrant, EngineIdentity};
use zero_abi::{
    ApprovalState, CallRequest, CapabilityDescriptor, DigestV1, EffectClass, GlobalRegistration,
    TelemetryRequestV1, WorkerRequestFrame, WorkerResponseFrame, WorkerTrace, canonical_json,
    encode_frame, sha256, validate_response_frame,
};
use zero_codemode::CancellationSignal;
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, HostError,
    MAX_INFLIGHT_CONNECTOR_CALLS,
};
use zero_ref::{ZeroRefV1, ZeroScheme};
use zero_store::{
    AttemptBindingV1, AttemptJournalPathsV1, AttemptRecoveryReceiptV1, AttemptStateV1,
    DurableProfileIdV1, SharedCas, atomic_write_file, current_reachability_snapshot, gc_project_id,
    mark_dispatch_crossed_v1, mark_indeterminate_v1, mark_succeeded_v1, prepare_attempt_v1,
    publish_reachability_snapshot, read_current_attempt_v1, recover_attempt_v1,
};
use zerostack_machine_permit::{
    MachinePermit, MachinePermitHeartbeat, PERMIT_HEARTBEAT_INTERVAL, PermitOwnerMetadata,
    try_scoped_permit_base_for,
};

use crate::adapter::{AdapterCall, DomainAdapter};
use crate::lower::{METHODS, lower};
use crate::verdict::{VerdictLoopEnvelope, VerdictLoopResult, VerdictMeter};

/// One approval grant consumed by the native session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionApprovalGrantV1 {
    pub schema: String,
    pub grant_id: String,
    pub engine: EngineIdentity,
    pub root: String,
    pub generation: u64,
    pub request_id: u64,
    pub operation: String,
    pub effect: EffectClass,
    pub authority_digest: String,
    pub policy_digest: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

pub const SESSION_APPROVAL_SCHEMA: &str = "zerostack.session.approval_grant.v1";
pub const MAX_SESSION_APPROVAL_GRANTS: usize = 64;
pub const MAX_SESSION_APPROVAL_LIFETIME_MS: u64 = 300_000;
pub const MAX_SESSION_CONSUMED_APPROVALS: usize = 65_536;

// Fixed session-owned dispatchers keep admission bounded and block on the
// channel while idle. Bursts may run at most this many in-process calls.
const AGGREGATE_DISPATCH_THREADS: usize = 3;

/// One in-process adapter call is admitted per engine at a time; a second
/// call for the same engine waits on that engine's mutex, which keeps native
/// memory bounded exactly like the process pool's per-engine serialization.
fn engine_index(engine: EngineIdentity) -> usize {
    match engine {
        EngineIdentity::FsZero => 0,
        EngineIdentity::GraphZero => 1,
        EngineIdentity::TokenZero => 2,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AggregateExecutionContext {
    pub generation: u64,
    pub request_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchPermitClass {
    Analysis,
    Index,
    Heavy,
}

impl DispatchPermitClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Index => "index",
            Self::Heavy => "heavy",
        }
    }
}

fn dispatch_permit_class(engine: EngineIdentity, operation: &str) -> Option<DispatchPermitClass> {
    if matches!(
        (engine, operation),
        (EngineIdentity::FsZero, "fs.expand")
            | (
                EngineIdentity::FsZero,
                "fs.read" | "fs.readMany" | "fs.ls" | "fs.listMany" | "fs.stat" | "fs.history"
            )
            | (EngineIdentity::GraphZero, "expand")
            | (EngineIdentity::TokenZero, "expand")
    ) {
        return None;
    }
    if engine == EngineIdentity::GraphZero && matches!(operation, "index" | "remember") {
        return Some(DispatchPermitClass::Index);
    }
    if matches!(
        (engine, operation),
        (EngineIdentity::FsZero, "fs.edit" | "fs.write")
            | (EngineIdentity::TokenZero, "ingest" | "shell")
    ) {
        return Some(DispatchPermitClass::Heavy);
    }
    Some(DispatchPermitClass::Analysis)
}

fn dispatch_permit_slots(class: DispatchPermitClass, cores: usize) -> usize {
    match class {
        DispatchPermitClass::Analysis => (cores / 4).clamp(1, 8),
        DispatchPermitClass::Index => (cores / 8).clamp(1, 2),
        DispatchPermitClass::Heavy => 1,
    }
}

fn available_cores() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
}

pub(crate) fn execution_session_ref(
    session_id: &str,
    context: AggregateExecutionContext,
) -> String {
    format!(
        "cm://session/{}/generation/{}",
        session_id, context.generation
    )
}

pub(crate) fn execution_cell_ref(session_id: &str, context: AggregateExecutionContext) -> String {
    format!(
        "cm://cell/{}/generation/{}/request/{}",
        session_id, context.generation, context.request_id
    )
}

/// Schema tag of the per-attempt sidecar manifest written at prepare time.
///
/// The manifest is informational identity (the journal entries themselves are
/// authoritative); it lets a native addon resuming a session name each
/// attempt's engine, operation, and effect class without re-deriving digests.
pub(crate) const ATTEMPT_MANIFEST_SCHEMA: &str = "zerostack.zsx.attempt_manifest.v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttemptManifestV1 {
    pub schema: String,
    pub session_id: String,
    pub generation: u64,
    pub request_id: u64,
    pub dispatch_id: String,
    pub engine: EngineIdentity,
    pub operation: String,
    pub effect_class: EffectClass,
}

/// One mutation attempt journal owned by a dispatch, from durable prepare
/// through the dispatch boundary and terminal resolution.
#[derive(Clone, Debug)]
struct MutationJournal {
    paths: AttemptJournalPathsV1,
    prepared_entry_digest: DigestV1,
    dispatch_entry_digest: Option<DigestV1>,
}

/// Read-only snapshot of one reconciled mutation attempt journal, returned by
/// the session reconciliation API. `state` is the terminal state after
/// recovery, so a Prepared journal surfaces as `SafeToRetry` and a
/// DispatchCrossed journal without evidence surfaces as `Indeterminate`.
#[derive(Clone, Debug, Serialize)]
pub struct ZsxAttemptJournalStatus {
    pub generation: u64,
    pub request_id: u64,
    /// Connector dispatch id (`<session>-g<generation>-r<request>-<seq>`).
    pub dispatch_id: String,
    /// Engine from the attempt manifest, when the manifest is present.
    pub engine: Option<EngineIdentity>,
    /// Canonical domain operation from the attempt manifest, when present.
    pub operation: Option<String>,
    /// Journaled effect class from the attempt manifest, when present.
    pub effect_class: Option<EffectClass>,
    /// Terminal attempt state after reconciliation.
    pub state: AttemptStateV1,
    /// Durable recovery receipt produced by `recover_attempt_v1`.
    pub recovery: AttemptRecoveryReceiptV1,
    /// Journal directory holding the immutable `attempt-<sequence>.json` chain.
    pub journal_directory: PathBuf,
}

/// Deterministic SHA-256 identity over canonical JSON.
fn attempt_digest(value: &Value) -> DigestV1 {
    DigestV1::from_bytes(sha256(canonical_json(value).as_bytes()))
}

/// Classify one canonical domain operation as a journaled mutation.
///
/// The table mirrors the existing dispatch permit classes (Heavy and Index
/// operations are the state-changing ones) plus the approval-grant operation
/// set; everything else is read-only and is never journaled.
fn mutation_effect_class(engine: EngineIdentity, operation: &str) -> Option<EffectClass> {
    match (engine, operation) {
        (EngineIdentity::FsZero, "fs.edit") => Some(EffectClass::ReversibleMutation),
        (EngineIdentity::FsZero, "fs.write") => Some(EffectClass::ApprovalRequiredMutation),
        (EngineIdentity::GraphZero, "index" | "remember") => Some(EffectClass::ReversibleMutation),
        (EngineIdentity::TokenZero, "ingest") => Some(EffectClass::Irreversible),
        (EngineIdentity::TokenZero, "shell") => Some(EffectClass::Irreversible),
        _ => None,
    }
}

/// Directory that hosts every mutation attempt journal below an explicit
/// session state root. No ambient store resolver may redirect it. Layout:
/// `<store>/attempts/g<generation>/r<request_id>/<seq>/attempt-<sequence>.json`.
pub(crate) fn attempts_root_for(root: &Path) -> PathBuf {
    root.join("attempts")
}

fn attempt_sequence_seed(session_id: &str) -> u64 {
    let digest = sha256(session_id.as_bytes());
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
    .max(1)
}

fn journal_dir_for(
    attempts_root: &Path,
    execution: AggregateExecutionContext,
    sequence: u64,
) -> PathBuf {
    attempts_root
        .join(format!("g{}", execution.generation))
        .join(format!("r{}", execution.request_id))
        .join(sequence.to_string())
}

/// Persist the Prepared entry (and the identity manifest) durably before the
/// dispatch is admitted. After this returns, the journal can only be
/// recovered or aborted; it can never dispatch until crossed.
fn prepare_mutation_journal(
    state: &ZsxState,
    execution: AggregateExecutionContext,
    journal_dir: &Path,
    request: &CallRequest,
    engine: EngineIdentity,
    effect_class: EffectClass,
) -> Result<MutationJournal, ConnectorError> {
    std::fs::create_dir_all(journal_dir).map_err(|error| {
        ConnectorError::new(format!(
            "cannot create mutation attempt journal directory: {error}"
        ))
    })?;
    let manifest = AttemptManifestV1 {
        schema: ATTEMPT_MANIFEST_SCHEMA.into(),
        session_id: state.session_id.clone(),
        generation: execution.generation,
        request_id: execution.request_id,
        dispatch_id: request.request_id.clone(),
        engine,
        operation: request.op.clone(),
        effect_class,
    };
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| {
        ConnectorError::new(format!("cannot encode mutation attempt manifest: {error}"))
    })?;
    atomic_write_file(&journal_dir.join("manifest.json"), &manifest_bytes).map_err(|error| {
        ConnectorError::new(format!("cannot persist mutation attempt manifest: {error}"))
    })?;
    let binding = AttemptBindingV1::new(
        DigestV1::from_bytes(sha256(request.request_id.as_bytes())),
        attempt_digest(&json!({
            "engine": engine.as_str(),
            "operation": request.op,
            "args": request.args,
        })),
        effect_class,
        attempt_digest(&json!({
            "session": execution_session_ref(&state.session_id, execution),
            "cell": execution_cell_ref(&state.session_id, execution),
        })),
        DurableProfileIdV1::PortableStrict,
        DigestV1::from_bytes(sha256(state.session_id.as_bytes())),
    );
    let paths = AttemptJournalPathsV1::new(journal_dir)
        .map_err(|error| ConnectorError::new(error.to_string()))?;
    let prepared = prepare_attempt_v1(&paths, binding.clone())
        .map_err(|error| ConnectorError::new(error.to_string()))?;
    let prepared_entry_digest = prepared
        .digest()
        .map_err(|error| ConnectorError::new(error.to_string()))?;
    Ok(MutationJournal {
        paths,
        prepared_entry_digest,
        dispatch_entry_digest: None,
    })
}

/// Persist the dispatch boundary immediately before the adapter call. A
/// failure here means the effect never ran and the journal remains Prepared
/// (recoverable only as SafeToRetry).
fn cross_mutation_journal(journal: &mut MutationJournal) -> Result<(), ConnectorError> {
    let crossed = mark_dispatch_crossed_v1(
        &journal.paths,
        journal.prepared_entry_digest,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX),
    )
    .map_err(|error| ConnectorError::new(error.to_string()))?;
    journal.dispatch_entry_digest = Some(
        crossed
            .digest()
            .map_err(|error| ConnectorError::new(error.to_string()))?,
    );
    Ok(())
}

/// Persist authoritative completion evidence for a crossed mutation.
fn succeed_mutation_journal(
    journal: &MutationJournal,
    receipt_digest: DigestV1,
) -> Result<(), ConnectorError> {
    let dispatch_entry_digest = journal
        .dispatch_entry_digest
        .ok_or_else(|| ConnectorError::new("mutation attempt never crossed dispatch"))?;
    mark_succeeded_v1(
        &journal.paths,
        dispatch_entry_digest,
        receipt_digest,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX),
    )
    .map_err(|error| ConnectorError::new(error.to_string()))?;
    Ok(())
}

/// Resolve a crossed mutation without authoritative evidence as terminal
/// Indeterminate. The effect may have run; it is never redispatched.
fn indeterminate_mutation_journal(journal: &MutationJournal) -> Result<(), ConnectorError> {
    let dispatch_entry_digest = journal
        .dispatch_entry_digest
        .ok_or_else(|| ConnectorError::new("mutation attempt never crossed dispatch"))?;
    mark_indeterminate_v1(&journal.paths, dispatch_entry_digest)
        .map_err(|error| ConnectorError::new(error.to_string()))?;
    Ok(())
}

/// Reconcile every mutation attempt journal of one request against the
/// zero-store recovery law: terminals are returned unchanged, a Prepared
/// journal is classified SafeToRetry (it never dispatched and can never
/// dispatch through this journal), and a DispatchCrossed journal without
/// authoritative evidence is classified Indeterminate. This never calls an
/// adapter and never writes a DispatchCrossed entry, so no recovered attempt
/// can be replayed.
pub(crate) fn reconcile_request_attempts(
    attempts_root: &Path,
    generation: u64,
    request_id: u64,
) -> Result<Vec<ZsxAttemptJournalStatus>, String> {
    let request_dir = attempts_root
        .join(format!("g{generation}"))
        .join(format!("r{request_id}"));
    let entries = match std::fs::read_dir(&request_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read attempt journals: {error}")),
    };
    let mut statuses = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read attempt journal entry: {error}"))?;
        let journal_dir = entry.path();
        if !journal_dir.is_dir() {
            continue;
        }
        let manifest = read_attempt_manifest(&journal_dir);
        let dispatch_id = manifest
            .as_ref()
            .map(|manifest| manifest.dispatch_id.clone())
            .unwrap_or_else(|| {
                journal_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned()
            });
        let paths = AttemptJournalPathsV1::new(&journal_dir).map_err(|error| error.to_string())?;
        let Some(current) = read_current_attempt_v1(&paths).map_err(|error| error.to_string())?
        else {
            continue;
        };
        let recovery = recover_attempt_v1(&paths, &current.binding, None)
            .map_err(|error| error.to_string())?;
        statuses.push(ZsxAttemptJournalStatus {
            generation,
            request_id,
            dispatch_id,
            engine: manifest.as_ref().map(|manifest| manifest.engine),
            operation: manifest.as_ref().map(|manifest| manifest.operation.clone()),
            effect_class: manifest.as_ref().map(|manifest| manifest.effect_class),
            state: recovery.terminal_state,
            recovery,
            journal_directory: journal_dir,
        });
    }
    statuses.sort_by(|left, right| left.dispatch_id.cmp(&right.dispatch_id));
    Ok(statuses)
}

/// Reconcile every durable mutation attempt found under one session store.
/// Unknown entries are ignored, symlinks are never followed, and recovery
/// never calls an adapter or redispatches an effect.
pub(crate) fn reconcile_all_attempts(
    attempts_root: &Path,
) -> Result<Vec<ZsxAttemptJournalStatus>, String> {
    let generations = match std::fs::read_dir(attempts_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read attempt generations: {error}")),
    };
    let mut statuses = Vec::new();
    for generation_entry in generations {
        let generation_entry = generation_entry
            .map_err(|error| format!("cannot read attempt generation entry: {error}"))?;
        if !generation_entry
            .file_type()
            .map_err(|error| format!("cannot inspect attempt generation entry: {error}"))?
            .is_dir()
        {
            continue;
        }
        let generation_name = generation_entry.file_name();
        let Some(generation) = generation_name
            .to_str()
            .and_then(|name| name.strip_prefix('g'))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let requests = std::fs::read_dir(generation_entry.path())
            .map_err(|error| format!("cannot read attempt requests: {error}"))?;
        for request_entry in requests {
            let request_entry = request_entry
                .map_err(|error| format!("cannot read attempt request entry: {error}"))?;
            if !request_entry
                .file_type()
                .map_err(|error| format!("cannot inspect attempt request entry: {error}"))?
                .is_dir()
            {
                continue;
            }
            let request_name = request_entry.file_name();
            let Some(request_id) = request_name
                .to_str()
                .and_then(|name| name.strip_prefix('r'))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            statuses.extend(reconcile_request_attempts(
                attempts_root,
                generation,
                request_id,
            )?);
        }
    }
    statuses.sort_by(|left, right| {
        (left.generation, left.request_id, &left.dispatch_id).cmp(&(
            right.generation,
            right.request_id,
            &right.dispatch_id,
        ))
    });
    Ok(statuses)
}

fn read_attempt_manifest(journal_dir: &Path) -> Option<AttemptManifestV1> {
    let bytes = std::fs::read(journal_dir.join("manifest.json")).ok()?;
    serde_json::from_slice::<AttemptManifestV1>(&bytes).ok()
}

struct ZsxDispatch {
    engine: EngineIdentity,
    request: CallRequest,
    context: DispatchContext,
    execution: AggregateExecutionContext,
    completion: ConnectorCompletion,
    /// Per-request cancellation token; shared with the host runtime and the
    /// adapter call for this dispatch.
    cancellation: CancellationSignal,
    /// Mutation attempt journal, present only for journaled mutations.
    journal: Option<MutationJournal>,
}

pub(crate) struct ZsxState {
    adapters: BTreeMap<EngineIdentity, Arc<dyn DomainAdapter>>,
    engine_locks: [Mutex<()>; 3],
    /// Authorized workspace root used for dispatch/grant policy only.
    workspace_root: PathBuf,
    /// Session-owned durable state root used for CAS, journals, and GC.
    state_root: PathBuf,
    session_id: String,
    reachable_blobs: Mutex<BTreeMap<EngineIdentity, BTreeSet<String>>>,
    /// Root of the per-request mutation attempt journals (durable store).
    attempts_root: PathBuf,
    consumed_approval_grants: Mutex<BTreeSet<String>>,
    engine_wall_ns: [AtomicU64; 3],
    engine_dispatches: [AtomicU64; 3],
    outstanding_dispatches: AtomicU64,
    verdict_meter: Mutex<Option<VerdictMeter>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EngineDispatchMetrics {
    pub wall_ns: [u64; 3],
    pub dispatches: [u64; 3],
}

#[derive(Default)]
struct ActiveApprovals {
    grants: Vec<SessionApprovalGrantV1>,
}

pub(crate) struct ZsxConnector {
    state: Arc<ZsxState>,
    dispatch_sender: Option<SyncSender<ZsxDispatch>>,
    dispatchers: Vec<JoinHandle<()>>,
    sequence: AtomicU64,
    approvals: Mutex<ActiveApprovals>,
    execution_context: Mutex<Option<AggregateExecutionContext>>,
    /// Token of the request currently executing; every admitted dispatch
    /// clones it so cancellation is per request, not whole-session.
    request_cancellation: Mutex<Option<CancellationSignal>>,
}

impl ZsxConnector {
    pub(crate) fn new(
        root: PathBuf,
        session_id: String,
        adapters: BTreeMap<EngineIdentity, Arc<dyn DomainAdapter>>,
    ) -> Result<Self, HostError> {
        Self::new_with_state_root(root.clone(), root, session_id, adapters)
    }

    pub(crate) fn new_with_state_root(
        workspace_root: PathBuf,
        state_root: PathBuf,
        session_id: String,
        adapters: BTreeMap<EngineIdentity, Arc<dyn DomainAdapter>>,
    ) -> Result<Self, HostError> {
        for (engine, adapter) in &adapters {
            adapter
                .binding()
                .validate()
                .map_err(|error| HostError::Connector(error.to_string()))?;
            if adapter.engine() != *engine {
                return Err(HostError::Connector(format!(
                    "adapter engine {} does not match registration slot {}",
                    adapter.engine().as_str(),
                    engine.as_str()
                )));
            }
        }
        let attempts_root = attempts_root_for(&state_root);
        let sequence_seed = attempt_sequence_seed(&session_id);
        let state = Arc::new(ZsxState {
            adapters,
            engine_locks: [Mutex::new(()), Mutex::new(()), Mutex::new(())],
            workspace_root,
            state_root,
            session_id,
            reachable_blobs: Mutex::new(BTreeMap::new()),
            attempts_root,
            consumed_approval_grants: Mutex::new(BTreeSet::new()),
            engine_wall_ns: [const { AtomicU64::new(0) }; 3],
            engine_dispatches: [const { AtomicU64::new(0) }; 3],
            outstanding_dispatches: AtomicU64::new(0),
            verdict_meter: Mutex::new(None),
        });
        let (dispatch_sender, dispatch_receiver) = mpsc::sync_channel(MAX_INFLIGHT_CONNECTOR_CALLS);
        let dispatch_receiver = Arc::new(Mutex::new(dispatch_receiver));
        let mut dispatchers: Vec<JoinHandle<()>> = Vec::with_capacity(AGGREGATE_DISPATCH_THREADS);
        for index in 0..AGGREGATE_DISPATCH_THREADS {
            let state = Arc::clone(&state);
            let receiver = Arc::clone(&dispatch_receiver);
            let handle = match thread::Builder::new()
                .name(format!("zsx-dispatch-{index}"))
                .spawn(move || aggregate_dispatch_loop(state, receiver))
            {
                Ok(handle) => handle,
                Err(error) => {
                    drop(dispatch_sender);
                    for dispatcher in dispatchers {
                        let _ = dispatcher.join();
                    }
                    return Err(HostError::Connector(format!(
                        "cannot start aggregate dispatcher: {error}"
                    )));
                }
            };
            dispatchers.push(handle);
        }
        Ok(Self {
            state,
            dispatch_sender: Some(dispatch_sender),
            dispatchers,
            sequence: AtomicU64::new(sequence_seed),
            approvals: Mutex::new(ActiveApprovals::default()),
            execution_context: Mutex::new(None),
            request_cancellation: Mutex::new(None),
        })
    }

    pub(crate) fn set_execution_context(
        &self,
        context: AggregateExecutionContext,
    ) -> Result<(), HostError> {
        let mut active = self
            .execution_context
            .lock()
            .map_err(|_| HostError::Connector("execution context lock poisoned".into()))?;
        *active = Some(context);
        Ok(())
    }

    pub(crate) fn clear_execution_context(&self) {
        if let Ok(mut active) = self.execution_context.lock() {
            *active = None;
        }
    }

    pub(crate) fn reset_dispatch_metrics(&self) {
        for value in self
            .state
            .engine_wall_ns
            .iter()
            .chain(self.state.engine_dispatches.iter())
        {
            value.store(0, Ordering::Relaxed);
        }
    }

    pub(crate) fn dispatch_metrics(&self) -> EngineDispatchMetrics {
        EngineDispatchMetrics {
            wall_ns: std::array::from_fn(|index| {
                self.state.engine_wall_ns[index].load(Ordering::Relaxed)
            }),
            dispatches: std::array::from_fn(|index| {
                self.state.engine_dispatches[index].load(Ordering::Relaxed)
            }),
        }
    }

    pub(crate) fn wait_for_dispatch_idle(&self, timeout: Duration) -> Result<(), HostError> {
        let deadline = Instant::now() + timeout;
        while self.state.outstanding_dispatches.load(Ordering::Acquire) != 0 {
            if Instant::now() >= deadline {
                return Err(HostError::Connector(
                    "aggregate dispatches did not stop after cancellation".into(),
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    pub(crate) fn install_verdict_meter(
        &self,
        envelope: Option<VerdictLoopEnvelope>,
    ) -> Result<(), HostError> {
        let mut active = self
            .state
            .verdict_meter
            .lock()
            .map_err(|_| HostError::Connector("verdict meter lock poisoned".into()))?;
        if active.is_some() {
            return Err(HostError::Connector(
                "verdict meter was not cleared after the prior execution".into(),
            ));
        }
        *active = envelope
            .map(VerdictMeter::new)
            .transpose()
            .map_err(HostError::VerdictRejected)?;
        Ok(())
    }

    pub(crate) fn finish_verdict_meter(
        &self,
        value: &Value,
    ) -> Result<Option<VerdictLoopResult>, HostError> {
        self.state
            .verdict_meter
            .lock()
            .map_err(|_| HostError::Connector("verdict meter lock poisoned".into()))?
            .take()
            .map(|meter| meter.finish(value).map_err(HostError::VerdictRejected))
            .transpose()
    }

    pub(crate) fn clear_verdict_meter(&self) {
        if let Ok(mut active) = self.state.verdict_meter.lock() {
            active.take();
        }
    }

    fn execution_context(&self) -> Result<AggregateExecutionContext, ConnectorError> {
        self.execution_context
            .lock()
            .map_err(|_| ConnectorError::new("execution context lock poisoned"))?
            .ok_or_else(|| ConnectorError::new("aggregate execution context missing"))
    }

    pub(crate) fn install_approvals(
        &self,
        grants: Vec<SessionApprovalGrantV1>,
    ) -> Result<(), HostError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| HostError::Connector("approval state lock poisoned".into()))?;
        if !active.grants.is_empty() {
            return Err(HostError::Connector(
                "approval state was not cleared after the prior execution".into(),
            ));
        }
        active.grants = grants;
        Ok(())
    }

    pub(crate) fn clear_approvals(&self) {
        if let Ok(mut active) = self.approvals.lock() {
            active.grants.clear();
        }
    }

    fn take_approval(
        &self,
        engine: EngineIdentity,
        operation: &str,
        worker_request_id: &str,
    ) -> Result<Option<(WorkerApprovalGrant, SessionApprovalGrantV1)>, ConnectorError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| ConnectorError::new("approval state lock poisoned"))?;
        let Some(index) = active
            .grants
            .iter()
            .position(|grant| grant.engine == engine && grant.operation == operation)
        else {
            return Ok(None);
        };
        let grant = active.grants.remove(index);
        let original = grant.clone();
        Ok(Some((
            WorkerApprovalGrant {
                grant_id: grant.grant_id,
                engine,
                root: grant.root,
                session_id: self.state.session_id.clone(),
                request_id: worker_request_id.to_owned(),
                operation: operation.to_owned(),
                effect: grant.effect,
                authority_digest: grant.authority_digest,
                policy_digest: grant.policy_digest,
                issued_at_unix_ms: grant.issued_at_unix_ms,
                expires_at_unix_ms: grant.expires_at_unix_ms,
            },
            original,
        )))
    }

    fn restore_approval(&self, grant: SessionApprovalGrantV1) -> Result<(), ConnectorError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| ConnectorError::new("approval state lock poisoned"))?;
        if active
            .grants
            .iter()
            .any(|active_grant| active_grant.grant_id == grant.grant_id)
        {
            return Err(ConnectorError::new("approval reservation restore conflict"));
        }
        active.grants.push(grant);
        Ok(())
    }

    pub(crate) fn publish_reachability(&self) -> Result<(), HostError> {
        publish_reachability(&self.state)
    }

    /// Install the token of the request that is about to execute. Every
    /// dispatch admitted while this is installed clones the same token.
    pub(crate) fn set_request_cancellation(&self, signal: CancellationSignal) {
        if let Ok(mut slot) = self.request_cancellation.lock() {
            *slot = Some(signal);
        }
    }

    pub(crate) fn clear_request_cancellation(&self) {
        if let Ok(mut slot) = self.request_cancellation.lock() {
            *slot = None;
        }
    }

    fn request_cancellation(&self) -> Result<CancellationSignal, ConnectorError> {
        self.request_cancellation
            .lock()
            .map_err(|_| ConnectorError::new("request cancellation lock poisoned"))?
            .clone()
            .ok_or_else(|| ConnectorError::new("active request cancellation missing"))
    }
}

impl Drop for ZsxConnector {
    fn drop(&mut self) {
        self.dispatch_sender.take();
        for dispatcher in self.dispatchers.drain(..) {
            let _ = dispatcher.join();
        }
    }
}

impl Connector for ZsxConnector {
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let input: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        let (engine, op, args) = lower(&capability.surface, &capability.method, input)?;
        let request_cancellation = self.request_cancellation()?;
        if context.is_expired() || request_cancellation.is_cancelled() {
            return Err(ConnectorError::new(
                "aggregate dispatch deadline or cancellation",
            ));
        }
        let execution = self.execution_context()?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "{}-g{}-r{}-{}",
            self.state.session_id, execution.generation, execution.request_id, sequence
        );
        // A journaled mutation must be durably Prepared before the dispatch
        // is admitted to the dispatcher queue. A failure here rejects the
        // call with no Prepared journal (or a Prepared journal that recovery
        // classifies SafeToRetry); the adapter is never admitted.
        let adapter = self
            .state
            .adapters
            .get(&engine)
            .ok_or_else(|| ConnectorError::new("domain adapter missing"))?;
        let binding = adapter.binding();
        let trace = WorkerTrace {
            runtime_id: self.state.session_id.clone(),
            cell_id: execution_cell_ref(&self.state.session_id, execution),
            request_id: id.clone(),
            trace_id: id.clone(),
            parent_span_id: Some(execution_session_ref(&self.state.session_id, execution)),
            worker_revision: binding.worker_revision.clone(),
            contract_digest: binding.semantic_contract_digest.clone(),
        };
        let sender = self
            .dispatch_sender
            .as_ref()
            .ok_or_else(|| ConnectorError::new("aggregate dispatcher closed"))?;
        let taken_approval = self.take_approval(engine, &op, &id)?;
        let approval_grant = taken_approval
            .as_ref()
            .map(|(worker_grant, _)| worker_grant.clone());
        let metered = self
            .state
            .verdict_meter
            .lock()
            .map_err(|_| ConnectorError::new("verdict meter lock poisoned"))?
            .is_some();
        let request = CallRequest {
            request_id: id,
            op,
            args,
            deadline_unix_ms: Some(
                now_ms().saturating_add(
                    context
                        .remaining()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                ),
            ),
            trace,
            approval_grant,
            telemetry_request: metered.then_some(TelemetryRequestV1 {
                engine_stage_timeline: false,
                worker_token_accounting: true,
            }),
        };
        if metered {
            let input_bytes = encode_frame(
                &WorkerRequestFrame::Call {
                    request: request.clone(),
                },
                zero_abi::DEFAULT_MAX_FRAME_BYTES,
            )
            .map_err(|error| ConnectorError::new(format!("meter request frame: {error}")))?
            .len()
            .try_into()
            .map_err(|_| ConnectorError::new("meter request frame size overflowed"))?;
            let reserve = self
                .state
                .verdict_meter
                .lock()
                .map_err(|_| ConnectorError::new("verdict meter lock poisoned"))?
                .as_mut()
                .expect("meter presence checked")
                .reserve_dispatch(input_bytes);
            if let Err(error) = reserve {
                if let Some((_, grant)) = taken_approval {
                    self.restore_approval(grant)?;
                }
                return Err(ConnectorError::new(error));
            }
        }
        let journal = mutation_effect_class(engine, &request.op)
            .map(|effect_class| {
                let journal_dir = journal_dir_for(&self.state.attempts_root, execution, sequence);
                prepare_mutation_journal(
                    &self.state,
                    execution,
                    &journal_dir,
                    &request,
                    engine,
                    effect_class,
                )
            })
            .transpose()?;
        let dispatch = ZsxDispatch {
            engine,
            request,
            context,
            execution,
            completion,
            cancellation: request_cancellation,
            journal,
        };
        self.state
            .outstanding_dispatches
            .fetch_add(1, Ordering::AcqRel);
        match sender.try_send(dispatch) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.state
                    .outstanding_dispatches
                    .fetch_sub(1, Ordering::Release);
                if let Some((_, grant)) = taken_approval {
                    self.restore_approval(grant)?;
                }
                Err(ConnectorError::new("aggregate dispatch capacity exhausted"))
            }
            Err(TrySendError::Disconnected(_)) => {
                self.state
                    .outstanding_dispatches
                    .fetch_sub(1, Ordering::Release);
                if let Some((_, grant)) = taken_approval {
                    self.restore_approval(grant)?;
                }
                Err(ConnectorError::new("aggregate dispatcher closed"))
            }
        }
    }
}

fn aggregate_dispatch_loop(state: Arc<ZsxState>, receiver: Arc<Mutex<Receiver<ZsxDispatch>>>) {
    loop {
        let dispatch = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(poisoned) => poisoned.into_inner().recv(),
        };
        let Ok(dispatch) = dispatch else {
            break;
        };
        let result = run_dispatch(&state, &dispatch);
        if let Err(error) = &result
            && let Ok(mut meter) = state.verdict_meter.lock()
            && let Some(meter) = meter.as_mut()
        {
            meter.fail(format!("verdict-loop connector failure: {error}"));
        }
        let _ = dispatch.completion.complete(result);
        state.outstanding_dispatches.fetch_sub(1, Ordering::Release);
    }
}

/// Worker-side validation performed in-process so adapters stay thin.
fn validate_adapter_result(
    state: &ZsxState,
    engine: EngineIdentity,
    request: &CallRequest,
    response: &crate::adapter::AdapterResponse,
) -> Result<(), ConnectorError> {
    let frame = WorkerResponseFrame::Result {
        request_id: request.request_id.clone(),
        result: response.result.clone(),
        engine_timeline: response.engine_timeline.clone(),
        worker_token_accounting: response.worker_token_accounting.clone(),
    };
    validate_response_frame(&frame)
        .map_err(|error| ConnectorError::new(format!("invalid adapter result: {error}")))?;
    if matches!(
        response.result.metadata.approval.state,
        ApprovalState::Required | ApprovalState::Denied
    ) {
        return Err(ConnectorError::new("adapter approval required or denied"));
    }
    if response.result.metadata.ownership.engine != engine
        || response.result.metadata.ownership.session_id != state.session_id
        || response.result.metadata.trace != request.trace
    {
        return Err(ConnectorError::new("adapter result binding mismatch"));
    }
    retain_reachability(state, engine, &response.result.metadata.ownership.refs)
}

fn run_dispatch(state: &ZsxState, dispatch: &ZsxDispatch) -> Result<String, ConnectorError> {
    if dispatch.context.is_expired() || dispatch.cancellation.is_cancelled() {
        return Err(ConnectorError::new(
            "aggregate dispatch deadline or cancellation",
        ));
    }
    let _permit = acquire_dispatch_permit(state, dispatch)?;
    let adapter = state
        .adapters
        .get(&dispatch.engine)
        .ok_or_else(|| ConnectorError::new("domain adapter missing"))?;
    let engine_busy = state.engine_locks[engine_index(dispatch.engine)]
        .lock()
        .map_err(|_| ConnectorError::new("engine serialization lock poisoned"))?;
    let mut journal = dispatch.journal.clone();
    let engine_started = Instant::now();
    let result = {
        let _engine_guard = engine_busy;
        if dispatch.context.is_expired() || dispatch.cancellation.is_cancelled() {
            return Err(ConnectorError::new(
                "aggregate dispatch deadline or cancellation",
            ));
        }
        // Consume and validate the approval grant exactly when the process
        // worker would have: immediately before the action.
        if dispatch.request.approval_grant.is_some() {
            let mut consumed = state
                .consumed_approval_grants
                .lock()
                .map_err(|_| ConnectorError::new("approval ledger lock poisoned"))?;
            dispatch
                .request
                .validate_approval_grant(
                    dispatch.engine,
                    state.workspace_root.to_str().unwrap_or_default(),
                    &state.session_id,
                    EffectClass::ApprovalRequiredMutation,
                    now_ms(),
                    &mut consumed,
                )
                .map_err(|rejection| {
                    ConnectorError::new(format!("approval grant rejected: {rejection:?}"))
                })?;
        }
        // Persist the dispatch boundary immediately before the adapter call.
        // After this point the effect may run, so every later failure is
        // resolved Indeterminate rather than SafeToRetry.
        if let Some(journal) = journal.as_mut() {
            cross_mutation_journal(journal)?;
        }
        adapter
            .call(AdapterCall {
                request: &dispatch.request,
                cancellation: &dispatch.cancellation,
            })
            .map_err(|error| {
                ConnectorError::new(format!("{} adapter: {error}", dispatch.engine.as_str()))
            })
    };
    let engine_index = engine_index(dispatch.engine);
    let engine_wall_ns = engine_started
        .elapsed()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX);
    state.engine_wall_ns[engine_index].fetch_add(engine_wall_ns, Ordering::Relaxed);
    state.engine_dispatches[engine_index].fetch_add(1, Ordering::Relaxed);
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            // The adapter call started (dispatch crossed); its failure is
            // ambiguous, so the journal is terminal Indeterminate.
            if let Some(journal) = journal.as_ref() {
                let _ = indeterminate_mutation_journal(journal);
            }
            return Err(error);
        }
    };
    if let Err(error) =
        validate_adapter_result(state, dispatch.engine, &dispatch.request, &response)
    {
        if let Some(journal) = journal.as_ref() {
            let _ = indeterminate_mutation_journal(journal);
        }
        return Err(error);
    }
    {
        let mut active = state
            .verdict_meter
            .lock()
            .map_err(|_| ConnectorError::new("verdict meter lock poisoned"))?;
        if let Some(meter) = active.as_mut() {
            let frame = WorkerResponseFrame::Result {
                request_id: dispatch.request.request_id.clone(),
                result: response.result.clone(),
                engine_timeline: response.engine_timeline.clone(),
                worker_token_accounting: response.worker_token_accounting.clone(),
            };
            let output_bytes = encode_frame(&frame, zero_abi::DEFAULT_MAX_FRAME_BYTES)
                .map_err(|error| ConnectorError::new(format!("meter response frame: {error}")))?
                .len()
                .try_into()
                .map_err(|_| ConnectorError::new("meter response frame size overflowed"))?;
            meter
                .record_response(output_bytes, response.worker_token_accounting.as_ref())
                .map_err(ConnectorError::new)?;
        }
    }
    let result_value = serde_json::to_value(&response.result)
        .map_err(|error| ConnectorError::new(error.to_string()))?;
    let value = match normalize_aggregate_result_value(
        dispatch.engine,
        &dispatch.request.op,
        response.result.value,
    ) {
        Ok(value) => value,
        Err(error) => {
            if let Some(journal) = journal.as_ref() {
                let _ = indeterminate_mutation_journal(journal);
            }
            return Err(error);
        }
    };
    if let Some(journal) = journal.as_ref() {
        let receipt_digest = attempt_digest(&result_value);
        if let Err(error) = succeed_mutation_journal(journal, receipt_digest) {
            // The effect completed but its receipt could not be persisted;
            // the journal cannot prove completion, so resolve Indeterminate.
            let _ = indeterminate_mutation_journal(journal);
            return Err(error);
        }
    }
    serde_json::to_string(&serde_json::json!({
        "value": value,
        "metadata": response.result.metadata
    }))
    .map_err(|error| ConnectorError::new(error.to_string()))
}

fn normalize_aggregate_result_value(
    engine: EngineIdentity,
    operation: &str,
    value: Value,
) -> Result<Value, ConnectorError> {
    if engine != EngineIdentity::TokenZero || operation != zero_abi::TOKEN_JOB_OPERATION_V1 {
        return Ok(value);
    }
    let result: zero_abi::TokenJobPollResultV1 = serde_json::from_value(value)
        .map_err(|error| ConnectorError::new(format!("invalid token.job result: {error}")))?;
    result
        .validate()
        .map_err(|error| ConnectorError::new(format!("invalid token.job result: {error}")))?;
    serde_json::to_value(result).map_err(|error| ConnectorError::new(error.to_string()))
}

fn acquire_dispatch_permit(
    state: &ZsxState,
    dispatch: &ZsxDispatch,
) -> Result<Option<MachinePermitHeartbeat>, ConnectorError> {
    let Some(class) = dispatch_permit_class(dispatch.engine, &dispatch.request.op) else {
        return Ok(None);
    };
    let base = try_scoped_permit_base_for(class.as_str(), Some(&state.workspace_root)).map_err(
        |error| ConnectorError::new(format!("resolve {} permit scope: {error}", class.as_str())),
    )?;
    let owner = PermitOwnerMetadata::new(
        state.workspace_root.to_string_lossy(),
        dispatch.request.op.clone(),
        execution_session_ref(&state.session_id, dispatch.execution),
        execution_cell_ref(&state.session_id, dispatch.execution),
    );
    let permit = MachinePermit::acquire_slots_with_owner_metadata(
        &base,
        dispatch_permit_slots(class, available_cores()),
        dispatch.context.deadline,
        owner,
    )
    .map_err(|error| ConnectorError::new(format!("{} permit: {error}", class.as_str())))?;
    permit
        .start_heartbeat(PERMIT_HEARTBEAT_INTERVAL)
        .map(Some)
        .map_err(|error| {
            ConnectorError::new(format!(
                "start {} permit heartbeat: {error}",
                class.as_str()
            ))
        })
}

fn engine_ref_scheme(engine: EngineIdentity) -> ZeroScheme {
    match engine {
        EngineIdentity::FsZero => ZeroScheme::Fz,
        EngineIdentity::GraphZero => ZeroScheme::Gz,
        EngineIdentity::TokenZero => ZeroScheme::Tz,
    }
}

fn retain_reachability(
    state: &ZsxState,
    engine: EngineIdentity,
    refs: &[String],
) -> Result<(), ConnectorError> {
    let cas = SharedCas::open(&state.state_root);
    let mut batch = BTreeSet::new();
    for reference in refs {
        if !reference.contains("://blob/") {
            continue;
        }
        let parsed = ZeroRefV1::parse(reference).map_err(|error| {
            ConnectorError::new(format!(
                "invalid portable adapter ref {reference:?}: {error}"
            ))
        })?;
        if parsed.scheme != engine_ref_scheme(engine) {
            return Err(ConnectorError::new(format!(
                "adapter ref {reference:?} is not owned by {}",
                engine.as_str()
            )));
        }
        cas.get_verified(&parsed.hash).map_err(|error| {
            ConnectorError::new(format!(
                "adapter ref {reference:?} is unavailable from authorized CAS: {error}"
            ))
        })?;
        batch.insert(parsed.hash);
    }
    let mut retained = state
        .reachable_blobs
        .lock()
        .map_err(|_| ConnectorError::new("reachability lock poisoned"))?;
    retained.entry(engine).or_default().extend(batch);
    Ok(())
}

fn publish_reachability(state: &ZsxState) -> Result<(), HostError> {
    let project_id = gc_project_id(&state.state_root)
        .map_err(|error| HostError::Connector(format!("derive GC project identity: {error}")))?;
    let retained = state
        .reachable_blobs
        .lock()
        .map_err(|_| HostError::Connector("reachability lock poisoned".into()))?
        .clone();
    let cas = SharedCas::open(&state.state_root);
    for engine in [
        EngineIdentity::FsZero,
        EngineIdentity::GraphZero,
        EngineIdentity::TokenZero,
    ] {
        let hashes = retained.get(&engine).cloned().unwrap_or_default();
        for hash in &hashes {
            cas.get_verified(hash).map_err(|error| {
                HostError::Connector(format!(
                    "{} reachability object {hash} failed closure verification: {error}",
                    engine.as_str()
                ))
            })?;
        }
        let producer = engine.as_str();
        let epoch = current_reachability_snapshot(&state.state_root, producer, &project_id)
            .map_err(|error| {
                HostError::Connector(format!("read {producer} reachability epoch: {error}"))
            })?
            .map_or(Ok(1), |snapshot| {
                snapshot.epoch.checked_add(1).ok_or_else(|| {
                    HostError::Connector(format!("{producer} reachability epoch overflow"))
                })
            })?;
        publish_reachability_snapshot(
            &state.state_root,
            producer,
            &project_id,
            epoch,
            &hashes.into_iter().collect::<Vec<_>>(),
        )
        .map_err(|error| {
            HostError::Connector(format!("publish {producer} reachability: {error}"))
        })?;
    }
    Ok(())
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn registration() -> GlobalRegistration {
    GlobalRegistration::zero(
        METHODS
            .iter()
            .map(|(s, m)| CapabilityDescriptor::new(*s, *m))
            .collect(),
    )
}

pub(crate) fn host_limits() -> Result<zero_codemode::HostLimits, HostError> {
    zero_codemode::HostLimits::new(
        128 * 1024 * 1024,
        1024 * 1024,
        Duration::from_secs(30),
        10_000_000,
        16_384,
        MAX_INFLIGHT_CONNECTOR_CALLS,
        256 * 1024,
        16 * 1024 * 1024,
    )
    .map_err(HostError::Limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zero_abi::raw_worker::EngineIdentity;
    use zero_store::{AttemptRecoveryOutcomeV1, mark_dispatch_crossed_v1};

    #[test]
    fn dispatch_permit_defaults_and_expand_exception_are_bounded() {
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 32), 8);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 32), 2);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Heavy, 128), 1);
        assert_eq!(
            dispatch_permit_class(EngineIdentity::TokenZero, "expand"),
            None
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.read"),
            None
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.search"),
            Some(DispatchPermitClass::Analysis)
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::GraphZero, "index"),
            Some(DispatchPermitClass::Index)
        );
    }

    #[test]
    fn execution_context_refs_bind_generation_and_request() {
        let context = AggregateExecutionContext {
            generation: 7,
            request_id: 19,
        };
        assert_eq!(
            execution_session_ref("session-7", context),
            "cm://session/session-7/generation/7"
        );
        assert_eq!(
            execution_cell_ref("session-7", context),
            "cm://cell/session-7/generation/7/request/19"
        );
    }

    #[test]
    fn token_job_result_is_revalidated_at_the_aggregate_boundary() {
        let canonical = serde_json::json!({
            "id":"tzjob-7","status":"running","pid":42,"tail":"ok\n",
            "tailUtf8Lossless":true,"tailBytes":3,"logBytes":3,"cursor":3,
            "version":2,"changed":true,"nextPollMs":20000
        });
        assert_eq!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                canonical.clone(),
            )
            .unwrap(),
            canonical
        );

        let mut unknown = canonical.clone();
        unknown["log"] = serde_json::json!("/private/session.log");
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                unknown,
            )
            .is_err()
        );

        let mut false_exactness = canonical;
        false_exactness["tailBytes"] = serde_json::json!(2);
        false_exactness["cursor"] = serde_json::json!(2);
        false_exactness["logBytes"] = serde_json::json!(2);
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                false_exactness,
            )
            .is_err()
        );
    }

    #[test]
    fn mutation_effect_class_covers_only_journaled_operations() {
        assert_eq!(
            mutation_effect_class(EngineIdentity::FsZero, "fs.edit"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::FsZero, "fs.write"),
            Some(EffectClass::ApprovalRequiredMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::GraphZero, "index"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::GraphZero, "remember"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "ingest"),
            Some(EffectClass::Irreversible)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "shell"),
            Some(EffectClass::Irreversible)
        );
        for (engine, read) in [
            (EngineIdentity::FsZero, "fs.ls"),
            (EngineIdentity::FsZero, "fs.read"),
            (EngineIdentity::FsZero, "fs.search"),
            (EngineIdentity::GraphZero, "blast"),
            (EngineIdentity::GraphZero, "query"),
            (EngineIdentity::TokenZero, "expand"),
            (EngineIdentity::TokenZero, "job"),
        ] {
            assert_eq!(
                mutation_effect_class(engine, read),
                None,
                "{read} must not be journaled"
            );
        }
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "expand"),
            None
        );
    }

    fn journal_test_state(root: &Path, session_id: &str) -> ZsxState {
        ZsxState {
            adapters: BTreeMap::new(),
            engine_locks: [Mutex::new(()), Mutex::new(()), Mutex::new(())],
            workspace_root: root.to_path_buf(),
            state_root: root.to_path_buf(),
            session_id: session_id.to_owned(),
            reachable_blobs: Mutex::new(BTreeMap::new()),
            attempts_root: attempts_root_for(root),
            consumed_approval_grants: Mutex::new(BTreeSet::new()),
            engine_wall_ns: [const { AtomicU64::new(0) }; 3],
            engine_dispatches: [const { AtomicU64::new(0) }; 3],
            outstanding_dispatches: AtomicU64::new(0),
        }
    }

    fn test_call_request(id: &str, op: &str, args: Value) -> CallRequest {
        CallRequest {
            request_id: id.to_owned(),
            op: op.to_owned(),
            args,
            deadline_unix_ms: None,
            trace: WorkerTrace {
                runtime_id: "sess-journal".into(),
                cell_id: "cm://cell/sess-journal/generation/7/request/19".into(),
                request_id: id.to_owned(),
                trace_id: id.to_owned(),
                parent_span_id: None,
                worker_revision: "test".into(),
                contract_digest: "0".repeat(64),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn mutation_journal_prepare_is_durable_before_admission_and_cross_is_immediate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 19,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 42);
        let mut journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request(
                "sess-journal-g7-r19-42",
                "fs.edit",
                json!({"path": "a.txt"}),
            ),
            EngineIdentity::FsZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare is durable before admission");

        let prepared = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("prepared entry present");
        assert_eq!(prepared.state, AttemptStateV1::Prepared);
        assert_eq!(prepared.sequence, 1);
        assert!(journal.dispatch_entry_digest.is_none());

        cross_mutation_journal(&mut journal).expect("dispatch boundary persists");
        let crossed = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("crossed entry present");
        assert_eq!(crossed.state, AttemptStateV1::DispatchCrossed);
        assert_eq!(crossed.sequence, 2);
        assert!(journal.dispatch_entry_digest.is_some());

        succeed_mutation_journal(&journal, attempt_digest(&json!({"ok": true})))
            .expect("completion evidence persists");
        let succeeded = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("terminal entry present");
        assert_eq!(succeeded.state, AttemptStateV1::Succeeded);
        assert_eq!(succeeded.sequence, 3);
    }

    #[test]
    fn prepared_journal_recovery_classifies_safe_to_retry_and_never_dispatches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 20,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 43);
        let journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request("sess-journal-g7-r20-43", "ingest", json!({"text": "x"})),
            EngineIdentity::TokenZero,
            EffectClass::Irreversible,
        )
        .expect("prepare");
        assert_eq!(journal.dispatch_entry_digest, None);

        let statuses = reconcile_request_attempts(&state.attempts_root, 7, 20).expect("reconcile");
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
        );
        assert_eq!(statuses[0].state, AttemptStateV1::SafeToRetry);
        assert_eq!(statuses[0].dispatch_id, "sess-journal-g7-r20-43");
        assert_eq!(statuses[0].operation.as_deref(), Some("ingest"));
        assert_eq!(statuses[0].effect_class, Some(EffectClass::Irreversible));

        // Recovery is idempotent and the journal can never cross dispatch.
        let again = reconcile_request_attempts(&state.attempts_root, 7, 20).expect("reconcile");
        assert_eq!(
            again[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::AlreadySafeToRetry
        );
        assert!(
            mark_dispatch_crossed_v1(&journal.paths, journal.prepared_entry_digest, 1,).is_err()
        );
    }

    #[test]
    fn crossed_journal_without_evidence_recovery_classifies_indeterminate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 21,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 44);
        let mut journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request(
                "sess-journal-g7-r21-44",
                "remember",
                json!({"text": "fact"}),
            ),
            EngineIdentity::GraphZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare");
        cross_mutation_journal(&mut journal).expect("cross");

        let statuses = reconcile_request_attempts(&state.attempts_root, 7, 21).expect("reconcile");
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
        );
        assert_eq!(statuses[0].state, AttemptStateV1::Indeterminate);
        assert_eq!(statuses[0].operation.as_deref(), Some("remember"));

        // A recovered journal is terminal: no transition can redispatch it.
        assert!(
            mark_dispatch_crossed_v1(&journal.paths, journal.prepared_entry_digest, 1,).is_err()
        );
    }

    #[test]
    fn all_attempt_recovery_classifies_every_request_without_redispatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let prepared_execution = AggregateExecutionContext {
            generation: 7,
            request_id: 22,
        };
        let prepared_dir = journal_dir_for(&state.attempts_root, prepared_execution, 45);
        let prepared = prepare_mutation_journal(
            &state,
            prepared_execution,
            &prepared_dir,
            &test_call_request("sess-journal-g7-r22-45", "fs.edit", json!({"path": "a"})),
            EngineIdentity::FsZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare first request");

        let crossed_execution = AggregateExecutionContext {
            generation: 8,
            request_id: 1,
        };
        let crossed_dir = journal_dir_for(&state.attempts_root, crossed_execution, 46);
        let mut crossed = prepare_mutation_journal(
            &state,
            crossed_execution,
            &crossed_dir,
            &test_call_request("sess-journal-g8-r1-46", "remember", json!({"text": "x"})),
            EngineIdentity::GraphZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare second request");
        cross_mutation_journal(&mut crossed).expect("cross second request");

        let statuses = reconcile_all_attempts(&state.attempts_root).expect("reconcile all");
        assert_eq!(statuses.len(), 2);
        assert_eq!((statuses[0].generation, statuses[0].request_id), (7, 22));
        assert_eq!(statuses[0].state, AttemptStateV1::SafeToRetry);
        assert_eq!((statuses[1].generation, statuses[1].request_id), (8, 1));
        assert_eq!(statuses[1].state, AttemptStateV1::Indeterminate);

        assert!(
            mark_dispatch_crossed_v1(&prepared.paths, prepared.prepared_entry_digest, 1).is_err()
        );
        assert!(
            mark_dispatch_crossed_v1(&crossed.paths, crossed.prepared_entry_digest, 1).is_err()
        );
    }

    #[test]
    fn attempts_root_is_stable_under_the_session_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let first = attempts_root_for(&root);
        let second = attempts_root_for(&root);
        assert_eq!(first, second);
        assert_eq!(first, root.join("attempts"));
    }

    #[test]
    fn distinct_state_root_contains_attempts_cas_and_gc_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let state_root = dir.path().join("state");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        std::fs::create_dir_all(&state_root).expect("state root");

        let connector = ZsxConnector::new_with_state_root(
            workspace_root.clone(),
            state_root.clone(),
            "sess-state-root".to_owned(),
            BTreeMap::new(),
        )
        .expect("connector");

        assert_eq!(connector.state.workspace_root, workspace_root);
        assert_eq!(connector.state.state_root, state_root);
        assert_eq!(connector.state.attempts_root, state_root.join("attempts"));
        let cas = SharedCas::open(&state_root);
        let hash = cas.put(b"state-root-only ref").expect("publish CAS object");
        retain_reachability(
            &connector.state,
            EngineIdentity::FsZero,
            &[format!("fz://blob/{hash}")],
        )
        .expect("retain state-root ref");
        connector
            .publish_reachability()
            .expect("publish reachability");

        assert!(!workspace_root.join("gc").exists());
        assert!(!workspace_root.join("blobs").exists());
        let project_id = gc_project_id(&state_root).expect("project identity");
        for producer in ["fszero", "graphzero", "tokenzero"] {
            let snapshot = current_reachability_snapshot(&state_root, producer, &project_id)
                .expect("read reachability")
                .unwrap_or_else(|| panic!("missing {producer} reachability under state root"));
            if producer == "fszero" {
                assert_eq!(snapshot.blob_hashes, vec![hash.clone()]);
            } else {
                assert!(snapshot.blob_hashes.is_empty());
            }
        }
    }
}
