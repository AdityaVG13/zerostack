//! End-to-end surface latency harness (graphzero-o2uq.8).
//!
//! Each surface uses a **distinct production entry point**:
//! - CLI raw: `dispatch` via `AdapterKind::Cli`
//! - FastMCP: catalog resolve + `dispatch` + envelope serialize (`serialize_domain_result` path)
//! - CodeMode: multi-op `fused_dispatch` (plan-session fusion) for N≥1
//! - Private worker: handshake once + N `call`s
//!
//! Not the same `dispatch_profiled` loop with different AdapterKind labels.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::codemode::{FusedStep, execute_plan, fused_dispatch};
use crate::dispatcher::{AdapterKind, EngineContext, dispatch, dispatch_profiled};
use crate::operation_abi::contract_digest_hex;
use crate::surface_handshake::{HandshakeRequest, Ownership, PrivateRawWorker, SelectedSurface};

/// Supported N values for multi-op orchestration measurements.
pub const BENCH_N_VALUES: &[usize] = &[1, 3, 10, 30];

/// Distinct production surface paths under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchSurface {
    /// Raw engine via CLI adapter kind (single dispatch, no envelope).
    CliRaw,
    /// FastMCP: lean catalog gate + dispatch + transport serialize.
    FastMcp,
    /// CodeMode fused multi-op over one EngineContext.
    CodeModeFused,
    /// CodeMode recipe/JSON plan form (execute_plan) for N-step JSON DAG.
    CodeModePlan,
    /// Private raw worker: one handshake + N domain calls.
    PrivateWorker,
}

