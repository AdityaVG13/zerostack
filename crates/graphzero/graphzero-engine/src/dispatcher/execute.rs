//! Core domain operation execution (transport-neutral).

use serde_json::{Value, json};

use graphzero_reserve::{
    DeclareRequest, IntentOperation, check_reservation_with_ttl, declare_reservation,
    list_active_reservations, release_reservation,
};
use graphzero_store::store::indexer;
use graphzero_store::store::query as store_query;
use graphzero_store::store::query::{ExportFormat, export_capsule};
use graphzero_store::{
    ClaimKind, ClaimVerifyConfig, ContentHash, ExpandResolver, GzRef, LocalSeenProvider,
    RememberInput, SeenProvider, SeenScope, Snapshot, append_verify_evidence_graph, touch_evidence,
    verify_claim,
};

use crate::blast::{blast_radius_with_depth, blast_to_value_budget, resume_blast_cursor};
use crate::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, Mutability, Operation, resolve_operation,
};
use crate::query_surface::{
    QuerySurfaceRequest, QuerySurfaceRouter, SURFACE_NAMES, keyword_surface,
    worktree_keyword_response,
};

use super::DispatchOutcome;
use super::context::EngineContext;
use super::profile::{
    attach_dispatch_phases, dispatch_phase_add, dispatch_phase_begin, dispatch_phase_ms,
    dispatch_phase_timing_enabled,
};

/// Canonical domain ops that have executors (excludes surface-meta plan ops).
/// Exhaustive coverage tests compare this list to the operation registry.
pub const DOMAIN_EXECUTABLE_OPS: &[&str] = &[
    "orient",
    "search",
    "snap",
    "remember",
    "recall",
    "expand",
    "index",
    "blast",
    "reserve",
    "verify",
    "query",
    "multi_query",
    "defs",
    "callers",
    "ctx_ref",
];

/// Surface-meta ops: adapter-owned (CodeMode plan tools), not domain dispatch targets.
pub const SURFACE_META_OPS: &[&str] = &[
    "execute_code",
    "codemode_search",
    "codemode_describe",
    "ctx_step",
];

/// Resolve `op` (canonical name or alias) and execute against the domain engine.
#[tracing::instrument(skip_all, fields(op = %op, adapter = ?ctx.adapter))]
pub fn dispatch(ctx: &EngineContext, op: &str, args: &Value) -> DispatchOutcome {
    let time = dispatch_phase_timing_enabled();
    if time {
        dispatch_phase_begin(op);
    }
    let total_start = time.then(std::time::Instant::now);
    let resolve_start = time.then(std::time::Instant::now);

    let Some(spec) = resolve_operation(op) else {
        // Legacy MCP alias tools map into reserve actions.
        if let Some(action) = legacy_reserve_tool_action(op) {
            let mut routed = args.clone();
            if let Some(obj) = routed.as_object_mut() {
                obj.insert("action".into(), json!(action));
            }
            return dispatch(ctx, "reserve", &routed);
        }
        if let Some(start) = total_start {
            dispatch_phase_add(|t| t.total_ms = dispatch_phase_ms(start.elapsed()));
        }
        return attach_dispatch_phases(Err(DomainError::new(
            DomainErrorKind::Validation,
            format!("unknown operation {op}"),
        )
        .with_op(op)));
    };
    if let Some(start) = resolve_start {
        dispatch_phase_add(|t| {
            t.resolve_ms = dispatch_phase_ms(start.elapsed());
            t.op = spec.name.to_string();
        });
    }
    let outcome = dispatch_resolved(ctx, spec, args);
    if let Some(start) = total_start {
        dispatch_phase_add(|t| t.total_ms = dispatch_phase_ms(start.elapsed()));
    }
    attach_dispatch_phases(outcome)
}

/// True when `GRAPHZERO_PROFILE_SENTINELS` is set (flamegraph stage frames).
fn profile_sentinels_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GRAPHZERO_PROFILE_SENTINELS").is_some())
}

#[inline(never)]
fn _profile_dispatch_resolved<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Execute an already-resolved registry operation.
#[tracing::instrument(skip_all, fields(op = %spec.name, adapter = ?ctx.adapter))]
pub fn dispatch_resolved(ctx: &EngineContext, spec: &Operation, args: &Value) -> DispatchOutcome {
    let body = || {
        let time = dispatch_phase_timing_enabled();
        let preflight_start = time.then(std::time::Instant::now);
        ctx.check_preflight(spec.name)?;
        if let Some(start) = preflight_start {
            dispatch_phase_add(|t| t.preflight_ms = dispatch_phase_ms(start.elapsed()));
        }
        // Mutability is classified here once; adapters must not re-classify.
        let _mutability = spec.mutability;
        let execute_start = time.then(std::time::Instant::now);
        let result = match spec.name {
            "orient" => op_orient(ctx, args),
            "search" => op_query_surface(ctx, "search", args),
            "recall" => op_query_surface(ctx, "recall", args),
            "query" => {
                let surface = args
                    .get("surface")
                    .and_then(|v| v.as_str())
                    .unwrap_or("context");
                op_query_surface(ctx, surface, args)
            }
            "multi_query" => op_multi_query(ctx, args),
            "defs" => op_query_surface(ctx, "symbol", args),
            "callers" => op_query_surface(ctx, "callers", args),
            "snap" => op_snap(ctx, args),
            "blast" => op_blast(ctx, args),
            "expand" => op_expand(ctx, args),
            "remember" => op_remember(ctx, args),
            "index" => op_index(ctx, args),
            "verify" => op_verify(ctx, args),
            "reserve" => op_reserve(ctx, args),
            // Orient sub-surfaces: name is orient.<surface>
            name if name.starts_with("orient.") => {
                let surface = name.strip_prefix("orient.").unwrap_or(name);
                op_query_surface(ctx, surface, args)
            }
            // Meta / plan surfaces are adapter-owned (CodeMode execute), not domain dispatch.
            name if SURFACE_META_OPS.contains(&name) => Err(DomainError::new(
                DomainErrorKind::Policy,
                format!("operation {name} is surface-meta and not a domain dispatch target"),
            )
            .with_op(name)),
            "ctx_ref" => op_ctx_ref(ctx, args),
            other => Err(DomainError::new(
                DomainErrorKind::Validation,
                format!("operation {other} has no domain executor yet"),
            )
            .with_op(other)),
        };
        if let Some(start) = execute_start {
            dispatch_phase_add(|t| t.execute_ms = dispatch_phase_ms(start.elapsed()));
        }
        let result = result
            .map(DomainResult::expose_primary_ref)
            .and_then(|result| apply_one_tp_budget(ctx, spec.name, args, result));
        // Cancellation can arrive while a synchronous domain op is running. If the
        // op reached a result before its next checkpoint, preserve that work instead
        // of forcing the cancelled client to repeat it.
        if ctx.is_cancelled()
            && let Ok(completed) = &result
        {
            return Err(cancelled_after_completion_error(ctx, spec.name, completed));
        }
        // Post-flight deadline: the op completed but overran. Preserve the finished
        // result behind a ref so the caller expands it instead of re-running.
        if let Some(deadline) = ctx.deadline
            && std::time::Instant::now() >= deadline
            && let Ok(completed) = &result
        {
            return Err(deadline_overrun_error(ctx, spec.name, completed));
        }
        let _ = _mutability;
        result
    };
    if profile_sentinels_enabled() {
        _profile_dispatch_resolved(body)
    } else {
        body()
    }
}

