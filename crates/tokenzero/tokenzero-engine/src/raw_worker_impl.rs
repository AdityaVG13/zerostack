use super::*;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokenzero_core::Accounting;
use zero_process::VerifiedChild;

/// Advertised and enforced raw-worker v2 output cap (9lwo): the serialized
/// `result.value` of any call must fit within this many bytes. One constant
/// drives both the handshake advertisement and the dispatch-time enforcement
/// so the two cannot drift.
pub(crate) const MAX_OUTPUT_BYTES: usize = 65_536;

#[derive(Default)]
pub struct RawWorkerSession {
    binding: Option<Binding>,
    shutdown: bool,
    expected_root: Option<String>,
    expected_session_id: Option<String>,
    cancel_registry: std::collections::HashMap<String, Arc<CancelState>>,
    /// Exact tree handles copied out of `cancel_registry` so the serve loop
    /// can drop the session mutex before `cancel_child` (subprocess teardown).
    pending_teardown: Vec<VerifiedChild>,
}

#[derive(Debug)]
struct Binding {
    root: String,
    session_id: String,
    revision: String,
    contract: String,
}

impl RawWorkerSession {
    pub fn for_binding(root: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            binding: None,
            shutdown: false,
            expected_root: Some(root.into()),
            expected_session_id: Some(session_id.into()),
            cancel_registry: std::collections::HashMap::new(),
            pending_teardown: Vec::new(),
        }
    }
}

/// Cancellation state for one in-flight v2 call. The cancel control frame
/// sets `flag` and signals the exact hub-owned tree handle; the worker
/// thread observes the flag after dispatch returns.
#[derive(Default)]
struct CancelState {
    flag: Arc<AtomicBool>,
    child: Mutex<Option<VerifiedChild>>,
}

/// The cancel state of the call currently executing on the serve worker
/// thread (at most one: `max_in_flight` is 1).
static ACTIVE_CANCEL: Mutex<Option<Arc<CancelState>>> = Mutex::new(None);

/// Bounded teardown of the exact owned tree under the TokenZero engine
/// binding. Numeric pid/pgid values are never signaled; the hub-owned handle
/// is the only authority.
fn cancel_child(child: &VerifiedChild) {
    let _ = child.signal_graceful_for(
        tokenzero_runtime::PROCESS_OWNER_SESSION,
        tokenzero_runtime::PROCESS_GENERATION,
        tokenzero_runtime::SHELL_TEARDOWN_GRACE,
    );
    let _ = child.revoke();
}

fn run_pending_teardown(children: Vec<VerifiedChild>) {
    for child in &children {
        cancel_child(child);
    }
}

/// shell_hooks evidence entry for the dispatched child. The pid/pgid values
/// are observation evidence only; when cancellation already landed, the
/// exact published tree handle is signaled (spawn/cancel race is decided in
/// favor of the cancel).
fn v2_note_child(_pid: Option<u32>, _pgid: Option<u32>, state: &'static str) {
    let Some(cancel) = ACTIVE_CANCEL
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned())
    else {
        return;
    };
    if state != "running" {
        return;
    }
    // The runtime publishes the exact handle before this evidence call, so
    // the cancel registry retains the owned tree instead of a numeric pid.
    if let Some(child) = crate::engine_shell::dispatch_child() {
        *cancel.child.lock().unwrap_or_else(|p| p.into_inner()) = Some(child.clone());
        if cancel.flag.load(Ordering::SeqCst) {
            cancel_child(&child);
        }
    }
}