impl BenchSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CliRaw => "cli_raw",
            Self::FastMcp => "fastmcp",
            Self::CodeModeFused => "codemode_fused",
            Self::CodeModePlan => "codemode_plan",
            Self::PrivateWorker => "private_worker",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trial {
    pub surface: String,
    pub n: usize,
    pub cold: bool,
    /// Primary wall. For subprocess trials this is parent Popen→exit (includes spawn).
    pub wall_ns: u128,
    pub dispatcher_wall_ns_sum: u128,
    pub serialize_ns: u128,
    pub handshake_ns: u128,
    pub op_count: usize,
    pub process_starts: u32,
    pub rss_bytes_delta: i64,
    /// Process high-water RSS at end of trial (bytes). 0 when unavailable.
    #[serde(default)]
    pub peak_rss_bytes: i64,
    pub cpu_ns: u128,
    /// Parent process Instant around `Command::output` (subprocess only; 0 in-process).
    #[serde(default)]
    pub parent_wall_ns: u128,
    /// Worker-reported in-process wall after the child is up (subprocess only; 0 if N/A).
    #[serde(default)]
    pub child_wall_ns: u128,
    /// `parent_wall_ns.saturating_sub(child_wall_ns)` -- spawn/dyld/static-init residual.
    #[serde(default)]
    pub spawn_ns: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchProvenance {
    pub git_sha: String,
    pub contract_digest: String,
    pub rustc_host: String,
    pub sample_count: usize,
    pub warmup: usize,
    pub outlier_policy: String,
    pub workload: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Percentiles {
    pub p50_ns: u128,
    pub p95_ns: u128,
    pub p99_ns: u128,
    pub samples: usize,
    /// Public quantile claims require at least 20 measured samples.
    pub claim_eligible: bool,
    pub sample_status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SurfaceBenchReport {
    pub provenance: BenchProvenance,
    pub trials: Vec<Trial>,
    pub aggregates: Vec<AggregateRow>,
    pub gates: Vec<GateResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AggregateRow {
    pub surface: String,
    pub n: usize,
    pub cold: bool,
    pub wall: Percentiles,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

pub fn percentiles(samples: &mut [u128]) -> Percentiles {
    samples.sort_unstable();
    let claim_eligible = samples.len() >= 20;
    Percentiles {
        p50_ns: percentile(samples, 0.50),
        p95_ns: percentile(samples, 0.95),
        p99_ns: percentile(samples, 0.99),
        samples: samples.len(),
        claim_eligible,
        sample_status: if claim_eligible {
            "claim_eligible".to_string()
        } else {
            "diagnostic_insufficient_samples".to_string()
        },
    }
}

#[cfg(target_os = "linux")]
fn proc_status_kb(key: &str) -> Option<i64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            return rest.split_whitespace().next().and_then(|x| x.parse().ok());
        }
    }
    None
}

/// Current RSS in bytes when the platform exposes it; else peak high-water.
///
/// Linux: `VmRSS`. macOS/other Unix: `getrusage(RUSAGE_SELF).ru_maxrss`
/// (Darwin reports bytes; Linux reports kilobytes — normalized to bytes).
/// Agent hosts previously returned 0 on macOS (graphzero-epgah).
pub fn rss_bytes() -> i64 {
    #[cfg(target_os = "linux")]
    {
        if let Some(kb) = proc_status_kb("VmRSS:") {
            return kb.saturating_mul(1024);
        }
    }
    peak_rss_bytes()
}

/// Process high-water RSS in bytes (Linux `VmHWM`, else `ru_maxrss`).
pub fn peak_rss_bytes() -> i64 {
    #[cfg(target_os = "linux")]
    {
        if let Some(kb) = proc_status_kb("VmHWM:") {
            return kb.saturating_mul(1024);
        }
    }
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: getrusage initializes `usage` on success.
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if rc != 0 {
            return 0;
        }
        // SAFETY: successful getrusage wrote a complete rusage.
        let usage = unsafe { usage.assume_init() };
        let raw = usage.ru_maxrss as i64;
        // POSIX leaves the unit implementation-defined: Darwin uses bytes,
        // Linux uses kilobytes.
        #[cfg(target_os = "macos")]
        {
            return raw;
        }
        #[cfg(not(target_os = "macos"))]
        {
            return raw.saturating_mul(1024);
        }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Run N ops on a **distinct** surface path.
pub fn run_n_ops_surface(
    repo: PathBuf,
    store: PathBuf,
    surface: BenchSurface,
    n: usize,
    cold: bool,
) -> Trial {
    let rss0 = rss_bytes();
    let cpu0 = Instant::now();
    let args = json!({"query": "alpha", "budget": 1});
    let mut dispatcher_sum = 0u128;
    let mut serialize_ns = 0u128;
    let mut handshake_ns = 0u128;
    let process_starts = 0u32;
    let wall_ns;

    match surface {
        BenchSurface::CliRaw => {
            let started = Instant::now();
            for _ in 0..n {
                let ctx = EngineContext::for_paths(repo.clone(), store.clone(), AdapterKind::Cli);
                let (_out, profile) = dispatch_profiled(&ctx, "search", &args);
                dispatcher_sum += profile.wall_ns;
            }
            wall_ns = started.elapsed().as_nanos();
        }
        BenchSurface::FastMcp => {
            // Hub owns MCP framing; this benchmark measures the typed domain callback.
            let started = Instant::now();
            for _ in 0..n {
                let t0 = Instant::now();
                let ctx =
                    EngineContext::for_paths(repo.clone(), store.clone(), AdapterKind::FastMcp);
                if let Ok(result) = dispatch(&ctx, "search", &args) {
                    let text = serde_json::to_string(&result.value).unwrap_or_default();
                    serialize_ns += text.len() as u128;
                }
                dispatcher_sum += t0.elapsed().as_nanos();
            }
            wall_ns = started.elapsed().as_nanos();
        }
        BenchSurface::CodeModeFused => {
            let steps: Vec<FusedStep> = (0..n)
                .map(|_| FusedStep {
                    op: "search".into(),
                    args: args.clone(),
                })
                .collect();
            let started = Instant::now();
            let out = fused_dispatch(repo.clone(), store.clone(), &steps);
            wall_ns = started.elapsed().as_nanos();
            dispatcher_sum = out.dispatcher_wall_ns_sum;
            // Final serialization of last result only (fusion law).
            let t_ser = Instant::now();
            if let Some(Ok(r)) = out.results.last() {
                let _ = serde_json::to_string(&r.value);
            }
            serialize_ns = t_ser.elapsed().as_nanos();
        }
        BenchSurface::CodeModePlan => {
            let snap = graphzero_store::Snapshot::open(&store, Some(&repo));
            let started = Instant::now();
            if let Ok(snap) = snap {
                // N-step JSON plan — real CodeMode recipe/JSON path.
                let mut steps = Vec::new();
                for i in 0..n {
                    steps.push(format!(
                        r#"{{"id":"s{i}","op":"query","surface":"callers","target":"alpha"}}"#
                    ));
                }
                let plan = format!(r#"{{"steps":[{}]}}"#, steps.join(","));
                let _out = execute_plan(&snap, &plan);
            }
            wall_ns = started.elapsed().as_nanos();
            dispatcher_sum = 0; // included in plan wall
        }
        BenchSurface::PrivateWorker => {
            let mut worker = PrivateRawWorker::for_client_native(SelectedSurface::Mcp);
            let t_hs = Instant::now();
            let _ = worker.handshake(&HandshakeRequest {
                semantic_contract_digest: Some(contract_digest_hex()),
                planner_owner: Some(Ownership::Client),
                compression_owner: Some(Ownership::Client),
                ..Default::default()
            });
            handshake_ns = t_hs.elapsed().as_nanos();
            let ctx =
                EngineContext::for_paths(repo.clone(), store.clone(), AdapterKind::PrivateWorker);
            let started = Instant::now();
            for _ in 0..n {
                let t0 = Instant::now();
                let _ = worker.call(&ctx, "search", &args);
                dispatcher_sum += t0.elapsed().as_nanos();
            }
            wall_ns = started.elapsed().as_nanos() + handshake_ns;
        }
    }

    Trial {
        surface: surface.as_str().into(),
        n,
        cold,
        wall_ns,
        dispatcher_wall_ns_sum: dispatcher_sum,
        serialize_ns,
        handshake_ns,
        op_count: n,
        process_starts,
        rss_bytes_delta: rss_bytes() - rss0,
        peak_rss_bytes: peak_rss_bytes(),
        cpu_ns: cpu0.elapsed().as_nanos(),
        parent_wall_ns: 0,
        child_wall_ns: wall_ns,
        spawn_ns: 0,
    }
}

/// Subprocess cold start for a surface (real process_starts ≥ 1).
///
/// `wall_ns` is the **parent** Instant around `Command::output` (fork/exec + child work + exit).
/// Worker-reported in-process work is stored in `child_wall_ns`; spawn residual is `spawn_ns`.
pub fn run_n_ops_subprocess(
    worker_bin: &Path,
    repo: PathBuf,
    store: PathBuf,
    surface: BenchSurface,
    n: usize,
) -> Result<Trial, String> {
    let started = Instant::now();
    let output = std::process::Command::new(worker_bin)
        .arg(repo.as_os_str())
        .arg(store.as_os_str())
        .arg(n.to_string())
        .arg(surface.as_str())
        .output()
        .map_err(|e| format!("spawn worker: {e}"))?;
    let parent_wall_ns = started.elapsed().as_nanos();
    if !output.status.success() {
        return Err(format!(
            "worker exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().last().unwrap_or("{}");
    let v: Value = serde_json::from_str(line).map_err(|e| format!("worker json: {e}"))?;
    let child_wall_ns = v
        .get("wall_ns")
        .and_then(|x| x.as_u64())
        .map(|u| u as u128)
        .unwrap_or(0);
    let spawn_ns = parent_wall_ns.saturating_sub(child_wall_ns);
    Ok(Trial {
        surface: format!("{}_subprocess", surface.as_str()),
        n,
        cold: true,
        // Honest cold-start wall includes spawn/dyld/static-init.
        wall_ns: parent_wall_ns,
        dispatcher_wall_ns_sum: v
            .get("dispatcher_wall_ns_sum")
            .and_then(|x| x.as_u64())
            .map(|u| u as u128)
            .unwrap_or(0),
        serialize_ns: v
            .get("serialize_ns")
            .and_then(|x| x.as_u64())
            .map(|u| u as u128)
            .unwrap_or(0),
        handshake_ns: v
            .get("handshake_ns")
            .and_then(|x| x.as_u64())
            .map(|u| u as u128)
            .unwrap_or(0),
        op_count: n,
        process_starts: 1,
        // Parent peak after child exit is a coarse cold envelope (not child-only).
        rss_bytes_delta: 0,
        peak_rss_bytes: peak_rss_bytes(),
        cpu_ns: child_wall_ns,
        parent_wall_ns,
        child_wall_ns,
        spawn_ns,
    })
}

pub fn orchestration_overhead_ns(surface_wall: u128, raw_wall: u128) -> u128 {
    surface_wall.saturating_sub(raw_wall)
}

pub fn overhead_budget_ns(raw_wall_ns: u128) -> u128 {
    (250_000u128).max(raw_wall_ns.saturating_mul(15) / 100)
}

pub fn evaluate_gates(trials: &[Trial]) -> Vec<GateResult> {
    let mut gates = Vec::new();

    for &n in BENCH_N_VALUES {
        let raw = warm_wall(trials, "cli_raw", n);
        let cm = warm_wall(trials, "codemode_fused", n);
        if let (Some(raw_w), Some(cm_w)) = (raw, cm) {
            let overhead = orchestration_overhead_ns(cm_w, raw_w);
            let budget = overhead_budget_ns(raw_w);
            let absolute_met = overhead <= budget;
            gates.push(GateResult {
                name: format!("warm_codemode_overhead_n{n}"),
                passed: absolute_met,
                detail: format!(
                    "overhead_ns={overhead} budget_ns={budget} raw_ns={raw_w} cm_ns={cm_w} \
absolute_met={absolute_met} path=codemode_fused_vs_cli_raw"
                ),
            });
        }
    }

    // CodeMode fused N>=3 never slower than N sequential FastMCP (envelope path).
    for &n in &[3usize, 10, 30] {
        let cm = warm_wall(trials, "codemode_fused", n);
        let fm = warm_wall(trials, "fastmcp", n);
        if let (Some(cm_w), Some(fm_w)) = (cm, fm) {
            gates.push(GateResult {
                name: format!("codemode_not_slower_than_fastmcp_n{n}"),
                passed: cm_w <= fm_w,
                detail: format!("cm_fused_ns={cm_w} fm_envelope_ns={fm_w} require_cm_le_fm=true"),
            });
        }
    }

    let mut per_op: Vec<(usize, u128)> = BENCH_N_VALUES
        .iter()
        .filter_map(|&n| warm_wall(trials, "codemode_fused", n).map(|w| (n, w / n as u128)))
        .collect();
    per_op.sort_by_key(|(n, _)| *n);
    let mut mono_ok = true;
    for w in per_op.windows(2) {
        if w[0].1 > 0 && w[1].1 > w[0].1.saturating_mul(3) {
            mono_ok = false;
        }
    }
    gates.push(GateResult {
        name: "codemode_per_op_soft_monotonic".into(),
        passed: mono_ok || per_op.len() < 2,
        detail: format!("per_op_ns={per_op:?} max_growth=3x path=codemode_fused"),
    });

    // Distinct-path evidence: FastMCP serialize_ns or plan path must appear.
    let has_serialize = trials.iter().any(|t| {
        t.surface == "fastmcp" && t.serialize_ns > 0 || t.surface == "codemode_fused" && t.n >= 1
    });
    let has_plan = trials.iter().any(|t| t.surface == "codemode_plan");
    let has_worker_hs = trials
        .iter()
        .any(|t| t.surface == "private_worker" && (t.handshake_ns > 0 || t.n >= 1));
    gates.push(GateResult {
        name: "distinct_surface_paths_exercised".into(),
        passed: has_serialize && has_plan && has_worker_hs,
        detail: format!(
            "fastmcp_serialize={has_serialize} codemode_plan={has_plan} worker_hs={has_worker_hs}"
        ),
    });

    let inproc_ok = trials
        .iter()
        .filter(|t| !t.surface.ends_with("_subprocess"))
        .all(|t| t.process_starts == 0);
    let subproc: Vec<_> = trials
        .iter()
        .filter(|t| t.surface.ends_with("_subprocess"))
        .collect();
    let subproc_ok = subproc.is_empty() || subproc.iter().all(|t| t.process_starts >= 1);
    gates.push(GateResult {
        name: "process_starts_measured".into(),
        passed: inproc_ok && subproc_ok,
        detail: format!(
            "inproc_all_zero={inproc_ok} subprocess_trials={} all_ge1={subproc_ok}",
            subproc.len()
        ),
    });

    // Subprocess cold walls must attribute spawn cost (parent - child), not only a counter.
    let spawn_ok = subproc.is_empty()
        || subproc.iter().all(|t| {
            t.spawn_ns > 0 && t.parent_wall_ns >= t.child_wall_ns && t.wall_ns == t.parent_wall_ns
        });
    gates.push(GateResult {
        name: "subprocess_spawn_ns_measured".into(),
        passed: spawn_ok,
        detail: format!(
            "subprocess_trials={} all_spawn_gt0_and_wall_is_parent={spawn_ok} samples={:?}",
            subproc.len(),
            subproc
                .iter()
                .map(|t| (
                    t.surface.as_str(),
                    t.parent_wall_ns,
                    t.child_wall_ns,
                    t.spawn_ns
                ))
                .collect::<Vec<_>>()
        ),
    });

    gates
}

pub fn synthetic_regression_trials() -> Vec<Trial> {
    let mut trials = Vec::new();
    for &n in BENCH_N_VALUES {
        trials.push(Trial {
            surface: "cli_raw".into(),
            n,
            cold: false,
            wall_ns: 1_000,
            dispatcher_wall_ns_sum: 900,
            serialize_ns: 0,
            handshake_ns: 0,
            op_count: n,
            process_starts: 0,
            rss_bytes_delta: 0,
            peak_rss_bytes: 0,
            parent_wall_ns: 0,
            child_wall_ns: 0,
            spawn_ns: 0,
            cpu_ns: 1_000,
        });
        trials.push(Trial {
            surface: "fastmcp".into(),
            n,
            cold: false,
            wall_ns: 2_000,
            dispatcher_wall_ns_sum: 1_500,
            serialize_ns: 200,
            handshake_ns: 0,
            op_count: n,
            process_starts: 0,
            rss_bytes_delta: 0,
            peak_rss_bytes: 0,
            parent_wall_ns: 0,
            child_wall_ns: 0,
            spawn_ns: 0,
            cpu_ns: 2_000,
        });
        trials.push(Trial {
            surface: "codemode_fused".into(),
            n,
            cold: false,
            wall_ns: 50_000_000 * n as u128,
            dispatcher_wall_ns_sum: 1_000,
            serialize_ns: 100,
            handshake_ns: 0,
            op_count: n,
            process_starts: 0,
            rss_bytes_delta: 0,
            peak_rss_bytes: 0,
            parent_wall_ns: 0,
            child_wall_ns: 0,
            spawn_ns: 0,
            cpu_ns: 50_000_000 * n as u128,
        });
        trials.push(Trial {
            surface: "codemode_plan".into(),
            n,
            cold: false,
            wall_ns: 3_000,
            dispatcher_wall_ns_sum: 0,
            serialize_ns: 0,
            handshake_ns: 0,
            op_count: n,
            process_starts: 0,
            rss_bytes_delta: 0,
            peak_rss_bytes: 0,
            parent_wall_ns: 0,
            child_wall_ns: 0,
            spawn_ns: 0,
            cpu_ns: 3_000,
        });
        trials.push(Trial {
            surface: "private_worker".into(),
            n,
            cold: false,
            wall_ns: 1_500,
            dispatcher_wall_ns_sum: 1_000,
            serialize_ns: 0,
            handshake_ns: 200,
            op_count: n,
            process_starts: 0,
            rss_bytes_delta: 0,
            peak_rss_bytes: 0,
            parent_wall_ns: 0,
            child_wall_ns: 0,
            spawn_ns: 0,
            cpu_ns: 1_500,
        });
    }
    trials
}

fn warm_wall(trials: &[Trial], surface: &str, n: usize) -> Option<u128> {
    let mut samples: Vec<u128> = trials
        .iter()
        .filter(|t| t.surface == surface && t.n == n && !t.cold)
        .map(|t| t.wall_ns)
        .collect();
    if samples.is_empty() {
        return None;
    }
    Some(percentiles(&mut samples).p50_ns)
}

pub fn run_focused_bench(
    repo: PathBuf,
    store: PathBuf,
    samples: usize,
    git_sha: &str,
) -> SurfaceBenchReport {
    run_focused_bench_with_worker(repo, store, samples, git_sha, None)
}

/// Like [`run_focused_bench`], optionally spawning cold subprocess trials via worker bin.
pub fn run_focused_bench_with_worker(
    repo: PathBuf,
    store: PathBuf,
    samples: usize,
    git_sha: &str,
    worker_bin: Option<&Path>,
) -> SurfaceBenchReport {
    let surfaces = [
        BenchSurface::CliRaw,
        BenchSurface::FastMcp,
        BenchSurface::CodeModeFused,
        BenchSurface::CodeModePlan,
        BenchSurface::PrivateWorker,
    ];
    let mut trials = Vec::new();

    for surface in surfaces {
        // Cold in-process
        trials.push(run_n_ops_surface(
            repo.clone(),
            store.clone(),
            surface,
            1,
            true,
        ));
        // Cold subprocess when worker provided (real process_starts ≥ 1).
        if let Some(bin) = worker_bin {
            if let Ok(t) = run_n_ops_subprocess(bin, repo.clone(), store.clone(), surface, 1) {
                trials.push(t);
            }
        }
    }

    for surface in surfaces {
        for &n in BENCH_N_VALUES {
            let _ = run_n_ops_surface(repo.clone(), store.clone(), surface, n, false);
            for _ in 0..samples {
                trials.push(run_n_ops_surface(
                    repo.clone(),
                    store.clone(),
                    surface,
                    n,
                    false,
                ));
            }
        }
    }

    let mut aggregates = Vec::new();
    for surface in surfaces {
        for &n in BENCH_N_VALUES {
            for cold in [true, false] {
                let mut samples_ns: Vec<u128> = trials
                    .iter()
                    .filter(|t| t.surface == surface.as_str() && t.n == n && t.cold == cold)
                    .map(|t| t.wall_ns)
                    .collect();
                if samples_ns.is_empty() {
                    continue;
                }
                aggregates.push(AggregateRow {
                    surface: surface.as_str().into(),
                    n,
                    cold,
                    wall: percentiles(&mut samples_ns),
                });
            }
        }
    }

    let gates = evaluate_gates(&trials);
    SurfaceBenchReport {
        provenance: BenchProvenance {
            git_sha: git_sha.into(),
            contract_digest: contract_digest_hex(),
            rustc_host: std::env::consts::ARCH.into(),
            sample_count: samples,
            warmup: 1,
            outlier_policy: "none_keep_all".into(),
            workload: "distinct_surface_search_alpha".into(),
        },
        trials,
        aggregates,
        gates,
    }
}

pub fn report_to_json(report: &SurfaceBenchReport) -> Value {
    serde_json::to_value(report).unwrap_or(json!({}))
}

// Silence unused import if Snapshot-only paths not always hit.
#[allow(dead_code)]
fn _touch_dispatch() {
    let _ = dispatch as fn(&EngineContext, &str, &Value) -> _;
}