fn apply_one_tp_budget(
    ctx: &EngineContext,
    operation: &str,
    args: &Value,
    mut result: DomainResult,
) -> DispatchOutcome {
    let Some(0) = args.get("budget").and_then(Value::as_u64) else {
        return Ok(result);
    };
    let snapshot = open_snapshot(ctx).map_err(|error| {
        DomainError::new(
            DomainErrorKind::Substrate,
            format!("1TP ordinal substrate unavailable: {}", error.message),
        )
        .with_op(operation)
        .with_retryable(false)
    })?;
    let sidecar = snapshot.ordinal_sidecar().map_err(|error| {
        DomainError::new(
            DomainErrorKind::Substrate,
            format!("1TP ordinal substrate invalid: {error:#}"),
        )
        .with_op(operation)
        .with_retryable(false)
    })?;
    let value = crate::one_tp::ack(snapshot.entry.snapshot_id, sidecar.counts(), operation)
        .map_err(|error| DomainError::new(DomainErrorKind::Substrate, error).with_op(operation))?;
    result.value = value;
    Ok(result)
}

fn cancelled_after_completion_error(
    ctx: &EngineContext,
    op: &str,
    completed: &DomainResult,
) -> DomainError {
    let spill = serde_json::to_string(&completed.value).unwrap_or_default();
    match store_query::persist_query_json(ctx.store(), &spill) {
        Ok(id) => {
            let reference = format!("gz://query/{id}");
            DomainError::new(
                DomainErrorKind::Cancelled,
                format!(
                    "op {op} completed while cancellation was pending; result preserved at {reference}; expand that ref, do not re-run"
                ),
            )
            .with_op(op)
            .with_recovery_ref(reference)
            .with_retryable(false)
        }
        Err(e) => DomainError::new(
            DomainErrorKind::Cancelled,
            format!(
                "op {op} completed while cancellation was pending; durable spill failed ({e}); do not assume a recovery ref exists"
            ),
        )
        .with_op(op)
        .with_retryable(false),
    }
}

/// Compact, resumable deadline error for work that already completed.
///
/// The finished result is spilled to `gz://query/<id>` and returned as
/// `recovery_ref`; retryable is false because re-running wastes the work —
/// the remediation is expanding the ref.
fn deadline_overrun_error(ctx: &EngineContext, op: &str, completed: &DomainResult) -> DomainError {
    let spill = serde_json::to_string(&completed.value).unwrap_or_default();
    match store_query::persist_query_json(ctx.store(), &spill) {
        Ok(id) => {
            let reference = format!("gz://query/{id}");
            DomainError::new(
                DomainErrorKind::DeadlineExceeded,
                format!(
                    "op {op} completed but overran the deadline; result preserved at {reference} — expand that ref, do not re-run"
                ),
            )
            .with_op(op)
            .with_recovery_ref(reference)
            .with_retryable(false)
        }
        Err(e) => DomainError::new(
            DomainErrorKind::DeadlineExceeded,
            format!(
                "op {op} completed but overran the deadline; durable spill failed ({e}); do not assume a recovery ref exists"
            ),
        )
        .with_op(op)
        .with_retryable(false),
    }
}

/// Map legacy MCP tool names onto reserve actions.
fn legacy_reserve_tool_action(name: &str) -> Option<&'static str> {
    match name {
        "semantic_reserve_declare" => Some("declare"),
        "semantic_reserve_check" => Some("check"),
        "semantic_reserve_release" => Some("release"),
        "semantic_reserve_query" => Some("list"),
        _ => None,
    }
}

fn parse_intent_ops(value: &Value) -> Result<Vec<IntentOperation>, DomainError> {
    let arr = value.as_array().ok_or_else(|| {
        DomainError::new(
            DomainErrorKind::Validation,
            "intent_ops must be a JSON array",
        )
        .with_op("reserve")
    })?;
    let mut out = Vec::new();
    for item in arr {
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("change_signature")
            .to_string();
        let target_symbol = item
            .get("target_symbol")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let intent_text = item
            .get("intent_text")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        out.push(IntentOperation {
            kind,
            target_symbol,
            intent_text,
        });
    }
    Ok(out)
}

