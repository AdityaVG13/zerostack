//! Real FSZero in-process adapter (feature `fszero`).
//!
//! [`FsZeroAdapter`] runs the immutable FSZero revision API in-process: one
//! [`FSZeroSession`] rooted at the session root, dispatched through the
//! canonical typed dispatcher [`fs_zero::core::dispatch_codemode_method`].
//! There is no worker process, no NDJSON framing, no session socket, and no
//! MCP or CodeMode runtime: the adapter converts a canonical raw-worker-v2
//! [`CallRequest`] into typed dispatcher arguments and converts the typed
//! [`DispatchOutcome`] back into a bound [`WorkerResult`] — the same boundary
//! conversion the raw worker performs, minus the transport.
//!
//! # Session thread
//!
//! The immutable revision's `FSZeroSession` owns a single-threaded fsqlite
//! connection (`Rc<RefCell<...>>`), so it is not `Send`. The adapter therefore
//! owns the session on one dedicated thread (named `zsx-fszero-session`) and
//! is a channel façade over it: `call()` sends the cloned [`CallRequest`]
//! plus the real session [`CancellationSignal`] and receives the fully
//! converted [`AdapterResponse`] back. This mirrors the process path, where
//! the FSZero worker owns its session in a single process, and matches the
//! connector's per-engine serialization: one in-flight dispatch per engine.
//!
//! # Cancellation and deadline
//!
//! `call()` checks [`AdapterCall::cancellation`] and
//! `request.deadline_unix_ms` before enqueueing, the session thread checks
//! both again immediately before dispatch and after it returns, and `call()`
//! re-checks after the reply arrives. The immutable revision's typed
//! dispatcher does not consult the session request guard during kernel work
//! (`request_expired` is only read by the MCP stdio/HTTP transports), so
//! boundary checks are the cooperative granularity the raw worker path also
//! provides.
//!
//! # Ref bridge
//!
//! FSZero mints `fz://blob/<sha256>` refs into its recovery store. The
//! aggregate connector verifies every emitted blob ref against the shared
//! CAS at `<session root>/blobs` ([`SharedCas::open`]); the adapter
//! re-publishes each emitted blob's bytes there via `session.expand` plus
//! [`SharedCas::put`], so reachability retention and later `fs.expand`
//! recovery work in-process regardless of FSZero's own store layout.
//!
//! # Outcome envelope
//!
//! The result `value` mirrors the raw worker's envelope: the serialized
//! [`DomainResult`] (operation, ok, ack, value, refs, mutated), the
//! dispatcher's inline evidence when present, and the recovery payload
//! (`ref` + `payload_utf8` / `payload_hex`) when the outcome minted one.
//!
//! # Approval mapping
//!
//! A mutation dispatched with a validated approval grant reports
//! [`EffectClass::ApprovalRequiredMutation`] with `Granted` (the connector
//! consumed and validated the grant before the call); a mutation without a
//! grant reports `ReversibleMutation` with `NotRequired`, exactly like the
//! raw worker — FSZero never gates on approvals itself.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fs_zero::core::{DispatchOutcome, dispatch_codemode_method};
use fs_zero::{
    DomainError, DomainResult, FSZeroSession, OPERATION_ABI_VERSION, RecoveryStore,
    operation_abi_digest,
};
use serde_json::Value;
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, EffectClass, EngineIdentity, EngineStageSpanV1,
    EngineStageTimelineV1, RefOwnership as WorkerRefOwnership, RevertMetadata, WorkerResult,
    WorkerResultMetadata,
};
use zero_codemode::CancellationSignal;
use zero_store::SharedCas;

use crate::adapter::{AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};

/// Canonical FSZero ref scheme.
pub const FSZERO_REF_SCHEME: &str = "fz://";

/// FSZero package version pinned by the immutable revision (`fs-zero` 0.1.0
/// at commit `82fd21a`), used as the default worker revision exactly like the
/// raw worker's `CARGO_PKG_VERSION` fallback.
pub const FSZERO_PINNED_VERSION: &str = "0.1.0";

