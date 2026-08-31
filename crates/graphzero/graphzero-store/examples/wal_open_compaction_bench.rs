use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use graphzero_store::Snapshot;
use graphzero_store::store::compaction::{MAX_SEGMENTS, append_entries, stats};
use graphzero_store::store::delta_log::{DeltaEntry, entry_type};
use graphzero_store::store::indexer::index_repo;
use serde_json::json;

// Public claim-eligible benches need ≥20 measured samples (see scripts/perf hyperfine floors).
const TRIALS: usize = 20;
const ENTRIES_PER_SEGMENT: usize = 256;

fn rust_file_count(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return true;
            };
            !matches!(name, "target" | ".git" | ".graphzero" | ".zerostack")
                && !name.starts_with(".rch-")
        })
        .map(|path| {
            if path.is_dir() {
                rust_file_count(&path)
            } else {
                usize::from(path.extension().is_some_and(|ext| ext == "rs"))
            }
        })
        .sum()
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            std::fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn enforce_gate(result: &serde_json::Value) -> Result<()> {
    let gate: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../benchmarks/structure/latency/wal_open_compaction_gate.json"
    ))?;
    ensure!(
        gate["schema_version"] == 1,
        "unsupported WAL open gate schema"
    );
    ensure!(
        gate["profile"] == "release",
        "WAL open gate must bind release profile"
    );
    ensure!(
        result["workload"] == gate["scenario"]["inputs"],
        "WAL open workload does not match frozen scenario"
    );
    ensure!(
        result["corpus"]["name"] == gate["scenario"]["corpus"]["name"],
        "WAL open corpus name does not match frozen scenario"
    );
    let rust_files = result["corpus"]["rust_files"].as_u64().unwrap_or(0);
    let min_rust_files = gate["scenario"]["corpus"]["min_rust_files"]
        .as_u64()
        .unwrap_or(u64::MAX);
    ensure!(
        rust_files >= min_rust_files,
        "WAL open corpus has {rust_files} Rust files; needs {min_rust_files}"
    );
    ensure!(
        result["before"]["wal_segment_count"]
            == gate["scenario"]["expected_output"]["before.wal_segment_count"],
        "WAL replay open did not preserve the expected segment count"
    );
    ensure!(
        result["after"]["wal_segment_count"]
            == gate["scenario"]["expected_output"]["after.wal_segment_count"],
        "compacting open did not reduce the WAL to the expected segment count"
    );
    for (observed, limit, label) in [
        (
            result["before"]["wal_replay_open"]["p50_ms"].as_f64(),
            gate["success_metrics"]["before.wal_replay_open.p50_max_ms"].as_f64(),
            "WAL replay open p50",
        ),
        (
            result["before"]["wal_replay_open"]["p95_ms"].as_f64(),
            gate["success_metrics"]["before.wal_replay_open.p95_max_ms"].as_f64(),
            "WAL replay open p95",
        ),
        (
            result["after"]["post_compaction_cached_open"]["p50_ms"].as_f64(),
            gate["success_metrics"]["after.post_compaction_cached_open.p50_max_ms"].as_f64(),
            "post-compaction cached open p50",
        ),
        (
            result["after"]["post_compaction_cached_open"]["p95_ms"].as_f64(),
            gate["success_metrics"]["after.post_compaction_cached_open.p95_max_ms"].as_f64(),
            "post-compaction cached open p95",
        ),
    ] {
        let observed = observed.context("missing WAL open metric")?;
        let limit = limit.context("missing WAL open threshold")?;
        ensure!(
            observed <= limit,
            "{label} {observed:.3}ms exceeds {limit:.3}ms"
        );
    }
    Ok(())
}

fn append_realistic_wal(store: &Path) -> Result<()> {
    let mut ordinal = 0_u64;
    for _ in 0..=MAX_SEGMENTS {
        let mut entries = Vec::with_capacity(ENTRIES_PER_SEGMENT);
        for _ in 0..ENTRIES_PER_SEGMENT {
            let mut blob_hash = [0_u8; 32];
            blob_hash[..8].copy_from_slice(&ordinal.to_le_bytes());
            entries.push(DeltaEntry {
                entry_type: entry_type::COVERAGE,
                blob_hash,
                payload: vec![0b001],
            });
            ordinal += 1;
        }
        append_entries(store, entries)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let repo = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: wal_open_compaction_bench <repo>")?;
    let temp = tempfile::tempdir()?;
    let base_store = temp.path().join("base-store");
    index_repo(&repo, &base_store)?;

    let mut replay_open_ms = Vec::with_capacity(TRIALS);
    let mut compacting_open_ms = Vec::with_capacity(TRIALS);
    let mut warm_open_ms = Vec::with_capacity(TRIALS);
    let mut before_segments = 0;
    let mut after_segments = 0;

    for trial in 0..TRIALS {
        let store = temp.path().join(format!("trial-{trial}"));
        copy_tree(&base_store, &store)?;
        append_realistic_wal(&store)?;
        before_segments = stats(&store)?.segment_count;

        let started = Instant::now();
        drop(Snapshot::open(&store, Some(&repo))?);
        replay_open_ms.push(elapsed_ms(started));

        Snapshot::clear_open_cache();
        let started = Instant::now();
        drop(Snapshot::open_cached(&store, Some(&repo))?);
        compacting_open_ms.push(elapsed_ms(started));
        after_segments = stats(&store)?.segment_count;

        let started = Instant::now();
        drop(Snapshot::open_cached(&store, Some(&repo))?);
        warm_open_ms.push(elapsed_ms(started));
    }

    let distribution = |values: &[f64]| {
        json!({
            "trials": values,
            "p50_ms": percentile(values, 0.50),
            "p95_ms": percentile(values, 0.95),
        })
    };
    let result = json!({
        "schema": "graphzero.wal-open-compaction-benchmark",
        "corpus": {
            "name": "graphzero-self-repo",
            "path": repo.display().to_string(),
            "rust_files": rust_file_count(&repo),
        },
        "workload": {
            "trials": TRIALS,
            "wal_segments": MAX_SEGMENTS + 1,
            "entries_per_segment": ENTRIES_PER_SEGMENT,
        },
        "before": {
            "wal_segment_count": before_segments,
            "wal_replay_open": distribution(&replay_open_ms),
        },
        "after": {
            "wal_segment_count": after_segments,
            "compacting_open": distribution(&compacting_open_ms),
            "post_compaction_cached_open": distribution(&warm_open_ms),
        }
    });
    enforce_gate(&result)?;
    println!(
        "WAL_OPEN_COMPACTION_RESULT={}",
        serde_json::to_string(&result)?
    );
    Ok(())
}