fn map_reserve_err(e: graphzero_reserve::ReserveError) -> DomainError {
    use graphzero_reserve::ReserveError;
    match e {
        ReserveError::Validation(s) => {
            DomainError::new(DomainErrorKind::Validation, s).with_op("reserve")
        }
        ReserveError::NotFound(s) => {
            DomainError::new(DomainErrorKind::NotFound, s).with_op("reserve")
        }
        ReserveError::Store(s) => DomainError::new(DomainErrorKind::Substrate, s)
            .with_op("reserve")
            .with_retryable(true),
    }
}

fn reserve_agent_id(args: &Value) -> Result<&str, DomainError> {
    args.get("agent_id")
        .or_else(|| args.get("agent"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DomainError::new(DomainErrorKind::Validation, "agent_id").with_op("reserve"))
}

fn remember_anchors(args: &Value) -> Vec<String> {
    if let Some(arr) = args.get("anchors").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
    }
    match args.get("anchor") {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Domain-owned multi-agent reservation semantics (no adapter hooks).
fn op_reserve(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let action = args.get("action").and_then(|v| v.as_str()).ok_or_else(|| {
        DomainError::new(DomainErrorKind::Validation, "reserve requires action").with_op("reserve")
    })?;
    // Accept legacy aliases as action values too.
    let action = match action {
        "declare" | "semantic_reserve_declare" => "declare",
        "check" | "semantic_reserve_check" => "check",
        "release" | "semantic_reserve_release" => "release",
        "list" | "query" | "semantic_reserve_query" => "list",
        other => {
            return Err(DomainError::new(
                DomainErrorKind::Validation,
                format!("unknown reserve action '{other}' (declare|check|release|list)"),
            )
            .with_op("reserve"));
        }
    };

    let store = ctx.store();
    let repo = ctx.repo();

    let value = match action {
        "declare" => {
            let agent_id = reserve_agent_id(args)?;
            let ops = parse_intent_ops(args.get("intent_ops").ok_or_else(|| {
                DomainError::new(DomainErrorKind::Validation, "intent_ops").with_op("reserve")
            })?)?;
            let ttl_seconds = args
                .get("ttl_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600);
            let resp = declare_reservation(
                store,
                repo,
                DeclareRequest {
                    agent_id: agent_id.to_string(),
                    intent_ops: ops,
                    ttl_seconds,
                },
            )
            .map_err(map_reserve_err)?;
            serde_json::to_value(&resp).map_err(|e| {
                DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("reserve")
            })?
        }
        "check" => {
            let agent_id = reserve_agent_id(args)?;
            let ops = parse_intent_ops(args.get("intent_ops").ok_or_else(|| {
                DomainError::new(DomainErrorKind::Validation, "intent_ops").with_op("reserve")
            })?)?;
            let acquire = args
                .get("acquire")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let ttl = args.get("ttl_seconds").and_then(|v| v.as_u64());
            let resp = check_reservation_with_ttl(store, repo, agent_id, &ops, acquire, ttl)
                .map_err(map_reserve_err)?;
            serde_json::to_value(&resp).map_err(|e| {
                DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("reserve")
            })?
        }
        "release" => {
            let agent_id = reserve_agent_id(args)?;
            let reservation_id = args
                .get("reservation_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    DomainError::new(DomainErrorKind::Validation, "reservation_id")
                        .with_op("reserve")
                })?;
            release_reservation(store, repo, agent_id, reservation_id).map_err(map_reserve_err)?;
            json!({ "status": "released" })
        }
        "list" => {
            let resp = list_active_reservations(store, repo).map_err(map_reserve_err)?;
            serde_json::to_value(&resp).map_err(|e| {
                DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("reserve")
            })?
        }
        _ => unreachable!(),
    };

    let mut refs = Vec::new();
    collect_string_refs(&value, &mut refs);
    Ok(
        DomainResult::new("reserve", json!({"ack":"C","action":action,"result":value}))
            .with_refs(refs)
            .with_telemetry(json!({"adapter": ctx.adapter.as_str(), "action": action})),
    )
}

fn open_snapshot(ctx: &EngineContext) -> Result<std::sync::Arc<Snapshot>, DomainError> {
    Snapshot::open_cached(ctx.store(), Some(ctx.repo()))
        .or_else(|_| Snapshot::open(ctx.store(), Some(ctx.repo())).map(std::sync::Arc::new))
        .map_err(|e| {
            DomainError::new(DomainErrorKind::Substrate, e.to_string())
                .with_op("open_snapshot")
                .with_retryable(true)
        })
}

fn op_orient(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let surface = args
        .get("surface")
        .and_then(|v| v.as_str())
        .unwrap_or("context");
    if !SURFACE_NAMES.contains(&surface) {
        return Err(DomainError::new(
            DomainErrorKind::Validation,
            crate::query_surface::unknown_surface_message(surface),
        )
        .with_op("orient"));
    }
    let mut routed = args.clone();
    if let Some(obj) = routed.as_object_mut() {
        obj.insert("surface".into(), json!(surface));
        if !obj.contains_key("query")
            && let Some(n) = obj.get("name").and_then(|v| v.as_str()).map(str::to_string)
        {
            obj.insert("query".into(), json!(n));
        }
    }
    op_query_surface(ctx, surface, &routed).map(|mut r| {
        r.op = "orient".into();
        r
    })
}

fn build_query_request(surface: &str, args: &Value) -> QuerySurfaceRequest {
    let query = args
        .get("query")
        .or_else(|| args.get("target"))
        .or_else(|| args.get("symbol"))
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            if surface == "outline" {
                query.clone()
            } else {
                None
            }
        });
    QuerySurfaceRequest {
        surface: surface.to_string(),
        name: args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| query.clone()),
        query,
        path,
        budget: args
            .get("budget")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize),
        session: args
            .get("session")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        cursor: args
            .get("cursor")
            .or_else(|| args.get("next_cursor"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

fn op_query_surface(ctx: &EngineContext, surface: &str, args: &Value) -> DispatchOutcome {
    if !SURFACE_NAMES.contains(&surface) && surface != "orient" {
        // allow only known surfaces
        if crate::query_surface::QuerySurface::parse_surface(surface).is_none() {
            return Err(DomainError::new(
                DomainErrorKind::Validation,
                crate::query_surface::unknown_surface_message(surface),
            )
            .with_op(surface));
        }
    }
    let req = build_query_request(surface, args);
    let budget = req.budget.unwrap_or(1);
    if let Some(cursor) = req.cursor.as_deref()
        && let Some(value) = QuerySurfaceRouter::resume_query_cursor(
            ctx.store(),
            cursor,
            budget,
            req.session.as_deref(),
        )
    {
        let mut refs = Vec::new();
        collect_string_refs(&value, &mut refs);
        return Ok(DomainResult::new(surface, value)
            .with_refs(refs)
            .with_telemetry(json!({
                "adapter": ctx.adapter.as_str(),
                "surface": surface,
                "budget": budget,
                "cursor": true,
            })));
    }
    let time = dispatch_phase_timing_enabled();
    let open_start = time.then(std::time::Instant::now);
    let snapshot = match open_snapshot(ctx) {
        Ok(snapshot) => snapshot,
        Err(error) if keyword_surface(surface) => {
            let Some(resp) = worktree_keyword_response(ctx.repo(), &req) else {
                return Err(error);
            };
            let value = QuerySurfaceRouter::to_json_value_with_budget_and_session(
                &resp,
                budget,
                Some(ctx.store()),
                req.session.as_deref(),
            );
            let mut refs = Vec::new();
            collect_string_refs(&value, &mut refs);
            return Ok(DomainResult::new(surface, value)
                .with_refs(refs)
                .with_telemetry(json!({
                    "adapter": ctx.adapter.as_str(),
                    "surface": surface,
                    "budget": budget,
                    "preindex": true,
                })));
        }
        Err(error) => return Err(error),
    };
    let open_ms = open_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);
    let open_detail = store_query::take_open_phase_timings();

    let router_start = time.then(std::time::Instant::now);
    let resp = QuerySurfaceRouter::execute(snapshot.as_ref(), &req).map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op(surface)
    })?;
    let router_ms = router_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);

    let ser_start = time.then(std::time::Instant::now);
    let value = QuerySurfaceRouter::to_json_value_with_budget_and_session(
        &resp,
        budget,
        Some(ctx.store()),
        req.session.as_deref(),
    );
    let serialize_ms = ser_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);

    let mut refs = Vec::new();
    if let Some(r) = resp.full_ref.clone() {
        refs.push(r);
    }
    collect_string_refs(&value, &mut refs);

    let mut telemetry = json!({
        "adapter": ctx.adapter.as_str(),
        "surface": surface,
        "budget": budget,
    });
    // Ordinal telemetry is opt-in (GRAPHZERO_ORDINAL_TIMING): the sidecar load
    // alone costs ~100ms (34MB JSON parse + integrity re-hash) and must not
    // land on the default query path. Re-gated after the store-resolution
    // merge resurrected an ungated copy of this block (graphzero perf).
    if std::env::var_os("GRAPHZERO_ORDINAL_TIMING").is_some()
        && let Ok(sidecar) = snapshot.ordinal_sidecar()
    {
        let target = req.query.as_deref().and_then(|name| {
            snapshot.global_view().ok().and_then(|view| {
                graphzero_store::SymbolTable::from_view(&view)
                    .ok()?
                    .get(name)
            })
        });
        if let Some(symbol_id) = target {
            if let Ok(reference) = sidecar.symbol_ref(symbol_id) {
                if let Some(object) = telemetry.as_object_mut() {
                    object.insert("ordinal_ref".into(), json!(reference.to_string()));
                }
                {
                    let start = std::time::Instant::now();
                    let _ = sidecar.symbol_ref(symbol_id);
                    let elapsed_ns = start.elapsed().as_nanos();
                    if let Some(object) = telemetry.as_object_mut() {
                        object.insert("ordinal_lookup_ns".into(), json!(elapsed_ns));
                    }
                    eprintln!(
                        "graphzero_ordinal_timing {{\"surface\":\"{surface}\",\"lookup_ns\":{elapsed_ns}}}"
                    );
                }
            }
        }
    }
    if time {
        graphzero_store::record_op_stages(
            surface,
            &[
                ("open_ms", open_ms),
                ("router_ms", router_ms),
                ("serialize_ms", serialize_ms),
            ],
        );
    }
    if std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some() {
        let mut phases = json!({
            "open_ms": open_ms,
            "router_ms": router_ms,
            "serialize_ms": serialize_ms,
        });
        if let Some(detail) = open_detail {
            if let Ok(v) = serde_json::to_value(&detail) {
                if let Some(obj) = phases.as_object_mut() {
                    obj.insert("open_detail".into(), v);
                }
            }
        }
        if let Some(obj) = telemetry.as_object_mut() {
            obj.insert("phases".into(), phases);
        }
    }
    Ok(DomainResult::new(surface, value)
        .with_refs(refs)
        .with_telemetry(telemetry))
}

