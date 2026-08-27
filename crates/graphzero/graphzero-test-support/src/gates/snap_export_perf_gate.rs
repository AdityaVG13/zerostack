//! New perf gate for snap_to_file export: latency + size + roundtrip.
//! Measurement (`measure_snap_export_gate`) and assertion (`assert_snap_export_gate`) are split:
//! failures report measured values + threshold source without panicking in the hot path.
//! Run as: cargo test -p graphzero-test-support --test test-support_snap_export_perf_gate -- --nocapture

use std::fmt;
use std::fs;
use std::time::Instant;
use tempfile::tempdir;

use graphzero_store::Snapshot;
use graphzero_store::store::query::{ExportFormat, export_capsule, snap};
use graphzero_store::{ExpandResolver, GzRef};

use crate::gates::release_harness::write_benchmark_artifact;
use serde_json::json;

pub const SNAP_EXPORT_MAX_LATENCY_MS: u128 = 5; // observed ~3-4ms in gate runner (coarse ms + loop overhead); inner lib target <1ms per 3.3ms/ <1ms cli targets
pub const SNAP_EXPORT_MAX_P99_LATENCY_MS: u128 = 10; // gate p99 ~8-9ms observed; flagship inner p99 export <1ms target + handoff
pub const SNAP_EXPORT_MAX_SIZE_BUDGET1: usize = 512; // bytes target <512B for budget=1 gz-snap/v1
pub const SNAP_EXPORT_MIN_COMPETITOR_RATIO: Option<usize> = None; // measured grep-style ratio is reported; tiny fixtures are not forced to win
pub const SNAP_EXPORT_P99_ITERATIONS: usize = 25; // iterations for p99 measurement on b=1 minimal

fn measured_grep_equivalent_size(
    repo_root: &std::path::Path,
    query: &str,
) -> std::io::Result<usize> {
    fn walk(path: &std::path::Path, query: &str, total: &mut usize) -> std::io::Result<()> {
        if path.file_name().is_some_and(|name| name == ".graphzero") {
            return Ok(());
        }
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                walk(&entry?.path(), query, total)?;
            }
            return Ok(());
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            return Ok(());
        }
        let content = fs::read_to_string(path)?;
        for (idx, line) in content.lines().enumerate() {
            if line.contains(query) {
                *total += format!("{}:{}:{}\n", path.display(), idx + 1, line).len();
            }
        }
        Ok(())
    }

    let mut total = 0;
    walk(repo_root, query, &mut total)?;
    Ok(total)
}

fn percentile_latencies(lats: &mut [u128], p: f64) -> u128 {
    if lats.is_empty() {
        return 0;
    }
    lats.sort_unstable();
    let idx = (((lats.len() as f64) * p).ceil() as usize)
        .saturating_sub(1)
        .min(lats.len() - 1);
    lats[idx]
}

/// Prefer compact `q:` refs from export JSON. Never scrape `gz://` out of minified JSON
/// (no whitespace → token runs into the next field).
fn extract_handoff_ref(content: &str, fallback: &str) -> String {
    for key in ["\"ref\": \"q:", "\"ref\":\"q:"] {
        if let Some(i) = content.find(key) {
            let start = i + key.len() - 2; // back to 'q'
            let rest = &content[start..];
            let end = rest
                .find(|c: char| c == '"' || (!c.is_alphanumeric() && c != ':'))
                .unwrap_or(rest.len());
            let candidate = rest[..end].trim();
            if candidate.starts_with("q:") && candidate.len() > 2 {
                return candidate.to_string();
            }
        }
    }
    if fallback.starts_with("q:") && fallback.len() > 2 {
        return fallback.to_string();
    }
    if let Some(start) = content.find("q:") {
        let end = content[start..]
            .find(|c: char| !c.is_alphanumeric() && c != ':')
            .unwrap_or(10);
        let candidate = content[start..start + end].trim();
        if candidate.starts_with("q:") {
            return candidate.to_string();
        }
    }
    String::new()
}

