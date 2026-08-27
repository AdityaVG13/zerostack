//! Graph operation step runners (query, blast, verify, snap, reserve, index, ctx.ref).

use serde_json::{Value, json};

use graphzero_store::Snapshot as StoreSnapshot;
use graphzero_store::store::query::{persist_query_json, tokens_for_str};

use super::errors::{
    approval_error, busy_error, cancelled_error, deadline_exceeded_error, not_found_error,
    policy_error, runtime_error, sandbox_error, substrate_error, validation_error,
};
use super::state::ExecutionState;
use super::types::{
    BindingResult, CodeModeError, REF_FIRST_PREVIEW_CHARS, REF_FIRST_STRING_TOKENS, StepRecord,
};
use super::utils::{canonical_query_ref, first_chars_flat, store_blob_ref};
use crate::operation_abi::{DomainError, DomainErrorKind};

/// Budget requested from the query surface for CodeMode. High enough that the
/// surface returns its structured hit records (budget 1 collapses to a bare ref
/// shell), low enough that a plan step stays cheap; the response is re-bounded
/// below by [`QUERY_INLINE_MAX_HITS`].
const CODEMODE_QUERY_BUDGET: usize = 16;

/// Hits inlined in a `graph.query` response. The canonical `gz://query/` ref
/// still carries the whole payload, so this only bounds what an agent sees.
const QUERY_INLINE_MAX_HITS: usize = 8;

/// Token ceiling for the inline hit list. Above it the content windows are
/// dropped (targets and the preview stay) and `truncated` is set, matching the
/// ref-first rule: inline small result sets, ref large ones, always preview.
const QUERY_INLINE_MAX_TOKENS: usize = 256;

/// One evidence-bearing record found anywhere in a query surface response.
struct RawQueryHit {
    evidence_ref: String,
    kind: String,
    sym: Option<String>,
    confidence: Option<f64>,
}

/// Every surface shape (edges, hits, reading_set, capsule destinations, …)
/// carries `evidence_ref` on its records, so collect generically rather than
/// per-surface: a new surface inlines correctly without a second code path.
fn collect_raw_query_hits(value: &Value, out: &mut Vec<RawQueryHit>) {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("evidence_ref").and_then(Value::as_str) {
                out.push(RawQueryHit {
                    evidence_ref: reference.to_string(),
                    kind: map
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("ref")
                        .to_string(),
                    sym: ["sym", "symbol", "label", "from", "target"]
                        .iter()
                        .find_map(|k| map.get(*k).and_then(Value::as_str))
                        .map(str::to_string),
                    confidence: map.get("confidence").and_then(Value::as_f64),
                });
            }
            for (key, child) in map {
                if key != "evidence_ref" {
                    collect_raw_query_hits(child, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_raw_query_hits(item, out);
            }
        }
        _ => {}
    }
}

/// `g:<id>` locate refs point at a blob span indirectly; resolve them so they
/// inline like any other evidence ref.
fn blob_span_evidence(snapshot: &StoreSnapshot, reference: &str) -> String {
    match reference
        .strip_prefix("g:")
        .and_then(|id| id.parse::<u32>().ok())
    {
        Some(loc) => graphzero_store::canonical_ref_for_loc(snapshot, loc)
            .unwrap_or_else(|_| reference.to_string()),
        None => reference.to_string(),
    }
}