fn op_multi_query(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let surface = args
        .get("surface")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DomainError::new(DomainErrorKind::Validation, "multi_query requires surface")
                .with_op("multi_query")
        })?;
    let targets = args
        .get("targets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            DomainError::new(DomainErrorKind::Validation, "multi_query requires targets")
                .with_op("multi_query")
        })?;
    let mut results = Vec::new();
    let mut refs = Vec::new();
    for t in targets {
        ctx.check_point("multi_query")?;
        let target = t.as_str().unwrap_or("");
        let one_args = json!({
            "surface": surface,
            "query": target,
            "target": target,
            "budget": args.get("budget").cloned().unwrap_or(json!(1)),
        });
        let r = op_query_surface(ctx, surface, &one_args)?;
        refs.extend(r.refs.clone());
        results.push(r.value);
    }
    refs.sort();
    refs.dedup();
    Ok(DomainResult::new(
        "multi_query",
        json!({"ack":"C","surface":surface,"refs":refs,"results":results}),
    )
    .with_refs(refs))
}

fn op_snap(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let query = args
        .get("query")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DomainError::new(DomainErrorKind::Validation, "snap requires query or symbol")
                .with_op("snap")
        })?;
    let budget = args.get("budget").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let session = args.get("session").and_then(|v| v.as_str());
    let snapshot = open_snapshot(ctx)?;
    // Edit-ready anchor (tcx3 / graphzero-fjv4): prefer snap-then-edit over grep-then-read.
    let edit = store_query::snap_to_edit(snapshot.as_ref(), query).ok();
    let capsule = store_query::snap(snapshot.as_ref(), query, budget, session, true)
        .map_err(|e| DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("snap"))?;
    let text = capsule.to_json(Some(ctx.store()));
    let mut value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    let mut refs = Vec::new();
    collect_string_refs(&value, &mut refs);
    if let Some(edit) = edit.as_ref() {
        let file_ref = format!("fz://file/{}#L{}", edit.best.path, edit.best.line);
        if value.is_string() || !value.is_object() {
            value = json!({
                "payload": value,
                "query": edit.query,
                "path": edit.best.path,
                "line": edit.best.line,
                "byte_span": edit.best.byte_span,
                "definition_kind": edit.best.definition_kind,
                "enclosing_block_span": edit.best.enclosing_block_span,
                "confidence": edit.best.confidence,
                "symbol": edit.best.symbol,
                "evidence_ref": edit.best.evidence_ref,
                "alternates": edit.alternates,
                "best": edit.best,
                "file_ref": file_ref,
                "dispatch_set": ["gz"],
            });
        } else if let Some(obj) = value.as_object_mut() {
            obj.insert("query".into(), json!(edit.query));
            obj.insert("path".into(), json!(edit.best.path));
            obj.insert("line".into(), json!(edit.best.line));
            obj.insert("byte_span".into(), json!(edit.best.byte_span));
            obj.insert("definition_kind".into(), json!(edit.best.definition_kind));
            obj.insert(
                "enclosing_block_span".into(),
                json!(edit.best.enclosing_block_span),
            );
            obj.insert("confidence".into(), json!(edit.best.confidence));
            obj.insert("symbol".into(), json!(edit.best.symbol));
            obj.insert("evidence_ref".into(), json!(edit.best.evidence_ref));
            obj.insert("alternates".into(), json!(edit.alternates));
            obj.insert("best".into(), json!(edit.best));
            obj.insert("file_ref".into(), json!(file_ref));
            obj.insert("dispatch_set".into(), json!(["gz"]));
        }
        if !edit.best.evidence_ref.is_empty() && !refs.iter().any(|r| r == &edit.best.evidence_ref)
        {
            refs.push(edit.best.evidence_ref.clone());
        }
    }

    // Optional export is part of the same domain execution (single snap, no re-run).
    // Format semantics match store::export_capsule (minimal/capsule/md/zst).
    if let Some(pstr) = args
        .get("export_path")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("export").and_then(|v| v.as_str()))
        .or_else(|| args.get("to_file").and_then(|v| v.as_str()))
    {
        let export_path = std::path::PathBuf::from(pstr);
        let fmt_str = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("minimal");
        let store_fmt = ExportFormat::parse_lossy(fmt_str);
        let art =
            export_capsule(&capsule, Some(ctx.store()), &export_path, store_fmt).map_err(|e| {
                DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("snap")
            })?;
        if !value.is_object() {
            // budget=1 may yield a bare ref string; wrap so export meta can attach.
            value = json!({ "payload": value });
        }
        if let Some(obj) = value.as_object_mut() {
            obj.insert("exported".into(), json!(art.path.display().to_string()));
            obj.insert("export_size".into(), json!(art.size_bytes));
            obj.insert("export_ref".into(), json!(art.ref_str));
            obj.insert("export_format".into(), json!(store_fmt.as_str()));
            // CLI snap-export meta fields historically used these names too.
            obj.insert("ref".into(), json!(art.ref_str));
            obj.insert("size_bytes".into(), json!(art.size_bytes));
            obj.insert("format".into(), json!(store_fmt.as_str()));
            obj.insert("query".into(), json!(capsule.query));
            obj.insert("snapshot_id".into(), json!(capsule.snapshot_id));
        }
        if !refs.iter().any(|r| r == &art.ref_str) {
            refs.push(art.ref_str);
        }
    }

    Ok(DomainResult::new("snap", value).with_refs(refs))
}

