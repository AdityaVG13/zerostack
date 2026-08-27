//! Multi-size latency scaling curve for skill-pass (graphzero-hkexf).
//!
//! Builds synthetic repos at N ∈ {1,10,50,100,500,1000}, measures blast wall
//! times, emits p50/p95 and p95_ratio vs N=1 as JSON.
//!
//! Print line: `SCALING_CURVE_RESULT=<json>`
//!
//! ```bash
//! rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero \
//!   cargo run --release -p graphzero-store --example scaling_curve_bench
//! ```

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use graphzero_engine::blast::blast_radius;
use graphzero_store::Snapshot;
use graphzero_store::store::indexer::index_repo;
use graphzero_test_support::git::git_commit_all;
use serde_json::json;

/// Documented skill sizes (shared with criterion `BENCH_FILE_SWEEP`).
const SCALE_SIZES: &[usize] = &[1, 10, 50, 100, 500, 1000];
/// Publication floor for non-orient distributions.
const TRIALS: usize = 20;
const WARMUPS: usize = 2;
const BLAST_BUDGET: usize = 800;
const INTENT: &str = "change_signature of func_0_0";

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn build_repo(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo_root = dir.path().join("repo");
    fs::create_dir_all(repo_root.join("src")).unwrap();
    for i in 0..n {
        let mut content = String::new();
        for j in 0..10 {
            content.push_str(&format!(
                "fn func_{i}_{j}(x: u64) -> u64 {{\n    let y = x + {j};\n    helper_{}(y)\n}}\n\n",
                (i + j) % 97
            ));
        }
        for j in 0..2 {
            content.push_str(&format!(
                "fn helper_{}(v: u64) -> u64 {{\n    v * 2\n}}\n\n",
                (i * 2 + j) % 97
            ));
        }
        fs::write(repo_root.join(format!("src/file_{i:05}.rs")), content).unwrap();
    }
    git_commit_all(&repo_root);
    let store_root = repo_root.join(".graphzero");
    index_repo(&repo_root, &store_root).expect("index");
    dir
}

fn measure_blast(repo_root: &PathBuf, store_root: &PathBuf) -> (f64, f64, Vec<f64>) {
    let snapshot = Snapshot::open(store_root, Some(repo_root)).expect("open snapshot");
    for _ in 0..WARMUPS {
        let _ = blast_radius(&snapshot, INTENT, BLAST_BUDGET).expect("warmup blast");
    }
    let mut samples = Vec::with_capacity(TRIALS);
    for _ in 0..TRIALS {
        let start = Instant::now();
        let cap = blast_radius(&snapshot, INTENT, BLAST_BUDGET).expect("blast");
        let ms = start.elapsed().as_secs_f64() * 1_000.0;
        // Touch result so work cannot be optimized out.
        std::hint::black_box(cap.break_sites.len() + cap.covering_tests.len());
        samples.push(ms);
    }
    (
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        samples,
    )
}

fn main() {
    let mut points = Vec::new();
    let mut p95_at_1: Option<f64> = None;

    for &n in SCALE_SIZES {
        let dir = build_repo(n);
        let repo_root = dir.path().join("repo");
        let store_root = repo_root.join(".graphzero");
        let index_start = Instant::now();
        // index already done in build_repo; re-record cold open cost only.
        let open_ms = {
            let start = Instant::now();
            let _ = Snapshot::open(&store_root, Some(&repo_root)).expect("open");
            start.elapsed().as_secs_f64() * 1_000.0
        };
        let (p50, p95, samples) = measure_blast(&repo_root, &store_root);
        if p95_at_1.is_none() {
            p95_at_1 = Some(p95);
        }
        let ratio = p95 / p95_at_1.unwrap().max(f64::EPSILON);
        points.push(json!({
            "files": n,
            "blast": {
                "p50_ms": p50,
                "p95_ms": p95,
                "p95_ratio_vs_n1": ratio,
                "runs": samples.len(),
                "samples_ms": samples,
            },
            "open_ms": open_ms,
            "fixture_build_includes_index": true,
            "index_wall_note": "index folded into fixture build; not reported separately",
            "build_ms_placeholder": index_start.elapsed().as_secs_f64() * 1_000.0,
        }));
        eprintln!(
            "scaling_curve n={n} blast_p50={p50:.3}ms blast_p95={p95:.3}ms p95_ratio={ratio:.3}"
        );
    }

    let result = json!({
        "schema_version": 1,
        "generated_by": "crates/graphzero/graphzero-store/examples/scaling_curve_bench.rs",
        "bead": "graphzero-hkexf",
        "metric": "blast_radius",
        "intent": INTENT,
        "budget": BLAST_BUDGET,
        "sizes": SCALE_SIZES,
        "trials": TRIALS,
        "warmups": WARMUPS,
        "profile_note": "run under --release or release-perf; host variance expected",
        "criterion_companion": "crates/graphzero/graphzero-store/benches/blast_precomp.rs::bench_blast_by_file_count",
        "points": points,
    });

    println!("SCALING_CURVE_RESULT={result}");
}
