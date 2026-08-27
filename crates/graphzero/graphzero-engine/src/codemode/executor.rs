//! Top-level CodeMode execution: plan dispatch, JSON DAG execution, step scheduling.
//!
//! ## JSON parallel groups (graphzero-zerostack-parity-b5ci.5)
//!
//! Ready steps with no inter-dependencies form a parallel group. Independent
//! **read-only** ops in a group run concurrently up to `max_parallel_width`
//! (default 2). Store-mutating JSON ops (`verify`, `remember`, `ref`, …) always
//! serialize. Failure policy: cancel siblings, wait for in-flight joins, return
//! the earliest-in-plan-order error. Results and quota counters merge in
//! schedule order so envelopes stay deterministic.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use graphzero_store::Snapshot as StoreSnapshot;

use crate::dispatcher::CancellationToken;

use super::CodeModeHostOps;
use super::errors::{cancelled_error, policy_error, validation_error};
use super::plan::{PlanKind, classify_plan, execute_recipe};
use super::response::finish_response;
use super::state::ExecutionState;
use super::steps::{
    run_blast_step, run_ctx_ref_step, run_expand_step, run_multi_query_step, run_query_step,
    run_query_step_with_budget, run_remember_value_step, run_snap_step, run_verify_step,
};
use super::types::{
    BindingResult, CodeModeError, CodeModeLimits, CodeModeResponse, CodeModeTelemetry,
    MAX_LOGICAL_OPS, StepRecord,
};
use super::utils::{execution_id, now_rfc3339ish};

/// Test/observability peak concurrency inside a parallel JSON group.
static PARALLEL_GROUP_PEAK: AtomicUsize = AtomicUsize::new(0);
static PARALLEL_GROUP_INFLIGHT: AtomicUsize = AtomicUsize::new(0);

fn execute_code_plan(
    _state: &mut ExecutionState<'_>,
    _code: &str,
) -> Result<BindingResult, CodeModeError> {
    Err(policy_error(
        "GraphZero does not embed a JavaScript runtime. Run JavaScript plans through the aggregate `zerostack-codemode-host` or `zsx` against the graphzero raw worker; recipe and JSON-DAG plans remain supported.",
        "aggregate-codemode-host",
    ))
}

/// Peak observed in-flight parallel JSON steps (reset between assertions in tests).
pub fn parallel_group_peak() -> usize {
    PARALLEL_GROUP_PEAK.load(AtomicOrdering::SeqCst)
}

pub fn reset_parallel_group_peak_for_tests() {
    PARALLEL_GROUP_PEAK.store(0, AtomicOrdering::SeqCst);
    PARALLEL_GROUP_INFLIGHT.store(0, AtomicOrdering::SeqCst);
}

/// Host-timed CodeMode plan stages (env `GRAPHZERO_CODEMODE_PHASE_TIMING=1`).
///
/// When set: classify / execute / finish walls are recorded and attached under
/// `telemetry.extra.phases`. When off: no Instant clocks beyond the existing
/// outer `wall_ms`.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CodeModePhaseTimings {
    pub classify_ms: f64,
    pub execute_ms: f64,
    pub finish_ms: f64,
    pub total_ms: f64,
    pub plan_kind: String,
}

fn codemode_phase_timing_enabled() -> bool {
    std::env::var_os("GRAPHZERO_CODEMODE_PHASE_TIMING").is_some()
}

fn codemode_phase_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn note_parallel_enter() {
    let cur = PARALLEL_GROUP_INFLIGHT.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    PARALLEL_GROUP_PEAK.fetch_max(cur, AtomicOrdering::SeqCst);
}

fn note_parallel_leave() {
    PARALLEL_GROUP_INFLIGHT.fetch_sub(1, AtomicOrdering::SeqCst);
}

pub fn execute_plan(snapshot: &StoreSnapshot, plan: &str) -> String {
    execute(snapshot, plan).compact_line()
}

pub fn execute(snapshot: &StoreSnapshot, plan: &str) -> CodeModeResponse {
    execute_with_host(snapshot, plan, None)
}

