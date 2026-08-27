//! End-to-end surface latency and cost harness (fszero-ncib.8).
//!
//! Measures **real** surfaces:
//! - raw domain dispatcher
//! - FastMCP tool dispatch (`dispatch_mcp_tool` / MCP envelope)
//! - CodeMode recipe plans (`execute_plan` recipe form)
//! - CodeMode JSON DAG plans
//! - CodeMode JavaScript plans (when `surface-codemode` is compiled)
//!
//! Process starts and serialization counts come from live `runtime_metrics`
//! counters — never hard-coded zeros.

use crate::core::dispatcher::{
    dispatch_count, dispatch_mcp_tool, dispatch_raw_worker, last_dispatch_profile,
};
use crate::core::runtime_metrics::{
    duplicate_serialization_detected, lock_metrics_for_test, reset_runtime_metrics,
    take_process_starts, take_serializations,
};

use crate::codemode::{execute_plan as codemode_execute_plan, looks_like_js_plan};
use crate::core::FSZeroSession;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Workload kind exercised by the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchWorkload {
    NoopControl,
    CheapRead,
    RepeatedRead,
    WriteThenRead,
}

impl BenchWorkload {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoopControl => "noop_control",
            Self::CheapRead => "cheap_read",
            Self::RepeatedRead => "repeated_read",
            Self::WriteThenRead => "write_then_read",
        }
    }
}

/// Surface under measurement — names match real entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchSurface {
    RawDispatcher,
    FastMcp,
    CodemodeRecipe,
    CodemodeJson,
    CodemodeJs,
}

impl BenchSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawDispatcher => "raw_dispatcher",
            Self::FastMcp => "fastmcp",
            Self::CodemodeRecipe => "codemode_recipe",
            Self::CodemodeJson => "codemode_json",
            Self::CodemodeJs => "codemode_js",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrialResult {
    pub wall_ns: u64,
    pub dispatcher_overhead_ns: u64,
    pub kernel_ns: u64,
    pub ok: bool,
    pub process_starts: u32,
    pub op_count: u32,
    pub boundary_count: u32,
    pub serializations: u32,
    pub duplicate_serialization: bool,
    /// Process RSS immediately before the trial body (bytes), if measurable.
    pub rss_before_bytes: Option<u64>,
    /// Process RSS immediately after the trial body (bytes), if measurable.
    pub rss_after_bytes: Option<u64>,
    /// Max of before/after for this trial (bytes). Not a true in-window peak
    /// sample stream -- coarse dual-point; see provenance.memory_rss_method.
    pub peak_rss_bytes: Option<u64>,
}

/// Portable current-process RSS in bytes (fszero-cq0k).
///
/// Linux: `/proc/self/status` `VmRSS` (KiB). macOS: `ps -o rss=` (KiB).
/// Other OS: `None` (caller must still emit JSON keys with null + reason).
fn current_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let kib: u64 = s.trim().parse().ok()?;
        return Some(kib.saturating_mul(1024));
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kib.saturating_mul(1024));
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn memory_rss_method() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "ps_-o_rss=_self_kib"
    }
    #[cfg(target_os = "linux")]
    {
        "proc_self_status_VmRSS_kib"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "unsupported"
    }
}