/// Bounded inline hit list for a query surface response: canonical
/// `path#Lstart-Lend` targets, intent metadata, and content windows for the
/// leading hits. Returns `(hits, total_hit_count, truncated)`.
fn inline_query_hits(
    snapshot: &StoreSnapshot,
    value: &Value,
    symbol: Option<&str>,
) -> (Vec<Value>, usize, bool) {
    let mut raw = Vec::new();
    if let Some(decl) = value.get("decl_ref").and_then(Value::as_str) {
        raw.push(RawQueryHit {
            evidence_ref: decl.to_string(),
            kind: "def".to_string(),
            sym: symbol.map(str::to_string),
            confidence: None,
        });
    }
    collect_raw_query_hits(value, &mut raw);
    let mut seen = std::collections::HashSet::new();
    raw.retain(|hit| seen.insert(hit.evidence_ref.clone()));

    let total = raw.len();
    let mut hits = Vec::new();
    for (rank, hit) in raw.iter().take(QUERY_INLINE_MAX_HITS).enumerate() {
        let evidence = blob_span_evidence(snapshot, &hit.evidence_ref);
        let Some(resolved) = graphzero_store::file_target_for_evidence(
            snapshot,
            &evidence,
            &hit.kind,
            hit.sym.as_deref(),
            rank < graphzero_store::TARGET_INLINE_TOP_HITS,
        ) else {
            continue;
        };
        let mut entry = json!({
            "target": resolved.target,
            "path": resolved.path,
            "start_line": resolved.start_line,
            "end_line": resolved.end_line,
            "kind": resolved.kind,
            "sym": resolved.symbol,
            "evidence_ref": evidence,
        });
        if !resolved.content.is_empty() {
            entry["content"] = json!(resolved.content);
        }
        if let Some(confidence) = hit.confidence {
            entry["confidence"] = json!(confidence);
        }
        hits.push(entry);
    }

    let mut truncated = total > hits.len();
    let inline_tokens = tokens_for_str(&serde_json::to_string(&hits).unwrap_or_default());
    if inline_tokens > QUERY_INLINE_MAX_TOKENS {
        for entry in &mut hits {
            if let Some(obj) = entry.as_object_mut() {
                obj.remove("content");
            }
        }
        truncated = true;
    }
    (hits, total, truncated)
}

/// `HIT <path>#L<start>-L<end> kind=<kind> sym=<symbol>` for the leading hit.
fn query_hit_preview(hit: &Value) -> Option<String> {
    let target = hit.get("target").and_then(Value::as_str)?;
    let kind = hit.get("kind").and_then(Value::as_str).unwrap_or("ref");
    let sym = hit
        .get("sym")
        .and_then(Value::as_str)
        .unwrap_or("(file-scope)");
    Some(format!("HIT {target} kind={kind} sym={sym}"))
}

fn durable_spill_ref(
    store_root: &std::path::Path,
    json_text: &str,
    step: &str,
) -> Result<String, CodeModeError> {
    let id = persist_query_json(store_root, json_text)
        .map_err(|e| substrate_error(format!("durable spill failed: {e}"), step))?;
    Ok(format!("gz://query/{id}"))
}

fn map_domain_error(e: DomainError, op: &str) -> CodeModeError {
    let msg = e.message.clone();
    let mut err = match e.kind {
        DomainErrorKind::Validation => validation_error(msg, Some(op)),
        DomainErrorKind::Policy | DomainErrorKind::Unauthorized => policy_error(msg, op),
        DomainErrorKind::Sandbox => sandbox_error(msg),
        DomainErrorKind::Cancelled => cancelled_error(msg),
        DomainErrorKind::DeadlineExceeded => deadline_exceeded_error(msg),
        DomainErrorKind::Busy => busy_error(msg),
        DomainErrorKind::Approval => approval_error(msg),
        DomainErrorKind::NotFound => not_found_error(msg),
        DomainErrorKind::Runtime => runtime_error(msg),
        DomainErrorKind::Substrate => substrate_error(msg, op),
    };
    err.retryable = e.retryable;
    err
}

/// Domain dispatch from CodeMode (graphzero-o2uq.2/9).
///
/// Reuses the plan-session [`ExecutionState`] engine context (single construction
/// for multi-op recipe/JSON plans) — the production fuse path for hot CodeMode.
fn codemode_dispatch(
    state: &mut ExecutionState<'_>,
    op: &str,
    args: &Value,
) -> Result<crate::operation_abi::DomainResult, CodeModeError> {
    let ctx = state.engine_context().clone();
    crate::dispatcher::dispatch(&ctx, op, args).map_err(|e| map_domain_error(e, op))
}

pub(crate) fn run_query_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    surface: &str,
    target: &str,
) -> Result<BindingResult, CodeModeError> {
    run_query_step_with_budget(state, step_id, surface, target, None)
}