pub fn execute_with_host(
    snapshot: &StoreSnapshot,
    plan: &str,
    host: Option<&dyn CodeModeHostOps>,
) -> CodeModeResponse {
    execute_with_host_and_limits(snapshot, plan, host, CodeModeLimits::default())
}

/// Execute a CodeMode plan with explicit limits (used for timeout regressions).
pub fn execute_with_host_and_limits(
    snapshot: &StoreSnapshot,
    plan: &str,
    host: Option<&dyn CodeModeHostOps>,
    limits: CodeModeLimits,
) -> CodeModeResponse {
    execute_with_host_options(snapshot, plan, host, limits, None)
}

pub fn execute_with_host_options(
    snapshot: &StoreSnapshot,
    plan: &str,
    host: Option<&dyn CodeModeHostOps>,
    limits: CodeModeLimits,
    form: Option<&str>,
) -> CodeModeResponse {
    execute_with_host_options_controlled(
        snapshot,
        plan,
        host,
        limits,
        form,
        CancellationToken::default(),
    )
}

pub fn execute_with_host_options_controlled(
    snapshot: &StoreSnapshot,
    plan: &str,
    host: Option<&dyn CodeModeHostOps>,
    limits: CodeModeLimits,
    form: Option<&str>,
    cancellation: CancellationToken,
) -> CodeModeResponse {
    let forced_kind = match form {
        Some("auto") | None => None,
        Some(form) => match PlanKind::from_form(form) {
            Some(kind) => Some(kind),
            None => {
                return materialize_failure(
                    snapshot.store_root.as_path(),
                    plan,
                    validation_error(format!("unknown CodeMode form '{form}'"), Some("form")),
                );
            }
        },
    };
    execute_with_host_and_limits_gated(snapshot, plan, host, limits, forced_kind, cancellation)
}