fn revision() -> String {
    std::env::var("ZEROSTACK_WORKER_REVISION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn error(id: Option<&str>, kind: &str, message: impl Into<String>, trace: Option<Value>) -> Value {
    error_with(id, kind, message, trace, false)
}

fn error_with(
    id: Option<&str>,
    kind: &str,
    message: impl Into<String>,
    trace: Option<Value>,
    retryable: bool,
) -> Value {
    let mut value = json!({"kind":"error","error":{"kind":kind,"message":message.into(),"retryable":retryable}});
    if let Some(id) = id {
        value["request_id"] = json!(id);
    }
    if let Some(trace) = trace {
        value["trace"] = trace;
    }
    value
}

fn worker_error_frame(
    request_id: Option<String>,
    kind: &str,
    message: &str,
) -> zero_abi::WorkerResponseFrame {
    zero_abi::WorkerResponseFrame::Error {
        request_id,
        error: zero_abi::WorkerError {
            kind: kind.to_string(),
            message: message.to_string(),
            retryable: false,
            details: None,
        },
        trace: None,
        engine_timeline: None,
        worker_token_accounting: None,
    }
}

fn encode_fallback(request_id: Option<String>, kind: &str, message: &str) -> Vec<u8> {
    let correlated = worker_error_frame(request_id, kind, message);
    if zero_abi::validate_response_frame(&correlated).is_ok()
        && let Ok(bytes) = zero_abi::encode_frame(&correlated, zero_abi::DEFAULT_MAX_FRAME_BYTES)
    {
        return bytes;
    }
    let fixed = worker_error_frame(None, kind, message);
    zero_abi::validate_response_frame(&fixed).expect("fixed worker error frame is valid");
    zero_abi::encode_frame(&fixed, zero_abi::DEFAULT_MAX_FRAME_BYTES)
        .expect("fixed worker error frame fits the protocol bound")
}

fn encode(value: Value) -> Vec<u8> {
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|request_id| !request_id.is_empty())
        .map(str::to_string);
    // Stage through the shared encoder so the hub decoder can enforce strict
    // wire rules (including unit-variant unknown fields) before final typed
    // encoding. TokenZero owns no response serializer or validator here.
    let staged = match zero_abi::encode_frame(&value, zero_abi::DEFAULT_MAX_FRAME_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => {
            return encode_fallback(
                request_id,
                error.kind(),
                "outbound frame exceeds the raw-worker protocol bound",
            );
        }
    };
    let frame = match zero_abi::decode_response_frame(&staged, zero_abi::DEFAULT_MAX_FRAME_BYTES) {
        Ok(frame) => frame,
        Err(_) => {
            return encode_fallback(
                request_id,
                "internal_contract",
                "worker produced an invalid response frame",
            );
        }
    };
    if zero_abi::validate_response_frame(&frame).is_err() {
        return encode_fallback(
            request_id,
            "internal_contract",
            "worker produced an invalid response frame",
        );
    }
    match zero_abi::encode_frame(&frame, zero_abi::DEFAULT_MAX_FRAME_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => encode_fallback(
            request_id,
            error.kind(),
            "outbound frame exceeds the raw-worker protocol bound",
        ),
    }
}

fn local_capability() -> Value {
    serde_json::to_value(crate::surface_handshake::build_surface_capability(
        crate::surface_handshake::HandshakeSurface::RawWorker,
    ))
    .expect("capability serializes")
}

fn refs(value: &Value, output: &mut Vec<Value>) {
    match value {
        Value::String(v) if v.starts_with("tz://") => output.push(json!(v)),
        Value::Array(v) => v.iter().for_each(|v| refs(v, output)),
        Value::Object(v) => v.values().for_each(|v| refs(v, output)),
        _ => {}
    }
}

fn forbidden(op: &str) -> bool {
    let op = op.to_ascii_lowercase();
    matches!(
        op.as_str(),
        "plan"
            | "planner"
            | "js"
            | "javascript"
            | "mcp"
            | "execute_code"
            | "tz_execute_code"
            | "codemode_search"
            | "tz_codemode_search"
            | "codemode_describe"
            | "tz_codemode_describe"
            | "tools/call"
            | "tools/list"
    ) || op.starts_with("planner.")
        || op.starts_with("javascript.")
        || op.starts_with("mcp.")
}

impl RawWorkerSession {
    fn register_cancel(&mut self, id: &str) -> Arc<CancelState> {
        let cancel = Arc::new(CancelState::default());
        self.cancel_registry.insert(id.to_string(), cancel.clone());
        cancel
    }

    fn finish_call(&mut self, id: &str) {
        self.cancel_registry.remove(id);
    }

    /// Cancel an in-flight call: set the flag, then queue the exact owned
    /// tree handle so the caller can drop the session mutex before
    /// `cancel_child` (subprocess teardown). Returns false for unknown or
    /// already-finished request ids.
    fn cancel_call(&mut self, id: &str) -> bool {
        match self.cancel_registry.remove(id) {
            Some(cancel) => {
                cancel.flag.store(true, Ordering::SeqCst);
                if let Some(child) = cancel
                    .child
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
                {
                    self.pending_teardown.push(child);
                }
                true
            }
            None => false,
        }
    }

    /// Cancel every active or queued call before the session dispatch thread
    /// is joined. A child spawned after this point observes the flag in
    /// `v2_note_child` and is signaled immediately through its exact handle.
    fn cancel_all(&mut self) {
        for (_, cancel) in self.cancel_registry.drain() {
            cancel.flag.store(true, Ordering::SeqCst);
            if let Some(child) = cancel
                .child
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
            {
                self.pending_teardown.push(child);
            }
        }
    }

