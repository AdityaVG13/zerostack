//! CodeMode host — contract-shaped visible payload + durable execution refs.

use super::discovery::{DESCRIBE_REF, SEARCH_REF};
use super::limits::MAX_OUTPUT_BYTES;
use super::runtime::{ERROR_REF, RESULT_REF, RuntimeOutcome, STEPS_REF};
use crate::core::{
    ExecutionPath, FSZeroSession, estimate_visible_tokens, record_opt_in_visible_accounting,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub const TELEMETRY_REF: &str = "codemode/telemetry";
pub const RESPONSE_REF: &str = "codemode/response";
const LATEST_EXEC_REF: &str = "codemode/execution/latest";
const PAYLOAD_INLINE_TOKEN_LIMIT: usize = 64;
const PREVIEW_CHAR_LIMIT: usize = 48;
/// Head-capsule budget for first-encounter (judgment) payload serves.
const CAPSULE_TOKEN_BUDGET: usize = 800;
/// Byte cap for inlining explicit `fs.expand` results (exactness escape hatch).
const EXPAND_INLINE_BYTE_LIMIT: usize = 64 * 1024;
static NEXT_EXECUTION_ID: AtomicU64 = AtomicU64::new(1);
/// Ok-path ring sample counter (every 16th ok; errors always record).
static OK_RING_TICK: AtomicU64 = AtomicU64::new(0);
const RING_CAP: usize = 256;

/// Align ok-path ring sampling at tick 0 so tests see a deterministic sample.
pub fn reset_ok_ring_tick_for_tests() {
    OK_RING_TICK.store(0, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub struct ContractError {
    pub kind: &'static str,
    pub message: String,
    pub retryable: bool,
}

macro_rules! contract_error_ctors {
    ($($name:ident => ($kind:literal, $retry:expr)),+ $(,)?) => {
        $(
            pub fn $name(message: impl Into<String>) -> Self { Self::new($kind, message, $retry) }
        )+ }; }

impl ContractError {
    pub fn new(kind: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
        }
    }

    contract_error_ctors! {
    validation => ("validation", false), sandbox => ("sandbox", false),
    runtime => ("runtime", false), plan => ("plan", false),
    substrate => ("substrate", false), policy => ("policy", false),
    deadline => ("deadline", true), cancelled => ("cancelled", true),
    busy => ("busy", true),
    // Substrate fault that may clear on a single bounded retry (store blip).
    substrate_retryable => ("substrate", true), }

    pub fn to_json(&self) -> Value {
        json!({"kind": self.kind, "message": self.message, "retryable": self.retryable})
    }

    /// Wire error with root + recoverable telemetry ref (fszero-szw).
    /// Keeps agents on CodeMode instead of inventing "falling back to shell".
    pub fn to_json_with_context(&self, root: &str, telemetry_ref: &str) -> Value {
        let mut v = self.to_json();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("root".into(), json!(root));
            obj.insert("telemetry_ref".into(), json!(telemetry_ref));
        }
        v
    }
}

#[inline]
fn exec_path(base: &str, suffix: &str) -> String {
    format!("{base}/{suffix}")
}

#[inline]
pub(crate) fn contains_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

pub fn classify_error(detail: &str) -> ContractError {
    let lower = detail.to_ascii_lowercase();
    // Fatal permit I/O must win over "codemode permit" / "denied" substrings
    // inside the wrapped message (fszero-k44 / graphzero-oy1n).
    if lower.contains("machine_permit_io") {
        ContractError::substrate(detail)
    } else if contains_any(
        &lower,
        &["machine_permit_busy", "codemode permit", "heavy_queue_full"],
    ) {
        ContractError::busy(detail)
    } else if contains_any(
        &lower,
        &[
            "request deadline",
            "deadline exceeded",
            "tools/call deadline",
        ],
    ) {
        ContractError::deadline(detail)
    } else if contains_any(
        &lower,
        &[
            "request cancelled",
            "request canceled",
            "notifications/cancelled",
        ],
    ) {
        ContractError::cancelled(detail)
    } else if contains_any(
        &lower,
        &["unknown compound", "unknown call", "unknown method"],
    ) {
        ContractError::plan(detail)
    } else if lower.starts_with("path is a directory") || lower.starts_with("path not found:") {
        ContractError::runtime(detail)
    } else if contains_any(
        &lower,
        &[
            "sandbox",
            "fetch",
            "process",
            "require",
            "settimeout",
            "sqlite",
            "native module",
            "host fs",
            "microtask",
            "memory",
        ],
    ) {
        ContractError::sandbox(detail)
    } else if contains_any(&lower, &["policy", "denied"]) {
        ContractError::policy(detail)
    } else if contains_any(
        &lower,
        &[
            "store failed",
            "search:0 (store failed",
            "missing response payload",
        ],
    ) {
        ContractError::substrate_retryable(detail)
    } else if contains_any(
        &lower,
        &["missing target", "not found", "unknown ref", "resource"],
    ) {
        ContractError::substrate(detail)
    } else if contains_any(&lower, &["json plan", "validate", "invalid", "exceeds"]) {
        ContractError::validation(detail)
    } else {
        ContractError::runtime(detail)
    }
}

/// Workspace root string for structured errors (empty when unset).
pub fn error_root(session: &FSZeroSession) -> String {
    path_display_or(session.workspace_root(), "")
}

/// Session store identity for telemetry / execution refs (computed once).
struct StoreDiag {
    store_id: String,
    store_root: String,
    store_db: String,
    workspace_root: String,
}

fn path_display_or(path: Option<impl AsRef<std::path::Path>>, fallback: &str) -> String {
    path.map(|p| p.as_ref().display().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn session_store_diag(session: &FSZeroSession) -> StoreDiag {
    StoreDiag {
        store_id: session.store_id().unwrap_or("memory").to_string(),
        store_root: path_display_or(session.store_root(), "memory"),
        store_db: path_display_or(session.store_db_path(), "memory"),
        workspace_root: path_display_or(session.workspace_root(), "(none)"),
    }
}

fn put_execution_artifacts(
    session: &mut FSZeroSession,
    base: &str,
    label: &str,
    steps_body: &str,
    result_bytes: &[u8],
    dag_body: Option<&str>,
) {
    for (suffix, data) in [
        ("code", label.as_bytes()),
        ("steps", steps_body.as_bytes()),
        ("result", result_bytes),
    ] {
        session.recovery.put_key(&format!("{base}/{suffix}"), data);
    }
    if let Some(dag) = dag_body {
        session.recovery.put_key(&format!("{base}/dag"), dag.as_bytes());
    }
}

pub fn finish(session: &mut FSZeroSession, outcome: &RuntimeOutcome) -> String {
    let started = Instant::now();
    // Sum each operation's observed payload exactly once. This is independent
    // of recovery-cache hits, so ref-only, error, mutation, history, and expand
    // paths all participate without internal bookkeeping reads inflating the
    // claimed baseline.
    let operation_bytes_materialized = session.codemode_materialized_bytes;
    // Write steps/result once and keep the bytes — avoid expand-then-re-put.
    let steps_body = super::runtime::format_steps_body(&outcome.steps, outcome.ok, outcome.dag.as_ref());
    session.recovery.put_key(STEPS_REF, steps_body.as_bytes());
    let result_bytes = if !outcome.ok || session.expand(RESULT_REF).is_none() {
        let b = outcome.summary.as_bytes().to_vec();
        session.recovery.put_key(RESULT_REF, &b);
        b
    } else {
        session
            .expand(RESULT_REF)
            .unwrap_or_else(|| outcome.summary.as_bytes().to_vec())
    };
    // The returned value is also materialized even when no filesystem step
    // ran (empty and validation/error paths). Use the larger observed body,
    // not a sum, because inline results may repeat an operation payload.
    let bytes_materialized = operation_bytes_materialized.max(result_bytes.len() as u64);
    let visible = if outcome.ok { "C" } else { "X0" };
    let execution_id = next_execution_id(&outcome.summary);
    let safe_id = execution_id.trim_start_matches("cm://exec/");
    let execution_base = format!("fz://codemode/execution/{safe_id}");
    let diag = session_store_diag(session);
    let mut refs = json!({
        "code": exec_path(&execution_base, "code"), "steps": exec_path(&execution_base, "steps"), "telemetry": exec_path(&execution_base, "telemetry"), "result": exec_path(&execution_base, "result"),
        // Store identity so hub/expand can route to the minting store even
        // when the next call passes a different workspace root
        // (fszero-store-root-fragmentation-jdl).
        "store_id": diag.store_id, "store_root": diag.store_root,
    });
    // The DAG structure (nodes, edges, batch-parallel levels) is part of the
    // plan receipt, not an opaque linear list (V6-F6 / ZS-EXEC-001).
    let dag_body = outcome.dag.as_ref().map(|dag| dag.to_json().to_string());
    if dag_body.is_some() {
        refs["dag"] = json!(exec_path(&execution_base, "dag"));
    }

    put_execution_artifacts(
        session,
        &execution_base,
        &outcome.label,
        &steps_body,
        &result_bytes,
        dag_body.as_deref(),
    );

    let error = if outcome.ok {
        None
    } else {
        let message = session
            .expand(ERROR_REF)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| outcome.summary.clone());
        let err = outcome
            .error
            .clone()
            .unwrap_or_else(|| classify_error(&message));
        // fszero-quer: always park under ERROR_REF so plan.rs can re-park after
        // durable exec-txn rollback and the CLI expand path still names the cause.
        put_contract_error(session, &err);
        let root = error_root(session);
        let telemetry_ref = exec_path(&execution_base, "telemetry");
        session.recovery.put_key(
            &exec_path(&execution_base, "error"),
            err.to_json_with_context(&root, &telemetry_ref)
                .to_string()
                .as_bytes(),
        );
        Some(err)
    };

    let (cache_hits, cache_misses, store_writes, _) = session.recovery.metric_snapshot();
    let raw_token_estimate = bytes_materialized.div_ceil(4);
    let operation_kinds = outcome
        .steps
        .iter()
        .map(|step| step.method.as_str())
        .collect::<Vec<_>>();
    let measurement_misses = session
        .codemode_measurement_misses
        .min(outcome.steps_run as u64);
    let operations_covered = (outcome.steps_run as u64).saturating_sub(measurement_misses);
    let coverage_status = if measurement_misses == 0 {
        "measured"
    } else if operations_covered > 0 {
        "partial"
    } else {
        "unmeasured"
    };
    let ws = session.watch_stats();
    let wr = session.watch_reconcile_state();
    let telemetry = json!({
        "kind": "codemode.execute", "status": if outcome.ok { "ok" } else { "error" },
        "logical_ops": outcome.logical_ops, "physical_ops": outcome.physical_ops, "batched_ops": outcome.batched_ops, "internal_actions": outcome.internal_actions,
        "cache_hits": cache_hits, "cache_misses": cache_misses, "store_writes": store_writes, "wall_ms": outcome.wall_ms.saturating_add(started.elapsed().as_millis() as u64),
        "cache_taxonomy": {
            "model": "payload_store_presence",
            "not": "3c_compulsory_capacity_conflict",
            "hits": "recovery payload key present",
            "misses": "recovery payload key absent or attributed miss",
        },
        "bytes_materialized": bytes_materialized,
        "raw_token_estimate": raw_token_estimate,
        "visible_token_estimate": estimate_visible_tokens(visible),
        "token_estimator": "estimator:utf8-bytes-div-4",
        "measurement_coverage": {
            "status": coverage_status,
            "stage": "execution",
            "operations_covered": operations_covered,
            "operations_total": outcome.steps_run,
            "operation_kinds": operation_kinds,
            "bytes": "observed",
            "materialization_basis": "unique-content(operation_payloads),max(result)", "misses": measurement_misses, "degraded_reasons": if measurement_misses > 0 { json!([{"kind":"recovery_expand_miss","count":measurement_misses}]) } else { json!([]) },
            "tokens": "estimated"
        },
        "extra": {
            "steps_run": outcome.steps_run, "primary_ref": outcome.primary_ref, "parallel_groups": outcome.parallel_groups, "parallel_wall_ms": outcome.parallel_wall_ms,
            "durable_degraded": session.durable_degraded,
            "watch": {
                "active": session.watch_active(), "events_seen": ws.events_seen, "files_updated": ws.files_updated, "files_removed": ws.files_removed,
                "rescans": ws.rescans, "drains": ws.drains, "rescan_priority_drains": ws.rescan_priority_drains,
                "truncated_rescans": ws.truncated_rescans, "index_trusted": session.watch_index_trusted(),
                "reconcile": {
                    "drain_backlog": wr.drain_backlog, "overflow_pending": wr.overflow_pending, "untrusted_removals": wr.untrusted_removals, "dirty_generation": wr.dirty_generation,
                }, },
            "transaction_rolled_back": outcome.transaction_rolled_back, "visible_tokens": estimate_visible_tokens(visible),
            "expand_refs": [SEARCH_REF, DESCRIBE_REF, TELEMETRY_REF, STEPS_REF, RESULT_REF],
            // Multi-project diagnosis: workspace root (FS ops) vs store root
            // (durable blobs). Distinct when ZEROSTACK_STORE_ROOT is shared.
            "workspace_root": diag.workspace_root, "store_root": diag.store_root, "store_id": diag.store_id, "store_db": diag.store_db,
        }
    });
    let telemetry_bytes = telemetry.to_string();
    session
        .recovery
        .put_key(TELEMETRY_REF, telemetry_bytes.as_bytes());
    session.recovery.put_key(
        &exec_path(&execution_base, "telemetry"),
        telemetry_bytes.as_bytes(),
    );
    // Bounded telemetry ring (fszero-qkn): newest RING_CAP execution summaries.
    // On the hot ok path, only sample every 16th plan (always on error) so
    // multi-op AI read plans avoid ring JSON read/parse/write each time.
    {
        let tick = OK_RING_TICK.fetch_add(1, Ordering::Relaxed);
        let sample_ring = !outcome.ok || tick % 16 == 0;
        if sample_ring {
            let violations = session.recovery.integrity_report().0;
            let mut ring: Vec<serde_json::Value> = session
                .recovery
                .payload("telemetry/ring")
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("entries").and_then(|e| e.as_array()).cloned())
                .unwrap_or_default();
            ring.push(json!({
                "ts": crate::core::unix_epoch_secs() as u64,
                "status": if outcome.ok { "ok" } else { "error" },
                "wall_ms": telemetry["wall_ms"], "logical_ops": outcome.logical_ops,
                "physical_ops": outcome.physical_ops, "store_writes": store_writes, "primary_ref": outcome.primary_ref, "integrity_violations": violations,
            }));
            if ring.len() > RING_CAP {
                let drop = ring.len() - RING_CAP;
                ring.drain(..drop);
            }
            session.recovery.put_key(
                "telemetry/ring",
                json!({"version": 1, "entries": ring})
                    .to_string()
                    .as_bytes(),
            );
        }
    }

    // Inline the plan's return value when it is small, valid JSON: the
    // envelope is the only thing MCP clients see synchronously, and refs-only
    // envelopes force a second round-trip to recover the result (the
    // "judgment path" bug). Oversize payloads fall back to the strip guard
    // below, and non-JSON RESULT_REF contents (the program summary line) are
    // intentionally skipped.
    let inline_result = session
        .expand(RESULT_REF)
        .filter(|bytes| bytes.len() <= MAX_OUTPUT_BYTES)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());

    let mut payload = if outcome.ok {
        json!({"ack": visible, "execution_id": execution_id, "refs": refs, "telemetry": telemetry})
    } else {
        let err = error.unwrap_or_else(|| ContractError::runtime("execution failed"));
        let error_ref = exec_path(&execution_base, "error");
        let root = error_root(session);
        let telemetry_ref = exec_path(&execution_base, "telemetry");
        // Healthy CodeMode failures stay on CodeMode: structured error only.
        // Native/shell fallback is never advertised here — only fail-open
        // substrate misses set `native_fallback` (see resolve_codemode_response).
        json!({
            "ack": visible, "execution_id": execution_id, "refs": refs, "error_ref": error_ref, "error": err.to_json_with_context(&root, &telemetry_ref),
            "telemetry": telemetry, "native_fallback": false,
        })
    };
    if outcome.ok {
        if let (Some(obj), Some(value)) = (payload.as_object_mut(), inline_result) {
            obj.insert("result".to_string(), value);
        }
    }
    let mut response_bytes = payload.to_string();
    if response_bytes.len() > MAX_OUTPUT_BYTES {
        payload = json!({"ack": visible, "execution_id": execution_id, "refs": refs, "telemetry": telemetry});
        response_bytes = payload.to_string();
    }
    session
        .recovery
        .put_key(RESPONSE_REF, response_bytes.as_bytes());
    session
        .recovery
        .put_key(LATEST_EXEC_REF, execution_base.as_bytes());
    session.recovery.record_execution_base(&execution_base);
    // Always stash in-memory (fszero-iod).
    session.stash_codemode_response(payload);
    visible.to_string()
}

