//! Dispatcher-only cost profiling for benchmark subtraction and env-gated stage
//! timings on production dispatch paths (graphzero-xgj4i).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::DispatchOutcome;
use super::context::{AdapterKind, EngineContext};
use super::execute::dispatch;
use crate::operation_abi::DomainResult;

/// Wall-clock cost of a single domain dispatch (excludes transport framing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchProfile {
    pub op: String,
    pub canonical_op: Option<String>,
    pub adapter: AdapterKind,
    pub wall_ns: u128,
    pub ok: bool,
}

/// Run [`dispatch`] and record dispatcher-only wall time.
pub fn dispatch_profiled(
    ctx: &EngineContext,
    op: &str,
    args: &Value,
) -> (DispatchOutcome, DispatchProfile) {
    let start = Instant::now();
    let outcome = dispatch(ctx, op, args);
    let wall_ns = start.elapsed().as_nanos();
    let canonical_op = match &outcome {
        Ok(r) => Some(r.op.clone()),
        Err(e) => e.op.clone(),
    };
    let profile = DispatchProfile {
        op: op.to_string(),
        canonical_op,
        adapter: ctx.adapter,
        wall_ns,
        ok: outcome.is_ok(),
    };
    (outcome, profile)
}

/// Host-timed dispatch stage breakdown (env `GRAPHZERO_DISPATCH_PHASE_TIMING=1`).
///
/// When set: resolve / preflight / execute walls are recorded for every domain
/// dispatch. Callers may drain via [`take_dispatch_phase_timings`]. Successful
/// results also receive `telemetry.phases` with the same fields (merged with any
/// op-local stage keys such as `open_ms`). When off: no Instant clocks.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct DispatchPhaseTimings {
    pub op: String,
    pub resolve_ms: f64,
    pub preflight_ms: f64,
    pub execute_ms: f64,
    pub total_ms: f64,
}

thread_local! {
    static DISPATCH_PHASE_TIMINGS: RefCell<Option<DispatchPhaseTimings>> =
        const { RefCell::new(None) };
}

/// True when `GRAPHZERO_DISPATCH_PHASE_TIMING` is set.
/// Also true when `GRAPHZERO_STAGE_HISTOGRAM` is set so samples feed the HDR sink.
/// Checked each call so tests can enable without process-once OnceLock stickiness.
pub fn dispatch_phase_timing_enabled() -> bool {
    std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some()
        || graphzero_store::stage_histogram_enabled()
        || graphzero_store::perf_profile_enabled()
}

pub(crate) fn dispatch_phase_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub(crate) fn dispatch_phase_begin(op: &str) {
    if !dispatch_phase_timing_enabled() {
        return;
    }
    DISPATCH_PHASE_TIMINGS.with(|slot| {
        *slot.borrow_mut() = Some(DispatchPhaseTimings {
            op: op.to_string(),
            ..DispatchPhaseTimings::default()
        });
    });
}

pub(crate) fn dispatch_phase_add(mut f: impl FnMut(&mut DispatchPhaseTimings)) {
    if !dispatch_phase_timing_enabled() {
        return;
    }
    DISPATCH_PHASE_TIMINGS.with(|slot| {
        if let Some(t) = slot.borrow_mut().as_mut() {
            f(t);
        }
    });
}

/// Take timings recorded by the last `dispatch` under phase timing.
pub fn take_dispatch_phase_timings() -> Option<DispatchPhaseTimings> {
    DISPATCH_PHASE_TIMINGS.with(|slot| slot.borrow_mut().take())
}

/// Peek (clone) current timings without clearing -- used to attach to telemetry
/// while still allowing [`take_dispatch_phase_timings`] for external consumers.
pub(crate) fn peek_dispatch_phase_timings() -> Option<DispatchPhaseTimings> {
    DISPATCH_PHASE_TIMINGS.with(|slot| slot.borrow().clone())
}

/// Merge dispatch-level phase fields into `telemetry.phases` on a successful result.
/// Op handlers may already have written stage keys (`open_ms`, …) under `phases`.
pub(crate) fn attach_dispatch_phases(mut outcome: DispatchOutcome) -> DispatchOutcome {
    if !dispatch_phase_timing_enabled() {
        return outcome;
    }
    let Some(phases) = peek_dispatch_phase_timings() else {
        return outcome;
    };
    // Opt-in HDR stage_ms sink (graphzero-rzu6s).
    graphzero_store::record_dispatch_phases(&[
        ("resolve_ms", phases.resolve_ms),
        ("preflight_ms", phases.preflight_ms),
        ("execute_ms", phases.execute_ms),
        ("total_ms", phases.total_ms),
    ]);
    // Only eprint when the explicit dispatch env is set (not histogram-only).
    if std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some() {
        if let Ok(phases_val) = serde_json::to_value(&phases) {
            eprintln!(
                "graphzero_dispatch_phase_timing {}",
                serde_json::to_string(&phases_val).unwrap_or_else(|_| "{}".into())
            );
        }
    }
    if let Ok(result) = outcome.as_mut() {
        // Attach telemetry.phases only when dispatch env is set; histogram-only
        // should not change result shape.
        if std::env::var_os("GRAPHZERO_DISPATCH_PHASE_TIMING").is_some() {
            merge_phases_into_telemetry(result, &phases);
        }
    }
    outcome
}

fn merge_phases_into_telemetry(result: &mut DomainResult, phases: &DispatchPhaseTimings) {
    let mut tel = result.telemetry.take().unwrap_or_else(|| json!({}));
    let mut phase_obj = match tel.get("phases").cloned() {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };
    if let Some(obj) = phase_obj.as_object_mut() {
        obj.insert("op".into(), json!(phases.op));
        obj.insert("resolve_ms".into(), json!(phases.resolve_ms));
        obj.insert("preflight_ms".into(), json!(phases.preflight_ms));
        obj.insert("execute_ms".into(), json!(phases.execute_ms));
        obj.insert("total_ms".into(), json!(phases.total_ms));
    }
    if let Some(obj) = tel.as_object_mut() {
        obj.insert("phases".into(), phase_obj);
    } else {
        tel = json!({ "phases": phase_obj });
    }
    result.telemetry = Some(tel);
}