/// How long [`FsZeroAdapter`]'s `Drop` waits for the session thread to stop.
const SESSION_THREAD_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`FsZeroAdapter::new`] waits for the session thread to open the
/// store (SQLite open plus the integrity gate can take a while on large
/// stores).
const SESSION_INIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum time a connector dispatcher waits before re-checking cancellation.
/// FSZero kernel work remains on its dedicated session thread; abandoning a
/// cancelled reply must never pin one of the shared aggregate dispatchers.
const CALL_REPLY_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Mirror of the raw worker's revision resolution: `ZEROSTACK_WORKER_REVISION`
/// wins when set, else the pinned revision version.
fn worker_revision() -> String {
    std::env::var("ZEROSTACK_WORKER_REVISION")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| FSZERO_PINNED_VERSION.to_owned())
}

/// True when the request deadline has already elapsed.
fn deadline_expired(request: &CallRequest) -> bool {
    request
        .deadline_unix_ms
        .is_some_and(|deadline| crate::connector::now_ms() >= deadline)
}

/// Typed adapter failure carrying the request trace.
fn adapter_error(kind: &str, message: impl Into<String>, request: &CallRequest) -> AdapterError {
    AdapterError::new(kind, message, false, Some(request.trace.clone()))
}

fn send_session_command(
    sender: &SyncSender<SessionCommand>,
    mut command: SessionCommand,
    cancellation: &CancellationSignal,
    request: &CallRequest,
) -> Result<(), AdapterError> {
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return Err(adapter_error(
                "cancelled",
                "fszero adapter cancelled while enqueueing dispatch",
                request,
            ));
        }
        if deadline_expired(request) {
            return Err(adapter_error(
                "deadline",
                "fszero adapter deadline exceeded while enqueueing dispatch",
                request,
            ));
        }
        if started.elapsed() >= SESSION_THREAD_STOP_TIMEOUT {
            return Err(adapter_error(
                "timeout",
                "fszero command channel did not accept within the drop bound",
                request,
            ));
        }
        match sender.try_send(command) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err(adapter_error(
                    "internal",
                    "fszero session thread is gone",
                    request,
                ));
            }
            Err(TrySendError::Full(returned)) => {
                command = returned;
                thread::sleep(CALL_REPLY_POLL_INTERVAL);
            }
        }
    }
}

fn join_session_thread(handle: JoinHandle<()>, timeout: Duration) {
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = handle.join();
        let _ = done_tx.send(());
    });
    if done_rx.recv_timeout(timeout).is_err() {
        eprintln!(
            "zsx-core: FSZero session thread did not stop within {}ms; detaching",
            timeout.as_millis()
        );
    }
}

fn receive_call_response(
    reply: &Receiver<Result<AdapterResponse, AdapterError>>,
    cancellation: &CancellationSignal,
    request: &CallRequest,
) -> Result<AdapterResponse, AdapterError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(adapter_error(
                "cancelled",
                "fszero adapter cancelled while awaiting dispatch",
                request,
            ));
        }
        if deadline_expired(request) {
            return Err(adapter_error(
                "deadline",
                "fszero adapter deadline exceeded while awaiting dispatch",
                request,
            ));
        }
        match reply.recv_timeout(CALL_REPLY_POLL_INTERVAL) {
            Ok(response) => return response,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(adapter_error(
                    "internal",
                    "fszero session thread is gone",
                    request,
                ));
            }
        }
    }
}

/// RW10 mask: shared [`zero_abi::is_rw10_forbidden_op`] (harness + fixture).
fn is_forbidden_operation(op: &str) -> bool {
    zero_abi::is_rw10_forbidden_op(op)
}

