//! Opt-in stage latency histograms (`GRAPHZERO_STAGE_HISTOGRAM=1`). Records `stage_ms` samples
//! into named HDR histograms so multi-run p50/p95/p99 tails are available without a metrics
//! exporter. Default off: zero Instant or histogram work on the hot path when the env is unset.

use std::collections::BTreeMap;
use std::sync::Mutex;

use hdrhistogram::Histogram;
use serde::Serialize;

/// Env flag enabling stage_ms histogram recording.
pub const STAGE_HISTOGRAM_ENV: &str = "GRAPHZERO_STAGE_HISTOGRAM";

/// Significant figures for HDR recording (trade memory for precision).
const SIG_FIGS: u8 = 3;
/// Max stage value in microseconds (~1 hour). Values above are clamped.
const MAX_VALUE_US: u64 = 3_600_000_000;

static HISTS: Mutex<Option<BTreeMap<String, Histogram<u64>>>> = Mutex::new(None);

/// True when `GRAPHZERO_STAGE_HISTOGRAM` is set.
pub fn stage_histogram_enabled() -> bool {
    std::env::var_os(STAGE_HISTOGRAM_ENV).is_some()
}

/// Record one stage latency sample in milliseconds.
/// No-op when the env flag is off. Clamps non-finite / negative values to 0.
pub fn record_stage_ms(stage: &str, ms: f64) {
    if !stage_histogram_enabled() && !crate::store::perf_profile::perf_profile_enabled() {
        return;
    }
    crate::store::perf_profile::perf_profile_sample_collected(stage, ms);
    if !stage_histogram_enabled() {
        return;
    }
    let us = ms_to_us(ms);
    let mut guard = HISTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(BTreeMap::new);
    let hist = map.entry(stage.to_string()).or_insert_with(|| {
        Histogram::new_with_max(MAX_VALUE_US, SIG_FIGS).expect("valid HDR max/sigfigs")
    });
    let _ = hist.record(us.min(MAX_VALUE_US));
}

/// Record every non-zero field of a flat `*_ms` map under a stage prefix.
/// Keys become `{prefix}.{field}` (e.g. `index.walk_ms`).
pub fn record_phase_map(prefix: &str, fields: &[(&str, f64)]) {
    if !stage_histogram_enabled() && !crate::store::perf_profile::perf_profile_enabled() {
        return;
    }
    for (name, ms) in fields {
        // Always record total/count stages even when zero so sample counts align.
        if *ms > 0.0 || name.ends_with("total_ms") || *name == "total_ms" {
            record_stage_ms(&format!("{prefix}.{name}"), *ms);
        }
    }
}

fn ms_to_us(ms: f64) -> u64 {
    if !ms.is_finite() || ms <= 0.0 {
        return 0;
    }
    (ms * 1000.0).round() as u64
}

/// One stage's multi-run percentile summary (milliseconds).
#[derive(Clone, Debug, Serialize)]
pub struct StageHistSummary {
    pub stage: String,
    pub count: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

fn us_to_ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

fn summary_from(stage: &str, hist: &Histogram<u64>) -> StageHistSummary {
    let count = hist.len();
    if count == 0 {
        return StageHistSummary {
            stage: stage.to_string(),
            count: 0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
        };
    }
    StageHistSummary {
        stage: stage.to_string(),
        count,
        p50_ms: us_to_ms(hist.value_at_quantile(0.50)),
        p95_ms: us_to_ms(hist.value_at_quantile(0.95)),
        p99_ms: us_to_ms(hist.value_at_quantile(0.99)),
        max_ms: us_to_ms(hist.max()),
        mean_ms: hist.mean() / 1000.0,
    }
}

/// Snapshot all recorded stage histograms as percentile summaries.
pub fn stage_hist_snapshot() -> Vec<StageHistSummary> {
    let guard = HISTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = guard.as_ref() else {
        return Vec::new();
    };
    map.iter()
        .map(|(stage, hist)| summary_from(stage, hist))
        .collect()
}

/// Clear all histograms (tests / fresh runs).
pub fn reset_stage_histograms() {
    let mut guard = HISTS.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// Record [`super::indexer::IndexPhaseTimings`] under the `index` prefix.
pub fn record_index_phases(t: &super::indexer::IndexPhaseTimings) {
    let fields = [
        ("walk_ms", t.walk_ms),
        ("extract_ms", t.extract_ms),
        ("blob_put_ms", t.blob_put_ms),
        ("blob_sync_ms", t.blob_sync_ms),
        ("scan_ms", t.scan_ms),
        ("assemble_ms", t.assemble_ms),
        ("history_ms", t.history_ms),
        ("sidecar_ms", t.sidecar_ms),
        ("write_snapshot_ms", t.write_snapshot_ms),
        ("manifest_publish_ms", t.manifest_publish_ms),
        ("fingerprint_save_ms", t.fingerprint_save_ms),
        ("total_ms", t.total_ms),
    ];
    record_phase_map("index", &fields);
    crate::store::perf_profile::perf_profile_span_summary(
        "index",
        &fields,
        serde_json::json!({
            "warm_shortcircuit": t.warm_shortcircuit,
            "file_count": t.file_count,
            "blob_fsync_count": t.blob_fsync_count,
        }),
    );
}

/// Record [`super::query::OpenPhaseTimings`] under the `open` prefix.
pub fn record_open_phases(t: &super::query::OpenPhaseTimings) {
    let fields = [
        ("compact_ms", t.compact_ms),
        ("manifest_ms", t.manifest_ms),
        ("shard_open_ms", t.shard_open_ms),
        ("wal_merge_ms", t.wal_merge_ms),
        ("paths_ms", t.paths_ms),
        ("hydrate_ms", t.hydrate_ms),
        ("total_ms", t.total_ms),
    ];
    record_phase_map("open", &fields);
    crate::store::perf_profile::perf_profile_span_summary(
        "open",
        &fields,
        serde_json::json!({ "cache_hit": t.cache_hit }),
    );
}

/// Record dispatch-level stage map under the `dispatch` prefix (query crate).
pub fn record_dispatch_phases(fields: &[(&str, f64)]) {
    record_phase_map("dispatch", fields);
    crate::store::perf_profile::perf_profile_span_summary(
        "dispatch",
        fields,
        serde_json::json!({}),
    );
}

/// Record op-local stages under an op prefix (e.g. `blast`, `query`).
pub fn record_op_stages(op: &str, fields: &[(&str, f64)]) {
    record_phase_map(op, fields);
    crate::store::perf_profile::perf_profile_span_summary(op, fields, serde_json::json!({}));
}