pub(crate) fn run_query_step_with_budget(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    surface: &str,
    target: &str,
    budget: Option<usize>,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    let key = format!("{surface}\0{target}");
    let was_seen = !state.seen_queries.insert(key);
    if was_seen {
        state.cache_hits += 1;
    } else {
        state.cache_misses += 1;
        state.physical_ops += 1;
    }
    state.guard_ops(step_id)?;
    let ctx = state.engine_context().clone();
    let res = query_once_with_ctx_budget(&ctx, state.current_snapshot(), surface, target, budget)?;
    state.bytes_materialized = state
        .bytes_materialized
        .saturating_add(res.bytes_materialized);
    for r in &res.refs {
        state.push_ref(r.clone())?;
    }
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: format!("query:{surface}"),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: if was_seen { 0 } else { 1 },
        refs: res.refs.clone(),
        error: None,
    });
    Ok(res)
}

pub(crate) fn run_multi_query_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    surface: &str,
    targets: &[String],
) -> Result<BindingResult, CodeModeError> {
    if targets.len() > state.limits.max_logical_ops as usize {
        return Err(policy_error(
            "multiQuery target count exceeds logical op limit",
            step_id,
        ));
    }
    state.logical_ops += targets.len() as u64;
    state.physical_ops += 1;
    state.batched_ops += 1;
    state.guard_ops(step_id)?;
    let mut results = Vec::with_capacity(targets.len());
    let mut refs = Vec::new();
    let mut bytes = 0usize;
    let ctx = state.engine_context().clone();
    for target in targets {
        let key = format!("{surface}\0{target}");
        if !state.seen_queries.insert(key) {
            state.cache_hits += 1;
        } else {
            state.cache_misses += 1;
        }
        let res = query_once_with_ctx(&ctx, state.current_snapshot(), surface, target)?;
        bytes = bytes.saturating_add(res.bytes_materialized);
        refs.extend(res.refs.clone());
        results.push(res.value);
    }
    refs.sort();
    refs.dedup();
    for r in &refs {
        state.push_ref(r.clone())?;
    }
    state.bytes_materialized = state.bytes_materialized.saturating_add(bytes);
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: format!("multiQuery:{surface}"),
        status: "completed".into(),
        logical_ops: targets.len() as u64,
        physical_ops: 1,
        refs: refs.clone(),
        error: None,
    });
    Ok(BindingResult {
        value: json!({"ack":"C","surface":surface,"refs":refs,"results":results}),
        refs,
        bytes_materialized: bytes,
    })
}

#[allow(dead_code)] // kept for external/unit helpers; plan path uses query_once_with_ctx
pub(crate) fn query_once(
    snapshot: &StoreSnapshot,
    surface: &str,
    target: &str,
) -> Result<BindingResult, CodeModeError> {
    // Standalone helper (tests); plan path uses query_once_with_ctx with warm context.
    let ctx = crate::dispatcher::EngineContext::from_snapshot(
        snapshot,
        crate::dispatcher::AdapterKind::CodeMode,
    );
    query_once_with_ctx(&ctx, snapshot, surface, target)
}

pub(crate) fn query_once_with_ctx(
    ctx: &crate::dispatcher::EngineContext,
    snapshot: &StoreSnapshot,
    surface: &str,
    target: &str,
) -> Result<BindingResult, CodeModeError> {
    query_once_with_ctx_budget(ctx, snapshot, surface, target, None)
}