/// Mirror of `domain_error_kind`: map FSZero error classes onto the
/// raw-worker-v2 `WorkerError.kind` vocabulary.
fn domain_error_kind(class: &str) -> String {
    match class {
        "invalid_argument" => "validation".into(),
        "permission_denied" | "incompatible_contract" => "policy".into(),
        "cancelled" => "cancelled".into(),
        "deadline_exceeded" => "deadline_exceeded".into(),
        "busy" => "busy".into(),
        other => other.into(),
    }
}

/// Mirror of `is_conformant_blob_ref`: an `fz://blob/` ref must carry exactly
/// 64 lowercase hex characters; non-blob refs pass.
fn is_conformant_blob_ref(reference: &str) -> bool {
    let Some(hash) = reference.strip_prefix("fz://blob/") else {
        return true;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Mirror of `collect_portable_refs`: gather `fz://`/`gz://`/`tz://` tokens
/// embedded anywhere in the value (strings, arrays, objects).
fn collect_portable_refs(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            for token in text.split_whitespace() {
                for prefix in ["fz://", "gz://", "tz://"] {
                    if let Some(start) = token.find(prefix) {
                        let candidate = token[start..]
                            .trim_end_matches(['"', '\'', ',', ';', ')', '}', ']'])
                            .to_string();
                        if !refs.contains(&candidate) {
                            refs.push(candidate);
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_portable_refs(item, refs);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_portable_refs(item, refs);
            }
        }
        _ => {}
    }
}

/// Mirror of `retain_valid_refs`: drop refs that are not byte-conformant and
/// not expandable, reporting the first rejected ref so the boundary can fail
/// closed exactly like the raw worker.
fn retain_valid_refs(session: &FSZeroSession, refs: &mut Vec<String>) -> Option<String> {
    let mut rejected = None;
    refs.retain(|reference| {
        let valid = is_conformant_blob_ref(reference)
            && (!reference.starts_with("fz://blob/") || session.expand(reference).is_some());
        if !valid && rejected.is_none() {
            rejected = Some(reference.clone());
        }
        valid
    });
    rejected
}

fn collect_and_conform_refs(
    session: &FSZeroSession,
    request: &CallRequest,
    result: &DomainResult,
) -> Result<Vec<String>, AdapterError> {
    let mut refs = result.refs.clone();
    if let Some(value) = &result.value {
        collect_portable_refs(value, &mut refs);
    }
    if let Some(reference) = request.args.get("ref").and_then(Value::as_str)
        && ["fz://", "gz://", "tz://"]
            .iter()
            .any(|prefix| reference.starts_with(prefix))
        && !refs.iter().any(|value| value == reference)
    {
        refs.push(reference.to_owned());
    }
    if let Some(rejected) = retain_valid_refs(session, &mut refs) {
        return Err(adapter_error(
            "ref_conformance",
            format!(
                "refusing to emit non-conformant ref {rejected:?}: fz://blob refs must be 64 lowercase hex characters and expandable"
            ),
            request,
        ));
    }
    Ok(refs)
}

fn enrich_recovery_payload(
    session: &FSZeroSession,
    recovery_key: Option<&str>,
    result: &mut DomainResult,
    refs: &[String],
) {
    if let Some(key) = recovery_key
        .or_else(|| result.refs.first().map(String::as_str))
        && let Some(bytes) = session.expand(key)
    {
        let portable_ref = refs
            .iter()
            .find(|value| value.starts_with("fz://"))
            .cloned()
            .unwrap_or_else(|| key.to_string());
        let payload = match String::from_utf8(bytes) {
            Ok(text) => serde_json::json!({
                "ref": portable_ref,
                "payload_utf8": text,
            }),
            Err(error) => {
                let bytes = error.into_bytes();
                let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                serde_json::json!({
                    "ref": portable_ref,
                    "payload_hex": hex,
                    "bytes_len": bytes.len(),
                })
            }
        };
        result.value = Some(match result.value.take() {
            Some(Value::Object(mut map)) => {
                if let Value::Object(payload) = payload {
                    map.extend(payload);
                }
                Value::Object(map)
            }
            Some(prior) => serde_json::json!({ "prior": prior, "recovered": payload }),
            None => payload,
        });
    }
}

fn publish_refs_to_cas(
    session: &FSZeroSession,
    root: &std::path::Path,
    request: &CallRequest,
    refs: &[String],
) -> Result<(), AdapterError> {
    let cas = SharedCas::open(root);
    for reference in refs {
        if !reference.starts_with("fz://blob/") {
            continue;
        }
        if reference.contains('#') {
            continue;
        }
        let Some(bytes) = session.expand(reference) else {
            continue;
        };
        cas.put(&bytes).map_err(|error| {
            adapter_error(
                "cas",
                format!("cannot publish fszero ref {reference} to shared CAS: {error}"),
                request,
            )
        })?;
    }
    Ok(())
}

/// One in-process dispatch command, executed on the session thread.
enum SessionCommand {
    Call {
        request: CallRequest,
        cancellation: CancellationSignal,
        reply: SyncSender<Result<AdapterResponse, AdapterError>>,
    },
    Shutdown {
        reply: SyncSender<()>,
    },
}

/// Execute one canonical call against `session` and convert the outcome,
/// entirely on the session thread (the session is not `Send`).
fn run_call(
    session: &mut FSZeroSession,
    request: &CallRequest,
    cancellation: &CancellationSignal,
    root: &std::path::Path,
    session_id: &str,
) -> Result<AdapterResponse, AdapterError> {
    if cancellation.is_cancelled() {
        return Err(adapter_error(
            "cancelled",
            "fszero adapter cancelled",
            request,
        ));
    }
    if deadline_expired(request) {
        return Err(adapter_error(
            "deadline",
            "fszero adapter deadline exceeded",
            request,
        ));
    }
    if let Some(delay_ms) = request.args.get("__delay_ms").and_then(Value::as_u64) {
        let budget = Duration::from_millis(delay_ms);
        let started = Instant::now();
        while started.elapsed() < budget {
            if cancellation.is_cancelled() {
                return Err(adapter_error(
                    "cancelled",
                    "fszero adapter cancelled during delay",
                    request,
                ));
            }
            if deadline_expired(request) {
                return Err(adapter_error(
                    "deadline",
                    "fszero adapter deadline exceeded during delay",
                    request,
                ));
            }
            thread::sleep(CALL_REPLY_POLL_INTERVAL);
        }
    }
    if is_forbidden_operation(&request.op) {
        return Err(adapter_error(
            "forbidden",
            format!(
                "fszero adapter refuses planner/JavaScript/MCP operation '{}'",
                request.op
            ),
            request,
        ));
    }
    let outcome: DispatchOutcome =
        match dispatch_codemode_method(session, &request.op, &request.args) {
            Ok(outcome) => outcome,
            Err(error) => return Err(domain_error_to_adapter(&error, request)),
        };
    if cancellation.is_cancelled() {
        return Err(adapter_error(
            "cancelled",
            "fszero adapter cancelled",
            request,
        ));
    }
    if deadline_expired(request) {
        return Err(adapter_error(
            "deadline",
            "fszero adapter deadline exceeded",
            request,
        ));
    }
    let wall_ns = outcome.wall_ns.max(1);
    let inline_evidence = outcome.inline_evidence.clone();
    let recovery_key = outcome.recovery_key.clone();
    let mut result: DomainResult = outcome.result;
    if !result.ok {
        let error = result.error.unwrap_or_else(|| {
            DomainError::internal(format!(
                "fszero operation '{}' failed without typed error",
                request.op
            ))
        });
        return Err(domain_error_to_adapter(&error, request));
    }

    let refs = collect_and_conform_refs(session, request, &result)?;
    enrich_recovery_payload(session, recovery_key.as_deref(), &mut result, &refs);
    result.refs = refs.clone();

    // Serialized DomainResult envelope plus inline evidence (worker parity:
    // raw_worker_v2 merges evidence into the value object).
    let mut value = serde_json::to_value(&result).unwrap_or(Value::Null);
    if let (Some(evidence), Value::Object(map)) = (inline_evidence, &mut value) {
        map.insert("evidence".into(), serde_json::json!(evidence));
    }

    publish_refs_to_cas(session, root, request, &refs)?;

    let mutated = result.mutated;
    let (effect, approval) = if let (true, Some(grant)) = (mutated, request.approval_grant.as_ref())
    {
        (
            EffectClass::ApprovalRequiredMutation,
            ApprovalMetadata {
                state: ApprovalState::Granted,
                approval_id: Some(grant.grant_id.clone()),
                policy: Some(grant.policy_digest.clone()),
            },
        )
    } else if mutated {
        (
            EffectClass::ReversibleMutation,
            ApprovalMetadata {
                state: ApprovalState::NotRequired,
                approval_id: None,
                policy: None,
            },
        )
    } else {
        (
            EffectClass::ReadOnly,
            ApprovalMetadata {
                state: ApprovalState::NotRequired,
                approval_id: None,
                policy: None,
            },
        )
    };
    let journal_id = if mutated {
        refs.iter()
            .find(|value| value.contains("journal") || value.contains("undo"))
            .cloned()
    } else {
        None
    };
    let engine_timeline = request
        .telemetry_request
        .as_ref()
        .is_some_and(|value| value.engine_stage_timeline)
        .then(|| EngineStageTimelineV1 {
            total_ns: wall_ns,
            spans: vec![EngineStageSpanV1 {
                stage: "fszero.dispatch".into(),
                start_ns: 0,
                duration_ns: wall_ns,
            }],
        });

    Ok(AdapterResponse {
        result: WorkerResult {
            value,
            metadata: WorkerResultMetadata {
                effect,
                approval,
                revert: RevertMetadata {
                    supported: mutated,
                    journal_id,
                    rollback_op: mutated.then(|| "undo".into()),
                },
                ownership: WorkerRefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: session_id.to_owned(),
                    refs,
                    snapshot: None,
                },
                trace: request.trace.clone(),
            },
        },
        engine_timeline,
        worker_token_accounting: None,
    })
}

/// Convert one typed dispatcher failure into an adapter failure.
fn domain_error_to_adapter(error: &DomainError, request: &CallRequest) -> AdapterError {
    AdapterError::new(
        domain_error_kind(&error.class),
        error.message.clone(),
        error.retryable,
        Some(request.trace.clone()),
    )
}

/// Session-thread main loop: owns the single-threaded [`FSZeroSession`] and
/// serves one dispatch at a time.
fn session_loop(
    mut session: FSZeroSession,
    receiver: Receiver<SessionCommand>,
    root: PathBuf,
    session_id: String,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Call {
                request,
                cancellation,
                reply,
            } => {
                let response = run_call(&mut session, &request, &cancellation, &root, &session_id);
                let _ = reply.send(response);
            }
            SessionCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// How a constructor asked to open the FSZero session.
enum SessionOpen {
    /// Durable repo store under `root` (`.zerostack` / `.fszero`).
    RepoStore,
    /// Explicit sqlite path under the caller-authorized state root.
    Database(PathBuf),
    /// Test/fixture in-memory recovery. Never used as a durable-open fallback.
    InMemory,
}

/// Real FSZero in-process adapter over one [`FSZeroSession`] owned by a
/// dedicated session thread.
pub struct FsZeroAdapter {
    sender: SyncSender<SessionCommand>,
    session_thread: Option<JoinHandle<()>>,
    root: PathBuf,
    state_root: PathBuf,
    binding: AdapterBinding,
    /// True when the caller asked for a durable store and it could not be
    /// opened. The adapter is then inert: no session thread, no `with_root`
    /// fallback, and `call` fails closed.
    degraded: bool,
}

impl FsZeroAdapter {
    // Owner choice (pz1y): keep `new` / `new_with_state_root` infallible.
    // Durable failure is an inert adapter (`degraded()==true`, no session
    // thread, no `FSZeroSession::with_root`). Tests that need a live
    // in-memory engine call [`Self::new_in_memory`]. Do not change `new()`
    // to `Result` and `.expect` at every call site.

    /// Build the adapter over the immutable FSZero revision API rooted at
    /// `root`, bound to `session_id`.
    ///
    /// Opens the durable repo store (`.zerostack`/`.fszero`) when possible.
    /// If that open fails the adapter is inert (`degraded()==true`) and does
    /// not start [`FSZeroSession::with_root`].
    pub fn new(root: impl Into<PathBuf>, session_id: &str) -> Self {
        let root = root.into();
        let root = root.canonicalize().unwrap_or(root);
        Self::build(root.clone(), root, session_id, SessionOpen::RepoStore)
    }

    /// Build over `workspace_root` while keeping all durable engine and CAS
    /// state below the caller-authorized `state_root`.
    ///
    /// mkdir or durable-open failure returns an inert adapter. It does not
    /// spawn `FSZeroSession::with_root`.
    pub fn new_with_state_root(
        workspace_root: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        session_id: &str,
    ) -> Self {
        let workspace_root = workspace_root.into();
        let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
        let state_root = state_root.into();
        let fszero_dir = state_root.join("fszero");
        if std::fs::create_dir_all(&fszero_dir).is_err() {
            eprintln!(
                "zsx-core: FSZero durable state root unusable; refusing with_root fallback"
            );
            return Self::inert(workspace_root, state_root);
        }
        let database = fszero_dir.join("store.sqlite3");
        Self::build(
            workspace_root,
            state_root,
            session_id,
            SessionOpen::Database(database),
        )
    }

    /// Explicit in-memory session for fixtures. Not a durable-open fallback.
    pub fn new_in_memory(root: impl Into<PathBuf>, session_id: &str) -> Self {
        let root = root.into();
        let root = root.canonicalize().unwrap_or(root);
        Self::build(root.clone(), root, session_id, SessionOpen::InMemory)
    }

    fn fszero_binding() -> AdapterBinding {
        // FSZero equates the semantic contract digest with the operation ABI
        // digest (surface_handshake::local_capability).
        let digest = operation_abi_digest();
        AdapterBinding::new(
            EngineIdentity::FsZero,
            worker_revision(),
            OPERATION_ABI_VERSION,
            digest.clone(),
            digest,
            FSZERO_REF_SCHEME,
        )
        .expect("fszero binding is valid") // ubs:ignore — AdapterBinding constants are schema-valid
    }

    fn inert(root: PathBuf, state_root: PathBuf) -> Self {
        let (sender, _receiver) = mpsc::sync_channel(1);
        Self {
            sender,
            session_thread: None,
            root,
            state_root,
            binding: Self::fszero_binding(),
            degraded: true,
        }
    }

    fn build(root: PathBuf, state_root: PathBuf, session_id: &str, open: SessionOpen) -> Self {
        let binding = Self::fszero_binding();
        let (sender, receiver) = mpsc::sync_channel(1);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let thread_session_id = session_id.to_owned();
        let thread_root = root.clone();
        let thread_state_root = state_root.clone();
        // The session is not `Send` (single-threaded fsqlite connection), so
        // it is created on the session thread itself; only the root path
        // crosses the spawn boundary.
        let session_thread = thread::Builder::new()
            .name("zsx-fszero-session".into())
            .spawn(move || {
                let opened = match open {
                    SessionOpen::InMemory => Ok(FSZeroSession::with_root(&thread_root)),
                    SessionOpen::RepoStore => FSZeroSession::try_with_repo_store(&thread_root),
                    SessionOpen::Database(database) => match RecoveryStore::try_with_durable(&database)
                    {
                        Ok(store) => {
                            drop(store);
                            Ok(FSZeroSession::with_durable_root(&thread_root, database))
                        }
                        Err(error) => Err(error),
                    },
                };
                match opened {
                    Ok(session) => {
                        let _ = init_tx.send(false);
                        session_loop(session, receiver, thread_state_root, thread_session_id);
                    }
                    Err(error) => {
                        eprintln!(
                            "zsx-core: FSZero durable store unavailable ({error}); refusing with_root fallback"
                        );
                        let _ = init_tx.send(true);
                    }
                }
            })
            .expect("cannot start fszero session thread"); // ubs:ignore — constructor is documented infallible; thread spawn failure is process-fatal
        let degraded = init_rx.recv_timeout(SESSION_INIT_TIMEOUT).unwrap_or(true);
        Self {
            sender,
            session_thread: Some(session_thread),
            root,
            state_root,
            binding,
            degraded,
        }
    }

    /// True when the caller asked for a durable store and it could not be
    /// opened. The adapter is then inert and must not be treated as a live
    /// in-memory engine.
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    #[cfg(test)]
    fn session_is_live(&self) -> bool {
        self.session_thread.is_some() && !self.degraded
    }

    /// The session root this adapter serves.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Root containing this adapter's durable state and shared CAS.
    pub fn state_root(&self) -> &std::path::Path {
        &self.state_root
    }
}

impl Drop for FsZeroAdapter {
    fn drop(&mut self) {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let mut command = SessionCommand::Shutdown { reply: reply_tx };
        let started = Instant::now();
        let sent = loop {
            match self.sender.try_send(command) {
                Ok(()) => break true,
                Err(TrySendError::Disconnected(_)) => break false,
                Err(TrySendError::Full(returned)) => {
                    if started.elapsed() >= SESSION_THREAD_STOP_TIMEOUT {
                        break false;
                    }
                    command = returned;
                    thread::sleep(CALL_REPLY_POLL_INTERVAL);
                }
            }
        };
        if sent {
            let _ = reply_rx.recv_timeout(SESSION_THREAD_STOP_TIMEOUT);
        }
        if let Some(handle) = self.session_thread.take() {
            join_session_thread(handle, SESSION_THREAD_STOP_TIMEOUT);
        }
    }
}

impl DomainAdapter for FsZeroAdapter {
    fn engine(&self) -> EngineIdentity {
        EngineIdentity::FsZero
    }

    fn binding(&self) -> AdapterBinding {
        self.binding.clone()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        let request = call.request;
        if self.degraded || self.session_thread.is_none() {
            return Err(adapter_error(
                "backend_unavailable",
                "FSZero durable store unavailable; refusing in-memory fallback",
                request,
            ));
        }
        // Cancellation and deadline are checked before enqueueing and again
        // after the reply arrives; the session thread re-checks both right
        // before and right after the dispatch itself.
        if call.cancellation.is_cancelled() {
            return Err(adapter_error(
                "cancelled",
                "fszero adapter cancelled",
                request,
            ));
        }
        if deadline_expired(request) {
            return Err(adapter_error(
                "deadline",
                "fszero adapter deadline exceeded",
                request,
            ));
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let command = SessionCommand::Call {
            request: request.clone(),
            cancellation: call.cancellation.clone(),
            reply: reply_tx,
        };
        send_session_command(&self.sender, command, call.cancellation, request)?;
        let response = receive_call_response(&reply_rx, &call.cancellation, request)?;
        if call.cancellation.is_cancelled() {
            return Err(adapter_error(
                "cancelled",
                "fszero adapter cancelled",
                request,
            ));
        }
        if deadline_expired(request) {
            return Err(adapter_error(
                "deadline",
                "fszero adapter deadline exceeded",
                request,
            ));
        }
        Ok(response)
    }
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx-core/unit/fszero.rs"]
mod tests;