#[tracing::instrument(
    skip_all,
    fields(
        plan_len = plan.len(),
        has_host = host.is_some(),
        forced_kind = forced_kind.map(|k| k.as_str()).unwrap_or("auto"),
    )
)]
fn execute_with_host_and_limits_gated(
    snapshot: &StoreSnapshot,
    plan: &str,
    host: Option<&dyn CodeModeHostOps>,
    limits: CodeModeLimits,
    forced_kind: Option<PlanKind>,
    cancellation: CancellationToken,
) -> CodeModeResponse {
    let started = now_rfc3339ish();
    let timer = Instant::now();
    let time_phases = codemode_phase_timing_enabled();
    let execution_id = execution_id(plan);
    let classify_start = time_phases.then(Instant::now);
    let kind = forced_kind.unwrap_or_else(|| classify_plan(plan));
    let classify_ms = classify_start
        .map(|s| codemode_phase_ms(s.elapsed()))
        .unwrap_or(0.0);
    let code = plan.trim();
    let mut state = ExecutionState::new(snapshot, host);
    state.limits = limits;
    let deadline = timer.checked_add(Duration::from_millis(
        u64::try_from(state.limits.max_wall_ms).unwrap_or(u64::MAX),
    ));
    state.set_control(cancellation, deadline);

    let execute_start = time_phases.then(Instant::now);
    let result = if let Err(error) = state.guard_ops("plan") {
        Err(error)
    } else if code.is_empty() {
        Err(validation_error("empty CodeMode plan", None))
    } else if code.len() > state.limits.max_code_bytes {
        Err(policy_error(
            format!(
                "code byte limit exceeded: {} > {}",
                code.len(),
                state.limits.max_code_bytes
            ),
            "code",
        ))
    } else {
        match kind {
            PlanKind::Recipe => execute_recipe(&mut state, code),
            PlanKind::Json => execute_json_plan(&mut state, code),
            PlanKind::Code => execute_code_plan(&mut state, code),
        }
    };

    let result = if state.cancellation_requested() {
        Err(cancelled_error("client cancelled during plan"))
    } else {
        result
    };
    let execute_ms = execute_start
        .map(|s| codemode_phase_ms(s.elapsed()))
        .unwrap_or(0.0);

    let wall_ms = timer.elapsed().as_millis();
    let finished = now_rfc3339ish();
    let mut extra = BTreeMap::new();
    if time_phases {
        let phases = CodeModePhaseTimings {
            classify_ms,
            execute_ms,
            // finish_ms filled after envelope build is not free here; measure
            // residual wall as finish (telemetry assembly + finish_response
            // attribution is outer). Record zero now; stamp total from timer.
            finish_ms: 0.0,
            total_ms: codemode_phase_ms(timer.elapsed()),
            plan_kind: kind.as_str().to_string(),
        };
        if let Ok(v) = serde_json::to_value(&phases) {
            eprintln!(
                "graphzero_codemode_phase_timing {}",
                serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
            );
            extra.insert("phases".into(), v);
        }
    }
    let mut telemetry = CodeModeTelemetry {
        execution_id: execution_id.clone(),
        kind: "codemode.execute".to_string(),
        status: if result.is_ok() { "ok" } else { "error" }.to_string(),
        plan_kind: kind.as_str().to_string(),
        visible_ack: if result.is_ok() { "C" } else { "X0" }.to_string(),
        steps_run: state.steps.len() as u64,
        logical_ops: state.logical_ops,
        physical_ops: state.physical_ops,
        batched_ops: state.batched_ops,
        internal_actions: state.physical_ops + state.store_writes,
        parallel_groups: state.parallel_groups,
        refs: state.refs.clone(),
        cache_hits: state.cache_hits,
        cache_misses: state.cache_misses,
        store_writes: state.store_writes,
        wall_ms,
        started_at: started.clone(),
        finished_at: finished.clone(),
        round_trips: 1,
        visible_ack_count: 1,
        bytes_materialized: state.bytes_materialized,
        raw_token_estimate: 0,
        visible_token_estimate: 0,
        measurement_coverage_pct: 0,
        extra,
    };

    let finish_start = time_phases.then(Instant::now);
    if wall_ms > state.limits.max_wall_ms && result.is_ok() {
        telemetry.status = "error".into();
        let resp = finish_response(
            snapshot.store_root.as_path(),
            execution_id,
            kind.as_str(),
            code,
            state.steps,
            telemetry,
            Err(policy_error(
                format!("wall time limit exceeded: {wall_ms}ms"),
                "wall_ms",
            )),
        );
        return stamp_codemode_finish_ms(resp, finish_start, timer, time_phases);
    }

    let resp = finish_response(
        snapshot.store_root.as_path(),
        execution_id,
        kind.as_str(),
        code,
        state.steps,
        telemetry,
        result,
    );
    stamp_codemode_finish_ms(resp, finish_start, timer, time_phases)
}

fn stamp_codemode_finish_ms(
    mut resp: CodeModeResponse,
    finish_start: Option<Instant>,
    timer: Instant,
    time_phases: bool,
) -> CodeModeResponse {
    if !time_phases {
        return resp;
    }
    let finish_ms = finish_start
        .map(|s| codemode_phase_ms(s.elapsed()))
        .unwrap_or(0.0);
    let total_ms = codemode_phase_ms(timer.elapsed());
    if let Some(phases) = resp.telemetry.extra.get_mut("phases") {
        if let Some(obj) = phases.as_object_mut() {
            obj.insert("finish_ms".into(), json!(finish_ms));
            obj.insert("total_ms".into(), json!(total_ms));
        }
    }
    resp
}