fn next_execution_id(seed: &str) -> String {
    let millis = crate::core::unix_epoch_millis();
    let seq = NEXT_EXECUTION_ID.fetch_add(1, Ordering::Relaxed);
    let mut h = Sha256::new();
    h.update(format!("{millis}:{seq}:{seed}").as_bytes());
    let hash = crate::core::hexutil::sha256_hex_of(h.finalize().into());
    format!("cm://exec/{millis}-{}", &hash[..12])
}

/// Park a ContractError JSON under ERROR_REF.
pub(crate) fn put_contract_error(session: &mut FSZeroSession, error: &ContractError) {
    session
        .recovery
        .put_key(ERROR_REF, error.to_json().to_string().as_bytes());
}

pub fn finish_error(session: &mut FSZeroSession, reason: &str) -> String {
    let err = classify_error(reason);
    put_contract_error(session, &err);
    finish(session, &RuntimeOutcome::failed("validation", reason, err))
}

fn tool_refs(keys: &[&'static str]) -> Vec<String> {
    keys.iter().map(|k| (*k).to_string()).collect()
}

pub fn codemode_tool_refs_for_plan() -> Vec<String> {
    tool_refs(&[
        RESPONSE_REF,
        TELEMETRY_REF,
        STEPS_REF,
        RESULT_REF,
        LATEST_EXEC_REF,
    ])
}

pub fn codemode_tool_refs_for_search() -> Vec<String> {
    tool_refs(&[SEARCH_REF, TELEMETRY_REF])
}
pub fn codemode_tool_refs_for_describe() -> Vec<String> {
    tool_refs(&[DESCRIBE_REF, TELEMETRY_REF])
}

pub fn ack_with_refs(ack: &str, ok: bool, refs: Vec<String>) -> Value {
    json!({
        "content": [{"type": "text", "text": ack}],
        "isError": !ok,
        "structuredContent": {"ack": ack, "ok": ok, "refs": refs}
    })
}

pub fn payload_tool_result(payload: Value, ok: bool) -> Value {
    json!({"content": [{"type": "text", "text": payload.to_string()}], "isError": !ok, "structuredContent": payload})
}

fn finish_codemode_receipt_txn(session: &mut FSZeroSession, deferred: bool) -> Result<(), String> {
    if deferred {
        session.recovery.commit_exec_txn(true);
        session.recovery.maintain_wal_cadence();
    }
    if let Some(error) = session.recovery.take_store_error() {
        session.codemode_relaxed_read_signature = None;
        eprintln!("fszero: CodeMode envelope receipt commit failed: {error}");
        return Err(error);
    }
    Ok(())
}

fn receipt_failure(error: String) -> Value {
    payload_tool_result(
        json!({"ack": "X0", "error": {"kind": "store", "message": error}}),
        false,
    )
}

fn codemode_envelope_ref(session: &mut FSZeroSession, bytes: &[u8]) -> Result<String, String> {
    let deferred = session.recovery.relaxed_exec_txn_active();
    // Only a repeated stable JSON read reaches this branch with a live NORMAL
    // transaction. First reads and every other plan mint the envelope at FULL.
    let reference = if deferred {
        session.recovery.put_codemode_receipt_content_ref(bytes)
    } else {
        session.recovery.put_content_ref(bytes)
    };
    finish_codemode_receipt_txn(session, deferred)?;
    Ok(reference)
}

pub fn plan_tool_result(
    session: &mut FSZeroSession,
    mut payload: Value,
    ok: bool,
    requested_envelope: Option<&str>,
) -> Value {
    if envelope_v1_enabled(requested_envelope) {
        let deferred = session.recovery.relaxed_exec_txn_active();
        if let Err(error) = finish_codemode_receipt_txn(session, deferred) {
            return receipt_failure(error);
        }
        let visible = payload.to_string();
        attach_wire_measurement(&mut payload, &visible);
        // The telemetry fields themselves enlarge the v1 visible payload. A
        // second pass converges the byte/token estimate without recursion.
        let visible = payload.to_string();
        attach_wire_measurement(&mut payload, &visible);
        return payload_tool_result(payload, ok);
    }

    let owner_refs = collect_durable_blob_refs(&payload);
    // fszero-cr3v: a scalar durable blob must be the visible primary text so
    // agents can expand it without digging through an envelope. Structured
    // results still get a durable envelope, plus every owner blob listed.
    if ok {
        if let Some(blob) = scalar_durable_blob_ref(&payload) {
            let bundle = envelope_bundle(session, &payload);
            let envelope_ref = match codemode_envelope_ref(session, bundle.to_string().as_bytes()) {
                Ok(reference) => reference,
                Err(error) => return receipt_failure(error),
            };
            let value = codemode_visible_value(session, &payload);
            attach_wire_measurement(&mut payload, &blob);
            record_opt_in_visible_accounting(
                ExecutionPath::Codemode,
                session.store_db_path(),
                &payload.to_string(),
                &blob,
            );
            let mut structured = json!({
                "ack": blob,
                "ref": blob,
                "envelope_ref": envelope_ref,
                "owner_refs": owner_refs,
            });
            if !value.is_null() {
                structured["value"] = value;
            }
            if let Some(refs) = payload.get("refs") {
                structured["refs"] = refs.clone();
            }
            if let Some(telemetry) = payload.get("telemetry") {
                structured["telemetry"] = telemetry.clone();
            }
            return json!({
                "content": [{"type": "text", "text": blob}],
                "isError": false,
                "structuredContent": structured
            });
        }
    }

    let bundle = envelope_bundle(session, &payload);
    let bundle_bytes = bundle.to_string();
    // fszero-cr3v: the ack is often the ONLY thing an agent sees, so the ref it
    // carries must be durable. A `fz://seq/...` mint is execution-scoped and
    // fails `seq_ref_scoped` from any other process, which stranded the agent
    // with no way to reach the blob refs nested inside the response. Content-
    // address the recovery envelope instead: it survives restart and expands
    // from any session.
    let full_ref = match codemode_envelope_ref(session, bundle_bytes.as_bytes()) {
        Ok(reference) => reference,
        Err(error) => return receipt_failure(error),
    };
    debug_assert!(
        !full_ref.contains("://seq/"),
        "plan envelope must never mint seq refs: {full_ref}"
    );
    let value = codemode_visible_value(session, &payload);
    let ack = if ok {
        success_ack(&payload, &full_ref)
    } else {
        error_ack(&payload, &full_ref)
    };
    attach_wire_measurement(&mut payload, &ack);
    record_opt_in_visible_accounting(
        ExecutionPath::Codemode,
        session.store_db_path(),
        &payload.to_string(),
        &ack,
    );
    let mut structured = json!({"ack": ack, "ref": full_ref, "owner_refs": owner_refs});
    if !value.is_null() {
        structured["value"] = value;
    }
    if let Some(refs) = payload.get("refs") {
        structured["refs"] = refs.clone();
    }
    if let Some(telemetry) = payload.get("telemetry") {
        structured["telemetry"] = telemetry.clone();
    }
    json!({"content": [{"type": "text", "text": ack}], "isError": !ok, "structuredContent": structured})
}

/// True when `reference` is a durable content-addressed ZeroRef (never seq).
fn is_durable_blob_ref(reference: &str) -> bool {
    let Some((scheme, rest)) = reference.split_once("://") else {
        return false;
    };
    matches!(scheme, "fz" | "gz" | "tz")
        && rest.starts_with("blob/")
        && !rest.is_empty()
        && !reference.chars().any(char::is_whitespace)
}

/// When the plan result is exactly one durable blob ref, return it.
fn scalar_durable_blob_ref(payload: &Value) -> Option<String> {
    let result = payload.get("result")?;
    if let Some(s) = result.as_str() {
        if is_durable_blob_ref(s) {
            return Some(s.to_string());
        }
    }
    // Legacy flat `{ref: fz://blob/...}` result object.
    if let Some(s) = result.get("ref").and_then(Value::as_str) {
        if is_durable_blob_ref(s) && result.get("content").is_none() {
            return Some(s.to_string());
        }
    }
    // zero-result tagged ref.
    let kind = result
        .pointer("/content/kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    if kind == "ref" {
        if let Some(s) = result
            .pointer("/content/ref")
            .and_then(Value::as_str)
            .or_else(|| result.pointer("/content/reference").and_then(Value::as_str))
        {
            if is_durable_blob_ref(s) {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Collect durable blob refs nested under a plan payload (for owner_refs).
fn collect_durable_blob_refs(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_durable_blob_refs_into(value, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_durable_blob_refs_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if is_durable_blob_ref(s) {
                out.push(s.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_durable_blob_refs_into(item, out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key == "telemetry" {
                    continue;
                }
                collect_durable_blob_refs_into(item, out);
            }
        }
        _ => {}
    }
}

fn attach_wire_measurement(payload: &mut Value, visible: &str) {
    let Some(telemetry) = payload.get_mut("telemetry").and_then(Value::as_object_mut) else {
        return;
    };
    telemetry.insert("visible_bytes".into(), json!(visible.len()));
    telemetry.insert(
        "visible_token_estimate".into(),
        json!(estimate_visible_tokens(visible)),
    );
    if let Some(coverage) = telemetry
        .get_mut("measurement_coverage")
        .and_then(Value::as_object_mut)
    {
        coverage.insert("stage".into(), json!("wire"));
    }
}

fn envelope_bundle(session: &FSZeroSession, payload: &Value) -> Value {
    let mut expanded = serde_json::Map::new();
    if let Some(refs) = payload.get("refs").and_then(Value::as_object) {
        for (name, value) in refs {
            if let Some(r) = value.as_str() {
                if let Some(bytes) = session.expand(r) {
                    expanded.insert(
                        name.clone(),
                        Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                    );
                }
            }
        }
    }
    json!({
        "envelope": "v2", "response": payload,
        "refs": payload.get("refs").cloned().unwrap_or_else(|| json!({})),
        "telemetry": payload.get("telemetry").cloned().unwrap_or(Value::Null),
        "expanded": expanded, "store": {"durable_degraded": session.durable_degraded},
    })
}

fn envelope_v1_enabled(requested_envelope: Option<&str>) -> bool {
    match requested_envelope {
        Some("v1") => true,
        Some("v2") => false,
        _ => std::env::var("ZERO_ENVELOPE")
            .map(|v| v.eq_ignore_ascii_case("v1"))
            .unwrap_or(false),
    }
}

fn success_ack(payload: &Value, full_ref: &str) -> String {
    let ops = payload
        .pointer("/telemetry/logical_ops")
        .and_then(Value::as_u64)
        .or_else(|| {
            payload
                .pointer("/telemetry/physical_ops")
                .and_then(Value::as_u64)
        })
        .unwrap_or(1)
        .max(1);
    let raw_tokens = estimate_visible_tokens(&payload.to_string()).max(1);
    let provisional = format!("ok fz{ops} - t:{full_ref}");
    let visible = estimate_visible_tokens(&provisional).max(1);
    let pct = raw_tokens.saturating_sub(visible) * 100 / raw_tokens;
    format!("ok fz{ops} {pct}% t:{full_ref}")
}

fn error_ack(payload: &Value, full_ref: &str) -> String {
    let err = payload.get("error").unwrap_or(&Value::Null);
    let kind = err.get("kind").and_then(Value::as_str).unwrap_or("runtime");
    let retry = if err
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "retryable"
    } else {
        "final"
    };
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("execution failed");
    // Never mid-truncate a ref in the visible error line (fz-ref acceptance:
    // truncation never in error strings). Prefer the full message when it
    // carries any durable ref; otherwise soft-cap for the 1-token ack.
    let visible_msg = if message.contains("fz://")
        || message.contains("tz://")
        || message.contains("gz://")
        || message.contains("ref_not_found")
        || message.contains("seq_ref_scoped")
    {
        message.to_string()
    } else {
        truncate_chars(message, 180)
    };
    format!("err {kind} {retry} {visible_msg} t:{full_ref}")
}

fn codemode_visible_value(session: &mut FSZeroSession, payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.remove("telemetry");
    }
    compact_payload_strings(session, value)
}

fn compact_payload_strings(session: &mut FSZeroSession, value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                if key == "payload" {
                    out.insert(key, compact_payload_value(session, value));
                } else {
                    out.insert(key, compact_payload_strings(session, value));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| compact_payload_strings(session, v))
                .collect(),
        ),
        other => other,
    }
}

fn compact_payload_value(session: &mut FSZeroSession, value: Value) -> Value {
    let Some(text) = value.as_str() else {
        return compact_payload_strings(session, value);
    };
    novelty_payload_value(session, text)
}

/// The visible wire walks the WHOLE plan value, so an exact `fs.expand` payload
/// is reachable at any shape the plan chose to return it in — bare string, step
/// object, or nested in a caller-built result. Keying the exemption off the step
/// shape therefore missed the common cases. `fs.expand` marks its bytes on the
/// session instead, and every string is checked against that mark here.
fn is_exact_serve(session: &FSZeroSession, text: &str) -> bool {
    session.is_exact_served_content(text)
}

/// Novelty-aware payload shape (bright-line rule): the first serve of content
/// this session is a judgment op — the agent asked to see bytes it has not
/// seen — so it gets a budgeted head CAPSULE inline, never just a one-line
/// preview that forces a second expand round-trip. A re-serve of identical
/// content is mechanical and collapses to ref + preview. Changed content has
/// a new hash, so it is novel again by construction.
fn novelty_payload_value(session: &mut FSZeroSession, text: &str) -> Value {
    if estimate_visible_tokens(text) <= PAYLOAD_INLINE_TOKEN_LIMIT {
        return Value::String(text.to_string());
    }
    // `fs.expand` is the exact-bytes escape hatch: never re-capsule what it
    // served (fszero-fs-read-content-broken-b4yg).
    if is_exact_serve(session, text) {
        return Value::String(text.to_string());
    }
    let r = session.recovery.put_content_ref(text.as_bytes());
    if session.note_served_content(text) {
        // First serve: budgeted head capsule. When truncated, emit DeepAgents-
        // style pagination so the caller can resume with start_line=next_offset
        // (or expand(ref#L…)) without bisecting the visible budget
        // (fszero-codemode-read-truncation-unreachable-1beg + cd0v).
        let page = capsule_head(text);
        let mut out = json!({
            "ref": r,
            "capsule": page.capsule,
            "truncated": page.truncated,
            "total_bytes": page.total_bytes,
            "capsule_bytes": page.capsule_bytes,
            "visible_budget_tokens": CAPSULE_TOKEN_BUDGET,
        });
        if page.truncated {
            out["range"] = json!([page.range_start, page.range_end]);
            out["next_offset"] = json!(page.next_offset);
            out["remaining"] = json!(page.remaining);
            out["total_lines"] = json!(page.total_lines);
            out["resume_hint"] = json!(format!(
                "start_line={} (or expand(ref#L{}-…)); full bytes at ref",
                page.next_offset.unwrap_or(page.range_end.saturating_add(1)),
                page.next_offset.unwrap_or(page.range_end.saturating_add(1))
            ));
        }
        out
    } else {
        json!({"ref": r, "preview": preview_line(text), "seen": true})
    }
}

/// Session-less path: only safe for already-stored content (same SHA). Prefer
/// [`payload_wire_value_with_session`] so large payloads always land in the store.
pub fn payload_wire_value(ref_hint: &str, payload: &[u8]) -> Value {
    payload_wire_inner(ref_hint, payload, None)
}

/// Wire-shape for a step payload. Large payloads are content-addressed via
/// [`RecoveryStore::put_content_ref`] so the emitted `fz://blob/…` always expands
/// to exact bytes (field bug: hash-only refs with no store write).
pub fn payload_wire_value_with_session(
    session: &mut FSZeroSession,
    ref_hint: &str,
    payload: &[u8],
) -> Value {
    payload_wire_inner(ref_hint, payload, Some(session))
}

/// Shared expand/inline/novelty wire shaping. `session` enables store writes +
/// novelty capsules; without it, large blobs use hash-only refs.
fn payload_wire_inner(
    ref_hint: &str,
    payload: &[u8],
    session: Option<&mut FSZeroSession>,
) -> Value {
    let text = String::from_utf8_lossy(payload).into_owned();
    // Pre-minted recovery key: surface the full capsule under the given ref.
    if session.is_some() && ref_hint.starts_with("fz://") {
        return json!({"ref": ref_hint, "capsule": text});
    }
    // `fs.expand` is the explicit exactness escape hatch: the caller opted in
    // to exact bytes, so inline up to a byte cap instead of the novelty/token
    // capsule rule (which would re-capsule the very bytes expand delivers).
    if ref_hint == "expand" {
        if payload.len() <= EXPAND_INLINE_BYTE_LIMIT {
            return Value::String(text);
        }
        let r = match session {
            Some(s) => s.recovery.put_content_ref(payload),
            None => blob_ref_for_bytes(payload),
        };
        return json!({
            "ref": r, "preview": preview_line(&text),
            "window_hint": "expand(ref#L<start>-<end>) for exact windows"
        });
    }
    match session {
        Some(s) => novelty_payload_value(s, &text),
        None if estimate_visible_tokens(&text) <= PAYLOAD_INLINE_TOKEN_LIMIT => Value::String(text),
        None => {
            let blob_ref = blob_ref_for_bytes(payload);
            json!({"ref": blob_ref, "preview": preview_line(&text)})
        }
    }
}

#[cfg_attr(not(feature = "surface-codemode"), allow(dead_code))]
pub fn dedup_detail_ref(detail: Option<String>, payload_wire: &Value) -> Option<String> {
    let Some(payload_ref) = payload_wire.get("ref").and_then(Value::as_str) else {
        return detail;
    };
    let needle = format!("ref={payload_ref}");
    detail.map(|d| {
        if !d.contains(&needle) {
            return d;
        }
        d.split_whitespace()
            .filter(|token| *token != needle.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn blob_ref_for_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!(
        "fz://blob/{}",
        crate::core::hexutil::sha256_hex_of(h.finalize().into())
    )
}

fn preview_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    truncate_chars(first, PREVIEW_CHAR_LIMIT)
}

/// Head capsule page: budgeted preview plus resume metadata (cd0v / 1beg).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapsulePage {
    capsule: String,
    truncated: bool,
    /// 1-indexed inclusive line range covered by the capsule (0,0 if empty).
    range_start: usize,
    range_end: usize,
    next_offset: Option<usize>,
    remaining: usize,
    total_lines: usize,
    total_bytes: usize,
    capsule_bytes: usize,
}

/// Head slice up to [`CAPSULE_TOKEN_BUDGET`] tokens, cut on line boundaries.
/// Cost is bounded by the budget, not file size. Reports distinct total_bytes
/// vs capsule_bytes so agents never confuse full-file size with visible head.
fn capsule_head(text: &str) -> CapsulePage {
    let total_bytes = text.len();
    let total_lines = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    if estimate_visible_tokens(text) <= CAPSULE_TOKEN_BUDGET {
        return CapsulePage {
            capsule: text.to_string(),
            truncated: false,
            range_start: if total_lines == 0 { 0 } else { 1 },
            range_end: total_lines,
            next_offset: None,
            remaining: 0,
            total_lines,
            total_bytes,
            capsule_bytes: total_bytes,
        };
    }
    // Match prior contract: include lines until budget exceeded (at most one-line overshoot).
    let mut out = String::new();
    let mut lines_kept = 0usize;
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
        lines_kept += 1;
        if estimate_visible_tokens(&out) > CAPSULE_TOKEN_BUDGET {
            break;
        }
    }
    let remaining = total_lines.saturating_sub(lines_kept);
    CapsulePage {
        capsule: out.clone(),
        truncated: true,
        range_start: if lines_kept == 0 { 0 } else { 1 },
        range_end: lines_kept,
        next_offset: if remaining > 0 {
            Some(lines_kept + 1)
        } else {
            None
        },
        remaining,
        total_lines,
        total_bytes,
        capsule_bytes: out.len(),
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
