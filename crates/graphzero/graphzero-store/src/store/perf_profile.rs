//! Structured `perf.profile.*` log contract (graphzero-jjvkx).
//!
//! Schema: `graphzero.perf.profile.v1`
//! Events (skill INSTRUMENTATION names):
//! - `perf.profile.run_start`
//! - `perf.profile.sample_collected`
//! - `perf.profile.span_summary`
//! - `perf.profile.hypothesis_evaluated`
//! - `perf.profile.run_complete`
//!
//! Default off. Enable with `GRAPHZERO_PERF_PROFILE=1` (or any non-empty value).
//! Lines are eprinted as single-line JSON for skill tooling to ingest.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

/// Env flag for structured perf.profile logs.
pub const PERF_PROFILE_ENV: &str = "GRAPHZERO_PERF_PROFILE";

/// Schema id stamped on every event line.
pub const PERF_PROFILE_SCHEMA: &str = "graphzero.perf.profile.v1";

static RUN_ID: AtomicU64 = AtomicU64::new(0);
static RUN_ACTIVE: AtomicBool = AtomicBool::new(false);
static SAMPLE_SEQ: AtomicU64 = AtomicU64::new(0);

/// True when `GRAPHZERO_PERF_PROFILE` is set.
pub fn perf_profile_enabled() -> bool {
    std::env::var_os(PERF_PROFILE_ENV).is_some()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn emit(event: &str, fields: Value) {
    if !perf_profile_enabled() {
        return;
    }
    let mut line = json!({
        "schema": PERF_PROFILE_SCHEMA,
        "event": event,
        "ts_ms": now_ms(),
        "run_id": RUN_ID.load(Ordering::Relaxed),
    });
    if let (Some(obj), Some(extra)) = (line.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    eprintln!(
        "{}",
        serde_json::to_string(&line).unwrap_or_else(|_| "{}".into())
    );
}

/// Begin a profiled run. Emits `perf.profile.run_start`.
///
/// Idempotent while a run is already active (no second run_start).
pub fn perf_profile_run_start(label: &str, meta: Value) {
    if !perf_profile_enabled() {
        return;
    }
    if RUN_ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    let id = RUN_ID.fetch_add(1, Ordering::SeqCst) + 1;
    RUN_ID.store(id, Ordering::SeqCst);
    SAMPLE_SEQ.store(0, Ordering::SeqCst);
    let mut fields = json!({ "label": label });
    if let (Some(obj), Some(extra)) = (fields.as_object_mut(), meta.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    emit("perf.profile.run_start", fields);
}

/// Ensure a run is active (starts one with `auto` label if needed).
pub fn perf_profile_ensure_run(label: &str) {
    if !perf_profile_enabled() {
        return;
    }
    if !RUN_ACTIVE.load(Ordering::SeqCst) {
        perf_profile_run_start(label, json!({}));
    }
}

/// Emit `perf.profile.sample_collected` for one stage sample.
pub fn perf_profile_sample_collected(stage: &str, ms: f64) {
    if !perf_profile_enabled() {
        return;
    }
    perf_profile_ensure_run("auto");
    let seq = SAMPLE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    emit(
        "perf.profile.sample_collected",
        json!({
            "stage": stage,
            "stage_ms": ms,
            "sample_seq": seq,
        }),
    );
}

/// Emit `perf.profile.span_summary` for a set of stage walls from one op.
pub fn perf_profile_span_summary(span: &str, stages: &[(&str, f64)], extra: Value) {
    if !perf_profile_enabled() {
        return;
    }
    perf_profile_ensure_run(span);
    let mut stage_map = serde_json::Map::new();
    let mut total = 0.0_f64;
    for (name, ms) in stages {
        stage_map.insert((*name).to_string(), json!(ms));
        total += *ms;
    }
    let mut fields = json!({
        "span": span,
        "stages": stage_map,
        "accounted_ms": total,
    });
    if let (Some(obj), Some(ex)) = (fields.as_object_mut(), extra.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    emit("perf.profile.span_summary", fields);
}

/// Emit `perf.profile.hypothesis_evaluated` (Amdahl / skill hypothesis ledger).
pub fn perf_profile_hypothesis_evaluated(hypothesis_id: &str, accepted: bool, detail: Value) {
    if !perf_profile_enabled() {
        return;
    }
    perf_profile_ensure_run("hypothesis");
    let mut fields = json!({
        "hypothesis_id": hypothesis_id,
        "accepted": accepted,
    });
    if let (Some(obj), Some(ex)) = (fields.as_object_mut(), detail.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    emit("perf.profile.hypothesis_evaluated", fields);
}

/// Emit `perf.profile.run_complete` and clear the active run flag.
pub fn perf_profile_run_complete(summary: Value) {
    if !perf_profile_enabled() {
        return;
    }
    if !RUN_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    let samples = SAMPLE_SEQ.load(Ordering::SeqCst);
    let mut fields = json!({ "samples": samples });
    if let (Some(obj), Some(ex)) = (fields.as_object_mut(), summary.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    emit("perf.profile.run_complete", fields);
    RUN_ACTIVE.store(false, Ordering::SeqCst);
}

/// Force-reset run state (tests).
pub fn reset_perf_profile_for_tests() {
    RUN_ACTIVE.store(false, Ordering::SeqCst);
    SAMPLE_SEQ.store(0, Ordering::SeqCst);
}

/// One event envelope (for tests / offline parsers).
#[derive(Clone, Debug, Serialize)]
pub struct PerfProfileEvent {
    pub schema: &'static str,
    pub event: String,
    pub run_id: u64,
    pub fields: Value,
}