pub(crate) fn query_once_with_ctx_budget(
    ctx: &crate::dispatcher::EngineContext,
    snapshot: &StoreSnapshot,
    surface: &str,
    target: &str,
    budget: Option<usize>,
) -> Result<BindingResult, CodeModeError> {
    // Single typed domain dispatcher (graphzero-o2uq.2/9) — shared with FastMCP/CLI.
    let surface = normalized_query_surface(surface);
    let args = json!({
        "surface": surface,
        "query": target,
        "target": target,
        "name": target,
        "budget": budget.unwrap_or(CODEMODE_QUERY_BUDGET),
    });
    let result = crate::dispatcher::dispatch(ctx, "query", &args)
        .map_err(|e| map_domain_error(e, "query"))?;
    let json_text = serde_json::to_string(&result.value).unwrap_or_default();
    if result.value.get("schema").and_then(Value::as_str) == Some(crate::one_tp::ONE_TP_SCHEMA) {
        return Ok(BindingResult {
            value: result.value,
            refs: result.refs,
            bytes_materialized: json_text.len(),
        });
    }
    let store_root = snapshot.store_root.as_path();
    // The full payload always stays behind a canonical ref. Ref-first refs from
    // the surface itself (`g:`, `gz://blob/…`) are not expandable as the whole
    // query response, so spill in that case rather than handing back a ref that
    // expands to something narrower than the result.
    let id = match result.refs.first().map(String::as_str) {
        Some(r) if r.starts_with("q:") || r.starts_with("gz://query/") => canonical_query_ref(r),
        _ => durable_spill_ref(store_root, &json_text, "query")?,
    };
    // Inline the bounded hit set so a plan can chain query -> read/edit in one
    // call (bead graphzero-2liik): a ref alone cannot be chained.
    let symbol = result.value.get("symbol").and_then(Value::as_str);
    let (hits, total, truncated) = inline_query_hits(snapshot, &result.value, symbol);
    let mut value = json!({
        "ack": "C",
        "surface": surface,
        "target": target,
        "ref": id,
        "hits": hits,
        "hit_count": total,
        "truncated": truncated,
    });
    if let Some(preview) = hits.first().and_then(query_hit_preview) {
        value["preview"] = json!(preview);
    }
    Ok(BindingResult {
        value,
        refs: vec![id],
        bytes_materialized: json_text.len(),
    })
}

pub(crate) fn normalized_query_surface(surface: &str) -> String {
    match surface.trim() {
        "impact" => "context".to_string(),
        "defs" => "symbol".to_string(),
        "tests" => "search".to_string(),
        "reading-set" | "readingset" | "readingSet" => "reading_set".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn run_blast_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    target: &str,
    depth: Option<u32>,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.guard_ops(step_id)?;
    let intent = if target.contains("change ") || target.contains(' ') {
        target.to_string()
    } else {
        format!("change signature of {target}")
    };
    let args = json!({
        "intent": intent,
        "budget": 100,
        "depth": depth.unwrap_or(4),
    });
    let result = codemode_dispatch(state, "blast", &args)?;
    // Soft miss already normalized by domain dispatcher.
    if result.value.get("found") == Some(&json!(false)) {
        state.steps.push(StepRecord {
            id: step_id.to_string(),
            op: "blast".into(),
            status: "completed".into(),
            logical_ops: 1,
            physical_ops: 1,
            refs: Vec::new(),
            error: None,
        });
        return Ok(BindingResult {
            value: result.value,
            refs: Vec::new(),
            bytes_materialized: 0,
        });
    }
    let json_text = serde_json::to_string(&result.value).unwrap_or_default();
    let primary_ref = match result.refs.first() {
        Some(r) => canonical_query_ref(r),
        None => durable_spill_ref(
            state.current_snapshot().store_root.as_path(),
            &json_text,
            step_id,
        )?,
    };
    state.bytes_materialized = state.bytes_materialized.saturating_add(json_text.len());
    state.push_ref(primary_ref.clone())?;
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "blast".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![primary_ref.clone()],
        error: None,
    });
    Ok(BindingResult {
        value: json!({"ack":"C","intent":intent,"ref":primary_ref}),
        refs: vec![primary_ref],
        bytes_materialized: json_text.len(),
    })
}