/// Materialize a durable failure envelope (error_ref + execution files) without
/// running a plan. Used when MCP setup fails so the host still gets expandable
/// gz:// recovery refs instead of a bare transport error.
pub fn materialize_failure(
    store_root: &std::path::Path,
    plan: &str,
    error: CodeModeError,
) -> CodeModeResponse {
    let kind = classify_plan(plan);
    let code = plan.trim();
    let execution_id = execution_id(plan);
    let now = now_rfc3339ish();
    let telemetry = CodeModeTelemetry {
        execution_id: execution_id.clone(),
        kind: "codemode.execute".to_string(),
        status: "error".to_string(),
        plan_kind: kind.as_str().to_string(),
        visible_ack: "X0".to_string(),
        steps_run: 0,
        logical_ops: 0,
        physical_ops: 0,
        batched_ops: 0,
        internal_actions: 0,
        parallel_groups: 0,
        refs: Vec::new(),
        cache_hits: 0,
        cache_misses: 0,
        store_writes: 0,
        wall_ms: 0,
        started_at: now.clone(),
        finished_at: now,
        round_trips: 1,
        visible_ack_count: 1,
        bytes_materialized: 0,
        raw_token_estimate: 0,
        visible_token_estimate: 0,
        measurement_coverage_pct: 0,
        extra: BTreeMap::new(),
    };
    finish_response(
        store_root,
        execution_id,
        kind.as_str(),
        code,
        Vec::new(),
        telemetry,
        Err(error),
    )
}

// ── JSON plan executor ──

pub(crate) fn execute_json_plan(
    state: &mut ExecutionState<'_>,
    plan: &str,
) -> Result<BindingResult, CodeModeError> {
    let value: Value = serde_json::from_str(plan)
        .map_err(|e| validation_error(format!("invalid JSON plan: {e}"), Some("json")))?;
    if let Some(recipe) = value.get("recipe").and_then(Value::as_str) {
        return execute_recipe(state, recipe);
    }
    let steps = value
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            validation_error(
                "JSON plan requires steps array or recipe",
                Some("json.steps"),
            )
        })?;
    let order = schedule_json_steps(steps)?;
    let mut outputs: BTreeMap<String, Value> = BTreeMap::new();
    let mut last = BindingResult {
        value: json!(null),
        refs: Vec::new(),
        bytes_materialized: 0,
    };
    for group in order {
        if group.len() > 1 {
            state.parallel_groups += 1;
        }
        let parallel_ok = group.len() > 1
            && group.iter().all(|&idx| {
                let op = steps[idx]
                    .get("op")
                    .and_then(Value::as_str)
                    .unwrap_or("query");
                is_parallel_safe_json_op(op)
            });
        if parallel_ok {
            last = execute_parallel_json_group(state, steps, &group, &mut outputs)?;
        } else {
            for idx in group {
                let res = run_json_step(state, &steps[idx], &outputs)?;
                let id = required_step_id(&steps[idx])?;
                outputs.insert(id.to_string(), res.value.clone());
                last = res;
            }
        }
    }
    Ok(last)
}

/// Read-only JSON ops that may share a snapshot across threads.
pub(crate) fn is_parallel_safe_json_op(op: &str) -> bool {
    matches!(
        op,
        "query"
            | "defs"
            | "callers"
            | "reading_set"
            | "readingSet"
            | "tests"
            | "orient"
            | "recall"
            | "snap"
            | "blast"
            | "expand"
            | "multiQuery"
            | "batch"
    )
}

struct ParallelStepOutcome {
    id: String,
    result: BindingResult,
    steps: Vec<StepRecord>,
    logical_ops: u64,
    physical_ops: u64,
    batched_ops: u64,
    store_writes: u64,
    bytes_materialized: usize,
}