    fn take_pending_teardown(&mut self) -> Vec<VerifiedChild> {
        std::mem::take(&mut self.pending_teardown)
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const DEFAULT_DEADLINE_MS: u64 = 30_000;

fn is_shell_op(op: &str) -> bool {
    matches!(op, "shell" | "tz_shell" | "zero.shell")
}

fn effect_class(op: &str) -> &'static str {
    match op {
        "shell" | "tz_shell" | "zero.shell" | "compact" | "tz_compact" | "zero.compact"
        | "ingest" | "tz_ingest" | "zero.ingest" => "irreversible",
        _ => "read_only",
    }
}

fn checked_u64_count(field: &str, value: usize) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} exceeds the raw-worker accounting range"))
}

fn encoded_len(field: &str, value: &Value) -> Result<u64, String> {
    serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode {field} for token accounting: {error}"))
        .and_then(|bytes| checked_u64_count(field, bytes.len()))
}

fn declared_recovery_bytes(value: &Value, allow_missing: bool) -> Result<u64, String> {
    let Some(refs) = value.get("refs") else {
        return if allow_missing {
            Ok(0)
        } else {
            Err("successful domain result omitted refs".to_string())
        };
    };
    let refs = refs
        .as_array()
        .ok_or_else(|| "successful domain result refs must be an array".to_string())?;
    refs.iter().try_fold(0_u64, |total, record| {
        let bytes = record
            .get("bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| "domain ref omitted a valid bytes count".to_string())?;
        total
            .checked_add(bytes)
            .ok_or_else(|| "declared recovery bytes overflowed".to_string())
    })
}

fn worker_token_accounting(
    op: &str,
    args: &Value,
    value: &Value,
) -> Result<raw_worker_protocol::WorkerTokenAccounting, String> {
    let is_job = op == zero_abi::TOKEN_JOB_OPERATION;
    let is_background_shell = is_shell_op(op) && args["background"] == true;
    let accounting_optional = is_job || is_background_shell;
    let mut accounting = value
        .get("accounting")
        .map(|accounting| {
            serde_json::from_value::<Accounting>(accounting.clone())
                .map_err(|error| format!("invalid domain accounting: {error}"))
        })
        .transpose()?;
    if accounting.is_none() && !accounting_optional {
        return Err("successful domain result omitted accounting".to_string());
    }
    if let Some(accounting) = accounting.as_mut() {
        accounting.stamp_tokenizer();
    }
    if accounting
        .as_ref()
        .is_some_and(|accounting| accounting.cached_tokens > accounting.billed_tokens)
    {
        return Err("worker token accounting cached_tokens exceeds billed_tokens".to_string());
    }
    // Domain accounting is the kernel-measure estimate (estimator: or tiktoken:),
    // never an invented ExactTokenizerIdentity. When the domain omitted
    // accounting (jobs / background shell), fall back to the byte-mass
    // estimator rather than an unlabeled conservative id.
    let input_bytes = encoded_len("request args", args)?;
    let output_bytes = encoded_len("domain result", value)?;
    let recovery_bytes = declared_recovery_bytes(value, accounting_optional)?;
    let raw_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("raw_tokens", accounting.raw_tokens))
        .transpose()?
        .unwrap_or(
            input_bytes
                .checked_add(output_bytes)
                .and_then(|value| value.checked_add(recovery_bytes))
                .ok_or_else(|| "raw token upper bound overflowed".to_string())?,
        );
    let visible_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("visible_tokens", accounting.visible_tokens))
        .transpose()?
        .unwrap_or(output_bytes);
    let recovery_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("recovery_tokens", accounting.recovery_tokens))
        .transpose()?
        .unwrap_or(recovery_bytes);
    let billed_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("billed_tokens", accounting.billed_tokens))
        .transpose()?
        .unwrap_or(output_bytes);
    let cached_tokens = accounting
        .as_ref()
        .map(|accounting| checked_u64_count("cached_tokens", accounting.cached_tokens))
        .transpose()?
        .unwrap_or(0);
    let exact_ref_tokens = accounting
        .as_ref()
        .and_then(|accounting| accounting.exact_ref_tokens)
        .map(|value| checked_u64_count("exact_ref_tokens", value))
        .transpose()?;
    let tokenizer_id = accounting
        .as_ref()
        .map(|accounting| accounting.tokenizer_id.clone())
        .unwrap_or_else(|| tokenzero_core::BYTES_ESTIMATOR_ID.to_string());
    let count_kind = if tokenizer_id.starts_with("estimator:") {
        raw_worker_protocol::WorkerTokenCountKind::Estimate
    } else {
        raw_worker_protocol::WorkerTokenCountKind::ConservativeUpperBound
    };
    let worker = raw_worker_protocol::WorkerTokenAccounting {
        tokenizer_version_digest: None,
        tokenizer_id,
        count_kind,
        raw_tokens,
        visible_tokens,
        recovery_tokens,
        billed_tokens,
        cached_tokens,
        exact_ref_tokens,
    };
    zero_abi::validate_worker_token_accounting(&worker)
        .map_err(|error| format!("invalid worker token accounting: {error}"))?;
    Ok(worker)
}

