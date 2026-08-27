//! Measured spike: baseline vs memmem vs lazy bigram+memmem (fszero-9yq),
//! plus incremental ingest bigram cost (fszero-9ot) and watch upsert
//! create/modify/delete parity+cost (fszero-kbo default-on bakeoff).
//!
//! Usage:
//!   ./scripts/profile_build.sh --cargo-command bench --bench search_prefilter_spike --features search-eval -- <corpus_dir>
//!
//! Prints JSON lines with per-query timings, ingest-incremental, and watch upsert.
//! Prefer the Python orchestrator (`benchmarks/search_prefilter_spike.py`).

use fs_zero::core::search_prefilter_eval::{
    LazyBigramIndex, apply_incremental, measure_ingest_bigram_cost, scan_baseline,
    scan_bigram_memmem, scan_memmem,
};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MIN_MEASURED_RUNS: usize = 20;

fn collect_keys(root: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                keys.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    keys
}

fn percentile(samples: &[f64], p: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn time_ms(iters: usize, mut f: impl FnMut()) -> Vec<f64> {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        f();
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    samples
}

fn rss_bytes() -> Option<u64> {
    // macOS: parse `ps -o rss=` (KiB). Linux: VmRSS from /proc/self/status.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let kib: u64 = s.trim().parse().ok()?;
        return Some(kib * 1024);
    }
    #[cfg(target_os = "linux")]
    {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kib * 1024);
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let corpus = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let iters: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(MIN_MEASURED_RUNS);
    if iters < MIN_MEASURED_RUNS {
        eprintln!("--iters must be at least {MIN_MEASURED_RUNS}");
        std::process::exit(2);
    }
    let limit: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(16);

    let mut keys = collect_keys(&corpus);
    let file_count = keys.len();
    let mut total_bytes = 0u64;
    for k in &keys {
        if let Ok(meta) = fs::metadata(corpus.join(k)) {
            total_bytes += meta.len();
        }
    }

    let queries: Vec<(&str, &str)> = vec![
        ("rare", "rare_zz9q_unique_needle"),
        ("common", "wrapping_add"),
        ("absent", "ABSENT_needle_xyz_never_9yq"),
        ("ascii", "alpha_bravo_marker"),
        ("unicode", "café_ユニーク_٩"),
    ];

    // Correctness gate before timing.
    let mut index = LazyBigramIndex::new();
    for (label, q) in &queries {
        let terms = vec![(*q).to_string()];
        let base = scan_baseline(&corpus, &keys, &terms, limit);
        let mem = scan_memmem(&corpus, &keys, &terms, limit);
        let big = scan_bigram_memmem(&corpus, &keys, &terms, limit, &mut index);
        if base != mem || base != big {
            eprintln!(
                "CORRECTNESS_FAIL label={label} base={} mem={} big={}",
                base.len(),
                mem.len(),
                big.len()
            );
            std::process::exit(1);
        }
    }

    let rss_before = rss_bytes();
    // Force full bigram materialization for memory accounting.
    // Owned keys so watch upsert can mutate `keys` without conflicting borrows.
    let sorted: Vec<String> = {
        let mut v: Vec<String> = keys.iter().cloned().collect();
        v.sort_unstable();
        v
    };
    let sorted_refs: Vec<&str> = sorted.iter().map(String::as_str).collect();
    index.ensure_files(&corpus, &sorted_refs);
    let bigram_bytes = index.approx_bytes();
    let rss_after = rss_bytes();

    println!(
        "{}",
        serde_json::json!({
            "event": "corpus",
            "files": file_count,
            "bytes": total_bytes,
            "bigram_index_approx_bytes": bigram_bytes,
            "rss_before_bytes": rss_before,
            "rss_after_bytes": rss_after,
        })
    );

    for (label, q) in &queries {
        let terms = vec![(*q).to_string()];

        let base_samples = time_ms(iters, || {
            let _ = scan_baseline(&corpus, &keys, &terms, limit);
        });
        let mem_samples = time_ms(iters, || {
            let _ = scan_memmem(&corpus, &keys, &terms, limit);
        });
        let cold_fill = {
            let mut cold = LazyBigramIndex::new();
            let t0 = Instant::now();
            let _ = scan_bigram_memmem(&corpus, &keys, &terms, limit, &mut cold);
            t0.elapsed()
        };
        let big_samples = time_ms(iters, || {
            let _ = scan_bigram_memmem(&corpus, &keys, &terms, limit, &mut index);
        });

        let hit_count = scan_baseline(&corpus, &keys, &terms, limit).len();

        println!(
            "{}",
            serde_json::json!({
                "event": "query",
                "label": label,
                "query": q,
                "hit_count": hit_count,
                "iters": iters,
                "baseline_p50_ms": percentile(&base_samples, 50.0),
                "baseline_p95_ms": percentile(&base_samples, 95.0),
                "baseline_p99_ms": percentile(&base_samples, 99.0),
                "baseline_samples_ms": base_samples,
                "memmem_p50_ms": percentile(&mem_samples, 50.0),
                "memmem_p95_ms": percentile(&mem_samples, 95.0),
                "memmem_p99_ms": percentile(&mem_samples, 99.0),
                "memmem_samples_ms": mem_samples,
                "bigram_warm_p50_ms": percentile(&big_samples, 50.0),
                "bigram_warm_p95_ms": percentile(&big_samples, 95.0),
                "bigram_warm_p99_ms": percentile(&big_samples, 99.0),
                "bigram_warm_samples_ms": big_samples,
                "bigram_cold_fill_ms": cold_fill.as_secs_f64() * 1000.0,
            })
        );
    }

    // fszero-9ot: per-file from_bytes during read+AST extract (not bulk rebuild).
    let ingest = measure_ingest_bigram_cost(&corpus, &keys);
    println!(
        "{}",
        serde_json::json!({
            "event": "ingest_incremental",
            "files": ingest.files,
            "bytes": ingest.bytes,
            "baseline_ingest_ms": ingest.baseline_ingest_ms,
            "with_bigram_ingest_ms": ingest.with_bigram_ingest_ms,
            "from_bytes_wall_ms": ingest.from_bytes_wall_ms,
            "from_bytes_sum_ms": ingest.from_bytes_sum_ms,
            "from_bytes_p50_us": ingest.from_bytes_p50_us,
            "from_bytes_p95_us": ingest.from_bytes_p95_us,
            "from_bytes_p99_us": ingest.from_bytes_p99_us,
            "from_bytes_samples_us": ingest.from_bytes_samples_us,
            "cold_ingest_regress_pct": ingest.cold_ingest_regress_pct,
            "index_approx_bytes": ingest.index_approx_bytes,
        })
    );

    // fszero-kbo: watch-like create/modify/delete upsert cost + hit parity on warm index.
    let watch = measure_watch_upsert(&corpus, &mut keys, &mut index, &sorted);
    println!(
        "{}",
        serde_json::json!({
            "event": "watch_upsert",
            "k": watch.k,
            "create_upsert_p50_us": watch.create_upsert_p50_us,
            "create_upsert_p95_us": watch.create_upsert_p95_us,
            "create_upsert_p99_us": watch.create_upsert_p99_us,
            "create_upsert_samples_us": watch.create_upsert_samples_us,
            "modify_upsert_p50_us": watch.modify_upsert_p50_us,
            "modify_upsert_p95_us": watch.modify_upsert_p95_us,
            "modify_upsert_p99_us": watch.modify_upsert_p99_us,
            "modify_upsert_samples_us": watch.modify_upsert_samples_us,
            "delete_remove_p50_us": watch.delete_remove_p50_us,
            "delete_remove_p95_us": watch.delete_remove_p95_us,
            "delete_remove_p99_us": watch.delete_remove_p99_us,
            "delete_remove_samples_us": watch.delete_remove_samples_us,
            "parity_ok": watch.parity_ok,
            "create_hit_count": watch.create_hit_count,
            "modify_hit_count": watch.modify_hit_count,
            "deleted_absent_ok": watch.deleted_absent_ok,
        })
    );

    // Retained for comparison with fszero-9yq REJECT (bulk rebuild proxy).
    let t0 = Instant::now();
    for k in &sorted {
        let _ = fs::read(corpus.join(k));
    }
    let read_all = t0.elapsed();
    let t1 = Instant::now();
    let mut rebuild = LazyBigramIndex::new();
    for k in &sorted {
        if let Ok(bytes) = fs::read(corpus.join(k)) {
            rebuild.upsert(k, &bytes);
        }
    }
    let build_bigrams = t1.elapsed();
    println!(
        "{}",
        serde_json::json!({
            "event": "amortization_bulk_proxy",
            "note": "fszero-9yq REJECT proxy; not the fszero-9ot gate",
            "read_all_ms": dur_ms(read_all),
            "build_bigrams_ms": dur_ms(build_bigrams),
            "build_over_read_ratio": dur_ms(build_bigrams) / dur_ms(read_all).max(0.001),
        })
    );
}