fn execute_parallel_json_group(
    state: &mut ExecutionState<'_>,
    steps: &[Value],
    group: &[usize],
    outputs: &mut BTreeMap<String, Value>,
) -> Result<BindingResult, CodeModeError> {
    let width = state.limits.max_parallel_width.max(1);
    let mut last = BindingResult {
        value: json!(null),
        refs: Vec::new(),
        bytes_materialized: 0,
    };
    // Pre-resolve targets against current outputs so workers need no shared map.
    let prepared: Result<Vec<(usize, Value)>, CodeModeError> = group
        .iter()
        .map(|&idx| {
            let step = &steps[idx];
            let op = step.get("op").and_then(Value::as_str).unwrap_or("query");
            if is_forbidden_mutating_op(op) {
                let id = required_step_id(step)?;
                return Err(policy_error(
                    format!("CodeMode is read-only; mutating JSON op '{op}' is unavailable"),
                    id,
                ));
            }
            // Materialize a step copy with resolved target fields for the worker.
            let mut cloned = step.clone();
            if let Some(obj) = cloned.as_object_mut() {
                if !obj.contains_key("target")
                    && matches!(
                        op,
                        "query"
                            | "defs"
                            | "callers"
                            | "reading_set"
                            | "readingSet"
                            | "tests"
                            | "blast"
                            | "recall"
                            | "verify"
                            | "expand"
                    )
                {
                    let target = step_target(step, outputs)?;
                    obj.insert("target".into(), json!(target));
                }
                if op == "orient" && !obj.contains_key("query") && !obj.contains_key("target") {
                    let target = step_target(step, outputs)?;
                    obj.insert("query".into(), json!(target));
                }
                if matches!(op, "snap") && !obj.contains_key("query") && !obj.contains_key("target")
                {
                    let target = step_target(step, outputs)?;
                    obj.insert("query".into(), json!(target));
                }
                if matches!(op, "multiQuery" | "batch") && !obj.contains_key("targets") {
                    let targets = step_targets(step, outputs)?;
                    obj.insert("targets".into(), json!(targets));
                }
            }
            Ok((idx, cloned))
        })
        .collect();
    let prepared = prepared?;

    for chunk in prepared.chunks(width) {
        if chunk.len() == 1 {
            let (_idx, step) = &chunk[0];
            let res = run_json_step(state, step, outputs)?;
            let id = required_step_id(step)?;
            outputs.insert(id.to_string(), res.value.clone());
            last = res;
            continue;
        }

        let snapshot = state.current_snapshot();
        let (cancellation, deadline) = state.control_handles();
        let limits = state.limits.clone();

        let outcomes: Vec<Result<ParallelStepOutcome, CodeModeError>> =
            std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(chunk.len());
                for (_idx, step) in chunk {
                    let cancellation = cancellation.clone();
                    let limits = limits.clone();
                    let step = step.clone();
                    handles.push(scope.spawn(move || {
                        note_parallel_enter();
                        // Optional stall so overlap tests can observe concurrency.
                        if let Ok(ms) = std::env::var("GRAPHZERO_PARALLEL_STALL_MS")
                            && let Ok(ms) = ms.parse::<u64>()
                            && ms > 0
                        {
                            std::thread::sleep(Duration::from_millis(ms));
                        }
                        let result = (|| {
                            if cancellation.is_cancelled() {
                                return Err(cancelled_error(
                                    "client cancelled during parallel JSON group",
                                ));
                            }
                            let mut local = ExecutionState::new_parallel(snapshot);
                            local.set_control(cancellation.clone(), deadline);
                            local.limits = limits;
                            let empty_outputs = BTreeMap::new();
                            let res = run_json_step(&mut local, &step, &empty_outputs)?;
                            let id = required_step_id(&step)?.to_string();
                            Ok(ParallelStepOutcome {
                                id,
                                result: res,
                                steps: local.steps,
                                logical_ops: local.logical_ops,
                                physical_ops: local.physical_ops,
                                batched_ops: local.batched_ops,
                                store_writes: local.store_writes,
                                bytes_materialized: local.bytes_materialized,
                            })
                        })();
                        if result.is_err() {
                            cancellation.cancel();
                        }
                        note_parallel_leave();
                        result
                    }));
                }
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err(cancelled_error("parallel worker panicked")))
                    })
                    .collect()
            });

        // Deterministic failure: earliest prepared index with Err wins.
        if let Some(err) = outcomes.iter().find_map(|o| o.as_ref().err()) {
            return Err(err.clone());
        }
        for outcome in outcomes {
            let outcome = outcome.expect("checked");
            state.logical_ops = state.logical_ops.saturating_add(outcome.logical_ops);
            state.physical_ops = state.physical_ops.saturating_add(outcome.physical_ops);
            state.batched_ops = state.batched_ops.saturating_add(outcome.batched_ops);
            state.store_writes = state.store_writes.saturating_add(outcome.store_writes);
            state.bytes_materialized = state
                .bytes_materialized
                .saturating_add(outcome.bytes_materialized);
            for r in &outcome.result.refs {
                state.push_ref(r.clone())?;
            }
            state.steps.extend(outcome.steps);
            outputs.insert(outcome.id, outcome.result.value.clone());
            last = outcome.result;
        }
        // Re-check parent quotas after merging a chunk.
        state.guard_ops("json.parallel")?;
    }
    Ok(last)
}