fn op_blast(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let intent = args
        .get("intent")
        .or_else(|| args.get("query"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DomainError::new(DomainErrorKind::Validation, "blast requires intent").with_op("blast")
        })?;
    let budget = args.get("budget").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as u32;
    if let Some(cursor) = args.get("cursor").and_then(|v| v.as_str())
        && let Some(value) = resume_blast_cursor(ctx.store(), cursor, budget)
    {
        let mut refs = Vec::new();
        collect_string_refs(&value, &mut refs);
        return Ok(DomainResult::new("blast", value)
            .with_refs(refs)
            .with_telemetry(json!({
                "adapter": ctx.adapter.as_str(),
                "budget": budget,
                "cursor": true,
            })));
    }
    let time = dispatch_phase_timing_enabled();
    let open_start = time.then(std::time::Instant::now);
    let snapshot = open_snapshot(ctx)?;
    let open_ms = open_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);
    let open_detail = store_query::take_open_phase_timings();

    let compute_start = time.then(std::time::Instant::now);
    let capsule = match blast_radius_with_depth(snapshot.as_ref(), intent, budget, depth) {
        Ok(c) => c,
        // Unresolvable symbol is a MISS (answer), not an exception — CodeMode parity.
        Err(e) if e.to_string().contains("symbol not found") => {
            let compute_ms = compute_start
                .map(|s| dispatch_phase_ms(s.elapsed()))
                .unwrap_or(0.0);
            let mut telemetry = json!({"adapter": ctx.adapter.as_str(), "miss": true});
            if time {
                graphzero_store::record_op_stages(
                    "blast",
                    &[
                        ("open_ms", open_ms),
                        ("compute_ms", compute_ms),
                        ("serialize_ms", 0.0),
                    ],
                );
            }
            if std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some() {
                if let Some(obj) = telemetry.as_object_mut() {
                    obj.insert(
                        "phases".into(),
                        json!({
                            "open_ms": open_ms,
                            "compute_ms": compute_ms,
                            "serialize_ms": 0.0,
                        }),
                    );
                }
            }
            return Ok(DomainResult::new(
                "blast",
                json!({"ack":"C","intent":intent,"found":false,"note":e.to_string()}),
            )
            .with_refs(vec![])
            .with_telemetry(telemetry));
        }
        Err(e) => {
            return Err(
                DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("blast")
            );
        }
    };
    let compute_ms = compute_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);

    let ser_start = time.then(std::time::Instant::now);
    let value = blast_to_value_budget(&capsule, budget, Some(ctx.store())).map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("blast")
    })?;
    let serialize_ms = ser_start
        .map(|s| dispatch_phase_ms(s.elapsed()))
        .unwrap_or(0.0);

    let mut refs = Vec::new();
    collect_string_refs(&value, &mut refs);

    let mut telemetry = json!({
        "adapter": ctx.adapter.as_str(),
        "budget": budget,
        "depth": depth,
    });
    // Ordinal telemetry is opt-in (GRAPHZERO_ORDINAL_TIMING): see the matching
    // gate on the orient path. Re-gated after the store-resolution merge
    // resurrected an ungated copy of this block (graphzero perf).
    if std::env::var_os("GRAPHZERO_ORDINAL_TIMING").is_some()
        && let Ok(sidecar) = snapshot.ordinal_sidecar()
        && let Ok(view) = snapshot.global_view()
        && let Ok(table) = graphzero_store::SymbolTable::from_view(&view)
        && let Some(symbol_id) = table.get(&capsule.target_symbol)
        && let Ok(reference) = sidecar.symbol_ref(symbol_id)
    {
        if let Some(object) = telemetry.as_object_mut() {
            object.insert("ordinal_ref".into(), json!(reference.to_string()));
        }
        {
            let start = std::time::Instant::now();
            let _ = sidecar.symbol_ref(symbol_id);
            let elapsed_ns = start.elapsed().as_nanos();
            if let Some(object) = telemetry.as_object_mut() {
                object.insert("ordinal_lookup_ns".into(), json!(elapsed_ns));
            }
            eprintln!(
                "graphzero_ordinal_timing {{\"surface\":\"blast\",\"lookup_ns\":{elapsed_ns}}}"
            );
        }
    }
    if time {
        graphzero_store::record_op_stages(
            "blast",
            &[
                ("open_ms", open_ms),
                ("compute_ms", compute_ms),
                ("serialize_ms", serialize_ms),
            ],
        );
    }
    if std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some() {
        let mut phases = json!({
            "open_ms": open_ms,
            "compute_ms": compute_ms,
            "serialize_ms": serialize_ms,
        });
        if let Some(detail) = open_detail {
            if let Ok(v) = serde_json::to_value(&detail) {
                if let Some(obj) = phases.as_object_mut() {
                    obj.insert("open_detail".into(), v);
                }
            }
        }
        if let Some(obj) = telemetry.as_object_mut() {
            obj.insert("phases".into(), phases);
        }
    }
    Ok(DomainResult::new("blast", value)
        .with_refs(refs)
        .with_telemetry(telemetry))
}