fn attach_engine_timeline(
    mut frame: Value,
    requested: bool,
    elapsed: std::time::Duration,
) -> Value {
    if requested {
        let duration_ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        let duration_ns = duration_ns.max(1);
        let timeline = raw_worker_protocol::EngineStageTimeline {
            total_ns: duration_ns,
            spans: vec![raw_worker_protocol::EngineStageSpan {
                stage: "tokenzero.raw_worker_call".to_string(),
                start_ns: 0,
                duration_ns,
            }],
        };
        frame["engine_timeline"] =
            serde_json::to_value(timeline).expect("engine timeline serializes");
    }
    frame
}

/// A validated call ready for dispatch, carrying cloned binding fields so
/// the session lock is never held while work runs.
#[derive(Debug)]
struct CallCtx {
    id: String,
    op: String,
    args: Value,
    trace: Value,
    deadline_unix_ms: Option<u64>,
    engine_stage_timeline_requested: bool,
    worker_token_accounting_requested: bool,
    session_id: String,
    contract: String,
}

enum RoutedFrame {
    Respond(Vec<u8>),
    Dispatch(CallCtx),
}

pub fn execute_raw_worker_frame(
    engine: &TokenZeroEngine,
    session: &mut RawWorkerSession,
    line: &[u8],
) -> Vec<u8> {
    match route_frame(session, line) {
        RoutedFrame::Respond(bytes) => {
            // SAFETY: `session` here is `&mut` (no mutex). Still run teardown
            // after routing so the Mutex serve-loop path and this path share
            // one cancel protocol: queue under the session, signal after.
            run_pending_teardown(session.take_pending_teardown());
            bytes
        }
        RoutedFrame::Dispatch(ctx) => {
            let id = ctx.id.clone();
            let cancel = session.register_cancel(&ctx.id);
            let value = run_call_registered(engine, ctx, cancel);
            session.finish_call(&id);
            encode(value)
        }
    }
}