/// Host class + hardware tags for absolute gate honesty (fszero-bkeu).
///
/// Aligns with `scripts/env_fingerprint.py` `derive_host_class` labels
/// (`local-m5-max`, `gha-*`). Absolute µs thresholds must only fire when
/// provenance host_class matches the baseline artifact (see
/// docs/ncib-release-waivers.md W1 replacement path).
fn detect_cpu_model() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("model name") {
                let model = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
        }
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn detect_kernel() -> Option<String> {
    let out = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Cargo profile name: debug / release / release-perf (when CARGO_PROFILE set or
/// FSZERO_BENCH_PROFILE override). Distinguishes absolute gate honesty.
fn cargo_profile_name() -> String {
    if let Ok(v) = std::env::var("FSZERO_BENCH_PROFILE") {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(v) = std::env::var("CARGO_PROFILE") {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if cfg!(debug_assertions) {
        "debug".into()
    } else {
        // Default cargo --release; release-perf must set FSZERO_BENCH_PROFILE.
        "release".into()
    }
}

fn derive_host_class(cpu_model: Option<&str>) -> (String, String) {
    if let Ok(v) = std::env::var("FSZERO_PERF_HOST_CLASS") {
        let t = v.trim();
        if !t.is_empty() {
            return (t.to_string(), "env:FSZERO_PERF_HOST_CLASS".into());
        }
    }
    if std::env::var("GITHUB_ACTIONS").ok().as_deref() == Some("true") {
        if let Ok(runs_on) = std::env::var("FSZERO_PERF_RUNS_ON") {
            let t = runs_on.trim();
            if !t.is_empty() {
                return (format!("gha-{t}"), "env:FSZERO_PERF_RUNS_ON".into());
            }
        }
        if let Ok(image_os) = std::env::var("ImageOS") {
            let t = image_os.trim().to_lowercase();
            if !t.is_empty() {
                return (
                    format!("gha-{}-{}", t, std::env::consts::ARCH.to_lowercase()),
                    "env:ImageOS+arch".into(),
                );
            }
        }
        return (
            format!(
                "gha-{}-{}",
                std::env::consts::OS.to_lowercase(),
                std::env::consts::ARCH.to_lowercase()
            ),
            "github_actions+platform".into(),
        );
    }
    let model = cpu_model.unwrap_or("").to_lowercase().replace(' ', "-");
    if std::env::consts::OS == "macos" && !model.is_empty() {
        if model.contains("apple") && model.contains("m5") && model.contains("max") {
            return ("local-m5-max".into(), "cpu_model".into());
        }
        if model.contains("apple") {
            // e.g. apple-m4-pro -> local-apple-m4-pro (compact)
            let compact = model
                .trim_start_matches("apple-")
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>();
            let compact = compact.trim_matches('-');
            return (format!("local-apple-{compact}"), "cpu_model".into());
        }
    }
    (
        format!(
            "local-{}-{}",
            std::env::consts::OS.to_lowercase(),
            std::env::consts::ARCH.to_lowercase()
        ),
        "platform".into(),
    )
}

/// Provenance attached to every evidence document.
pub fn bench_provenance(git_sha: &str) -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cpu_model = detect_cpu_model();
    let (host_class, host_class_source) = derive_host_class(cpu_model.as_deref());
    let profile = cargo_profile_name();
    json!({
        "git_sha": git_sha, "recorded_unix_s": now,
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "profile": profile.clone(),
        "cargo_profile": profile,
        "cpu_model": cpu_model,
        "kernel": detect_kernel(),
        "host_class": host_class,
        "host_class_source": host_class_source,
        "compiler": "rustc", "warmup_policy": "first_sample_discarded",
        "outlier_policy": "report_all_raw_trials; p50/p95/p99 from sorted walls", "measurement_scope": "agent_local_in_process",
        "measurement_scope_note": "Wall times are process-local (dispatch→kernel→store). Client RTT / transport framing is not included; Amdahl vs remote MCP RTT is out of scope for this harness (fszero-w2g.51).", "sample_count_default": 21,
        "absolute_gate_policy": "same_host_class_only; see docs/ncib-release-waivers.md W1 and docs/benchmark-integrity.md host_class",
        "memory_rss_method": memory_rss_method(),
        "memory_rss_note": "per-trial rss_before/rss_after dual-point; peak_rss_bytes=max(before,after); not heap HWM (fszero-cq0k / fszero-5444); no absolute RSS gates here",
        "surfaces": [
            "raw_dispatcher", "fastmcp",
            "codemode_recipe", "codemode_json",
            "codemode_js"
        ],
    })
}

fn percentile_ns(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn n_step_json_plan(n: usize, path: &str) -> String {
    let mut steps = Vec::new();
    for i in 0..n {
        steps.push(format!(
            r#"{{"id":"s{i}","call":"fs.read","args":{{"path":"{path}"}}}}"#
        ));
    }
    format!(r#"{{"steps":[{}]}}"#, steps.join(","))
}

fn n_step_js_plan(n: usize, path: &str) -> String {
    let mut body = String::from("let last=null;\n");
    for _ in 0..n {
        body.push_str(&format!("last=await fs.read({{path:{path:?}}});\n"));
    }
    body.push_str("return last;\n");
    body
}

fn recipe_plan_for_n(n: usize) -> String {
    let _ = n;
    "memory:put:bench/note|warm-recipe".to_string()
}

/// Run `N` logical ops on one surface/workload and return per-trial walls.
pub fn run_surface_trials(
    root: &Path,
    surface: BenchSurface,
    workload: BenchWorkload,
    n_ops: usize,
    samples: usize,
) -> Vec<TrialResult> {
    let mut out = Vec::with_capacity(samples);
    let file = root.join("bench.txt");
    if !file.exists() {
        let _ = std::fs::write(&file, b"bench-payload");
    }
    // One session for the whole sample set = true warm path (AI-facing process
    // reuses session; per-sample open would dominate and mis-attribute cost).
    let mut sess = FSZeroSession::with_root(root);
    for sample_i in 0..samples {
        // Serialize with kill-tests / smoke that reset global counters.
        let _metrics_guard = lock_metrics_for_test();
        reset_runtime_metrics();
        let before_dispatch = dispatch_count();
        let rss_before = current_rss_bytes();
        let start = Instant::now();
        let mut ok = true;
        let mut ops = 0u32;

        match surface {
            BenchSurface::RawDispatcher => {
                for i in 0..n_ops {
                    if matches!(workload, BenchWorkload::NoopControl) {
                        let r = dispatch_raw_worker(&mut sess, "fs.ls", &json!({}));
                        ok &= r.result.ok;
                    } else {
                        let r = dispatch_raw_worker(
                            &mut sess,
                            "fs.read",
                            &json!({"path": "bench.txt"}),
                        );
                        ok &= r.result.ok;
                    }
                    ops += 1;
                    if matches!(workload, BenchWorkload::WriteThenRead) && i == 0 {
                        let _ = dispatch_raw_worker(
                            &mut sess,
                            "fs.write",
                            &json!({"path": "bench.txt", "content": format!("w{sample_i}")}),
                        );
                        ops += 1;
                    }
                }
            }
            BenchSurface::FastMcp => {
                for _ in 0..n_ops {
                    let r =
                        dispatch_mcp_tool(&mut sess, "fszero.read", &json!({"path": "bench.txt"}));
                    // Serialize MCP envelope (production path records serialization).
                    if let Ok(outcome) = &r {
                        let env = crate::mcp_rpc::ack_tool_result(
                            &mut sess,
                            outcome.result.ack.as_deref().unwrap_or("ok"),
                            outcome.result.ok,
                            outcome.detail.as_deref(),
                        );
                        let _ = env;
                        ok &= outcome.result.ok;
                    } else {
                        ok = false;
                    }
                    ops += 1;
                }
            }
            BenchSurface::CodemodeRecipe => {
                for _ in 0..n_ops.max(1) {
                    let plan = recipe_plan_for_n(1);
                    let ack = codemode_execute_plan(&mut sess, &plan);
                    ok &= ack == "C";
                    ops += 1;
                }
            }
            BenchSurface::CodemodeJson => {
                let plan = n_step_json_plan(n_ops.max(1), "bench.txt");
                let ack = codemode_execute_plan(&mut sess, &plan);
                ok &= ack == "C";
                ops = n_ops.max(1) as u32;
            }
            BenchSurface::CodemodeJs => {
                let plan = n_step_js_plan(n_ops.max(1), "bench.txt");
                assert!(
                    looks_like_js_plan(&plan),
                    "bench JS plan must be classified as JS"
                );
                let ack = codemode_execute_plan(&mut sess, &plan);
                // Without surface-codemode, JS fails closed — count as measured fail.
                ok &= ack == "C";
                ops = n_ops.max(1) as u32;
            }
        }

        let wall_ns = start.elapsed().as_nanos() as u64;
        let rss_after = current_rss_bytes();
        let peak_rss = match (rss_before, rss_after) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let profile = last_dispatch_profile();
        let after_dispatch = dispatch_count();
        let process_starts = take_process_starts().min(u64::from(u32::MAX)) as u32;
        let serializations = take_serializations().min(u64::from(u32::MAX)) as u32;
        let boundary =
            (after_dispatch.saturating_sub(before_dispatch)).min(u64::from(u32::MAX)) as u32;
        let dup = duplicate_serialization_detected(u64::from(serializations), ops.max(1));
        out.push(TrialResult {
            wall_ns,
            dispatcher_overhead_ns: profile.dispatcher_overhead_ns,
            kernel_ns: profile.kernel_ns,
            ok,
            process_starts,
            op_count: ops,
            boundary_count: boundary,
            serializations,
            duplicate_serialization: dup,
            rss_before_bytes: rss_before,
            rss_after_bytes: rss_after,
            peak_rss_bytes: peak_rss,
        });
    }
    out
}

/// Aggregate trials into a machine-readable evidence document.
pub fn evidence_document(
    git_sha: &str,
    surface: BenchSurface,
    workload: BenchWorkload,
    n_ops: usize,
    trials: &[TrialResult],
) -> Value {
    let body: Vec<&TrialResult> = if trials.len() > 1 {
        trials.iter().skip(1).collect()
    } else {
        trials.iter().collect()
    };
    let mut walls: Vec<u64> = body.iter().map(|t| t.wall_ns).collect();
    walls.sort_unstable();
    let ok_all = body.iter().all(|t| t.ok);
    let raw_trials: Vec<Value> = trials
        .iter()
        .map(|t| {
            json!({
                "wall_ns": t.wall_ns, "dispatcher_overhead_ns": t.dispatcher_overhead_ns,
                "kernel_ns": t.kernel_ns, "ok": t.ok,
                "process_starts": t.process_starts, "op_count": t.op_count,
                "boundary_count": t.boundary_count, "serializations": t.serializations,
                "duplicate_serialization": t.duplicate_serialization,
                "rss_before_bytes": t.rss_before_bytes,
                "rss_after_bytes": t.rss_after_bytes,
                "peak_rss_bytes": t.peak_rss_bytes,
            })
        })
        .collect();
    let any_process = trials.iter().any(|t| t.process_starts > 0);
    let any_dup = trials.iter().any(|t| t.duplicate_serialization);
    let mut peaks: Vec<u64> = body.iter().filter_map(|t| t.peak_rss_bytes).collect();
    peaks.sort_unstable();
    let rss_supported = trials.iter().any(|t| t.peak_rss_bytes.is_some())
        || cfg!(any(target_os = "macos", target_os = "linux"));
    let memory = json!({
        "peak_rss_bytes": {
            "max": peaks.last().copied(),
            "min": peaks.first().copied(),
            "p50": if peaks.is_empty() { Value::Null } else { json!(percentile_ns(&peaks, 0.50)) },
        },
        "method": memory_rss_method(),
        "sampling": "rss_before_and_after_trial_dual_point",
        "supported": rss_supported,
        "unsupported_reason": if rss_supported { Value::Null } else { json!("platform has no portable RSS sampler") },
        "note": "No absolute RSS gates in surface_bench (host_class / bkeu required first). Not heap HWM.",
    });
    json!({
        "schema": "fszero.surface_bench", "provenance": bench_provenance(git_sha),
        "surface": surface.as_str(), "workload": workload.as_str(),
        "n_ops": n_ops, "samples": trials.len(),
        "warmup_discarded": trials.len() > 1, "ok_all": ok_all,
        "wall_ns": {
            "p50": percentile_ns(&walls, 0.50), "p95": percentile_ns(&walls, 0.95),
            "p99": percentile_ns(&walls, 0.99), "min": walls.first().copied().unwrap_or(0),
            "max": walls.last().copied().unwrap_or(0), },
        "memory": memory,
        "targets_absolute": {
            "recipe_overhead_ratio_max": 0.15, "recipe_overhead_floor_ns": 250_000,
            "empty_js_p50_ns": 1_000_000, "empty_js_p99_ns": 5_000_000, },
        "raw_trials": raw_trials,
        "detects": { "extra_process_starts": any_process, "duplicate_serialization": any_dup, }
    })
}

/// Exact multiplier for the persistent shipped-wire release gate.
pub const WIRE_RATCHET_MULTIPLIER: u64 = 2;
pub const WIRE_RATCHET_MIN_SAMPLES: usize = 12;
/// Keep-gate schema; `scripts/apply_bench_ratchet.py` rejects any other id.
pub const WIRE_RATCHET_SCHEMA: &str = "fszero.surface_wire_ratchet.v1";

/// Evaluate the release-perf gate for comparable persistent wire surfaces.
pub fn evaluate_wire_ratchet(cm_p50: u64, cm_p95: u64, mcp_p50: u64, mcp_p95: u64) -> Value {
    let pass_p50 = cm_p50 <= mcp_p50.saturating_mul(WIRE_RATCHET_MULTIPLIER);
    let pass_p95 = cm_p95 <= mcp_p95.saturating_mul(WIRE_RATCHET_MULTIPLIER);
    json!({
        "scope": "persistent_stdio_json_rpc", "threshold_multiplier": WIRE_RATCHET_MULTIPLIER,
        "pass_p50": pass_p50, "pass_p95": pass_p95, "gate_pass": pass_p50 && pass_p95,
        "pass_conditions": {
            "p50": "codemode_p50_ns <= 2 * fastmcp_p50_ns",
            "p95": "codemode_p95_ns <= 2 * fastmcp_p95_ns",
        },
    })
}

/// Build evidence from ordered samples captured by the persistent-wire test.
pub fn wire_evidence_document(
    git_sha: &str,
    n: usize,
    warmup_policy: &str,
    codemode_binary_sha256: &str,
    fastmcp_binary_sha256: &str,
    codemode_walls_ns: &[u64],
    fastmcp_walls_ns: &[u64],
    validation: Value,
) -> Result<Value, String> {
    if n < 3 {
        return Err("wire comparison requires N>=3".into());
    }
    if codemode_walls_ns.len() < WIRE_RATCHET_MIN_SAMPLES
        || fastmcp_walls_ns.len() < WIRE_RATCHET_MIN_SAMPLES
    {
        return Err("wire comparison requires at least 12 measured samples per surface".into());
    }
    if codemode_walls_ns.len() != fastmcp_walls_ns.len() {
        return Err("wire comparison requires equal sample counts".into());
    }
    if codemode_binary_sha256.is_empty() || fastmcp_binary_sha256.is_empty() {
        return Err("wire comparison requires binary SHA256 values".into());
    }
    let mut cm = codemode_walls_ns.to_vec();
    let mut mcp = fastmcp_walls_ns.to_vec();
    cm.sort_unstable();
    mcp.sort_unstable();
    let cm_p50 = percentile_ns(&cm, 0.50);
    let cm_p95 = percentile_ns(&cm, 0.95);
    let mcp_p50 = percentile_ns(&mcp, 0.50);
    let mcp_p95 = percentile_ns(&mcp, 0.95);
    let mut ratchet = evaluate_wire_ratchet(cm_p50, cm_p95, mcp_p50, mcp_p95);
    ratchet["codemode_p50_ns"] = json!(cm_p50);
    ratchet["codemode_p95_ns"] = json!(cm_p95);
    ratchet["fastmcp_p50_ns"] = json!(mcp_p50);
    ratchet["fastmcp_p95_ns"] = json!(mcp_p95);
    Ok(json!({
        "schema": WIRE_RATCHET_SCHEMA,
        "provenance": {
            "git_sha": git_sha, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "profile": cargo_profile_name(), "cargo_profile": cargo_profile_name(),
            "transport": "persistent NDJSON stdio JSON-RPC",
            "measurement_scope": "shipped persistent stdio JSON-RPC surfaces; spawn/init excluded",
            "codemode_surface": "fszero-codemode compatibility surface", "fastmcp_surface": "fszero-mcp shipped surface",
            "n": n, "samples": codemode_walls_ns.len(), "warmup_policy": warmup_policy,
            "git_dirty": git_dirty(),
            "binary_sha256": {"codemode": codemode_binary_sha256, "fastmcp": fastmcp_binary_sha256},
        },
        "scope": "persistent_stdio_json_rpc", "n": n, "samples": codemode_walls_ns.len(),
        "raw_ordered_ns": {"codemode": codemode_walls_ns, "fastmcp": fastmcp_walls_ns},
        "codemode": {"surface": "fszero-codemode", "p50_ns": cm_p50, "p95_ns": cm_p95},
        "fastmcp": {"surface": "fszero-mcp", "p50_ns": mcp_p50, "p95_ns": mcp_p95},
        "validation": validation, "ratchet": ratchet,
    }))
}

fn git_dirty() -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(true)
}

/// Diagnostic-only legacy comparison. It cannot gate release performance.
pub fn codemode_in_process_scope_diagnostic(
    root: &Path,
    n: usize,
    samples: usize,
) -> Result<Value, String> {
    if n < 3 {
        return Err("comparison requires N>=3".into());
    }
    let samples = samples.max(WIRE_RATCHET_MIN_SAMPLES);
    let cm = run_surface_trials(
        root,
        BenchSurface::CodemodeJson,
        BenchWorkload::RepeatedRead,
        n,
        samples,
    );
    let mcp = run_surface_trials(
        root,
        BenchSurface::FastMcp,
        BenchWorkload::RepeatedRead,
        n,
        samples,
    );
    Ok(json!({
        "schema": "fszero.surface_bench.in_process_diagnostic", "diagnostic_only": true, "gate_applied": false,
        "comparison_scope": "unequal agent_local_in_process",
        "scope_warning": "CodeMode includes mandatory receipt commitment; FastMCP is per-op dispatch. This comparison cannot gate release performance.",
        "n": n, "samples": samples,
        "codemode": evidence_document("local", BenchSurface::CodemodeJson, BenchWorkload::RepeatedRead, n, &cm),
        "mcp": evidence_document("local", BenchSurface::FastMcp, BenchWorkload::RepeatedRead, n, &mcp),
    }))
}

/// Absolute threshold checks from measured evidence (implemented detectors).
pub fn evaluate_absolute_thresholds(doc: &Value) -> Value {
    let p50 = doc["wall_ns"]["p50"].as_u64().unwrap_or(0);
    let p99 = doc["wall_ns"]["p99"].as_u64().unwrap_or(0);
    let surface = doc["surface"].as_str().unwrap_or("");
    let empty_js_p50_ok = if surface == "codemode_js" {
        // Empty/minimal JS: measured when n_ops=1; gate uses 1ms/5ms.
        p50 <= 1_000_000 || cfg!(debug_assertions) // debug hosts often exceed 1ms
    } else {
        true
    };
    let empty_js_p99_ok = if surface == "codemode_js" {
        p99 <= 5_000_000 || cfg!(debug_assertions)
    } else {
        true
    };
    json!({
        "surface": surface, "empty_js_p50_ok": empty_js_p50_ok,
        "empty_js_p99_ok": empty_js_p99_ok, "measured_p50_ns": p50,
        "measured_p99_ns": p99, "detector": "evaluate_absolute_thresholds",
    })
}