pub(crate) fn run_remember_value_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    payload: &Value,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.store_writes += 1;
    state.guard_ops(step_id)?;
    let result = codemode_dispatch(state, "remember", payload)?;
    let primary_ref = result
        .refs
        .first()
        .cloned()
        .or_else(|| {
            result
                .value
                .get("ref")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    state.push_ref(primary_ref.clone())?;
    let bytes = serde_json::to_vec(&result.value).unwrap_or_default();
    state.bytes_materialized = state.bytes_materialized.saturating_add(bytes.len());
    state.steps.push(StepRecord {
        id: step_id.into(),
        op: "remember".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![primary_ref.clone()],
        error: None,
    });
    Ok(BindingResult {
        value: result.value,
        refs: vec![primary_ref],
        bytes_materialized: bytes.len(),
    })
}

/// Post-edit graph claim verification (parity with the MCP `verify` tool).
pub(crate) fn run_verify_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    target: &str,
    claim: &str,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.guard_ops(step_id)?;
    let args = json!({ "target": target, "claim": claim });
    let result = codemode_dispatch(state, "verify", &args)?;
    let json_text = serde_json::to_string(&result.value).unwrap_or_default();
    let verdict = result.value.clone();
    let primary_ref = match result.refs.first() {
        Some(r) => canonical_query_ref(r),
        None => durable_spill_ref(
            state.current_snapshot().store_root.as_path(),
            &json_text,
            step_id,
        )?,
    };
    state.bytes_materialized = state.bytes_materialized.saturating_add(json_text.len());
    state.push_ref(primary_ref.clone())?;
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "verify".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![primary_ref.clone()],
        error: None,
    });
    Ok(BindingResult {
        value: json!({"ack":"C","ref":primary_ref,"verify":verdict}),
        refs: vec![primary_ref],
        bytes_materialized: json_text.len(),
    })
}

/// Budgeted snap capsule plus edit-ready anchor (tcx3 / graphzero-fjv4).
/// Prefer `zero.graph.snap(symbol)` over grep-then-read.
pub(crate) fn run_snap_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    query: &str,
    budget: usize,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.guard_ops(step_id)?;
    let args = json!({ "query": query, "budget": budget.max(1) });
    let result = codemode_dispatch(state, "snap", &args)?;
    let json_text = serde_json::to_string(&result.value).unwrap_or_default();
    let primary_ref = match result
        .refs
        .first()
        .cloned()
        .or_else(|| {
            // budget=1 snap may return a bare ref string
            result.value.as_str().map(str::to_string)
        })
        .or_else(|| {
            result
                .value
                .get("evidence_ref")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }) {
        Some(r) => r,
        None => durable_spill_ref(
            state.current_snapshot().store_root.as_path(),
            &json_text,
            step_id,
        )?,
    };
    state.bytes_materialized = state.bytes_materialized.saturating_add(json_text.len());
    state.push_ref(primary_ref.clone())?;
    let mut value = result.value.clone();
    if value.is_string() || !value.is_object() {
        value = json!({"ack":"C","ref":primary_ref,"payload":value});
    } else if let Some(obj) = value.as_object_mut() {
        obj.entry("ack".to_string()).or_insert(json!("C"));
        obj.entry("ref".to_string()).or_insert(json!(primary_ref));
    }
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "snap".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![primary_ref.clone()],
        error: None,
    });
    Ok(BindingResult {
        value,
        refs: vec![primary_ref],
        bytes_materialized: json_text.len(),
    })
}

/// Multi-agent edit reservations (parity with the MCP `reserve` tool).
/// Domain dispatcher owns all reserve semantics (no host hooks).
pub(crate) fn run_reserve_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    action: &str,
    args: &Value,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.store_writes += 1;
    state.guard_ops(step_id)?;
    let mut routed = args.clone();
    if let Some(obj) = routed.as_object_mut() {
        obj.insert("action".into(), json!(action));
    }
    let result = codemode_dispatch(state, "reserve", &routed)?;
    let bytes = serde_json::to_vec(&result.value).unwrap_or_default();
    state.bytes_materialized = state.bytes_materialized.saturating_add(bytes.len());
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "reserve".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: result.refs.clone(),
        error: None,
    });
    Ok(BindingResult {
        value: result.value,
        refs: result.refs,
        bytes_materialized: bytes.len(),
    })
}