/// Frame routing without dispatch: control frames (handshake/shutdown/cancel)
/// and validation failures produce an encoded response; valid calls come back
/// for dispatch so the serve loop can run them off the read loop.
fn route_frame(session: &mut RawWorkerSession, line: &[u8]) -> RoutedFrame {
    if let Err(e) = raw_worker_protocol::decode_request_frame(
        line,
        raw_worker_protocol::DEFAULT_MAX_FRAME_BYTES,
    ) {
        return RoutedFrame::Respond(encode(error(None, e.kind(), e.to_string(), None)));
    }
    let frame: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(e) => {
            return RoutedFrame::Respond(encode(error(None, "invalid_json", e.to_string(), None)));
        }
    };
    let kind = frame["kind"].as_str().unwrap_or_default();
    let request = &frame["request"];

    if kind == "handshake" {
        let cap = local_capability();
        let rev = revision();
        let contract = cap["semantic_contract_digest"].as_str().unwrap_or_default();
        let registry = cap["operation_registry_digest"]
            .as_str()
            .unwrap_or_default();
        let root = request["root"].as_str().unwrap_or_default();
        let session_id = request["session_id"].as_str().unwrap_or_default();
        if let Some(existing) = session.binding.as_ref() {
            // A revision swap on the host side is survivable: the same
            // root+session may re-handshake to rebind. A different root or
            // session on an established binding stays terminal.
            if existing.root != root || existing.session_id != session_id {
                return RoutedFrame::Respond(encode(error(
                    None,
                    "already_bound",
                    "session is already bound",
                    None,
                )));
            }
        }
        let revision_mismatch = request
            .get("expected_worker_revision")
            .and_then(Value::as_str)
            .is_some_and(|v| v != rev);
        let mismatch = request["protocol_version"].as_str()
            != Some(raw_worker_protocol::RAW_WORKER_PROTOCOL_VERSION)
            || root.is_empty()
            || session_id.is_empty()
            || session
                .expected_root
                .as_deref()
                .is_some_and(|expected| expected != root)
            || session
                .expected_session_id
                .as_deref()
                .is_some_and(|expected| expected != session_id)
            || request["expected_engine"].as_str() != Some("tokenzero")
            || request["expected_contract_digest"].as_str() != Some(contract)
            || request
                .get("expected_registry_digest")
                .and_then(Value::as_str)
                .is_some_and(|v| v != registry);
        if mismatch {
            return RoutedFrame::Respond(encode(error(
                None,
                "binding_mismatch",
                "worker handshake binding mismatch",
                None,
            )));
        }
        if revision_mismatch {
            // Stale revision pin: retryable so the host re-handshakes against
            // the current revision instead of terminally aborting the plan.
            return RoutedFrame::Respond(encode(error_with(
                None,
                "worker_revision_changed",
                "worker revision changed; re-handshake without the stale expected_worker_revision pin",
                None,
                true,
            )));
        }
        session.binding = Some(Binding {
            root: root.into(),
            session_id: session_id.into(),
            revision: rev.clone(),
            contract: contract.into(),
        });
        return RoutedFrame::Respond(encode(json!({"kind":"handshake_ack","ack":{
            "protocol_version":raw_worker_protocol::RAW_WORKER_PROTOCOL_VERSION,
            "binding":{"engine":"tokenzero","root":root,"session_id":session_id,"worker_revision":rev,
                "semantic_contract_version":cap["semantic_contract_version"],"semantic_contract_digest":contract,
                "operation_registry_digest":registry,"ref_scheme":"tz://"},
            "capabilities":{"cancellation":true,"deadlines":true,"approvals":false,"revert":false,"snapshots":false},
            "limits":{"max_frame_bytes":1048576,"max_output_bytes":MAX_OUTPUT_BYTES,"max_in_flight":1,"default_deadline_ms":DEFAULT_DEADLINE_MS},
            "protocol_digest":raw_worker_protocol::raw_worker_protocol_digest_hex()
        }})));
    }
    if kind == "shutdown" {
        session.shutdown = true;
        return RoutedFrame::Respond(encode(json!({"kind":"shutdown_ack"})));
    }
    if session.binding.is_none() {
        return RoutedFrame::Respond(encode(error(
            None,
            "handshake_required",
            "v2 handshake required before calls",
            None,
        )));
    }
    if session.shutdown {
        return RoutedFrame::Respond(encode(error(
            None,
            "session_shutdown",
            "session has shut down",
            None,
        )));
    }
    if kind == "cancel" {
        let cancelled = session.cancel_call(request["request_id"].as_str().unwrap_or_default());
        return RoutedFrame::Respond(encode(
            json!({"kind":"cancel_ack","request_id":request["request_id"],"cancelled":cancelled}),
        ));
    }
    let binding = session.binding.as_ref().expect("binding checked above");
    let validation_started = Instant::now();
    match validate_call(binding, &frame) {
        Ok(ctx) => RoutedFrame::Dispatch(ctx),
        Err(value) => {
            let timeline_requested = request
                .get("telemetry_request")
                .and_then(|request| request.get("engine_stage_timeline"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            RoutedFrame::Respond(encode(attach_engine_timeline(
                value,
                timeline_requested,
                validation_started.elapsed(),
            )))
        }
    }
}

fn validate_call(binding: &Binding, frame: &Value) -> Result<CallCtx, Value> {
    let request = &frame["request"];
    let id = request["request_id"].as_str().unwrap_or_default();
    let op = request["op"].as_str().unwrap_or_default();
    let trace = request["trace"].clone();
    if trace["request_id"].as_str() != Some(id)
        || trace["contract_digest"].as_str() != Some(binding.contract.as_str())
    {
        return Err(error(
            Some(id),
            "trace_binding_mismatch",
            "trace does not match handshake binding",
            Some(trace),
        ));
    }
    if trace["worker_revision"].as_str() != Some(binding.revision.as_str()) {
        // Revision drift between handshake and call: typed retryable so the
        // host re-handshakes and retries instead of killing the plan.
        return Err(error_with(
            Some(id),
            "worker_revision_changed",
            "worker revision changed since handshake; re-handshake and retry the call",
            Some(trace),
            true,
        ));
    }
    let deadline_unix_ms = request.get("deadline_unix_ms").and_then(Value::as_u64);
    let telemetry_request = request.get("telemetry_request");
    let engine_stage_timeline_requested = telemetry_request
        .and_then(|request| request.get("engine_stage_timeline"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let worker_token_accounting_requested = telemetry_request
        .and_then(|request| request.get("worker_token_accounting"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if deadline_unix_ms.is_some_and(|v| v <= unix_ms()) {
        return Err(error(
            Some(id),
            "deadline_exceeded",
            "deadline expired before dispatch",
            Some(trace),
        ));
    }
    if forbidden(op) {
        return Err(error(
            Some(id),
            "unsupported_operation",
            "planner, JavaScript, and MCP operations are forbidden",
            Some(trace),
        ));
    }
    Ok(CallCtx {
        id: id.to_string(),
        op: op.to_string(),
        args: request["args"].clone(),
        trace,
        deadline_unix_ms,
        engine_stage_timeline_requested,
        worker_token_accounting_requested,
        session_id: binding.session_id.clone(),
        contract: binding.contract.clone(),
    })
}

/// Run a validated call under the active cancel registration and the wall
/// deadline derived from `deadline_unix_ms` (default 30 s, matching the
/// advertised handshake limit).
fn run_call_registered(engine: &TokenZeroEngine, ctx: CallCtx, cancel: Arc<CancelState>) -> Value {
    let started = Instant::now();
    let value = if cancel.flag.load(Ordering::SeqCst) {
        json!({"kind":"error","request_id":ctx.id.clone(),"error":{
            "kind":"cancelled","message":"call cancelled before dispatch","retryable":false
        },"trace":ctx.trace.clone()})
    } else {
        *ACTIVE_CANCEL.lock().unwrap_or_else(|p| p.into_inner()) = Some(cancel.clone());
        let value = dispatch_call(engine, &ctx, &cancel);
        *ACTIVE_CANCEL.lock().unwrap_or_else(|p| p.into_inner()) = None;
        value
    };
    attach_engine_timeline(
        value,
        ctx.engine_stage_timeline_requested,
        started.elapsed(),
    )
}

fn verified_cancelled_shell_partial_result(ctx: &CallCtx, response: &Value) -> Option<Value> {
    if !is_shell_op(&ctx.op) {
        return None;
    }
    let result = response.get("result")?;
    let tool_response = result.get("tool_response")?;
    let refs = tool_response.get("refs")?.as_array()?;
    let verified = !refs.is_empty()
        && tool_response["safety"]["refs_cover_full_output"].as_bool() == Some(true)
        && tool_response["telemetry"]["refs_cover_full_output"].as_bool() == Some(true);
    verified.then(|| result.clone())
}

/// Dispatch a validated call. Cancellation observed after dispatch maps to a
/// typed `cancelled` error; the remaining deadline is pushed into shell work
/// as a process timeout and into search/expand loops as wall checkpoints.
fn dispatch_call(engine: &TokenZeroEngine, ctx: &CallCtx, cancel: &Arc<CancelState>) -> Value {
    let remaining = ctx
        .deadline_unix_ms
        .map(|deadline| deadline.saturating_sub(unix_ms()))
        .unwrap_or(DEFAULT_DEADLINE_MS)
        .max(1);
    let wall = crate::wall::WallDeadline::new(Instant::now(), remaining);
    let response = crate::wall::with_host_wall_deadline_and_cancel(
        wall,
        Arc::clone(&cancel.flag),
        || {
            let mut args = ctx.args.clone();
            if is_shell_op(&ctx.op)
                && let Value::Object(ref mut map) = args
            {
                let requested = ["timeout_ms", "timeoutMs", "shell_timeout_ms"]
                    .iter()
                    .find_map(|key| map.get(*key).and_then(Value::as_u64));
                map.insert(
                    "timeout_ms".to_string(),
                    json!(requested.map_or(remaining, |r| r.min(remaining))),
                );
            }
            match crate::domain::execute_embedded_value(engine, &ctx.op, &args) {
                Some(Ok(value)) => json!({"ok":true,"result":value}),
                Some(Err(error)) => json!({"ok":false,"error":{
                    "kind":error.kind,"message":error.message,"retryable":false
                }}),
                None => {
                    let v1 = json!({"id":ctx.id.clone(),"op":ctx.op.clone(),"args":args,"peer_contract_digest":ctx.contract.clone()});
                    execute_raw_worker_json(engine, &v1)
                }
            }
        },
    );
    if cancel.flag.load(Ordering::SeqCst) {
        let mut cancelled = json!({"kind":"error","request_id":ctx.id.clone(),"error":{
            "kind":"cancelled","message":"call cancelled by control frame","retryable":false
        },"trace":ctx.trace.clone()});
        if let Some(partial_result) = verified_cancelled_shell_partial_result(ctx, &response) {
            cancelled["error"]["details"] = json!({
                "partial_result": partial_result,
                "artifact_scope": "full_observed_stdout_stderr_streams",
                "temporal_interleaving_claimed": false
            });
        }
        return cancelled;
    }
    if response["ok"].as_bool() != Some(true) {
        let e = &response["error"];
        return json!({"kind":"error","request_id":ctx.id.clone(),"error":{
            "kind":e["kind"].as_str().unwrap_or("operation_failed"),
            "message":e["message"].as_str().unwrap_or("operation failed"),
            "retryable":e["retryable"].as_bool().unwrap_or(false),"details":e.get("details").cloned().unwrap_or(Value::Null)
        },"trace":ctx.trace.clone()});
    }
    let value = response.get("result").cloned().unwrap_or(Value::Null);
    // 9lwo: advertised limits must be effective. Measure the serialized
    // result.value bytes (not framing/metadata); an oversized value becomes a
    // typed, correlated error naming the limit and the actual/cap sizes --
    // never a truncated result, and the error frame stays far below
    // max_frame_bytes because the payload is not included.
    let output_bytes = serde_json::to_vec(&value).map_or(0, |bytes| bytes.len());
    if output_bytes > MAX_OUTPUT_BYTES {
        return json!({"kind":"error","request_id":ctx.id.clone(),"error":{
            "kind":"output_too_large",
            "message":format!(
                "operation result is {output_bytes} bytes; the advertised max_output_bytes limit is {MAX_OUTPUT_BYTES}"
            ),
            "retryable":false,
            "details":{
                "limit_name":"max_output_bytes",
                "limit_bytes":MAX_OUTPUT_BYTES,
                "actual_bytes":output_bytes,
                "frame_limit_bytes":zero_abi::DEFAULT_MAX_FRAME_BYTES
            }
        },"trace":ctx.trace.clone()});
    }
    let worker_token_accounting = if ctx.worker_token_accounting_requested {
        match worker_token_accounting(&ctx.op, &ctx.args, &value) {
            Ok(accounting) => Some(accounting),
            Err(message) => {
                return json!({"kind":"error","request_id":ctx.id.clone(),"error":{
                    "kind":"invalid_token_accounting","message":message,"retryable":false
                },"trace":ctx.trace.clone()});
            }
        }
    } else {
        None
    };
    let mut owned_refs = Vec::new();
    // Job tails are arbitrary shell bytes. A line beginning with `tz://` is
    // content, not a minted ref, so job results never contribute ownership.
    if ctx.op != zero_abi::TOKEN_JOB_OPERATION {
        refs(&value, &mut owned_refs);
    }
    let mut frame = json!({"kind":"result","request_id":ctx.id.clone(),"result":{"value":value,"metadata":{
        "effect":effect_class(ctx.op.as_str()),
        "approval":{"state":"not_required"},"revert":{"supported":false},
        "ownership":{"engine":"tokenzero","session_id":ctx.session_id.clone(),"refs":owned_refs},"trace":ctx.trace.clone()
    }}});
    if let Some(accounting) = worker_token_accounting {
        frame["worker_token_accounting"] =
            serde_json::to_value(accounting).expect("worker token accounting serializes");
    }
    frame
}

struct CallJob {
    ctx: CallCtx,
    cancel: Arc<CancelState>,
}

fn write_response(writer: &Mutex<std::io::Stdout>, response: &[u8]) -> std::io::Result<()> {
    let mut out = writer
        .lock()
        .map_err(|_| std::io::Error::other("writer poisoned"))?;
    out.write_all(response)?;
    out.flush()
}

fn terminate_raw_worker_session(session: &Mutex<RawWorkerSession>) {
    let teardown = {
        let mut guard = session.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.cancel_all();
        guard.take_pending_teardown()
    };
    // SAFETY: `session` is the cancel-registry occupancy lock, not a persist
    // gate. `cancel_child` waits on subprocess teardown. Sibling of
    // `BackgroundJobRegistry::terminate_all`: copy handles out, drop, then
    // signal so `finish_call` cannot stall on SIGTERM/grace.
    run_pending_teardown(teardown);
    // The job registry is process-global and therefore has no reliable static
    // destructor. Mark every live job for termination before serve can exit;
    // a child published after this scan observes the mark and is killed too.
    crate::engine_shell::terminate_all_background_jobs();
}

/// Serve loop: the read loop handles control frames immediately (handshake,
/// shutdown, and cancel — cancellation must reach active work, so it can
/// never queue behind a running call) while calls dispatch on a single worker
/// thread, preserving the advertised `max_in_flight: 1` execution bound.
pub fn run_raw_worker_protocol_serve(opts: &RawWorkerServeOptions) -> i32 {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let writer = Arc::new(Mutex::new(std::io::stdout()));
    let session_id =
        std::env::var("ZEROSTACK_SESSION_ID").unwrap_or_else(|_| "tokenzero-raw-worker".into());
    let session = Arc::new(Mutex::new(RawWorkerSession::for_binding(
        opts.root.to_string_lossy().into_owned(),
        session_id,
    )));
    crate::shell_hooks::install(crate::shell_hooks::ProcessHooks::with_note_child(
        v2_note_child,
    ));
    let (tx, rx) = std::sync::mpsc::channel::<CallJob>();
    let worker = {
        let session = Arc::clone(&session);
        let writer = Arc::clone(&writer);
        let worker_opts = RawWorkerServeOptions {
            root: opts.root.clone(),
            cache_path: opts.cache_path.clone(),
            handshake_only: false,
            once_json: None,
        };
        std::thread::spawn(move || {
            let engine = engine_from_options(&worker_opts);
            while let Ok(job) = rx.recv() {
                let id = job.ctx.id.clone();
                let value = run_call_registered(&engine, job.ctx, job.cancel);
                if let Ok(mut guard) = session.lock() {
                    guard.finish_call(&id);
                }
                if write_response(&writer, &encode(value)).is_err() {
                    return;
                }
            }
        })
    };
    let exit_code = loop {
        match read_bounded_frame(&mut input, raw_worker_protocol::DEFAULT_MAX_FRAME_BYTES) {
            Ok(BoundedFrame::Eof) => break 0,
            Ok(BoundedFrame::TooLarge) => {
                let response = encode(error(
                    None,
                    "frame_too_large",
                    "inbound frame exceeds 1 MiB",
                    None,
                ));
                if write_response(&writer, &response).is_err() {
                    break 2;
                }
            }
            Ok(BoundedFrame::Line(line)) => {
                let mut guard = session.lock().unwrap_or_else(|p| p.into_inner());
                match route_frame(&mut guard, &line) {
                    RoutedFrame::Respond(response) => {
                        let shutdown = guard.shutdown;
                        let teardown = guard.take_pending_teardown();
                        drop(guard);
                        // SAFETY: drop the session mutex before `cancel_child`.
                        // T1 (stdin) previously held `session` across SIGTERM
                        // grace; T2 (worker) blocked on `finish_call`.
                        run_pending_teardown(teardown);
                        if write_response(&writer, &response).is_err() {
                            break 2;
                        }
                        if shutdown {
                            break 0;
                        }
                    }
                    RoutedFrame::Dispatch(ctx) => {
                        let cancel = guard.register_cancel(&ctx.id);
                        drop(guard);
                        if tx.send(CallJob { ctx, cancel }).is_err() {
                            break 2;
                        }
                    }
                }
            }
            Err(_) => break 2,
        }
    };
    terminate_raw_worker_session(&session);
    drop(tx);
    // Raw-worker entrypoints immediately pass this code to `process::exit`.
    // Joining here would let disconnected work retain the dedicated process
    // past the session boundary; dropping the handle lets process teardown
    // stop any non-cooperative in-process work after descendants are killed.
    drop(worker);
    exit_code
}

enum BoundedFrame {
    Eof,
    Line(Vec<u8>),
    TooLarge,
}

fn read_bounded_frame<R: BufRead>(reader: &mut R, maximum: usize) -> std::io::Result<BoundedFrame> {
    let mut line = Vec::with_capacity(4096);
    let mut too_large = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !too_large {
                return Ok(BoundedFrame::Eof);
            }
            return Ok(if too_large || line.len() > maximum {
                BoundedFrame::TooLarge
            } else {
                BoundedFrame::Line(line)
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if !too_large {
            if line.len().saturating_add(take) > maximum.saturating_add(1) {
                too_large = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            let content = line.strip_suffix(b"\n").unwrap_or(&line);
            let content_len = content.strip_suffix(b"\r").unwrap_or(content).len();
            return Ok(if too_large || content_len > maximum {
                BoundedFrame::TooLarge
            } else {
                BoundedFrame::Line(line)
            });
        }
    }
}

#[cfg(test)]
mod accounting_tests {
    use super::*;
    use serde_json::json;
    use tokenzero_core::{Accounting, LEXICAL_ESTIMATOR_ID, Mode, ToolResponse};

    #[test]
    fn worker_accounting_stamps_kernel_estimator_from_domain() {
        let mut domain = serde_json::to_value(ToolResponse::ok(
            "mem",
            Mode::Auto,
            "ok".into(),
            Vec::new(),
            Accounting::measured(8, 3, 1, 3, 0, Some(0)),
        ))
        .expect("domain json");
        domain["refs"] = json!([]);
        let worker = worker_token_accounting("mem", &json!({}), &domain).expect("accounting");
        assert_eq!(worker.tokenizer_id, LEXICAL_ESTIMATOR_ID);
        assert_eq!(
            worker.count_kind,
            raw_worker_protocol::WorkerTokenCountKind::Estimate
        );
        assert_eq!(worker.raw_tokens, 8);
        assert_eq!(worker.visible_tokens, 3);
        assert_eq!(worker.recovery_tokens, 1);
        assert_eq!(worker.billed_tokens, 3);
        assert_eq!(worker.cached_tokens, 0);
        assert_eq!(worker.exact_ref_tokens, Some(0));
        assert!(
            !worker.tokenizer_id.contains('@'),
            "must not invent ExactTokenizerIdentity"
        );
    }
}