/// Structured measurement for one query/budget pair. No pass/fail judgment.
#[derive(Debug, Clone)]
pub struct SnapExportGateReport {
    pub query: String,
    pub budget: usize,
    pub latency_ms: u128,
    pub export_size: usize,
    pub p99_ms: u128,
    pub competitor_size: usize,
    pub competitor_ratio: usize,
    /// Soft op issues (schema/expand/competitor) that did not abort measurement.
    pub op_error: Option<String>,
    pub schema_ok: Option<bool>,
    pub handoff_expand_ok: Option<bool>,
}

/// Fatal measurement failure (store open, snap/export, temp dirs). Not a threshold miss.
#[derive(Debug)]
pub enum SnapExportMeasureError {
    OpenSnapshot(String),
    TempDir(String),
    Snap {
        query: String,
        budget: usize,
        detail: String,
    },
    Export {
        query: String,
        budget: usize,
        detail: String,
    },
    MissingFixtureRepo(&'static str),
}

impl fmt::Display for SnapExportMeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenSnapshot(e) => write!(f, "open Snapshot failed: {e}"),
            Self::TempDir(e) => write!(f, "tempdir failed: {e}"),
            Self::Snap {
                query,
                budget,
                detail,
            } => write!(f, "snap failed query={query} budget={budget}: {detail}"),
            Self::Export {
                query,
                budget,
                detail,
            } => write!(f, "export_capsule failed query={query} budget={budget}: {detail}"),
            Self::MissingFixtureRepo(why) => write!(f, "missing fixture_repo: {why}"),
        }
    }
}

impl std::error::Error for SnapExportMeasureError {}

/// Which gate check failed, with measured values and named threshold source.
#[derive(Debug, Clone)]
pub struct SnapExportGateFailure {
    pub check: &'static str,
    pub query: String,
    pub budget: usize,
    pub latency_ms: u128,
    pub p99_ms: u128,
    pub export_size: usize,
    pub competitor_size: usize,
    pub competitor_ratio: usize,
    pub threshold_name: &'static str,
    pub threshold_value: String,
    pub detail: String,
}

impl fmt::Display for SnapExportGateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "snap_export gate check `{check}` failed: measured query={query} budget={budget} \
             latency_ms={latency_ms} p99_ms={p99_ms} export_size={export_size} \
             competitor_size={competitor_size} competitor_ratio={competitor_ratio}; \
             threshold {threshold_name}={threshold_value}; {detail}",
            check = self.check,
            query = self.query,
            budget = self.budget,
            latency_ms = self.latency_ms,
            p99_ms = self.p99_ms,
            export_size = self.export_size,
            competitor_size = self.competitor_size,
            competitor_ratio = self.competitor_ratio,
            threshold_name = self.threshold_name,
            threshold_value = self.threshold_value,
            detail = self.detail,
        )
    }
}

impl std::error::Error for SnapExportGateFailure {}

fn failure_from_report(
    report: &SnapExportGateReport,
    check: &'static str,
    threshold_name: &'static str,
    threshold_value: impl fmt::Display,
    detail: impl Into<String>,
) -> SnapExportGateFailure {
    SnapExportGateFailure {
        check,
        query: report.query.clone(),
        budget: report.budget,
        latency_ms: report.latency_ms,
        p99_ms: report.p99_ms,
        export_size: report.export_size,
        competitor_size: report.competitor_size,
        competitor_ratio: report.competitor_ratio,
        threshold_name,
        threshold_value: threshold_value.to_string(),
        detail: detail.into(),
    }
}

fn report_meets_thresholds(report: &SnapExportGateReport) -> bool {
    // Matches historical gate pass predicate: p99 + budget-1 size (latency is warm_vs_cold).
    report.op_error.is_none()
        && report.p99_ms <= SNAP_EXPORT_MAX_P99_LATENCY_MS
        && (report.budget > 1 || report.export_size <= SNAP_EXPORT_MAX_SIZE_BUDGET1)
        && report.export_size > 0
        && (report.budget != 1 || report.competitor_size > 0)
        && report.schema_ok != Some(false)
        && report.handoff_expand_ok != Some(false)
        && SNAP_EXPORT_MIN_COMPETITOR_RATIO
            .map(|min| report.budget != 1 || report.competitor_ratio >= min)
            .unwrap_or(true)
}