/// A-RAG already-read key: hash of the exact delivered bytes + the ref
/// fragment, so only byte-identical responses for the same slice dedup.
fn expand_seen_key(delivered: &str, reference: &str) -> String {
    let sha = ContentHash::of(delivered.as_bytes()).to_hex();
    let fragment = reference.split_once('#').map(|(_, f)| f).unwrap_or("");
    format!("expand:{sha}#{fragment}")
}

/// Session-scoped already-read check for one expand payload.
///
/// Marks the delivered bytes as seen and reports whether an identical payload
/// was already returned in this session. No session → never dedups.
fn expand_already_returned(session: Option<&str>, delivered: &str, reference: &str) -> bool {
    let Some(session) = session else {
        return false;
    };
    let scope = SeenScope::Session(session.to_string());
    let key = expand_seen_key(delivered, reference);
    let seen = LocalSeenProvider.is_seen(&scope, &key);
    LocalSeenProvider.mark_seen(&scope, &key);
    seen
}

fn expand_already_returned_notice(reference: &str) -> String {
    format!("already returned as {reference} in this session; pass full=true to re-send the bytes")
}

fn op_expand(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let mut refs: Vec<String> = args
        .get("references")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if refs.is_empty()
        && let Some(s) = args.get("reference").and_then(|v| v.as_str())
    {
        refs.push(s.to_string());
    }
    if refs.is_empty() {
        return Err(DomainError::new(
            DomainErrorKind::Validation,
            "expand requires reference or references",
        )
        .with_op("expand"));
    }
    let mut resolver = ExpandResolver::new(ctx.store(), Some(ctx.repo())).map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("expand")
    })?;
    resolver = apply_expand_session_auth(resolver, args, ctx);
    let max_bytes = args
        .get("maxBytes")
        .or_else(|| args.get("max_bytes"))
        .or_else(|| args.get("budget_bytes"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let session = args.get("session").and_then(|v| v.as_str());
    let force_full = args
        .get("full")
        .or_else(|| args.get("force"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if refs.len() == 1 {
        let reference = &refs[0];
        let gz = GzRef::parse(reference).map_err(|e| {
            DomainError::new(DomainErrorKind::Validation, e.to_string()).with_op("expand")
        })?;
        match resolver.resolve(&gz, reference) {
            Ok(res) => {
                let full = String::from_utf8_lossy(&res.bytes).into_owned();
                let bytes_total = res.bytes.len();
                let (text, truncated) = match max_bytes {
                    Some(0) | None => (full, false),
                    Some(budget) if full.len() > budget => {
                        let mut end = budget;
                        while end > 0 && !full.is_char_boundary(end) {
                            end -= 1;
                        }
                        (full[..end].to_string(), true)
                    }
                    Some(_) => (full, false),
                };
                if expand_already_returned(session, &text, reference) && !force_full {
                    return Ok(DomainResult::new(
                        "expand",
                        json!({
                            "ack": "C",
                            "ref": reference,
                            "already_returned": true,
                            "notice": expand_already_returned_notice(reference),
                            "truncated": truncated,
                            "bytes": bytes_total,
                            "source": res.source,
                            "reference": reference,
                        }),
                    )
                    .with_refs(vec![reference.clone()]));
                }
                let _ = touch_evidence(ctx.store(), reference, graphzero_store::frecency_now());
                Ok(DomainResult::new(
                    "expand",
                    json!({
                        "ack": "C",
                        "ref": reference,
                        "text": text,
                        "truncated": truncated,
                        "bytes": bytes_total,
                        "source": res.source,
                        "reference": reference,
                    }),
                )
                .with_refs(vec![reference.clone()]))
            }
            Err(e) => Err(expand_domain_error(e)),
        }
    } else {
        let items: Vec<Value> = refs
            .iter()
            .map(|reference| match GzRef::parse(reference) {
                Ok(gz) => match resolver.resolve(&gz, reference) {
                    Ok(res) => {
                        let text = String::from_utf8_lossy(&res.bytes).into_owned();
                        if expand_already_returned(session, &text, reference) && !force_full {
                            json!({
                                "reference": reference,
                                "ok": true,
                                "already_returned": true,
                                "notice": expand_already_returned_notice(reference),
                            })
                        } else {
                            let _ = touch_evidence(
                                ctx.store(),
                                reference,
                                graphzero_store::frecency_now(),
                            );
                            json!({ "reference": reference, "ok": true, "text": text })
                        }
                    }
                    Err(e) => json!({
                        "reference": reference,
                        "ok": false,
                        "error": e.reason,
                        "kind": e.kind.as_str(),
                        "ref": e.reference,
                        "trace": e.trace.iter().map(|s| json!({
                            "store": s.store,
                            "result": s.result
                        })).collect::<Vec<_>>(),
                    }),
                },
                Err(e) => json!({ "reference": reference, "ok": false, "error": e.to_string() }),
            })
            .collect();
        Ok(DomainResult::new("expand", json!({ "results": items })).with_refs(refs))
    }
}

fn apply_expand_session_auth(
    mut resolver: ExpandResolver,
    args: &Value,
    ctx: &EngineContext,
) -> ExpandResolver {
    let mut roots: Vec<std::path::PathBuf> = args
        .get("authorized_roots")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(std::path::PathBuf::from))
        .collect();
    if args
        .get("bind_store_root")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        roots.push(ctx.store().to_path_buf());
    }
    if !roots.is_empty() {
        resolver = resolver.with_authorized_roots(roots);
    }
    let expected_digest = args
        .get("expected_contract_digest")
        .and_then(|v| v.as_str());
    let actual_digest = args.get("actual_contract_digest").and_then(|v| v.as_str());
    let expected_rev = args
        .get("expected_worker_revision")
        .and_then(|v| v.as_str());
    let actual_rev = args.get("actual_worker_revision").and_then(|v| v.as_str());
    if let (Some(ed), Some(ad), Some(er), Some(ar)) =
        (expected_digest, actual_digest, expected_rev, actual_rev)
    {
        resolver = resolver.with_worker_identity(ed, ad, er, ar);
    }
    resolver
}

fn expand_domain_error(err: graphzero_store::ExpandError) -> DomainError {
    use graphzero_store::ExpandErrorKind;
    let kind = match err.kind {
        ExpandErrorKind::WrongRoot => DomainErrorKind::Unauthorized,
        ExpandErrorKind::Expired => DomainErrorKind::DeadlineExceeded,
        ExpandErrorKind::WorkerSkew => DomainErrorKind::Policy,
        ExpandErrorKind::InvalidRef => DomainErrorKind::Validation,
        ExpandErrorKind::DigestMismatch => DomainErrorKind::Substrate,
        ExpandErrorKind::NotFound | ExpandErrorKind::Other => DomainErrorKind::NotFound,
    };
    DomainError::new(kind, err.reason.clone())
        .with_op("expand")
        .with_retryable(false)
        // recovery_ref carries ExpandError::to_json once so MCP envelopes can
        // surface `trace`/`kind` without nesting JSON inside DomainError.message.
        .with_recovery_ref(err.to_json())
}

fn op_remember(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DomainError::new(DomainErrorKind::Validation, "remember requires text")
                .with_op("remember")
        })?
        .to_string();
    let anchors: Vec<String> = remember_anchors(args);
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let supersedes: Vec<String> = args
        .get("supersedes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let snapshot = open_snapshot(ctx)?;
    let fact = graphzero_store::remember_fact(
        snapshot.as_ref(),
        RememberInput {
            text,
            anchors,
            kind,
            supersedes,
        },
    )
    .map_err(|e| DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("remember"))?;
    let mem = graphzero_store::mem_ref(&fact.id);
    Ok(
        DomainResult::new("remember", json!({ "ack": "C", "ref": mem, "id": fact.id }))
            .with_refs(vec![mem]),
    )
}