fn run_json_step(
    state: &mut ExecutionState<'_>,
    step: &Value,
    outputs: &BTreeMap<String, Value>,
) -> Result<BindingResult, CodeModeError> {
    let id = required_step_id(step)?;
    let op = step.get("op").and_then(Value::as_str).unwrap_or("query");
    if is_forbidden_mutating_op(op) {
        return Err(policy_error(
            format!("CodeMode is read-only; mutating JSON op '{op}' is unavailable"),
            id,
        ));
    }
    match op {
        "query" | "defs" | "callers" | "reading_set" | "readingSet" | "tests" => {
            let surface = step
                .get("surface")
                .and_then(Value::as_str)
                .unwrap_or(match op {
                    "defs" => "symbol",
                    "callers" => "callers",
                    "reading_set" | "readingSet" => "reading_set",
                    "tests" => "search",
                    _ => "symbol",
                });
            let target = step_target(step, outputs)?;
            let budget = step
                .get("budget")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            run_query_step_with_budget(state, id, surface, &target, budget)
        }
        "multiQuery" | "batch" => {
            let surface = step
                .get("surface")
                .and_then(Value::as_str)
                .unwrap_or("search");
            let targets = step_targets(step, outputs)?;
            run_multi_query_step(state, id, surface, &targets)
        }
        "blast" => {
            let target = step_target(step, outputs)?;
            let depth = step
                .get("depth")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            run_blast_step(state, id, &target, depth)
        }
        "orient" => {
            let surface = step
                .get("surface")
                .and_then(Value::as_str)
                .unwrap_or("context");
            let target = step
                .get("query")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(step_target(step, outputs)?);
            let budget = step
                .get("budget")
                .and_then(Value::as_u64)
                .map(|v| v as usize);
            run_query_step_with_budget(state, id, surface, &target, budget)
        }
        "recall" => {
            let target = step_target(step, outputs)?;
            run_query_step(state, id, "recall", &target)
        }
        "verify" => {
            let target = step_target(step, outputs)?;
            let claim = step
                .get("claim")
                .and_then(Value::as_str)
                .unwrap_or("no_remaining_callers");
            run_verify_step(state, id, &target, claim)
        }
        "snap" => {
            let target = step
                .get("query")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(step_target(step, outputs)?);
            let budget = step.get("budget").and_then(Value::as_u64).unwrap_or(1) as usize;
            run_snap_step(state, id, &target, budget)
        }
        "expand" => {
            let target = step_target(step, outputs)?;
            let budget = step.get("maxBytes").and_then(Value::as_u64).unwrap_or(0) as usize;
            run_expand_step(state, id, &target, budget)
        }
        "remember" => {
            let payload = step.get("value").cloned().unwrap_or_else(|| json!(step));
            run_remember_value_step(state, id, &payload)
        }
        "ref" => {
            let payload = step.get("value").cloned().unwrap_or_else(|| json!(outputs));
            run_ctx_ref_step(state, id, &payload)
        }
        other => Err(validation_error(
            format!("unknown JSON op {other}"),
            Some(id),
        )),
    }
}

// ── step scheduling ──

pub(crate) fn required_step_id(step: &Value) -> Result<&str, CodeModeError> {
    step.get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| validation_error("JSON DAG steps require stable id", Some("json.steps.id")))
}