/// Store re-index (parity with the MCP `index` tool). On success the
/// execution snapshot is reopened so subsequent in-plan reads see the publish.
pub(crate) fn run_index_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.store_writes += 1;
    state.guard_ops(step_id)?;
    let repo_root = state.current_snapshot().repo_root.clone().ok_or_else(|| {
        validation_error("index requires a repo root on the snapshot", Some(step_id))
    })?;
    let args = json!({ "path": repo_root.display().to_string() });
    let result = codemode_dispatch(state, "index", &args)?;
    state.refresh_snapshot()?;
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "index".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: Vec::new(),
        error: None,
    });
    Ok(BindingResult {
        value: result.value,
        refs: Vec::new(),
        bytes_materialized: 0,
    })
}

/// Materialize a `gz://` ref into inline text. This is the judgment
/// counterpart to ref-first results: the model explicitly asked to SEE the
/// payload, so the text is delivered inline (budget-capped) and exempted from
/// ref-first re-wrapping on return. `budget_bytes == 0` means the default
/// `max_output_bytes` limit.
pub(crate) fn run_expand_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    reference: &str,
    budget_bytes: usize,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.guard_ops(step_id)?;
    let budget = if budget_bytes == 0 {
        state.limits.max_output_bytes
    } else {
        budget_bytes
    };
    let args = json!({
        "reference": reference,
        "maxBytes": budget,
    });
    let result = codemode_dispatch(state, "expand", &args)?;
    let visible = result
        .value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let bytes_total = result
        .value
        .get("bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(visible.len() as u64) as usize;
    state.bytes_materialized = state.bytes_materialized.saturating_add(bytes_total);
    state.note_materialized(&visible);
    state.push_ref(reference.to_string())?;
    state.steps.push(StepRecord {
        id: step_id.to_string(),
        op: "expand".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![reference.to_string()],
        error: None,
    });
    Ok(BindingResult {
        value: result.value,
        refs: vec![reference.to_string()],
        bytes_materialized: bytes_total,
    })
}

pub(crate) fn ref_first_return_value(
    state: &mut ExecutionState<'_>,
    value: &Value,
) -> Result<Value, CodeModeError> {
    match value {
        Value::String(s)
            if tokens_for_str(s) > REF_FIRST_STRING_TOKENS && !state.is_materialized(s) =>
        {
            let r = store_blob_ref(state.current_snapshot().store_root.as_path(), s.as_bytes())
                .map_err(|e| substrate_error(e.to_string(), "return"))?;
            state.store_writes += 1;
            state.push_ref(r.clone())?;
            state.bytes_materialized = state.bytes_materialized.saturating_add(s.len());
            Ok(json!({ "ref": r, "preview": first_chars_flat(s, REF_FIRST_PREVIEW_CHARS) }))
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(ref_first_return_value(state, item)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), ref_first_return_value(state, v)?);
            }
            Ok(Value::Object(out))
        }
        _ => Ok(value.clone()),
    }
}

pub(crate) fn run_ctx_ref_step(
    state: &mut ExecutionState<'_>,
    step_id: &str,
    payload: &Value,
) -> Result<BindingResult, CodeModeError> {
    state.logical_ops += 1;
    state.physical_ops += 1;
    state.store_writes += 1;
    state.guard_ops(step_id)?;
    let bytes =
        serde_json::to_vec(payload).map_err(|e| validation_error(e.to_string(), Some(step_id)))?;
    if bytes.len() > state.limits.max_result_ref_bytes {
        return Err(policy_error(
            "ctx.ref payload exceeds max_result_ref_bytes",
            step_id,
        ));
    }
    let args = json!({ "value": payload });
    let result = codemode_dispatch(state, "ctx_ref", &args)?;
    let primary_ref = result
        .refs
        .first()
        .cloned()
        .or_else(|| {
            result
                .value
                .get("ref")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    state.push_ref(primary_ref.clone())?;
    state.bytes_materialized = state.bytes_materialized.saturating_add(bytes.len());
    state.steps.push(StepRecord {
        id: step_id.into(),
        op: "ctx.ref".into(),
        status: "completed".into(),
        logical_ops: 1,
        physical_ops: 1,
        refs: vec![primary_ref.clone()],
        error: None,
    });
    Ok(BindingResult {
        value: json!({"ack":"C","ref":primary_ref}),
        refs: vec![primary_ref],
        bytes_materialized: bytes.len(),
    })
}