fn op_index(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.repo_root.clone());
    let store = if path == ctx.repo_root {
        ctx.store_root.clone()
    } else {
        graphzero_store::resolve_graphzero_store_root(&path)
    };
    ctx.check_point("index")?;
    let entry = indexer::index_repo_with_budget(
        &path,
        &store,
        ctx.deadline,
        Some(ctx.cancellation_token().as_arc()),
    )
    .map_err(|e| {
        let text = e.to_string();
        if text.contains("cancelled") {
            DomainError::new(DomainErrorKind::Cancelled, text).with_op("index")
        } else if text.contains("deadline") {
            DomainError::new(DomainErrorKind::DeadlineExceeded, text)
                .with_op("index")
                .with_retryable(true)
        } else {
            DomainError::new(DomainErrorKind::Substrate, text).with_op("index")
        }
    })?;
    Snapshot::clear_open_cache();
    let mut value = json!({
        "ack": "C",
        "indexed": true,
        "snapshot": entry.snapshot_id,
        "shards": entry.shard_hashes.len(),
        "store": store.display().to_string(),
        "path": path.display().to_string(),
        "snapshot_refreshed": true,
    });
    // Env-gated host phase clocks (GRAPHZERO_INDEX_PHASE_TIMING): attach + eprint.
    if let Some(phases) = indexer::take_index_phase_timings() {
        if let Ok(phases_val) = serde_json::to_value(&phases) {
            eprintln!(
                "graphzero_index_phase_timing {}",
                serde_json::to_string(&phases_val).unwrap_or_else(|_| "{}".into())
            );
            if let Some(obj) = value.as_object_mut() {
                obj.insert("phases".into(), phases_val);
            }
        }
    }
    Ok(DomainResult::new("index", value).with_telemetry(json!({"adapter": ctx.adapter.as_str()})))
}