pub(crate) fn schedule_json_steps(steps: &[Value]) -> Result<Vec<Vec<usize>>, CodeModeError> {
    if steps.len() > MAX_LOGICAL_OPS as usize {
        return Err(policy_error("JSON step limit exceeded", "json.steps"));
    }
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, step) in steps.iter().enumerate() {
        let id = required_step_id(step)?;
        if by_id.insert(id.to_string(), idx).is_some() {
            return Err(validation_error(
                format!("duplicate JSON step id {id}"),
                Some(id),
            ));
        }
    }
    let mut deps: Vec<BTreeSet<usize>> = Vec::with_capacity(steps.len());
    for step in steps {
        let id = required_step_id(step)?;
        let mut set = BTreeSet::new();
        for dep in step_needs(step)? {
            let Some(dep_idx) = by_id.get(&dep).copied() else {
                return Err(validation_error(
                    format!("missing JSON dependency '{dep}' for step '{id}'"),
                    Some(id),
                ));
            };
            set.insert(dep_idx);
        }
        deps.push(set);
    }
    let mut done = BTreeSet::new();
    let mut groups = Vec::new();
    while done.len() < steps.len() {
        let ready: Vec<usize> = (0..steps.len())
            .filter(|idx| !done.contains(idx) && deps[*idx].iter().all(|dep| done.contains(dep)))
            .collect();
        if ready.is_empty() {
            return Err(validation_error(
                "JSON dependency cycle detected",
                Some("json.needs"),
            ));
        }
        for idx in &ready {
            done.insert(*idx);
        }
        groups.push(ready);
    }
    Ok(groups)
}

pub(crate) fn step_needs(step: &Value) -> Result<Vec<String>, CodeModeError> {
    match step.get("needs") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    validation_error("needs entries must be step ids", Some("needs"))
                })
            })
            .collect(),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(_) => Err(validation_error(
            "needs must be a string or array",
            Some("needs"),
        )),
    }
}

pub(crate) fn is_forbidden_mutating_op(op: &str) -> bool {
    matches!(
        op,
        "write"
            | "delete"
            | "edit"
            | "patch"
            | "publish"
            | "reserve"
            | "claim"
            | "index"
            | "mutate"
    )
}

pub(crate) fn step_target(
    step: &Value,
    outputs: &BTreeMap<String, Value>,
) -> Result<String, CodeModeError> {
    let raw = step
        .get("target")
        .or_else(|| step.get("query"))
        .or_else(|| step.get("name"));
    match raw {
        Some(Value::String(s)) if s.starts_with('$') => resolve_binding_string(s, outputs),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) => Ok(v.to_string()),
        None => Err(validation_error(
            "step requires target/query/name",
            Some("target"),
        )),
    }
}

pub(crate) fn step_targets(
    step: &Value,
    outputs: &BTreeMap<String, Value>,
) -> Result<Vec<String>, CodeModeError> {
    let Some(raw) = step
        .get("targets")
        .or_else(|| step.get("queries"))
        .or_else(|| step.get("names"))
    else {
        return Err(validation_error(
            "batch step requires targets",
            Some("targets"),
        ));
    };
    match raw {
        Value::Array(items) => Ok(items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()),
        Value::String(s) if s.starts_with('$') => {
            let v = resolve_binding_value(s, outputs)?;
            Ok(v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default())
        }
        Value::String(s) => Ok(vec![s.clone()]),
        _ => Err(validation_error("targets must be strings", Some("targets"))),
    }
}

pub(crate) fn resolve_binding_string(
    s: &str,
    outputs: &BTreeMap<String, Value>,
) -> Result<String, CodeModeError> {
    let v = resolve_binding_value(s, outputs)?;
    Ok(v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string()))
}

pub(crate) fn resolve_binding_value<'a>(
    s: &str,
    outputs: &'a BTreeMap<String, Value>,
) -> Result<&'a Value, CodeModeError> {
    let key = s
        .trim_start_matches('$')
        .split('.')
        .next()
        .unwrap_or_default();
    outputs
        .get(key)
        .ok_or_else(|| validation_error(format!("unknown binding {s}"), Some("binding")))
}