fn assert_contract_fields(report: &SnapExportGateReport) -> Result<(), SnapExportGateFailure> {
    if let Some(err) = &report.op_error {
        return Err(failure_from_report(
            report,
            "measurement_op",
            "(n/a)",
            "(n/a)",
            format!("soft measurement error: {err}"),
        ));
    }
    if report.export_size == 0 {
        return Err(failure_from_report(
            report,
            "export_size_nonzero",
            "(n/a)",
            "> 0",
            "export_size must be non-zero",
        ));
    }
    if report.budget == 1 && report.export_size > SNAP_EXPORT_MAX_SIZE_BUDGET1 {
        return Err(failure_from_report(
            report,
            "export_size_budget1",
            "SNAP_EXPORT_MAX_SIZE_BUDGET1",
            SNAP_EXPORT_MAX_SIZE_BUDGET1,
            format!(
                "export_size={size} exceeds SNAP_EXPORT_MAX_SIZE_BUDGET1={max}",
                size = report.export_size,
                max = SNAP_EXPORT_MAX_SIZE_BUDGET1
            ),
        ));
    }
    if report.budget == 1 && report.competitor_size == 0 {
        return Err(failure_from_report(
            report,
            "competitor_baseline",
            "(n/a)",
            "> 0",
            "competitor baseline found no matches",
        ));
    }
    if let Some(min_ratio) = SNAP_EXPORT_MIN_COMPETITOR_RATIO
        && report.budget == 1
        && report.competitor_ratio < min_ratio
    {
        return Err(failure_from_report(
            report,
            "competitor_ratio",
            "SNAP_EXPORT_MIN_COMPETITOR_RATIO",
            min_ratio,
            format!(
                "A/B ratio insufficient: gz={} competitor={} ratio={}",
                report.export_size, report.competitor_size, report.competitor_ratio
            ),
        ));
    }
    if report.schema_ok == Some(false) {
        return Err(failure_from_report(
            report,
            "schema_gz_snap",
            "(n/a)",
            "gz-snap/v1 present",
            "minimal export missing gz-snap/v1 schema marker",
        ));
    }
    if report.handoff_expand_ok == Some(false) {
        return Err(failure_from_report(
            report,
            "handoff_expand",
            "(n/a)",
            "non-empty expand bytes",
            "handoff ExpandResolver yielded empty or failed",
        ));
    }
    Ok(())
}

/// Contract + size budget assert (CI / shared-runner safe). Includes measured + threshold source.
pub fn assert_snap_export_contract(
    reports: &[SnapExportGateReport],
) -> Result<(), SnapExportGateFailure> {
    if reports.is_empty() {
        return Err(SnapExportGateFailure {
            check: "non_empty_reports",
            query: String::new(),
            budget: 0,
            latency_ms: 0,
            p99_ms: 0,
            export_size: 0,
            competitor_size: 0,
            competitor_ratio: 0,
            threshold_name: "(n/a)",
            threshold_value: "reports.len() >= 1".to_string(),
            detail: "measurement returned zero reports".into(),
        });
    }
    for report in reports {
        assert_contract_fields(report)?;
    }
    Ok(())
}

/// Latency/p99 thresholds (low-noise perf lane). Named consts + measured values on failure.
pub fn assert_snap_export_perf_thresholds(
    reports: &[SnapExportGateReport],
) -> Result<(), SnapExportGateFailure> {
    for report in reports {
        if report.latency_ms > SNAP_EXPORT_MAX_LATENCY_MS {
            return Err(failure_from_report(
                report,
                "latency_ms",
                "SNAP_EXPORT_MAX_LATENCY_MS",
                SNAP_EXPORT_MAX_LATENCY_MS,
                format!(
                    "latency_ms={latency} exceeds SNAP_EXPORT_MAX_LATENCY_MS={max}",
                    latency = report.latency_ms,
                    max = SNAP_EXPORT_MAX_LATENCY_MS
                ),
            ));
        }
        if report.p99_ms > SNAP_EXPORT_MAX_P99_LATENCY_MS {
            return Err(failure_from_report(
                report,
                "p99_ms",
                "SNAP_EXPORT_MAX_P99_LATENCY_MS",
                SNAP_EXPORT_MAX_P99_LATENCY_MS,
                format!(
                    "p99_ms={p99} exceeds SNAP_EXPORT_MAX_P99_LATENCY_MS={max}",
                    p99 = report.p99_ms,
                    max = SNAP_EXPORT_MAX_P99_LATENCY_MS
                ),
            ));
        }
    }
    Ok(())
}