fn op_verify(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let target = args.get("target").and_then(|v| v.as_str()).ok_or_else(|| {
        DomainError::new(DomainErrorKind::Validation, "verify requires target").with_op("verify")
    })?;
    let claim = args
        .get("claim")
        .and_then(|v| v.as_str())
        .unwrap_or("no_remaining_callers");
    let kind = ClaimKind::parse_claim_kind(claim).ok_or_else(|| {
        DomainError::new(
            DomainErrorKind::Validation,
            format!("unknown claim {claim:?}"),
        )
        .with_op("verify")
    })?;
    let snapshot = open_snapshot(ctx)?;
    let result = verify_claim(
        snapshot.as_ref(),
        kind,
        target,
        ClaimVerifyConfig::default(),
    )
    .map_err(|e| DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("verify"))?;
    // Same-execution graph augmentation (pre-bead CLI behavior); not a second verify.
    let graph =
        append_verify_evidence_graph(ctx.store(), &result, "graphzero.verify").map_err(|e| {
            DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("verify")
        })?;
    let text = result.to_json_string().map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("verify")
    })?;
    let mut value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if let Some(graph) = graph
        && let Some(obj) = value.as_object_mut()
    {
        obj.insert("verification_graph".into(), graph);
    }
    let mut refs = Vec::new();
    collect_string_refs(&value, &mut refs);
    Ok(DomainResult::new("verify", value).with_refs(refs))
}

fn op_ctx_ref(ctx: &EngineContext, args: &Value) -> DispatchOutcome {
    let value = args.get("value").cloned().ok_or_else(|| {
        DomainError::new(DomainErrorKind::Validation, "ctx_ref requires value").with_op("ctx_ref")
    })?;
    let bytes = serde_json::to_vec(&value).map_err(|e| {
        DomainError::new(DomainErrorKind::Validation, e.to_string()).with_op("ctx_ref")
    })?;
    let store = graphzero_store::BlobStore::open(ctx.store()).map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("ctx_ref")
    })?;
    let hash = store.put(&bytes).map_err(|e| {
        DomainError::new(DomainErrorKind::Substrate, e.to_string()).with_op("ctx_ref")
    })?;
    let r = format!("gz://blob/{}", hash.to_hex());
    Ok(DomainResult::new("ctx_ref", json!({ "ref": r })).with_refs(vec![r]))
}

fn collect_string_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s)
            if s.starts_with("gz://") || s.starts_with("q:") || s.starts_with("g:") =>
        {
            out.push(s.clone());
        }
        Value::Array(a) => {
            for v in a {
                collect_string_refs(v, out);
            }
        }
        Value::Object(m) => {
            for (k, v) in m {
                if k == "ref" || k == "full_ref" || k.ends_with("_ref") {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                }
                collect_string_refs(v, out);
            }
        }
        _ => {}
    }
    out.sort();
    out.dedup();
}

// Silence unused Mutability warning in some builds by using it in a const assert style helper.
const _: fn(Mutability) = |m| {
    let _ = matches!(m, Mutability::ReadOnly | Mutability::StoreOnly);
};

use std::path::PathBuf;