struct WatchUpsertCost {
    k: usize,
    create_upsert_p50_us: f64,
    create_upsert_p95_us: f64,
    create_upsert_p99_us: f64,
    create_upsert_samples_us: Vec<f64>,
    modify_upsert_p50_us: f64,
    modify_upsert_p95_us: f64,
    modify_upsert_p99_us: f64,
    modify_upsert_samples_us: Vec<f64>,
    delete_remove_p50_us: f64,
    delete_remove_p95_us: f64,
    delete_remove_p99_us: f64,
    delete_remove_samples_us: Vec<f64>,
    parity_ok: bool,
    create_hit_count: usize,
    modify_hit_count: usize,
    deleted_absent_ok: bool,
}

fn measure_watch_upsert(
    corpus: &Path,
    keys: &mut HashSet<String>,
    index: &mut LazyBigramIndex,
    sorted: &[String],
) -> WatchUpsertCost {
    const K: usize = MIN_MEASURED_RUNS;
    let create_marker = "kbo_watch_create_marker_zz";
    let modify_marker = "kbo_watch_modify_marker_zz";

    let mut create_us = Vec::with_capacity(K);
    let mut created: Vec<String> = Vec::with_capacity(K);
    for i in 0..K {
        let rel = format!("__kbo_watch__/create_{i:03}.rs");
        let body = format!("// {create_marker}_{i}\npub fn kbo_create_{i}() {{}}\n");
        let t0 = Instant::now();
        apply_incremental(index, corpus, "create", &rel, Some(body.as_bytes()));
        create_us.push(t0.elapsed().as_secs_f64() * 1_000_000.0);
        keys.insert(rel.clone());
        created.push(rel);
    }
    let mut modify_us = Vec::with_capacity(K);
    let mut modified: Vec<String> = Vec::with_capacity(K);
    for i in 0..K {
        let rel = sorted[i % sorted.len()].clone();
        let body =
            format!("// {modify_marker}_{i}\npub fn kbo_modify_{i}() {{ wrapping_add(1); }}\n");
        let t0 = Instant::now();
        apply_incremental(index, corpus, "modify", &rel, Some(body.as_bytes()));
        modify_us.push(t0.elapsed().as_secs_f64() * 1_000_000.0);
        modified.push(rel);
    }
    let create_terms = vec![create_marker.to_string()];
    let modify_terms = vec![modify_marker.to_string()];
    let base_create = scan_baseline(corpus, keys, &create_terms, 64);
    let big_create = scan_bigram_memmem(corpus, keys, &create_terms, 64, index);
    let base_modify = scan_baseline(corpus, keys, &modify_terms, 64);
    let big_modify = scan_bigram_memmem(corpus, keys, &modify_terms, 64, index);
    let parity_ok = base_create == big_create && base_modify == big_modify;

    let mut delete_us = Vec::with_capacity(K);
    for rel in &created {
        let t0 = Instant::now();
        apply_incremental(index, corpus, "delete", rel, None);
        delete_us.push(t0.elapsed().as_secs_f64() * 1_000_000.0);
        keys.remove(rel);
    }
    let after_delete = scan_bigram_memmem(corpus, keys, &create_terms, 64, index);
    let deleted_absent_ok = after_delete.is_empty();

    // Restore modified files from disk is not required for the gate; corpus is ephemeral.
    let _ = modified;
    let create_percentiles = (
        percentile(&create_us, 50.0),
        percentile(&create_us, 95.0),
        percentile(&create_us, 99.0),
    );
    let modify_percentiles = (
        percentile(&modify_us, 50.0),
        percentile(&modify_us, 95.0),
        percentile(&modify_us, 99.0),
    );
    let delete_percentiles = (
        percentile(&delete_us, 50.0),
        percentile(&delete_us, 95.0),
        percentile(&delete_us, 99.0),
    );

    WatchUpsertCost {
        k: K,
        create_upsert_p50_us: create_percentiles.0,
        create_upsert_p95_us: create_percentiles.1,
        create_upsert_p99_us: create_percentiles.2,
        create_upsert_samples_us: create_us,
        modify_upsert_p50_us: modify_percentiles.0,
        modify_upsert_p95_us: modify_percentiles.1,
        modify_upsert_p99_us: modify_percentiles.2,
        modify_upsert_samples_us: modify_us,
        delete_remove_p50_us: delete_percentiles.0,
        delete_remove_p95_us: delete_percentiles.1,
        delete_remove_p99_us: delete_percentiles.2,
        delete_remove_samples_us: delete_us,
        parity_ok,
        create_hit_count: base_create.len(),
        modify_hit_count: base_modify.len(),
        deleted_absent_ok,
    }
}

fn dur_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