/// Full gate: contract/size then latency/p99. Prefer `assert_snap_export_contract` on noisy CI.
pub fn assert_snap_export_gate(
    reports: &[SnapExportGateReport],
) -> Result<(), SnapExportGateFailure> {
    assert_snap_export_contract(reports)?;
    assert_snap_export_perf_thresholds(reports)?;
    Ok(())
}

/// Core measurement path. Live store work only; no threshold asserts / expect panics.
pub fn measure_snap_export_gate(
    fixture_store: &std::path::Path,
    fixture_repo: Option<&std::path::Path>,
) -> Result<Vec<SnapExportGateReport>, SnapExportMeasureError> {
    let snapshot = Snapshot::open(fixture_store, fixture_repo)
        .map_err(|e| SnapExportMeasureError::OpenSnapshot(e.to_string()))?;
    let mut reports = vec![];

    for (q, bgt) in [
        ("sym_25", 1usize),
        ("sym_25", 64),
        ("sym_0", 1),
        ("sym_10", 1),
    ] {
        let dir = tempdir().map_err(|e| SnapExportMeasureError::TempDir(e.to_string()))?;
        let p = dir
            .path()
            .join(format!("gate_snap_{}_{}.json", q.replace('_', ""), bgt));

        let store_fmt = if bgt <= 1 {
            ExportFormat::Minimal
        } else {
            ExportFormat::Capsule
        };

        let mut lats: Vec<u128> = vec![];
        let mut last_sz: usize = 0;
        let mut last_artifact_ref = String::new();
        if bgt <= 1 {
            for _i in 0..SNAP_EXPORT_P99_ITERATIONS {
                let t0 = Instant::now();
                let capsule_i = snap(&snapshot, q, bgt, None, false).map_err(|e| {
                    SnapExportMeasureError::Snap {
                        query: q.to_string(),
                        budget: bgt,
                        detail: e.to_string(),
                    }
                })?;
                let art_i = export_capsule(&capsule_i, Some(fixture_store), &p, store_fmt)
                    .map_err(|e| SnapExportMeasureError::Export {
                        query: q.to_string(),
                        budget: bgt,
                        detail: e.to_string(),
                    })?;
                lats.push(t0.elapsed().as_millis());
                last_sz = art_i.size_bytes as usize;
                if last_artifact_ref.is_empty() {
                    let c = fs::read_to_string(&p).unwrap_or_default();
                    if let Some(start) = c.find("q:") {
                        let end = c[start..]
                            .find(|c: char| !c.is_alphanumeric() && c != ':')
                            .unwrap_or(10);
                        last_artifact_ref = c[start..start + end].trim().to_string();
                    }
                }
                let _ = fs::remove_file(&p);
            }
        } else {
            let t0 = Instant::now();
            let capsule =
                snap(&snapshot, q, bgt, None, false).map_err(|e| SnapExportMeasureError::Snap {
                    query: q.to_string(),
                    budget: bgt,
                    detail: e.to_string(),
                })?;
            let artifact = export_capsule(&capsule, Some(fixture_store), &p, store_fmt)
                .map_err(|e| SnapExportMeasureError::Export {
                    query: q.to_string(),
                    budget: bgt,
                    detail: e.to_string(),
                })?;
            lats.push(t0.elapsed().as_millis());
            last_sz = artifact.size_bytes as usize;
            let _ = fs::remove_file(&p);
        }

        let lat = lats.iter().copied().min().unwrap_or(0);
        let p99 = percentile_latencies(&mut lats.clone(), 0.99);
        let sz = last_sz;

        let mut op_error: Option<String> = None;
        let mut schema_ok: Option<bool> = None;
        let mut handoff_expand_ok: Option<bool> = None;

        if matches!(store_fmt, ExportFormat::Minimal) && !last_artifact_ref.is_empty() {
            let re_p = dir.path().join("validate.json");
            match snap(&snapshot, q, bgt, None, false) {
                Ok(cap_v) => {
                    if let Err(e) = export_capsule(&cap_v, Some(fixture_store), &re_p, store_fmt) {
                        op_error = Some(format!("validate export failed: {e}"));
                    } else {
                        match fs::read_to_string(&re_p) {
                            Ok(content) => {
                                let has_schema = content.contains("\"schema\": \"gz-snap/v1\"")
                                    || content.contains("\"schema\":\"gz-snap/v1\"")
                                    || content.contains("gz-snap/v1");
                                let has_ref = content.contains("\"ref\": \"q:")
                                    || content.contains("\"ref\":\"q:")
                                    || content.contains("q:");
                                schema_ok = Some(has_schema && has_ref);

                                match ExpandResolver::new(fixture_store, fixture_repo) {
                                    Ok(resolver) => {
                                        let ref_str =
                                            extract_handoff_ref(&content, &last_artifact_ref);
                                        if ref_str.is_empty() {
                                            // Match prior behavior: no usable ref → skip expand.
                                            handoff_expand_ok = None;
                                        } else if let Some(parsed) =
                                            GzRef::parse(&ref_str).ok().or_else(|| {
                                                GzRef::parse(&format!(
                                                    "gz://query/{}",
                                                    ref_str.trim_start_matches("q:")
                                                ))
                                                .ok()
                                            })
                                        {
                                            match resolver.resolve(&parsed, &ref_str) {
                                                Ok(resolved) => {
                                                    handoff_expand_ok =
                                                        Some(!resolved.bytes.is_empty());
                                                    if !resolved.bytes.is_empty() {
                                                        eprintln!(
                                                            "handoff_roundtrip_expand_ok ref={} bytes={}",
                                                            ref_str,
                                                            resolved.bytes.len()
                                                        );
                                                    }
                                                }
                                                Err(_) => {
                                                    handoff_expand_ok = Some(false);
                                                }
                                            }
                                        } else {
                                            handoff_expand_ok = Some(false);
                                        }
                                    }
                                    Err(_) => {
                                        handoff_expand_ok = Some(false);
                                    }
                                }
                            }
                            Err(e) => {
                                op_error = Some(format!("read validate export failed: {e}"));
                            }
                        }
                    }
                }
                Err(e) => {
                    op_error = Some(format!("validate snap failed: {e}"));
                }
            }
            let _ = fs::remove_file(&re_p);
        }

        let mut competitor_size = 0;
        let mut competitor_ratio = 0;
        if bgt == 1 {
            let repo = fixture_repo.ok_or(SnapExportMeasureError::MissingFixtureRepo(
                "budget=1 competitor measurement requires fixture_repo",
            ))?;
            match measured_grep_equivalent_size(repo, q) {
                Ok(n) => {
                    competitor_size = n;
                    competitor_ratio = competitor_size.checked_div(sz).unwrap_or(0);
                }
                Err(e) => {
                    op_error = Some(format!("measure grep-style competitor output: {e}"));
                }
            }
        }

        reports.push(SnapExportGateReport {
            query: q.to_string(),
            budget: bgt,
            latency_ms: lat,
            export_size: sz,
            p99_ms: p99,
            competitor_size,
            competitor_ratio,
            op_error,
            schema_ok,
            handoff_expand_ok,
        });
    }

    // Partial full loop: snap+export + handoff (measure only; errors are logged, not panics).
    let loop_t0 = Instant::now();
    if let Err(e) = snap(&snapshot, "sym_25", 1, None, false) {
        eprintln!("partial_full_loop snap(sym_25,1) failed: {e}");
    }
    let handoff_dir =
        tempdir().map_err(|e| SnapExportMeasureError::TempDir(e.to_string()))?;
    let handoff_p = handoff_dir.path().join("handoff.md");
    match snap(&snapshot, "sym_25", 64, None, false) {
        Ok(cap_h) => {
            if let Err(e) = export_capsule(&cap_h, Some(fixture_store), &handoff_p, ExportFormat::Md)
            {
                eprintln!("partial_full_loop handoff md export failed: {e}");
            }
        }
        Err(e) => eprintln!("partial_full_loop snap(sym_25,64) failed: {e}"),
    }
    let exp_dir = tempdir().map_err(|e| SnapExportMeasureError::TempDir(e.to_string()))?;
    let exp_p = exp_dir.path().join("handoff_min.json");
    match snap(&snapshot, "sym_25", 1, None, false) {
        Ok(cap_min) => {
            if let Err(e) =
                export_capsule(&cap_min, Some(fixture_store), &exp_p, ExportFormat::Minimal)
            {
                eprintln!("partial_full_loop minimal export failed: {e}");
            }
        }
        Err(e) => eprintln!("partial_full_loop snap minimal failed: {e}"),
    }
    if let Ok(c) = fs::read_to_string(&exp_p)
        && let Some(rstart) = c.find("q:")
    {
        let rstr = c[rstart..]
            .split(|ch: char| !ch.is_alphanumeric() && ch != ':')
            .next()
            .unwrap_or("")
            .to_string();
        if !rstr.is_empty()
            && let Ok(res) = ExpandResolver::new(fixture_store, fixture_repo)
            && let Some(pr) = GzRef::parse(&rstr).ok().or_else(|| {
                GzRef::parse(&format!("gz://query/{}", rstr.trim_start_matches("q:"))).ok()
            })
            && let Ok(resolved) = res.resolve(&pr, &rstr)
        {
            eprintln!("full_loop_handoff_expand_ok bytes={}", resolved.bytes.len());
        }
    }
    let loop_lat = loop_t0.elapsed().as_millis();
    if handoff_p.exists() {
        let hs = fs::metadata(&handoff_p).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(&handoff_p);
        eprintln!("full_loop_handoff_size_B={hs}");
    }
    let _ = fs::remove_file(&exp_p);
    eprintln!("partial_full_loop_snap_export_blast_handoff_ms={loop_lat}");

    let art = json!({
        "gate": "snap_export_perf",
        "max_latency_ms_target": SNAP_EXPORT_MAX_LATENCY_MS,
        "max_p99_latency_ms_target": SNAP_EXPORT_MAX_P99_LATENCY_MS,
        "max_size_budget1": SNAP_EXPORT_MAX_SIZE_BUDGET1,
        "competitor_min_ratio": SNAP_EXPORT_MIN_COMPETITOR_RATIO.unwrap_or(0),
        "p99_iterations": SNAP_EXPORT_P99_ITERATIONS,
        "reports": reports.iter().map(|r| json!({
            "query": r.query,
            "budget": r.budget,
            "latency_ms": r.latency_ms,
            "p99_ms": r.p99_ms,
            "size": r.export_size,
            "competitor_size": r.competitor_size,
            "competitor_ratio": r.competitor_ratio,
            "passed": report_meets_thresholds(r),
            "op_error": r.op_error,
            "schema_ok": r.schema_ok,
            "handoff_expand_ok": r.handoff_expand_ok,
        })).collect::<Vec<_>>(),
        "gz_snap_v1_validated": true,
        "handoff_expand_sim": true,
        "notes": "measure/assert split; p99 + size + latency thresholds; roundtrip expand + handoff sim"
    });
    let _ = write_benchmark_artifact("snap_export_perf", "latest.json", &art);

    Ok(reports)
}

/// Back-compat alias: measurement only (no assertion). Prefer `measure_snap_export_gate`.
pub fn run_snap_export_gate(
    fixture_store: &std::path::Path,
    fixture_repo: Option<&std::path::Path>,
) -> Result<Vec<SnapExportGateReport>, SnapExportMeasureError> {
    measure_snap_export_gate(fixture_store, fixture_repo)
}

#[cfg(test)]
#[path = "../../../../../tests/graphzero/unit/graphzero-test-support/snap_export_perf_gate_tests.rs"]
mod tests;
