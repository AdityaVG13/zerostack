//! Cold-path decomposition: manifest load, shard open, WAL scan.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::manifest::Manifest;
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::shard::ShardReader;

fn bench_cold_breakdown(c: &mut Criterion) {
    let fx = common::fixture();
    let mut group = c.benchmark_group("cold_breakdown");

    group.bench_function("cold_no_freshness", |b| {
        b.iter(|| {
            let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
            std::hint::black_box(
                snapshot
                    .query("func_42_3", 800, false)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });

    group.bench_function("manifest_load", |b| {
        b.iter(|| std::hint::black_box(Manifest::load(&fx.store_root).unwrap()))
    });

    group.bench_function("shard_open", |b| {
        let manifest = Manifest::load(&fx.store_root).unwrap();
        let entry = manifest.latest().unwrap();
        let path =
            fx.store_root
                .join("shards")
                .join(graphzero_store::store::indexer::global_file_name(
                    entry.snapshot_id,
                ));
        b.iter(|| std::hint::black_box(ShardReader::open(&path).unwrap()))
    });

    group.bench_function("snapshot_open", |b| {
        b.iter(|| {
            std::hint::black_box(Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap())
        })
    });

    group.bench_function("cold_query_full", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::cold(&fx.store_root, Some(&fx.repo_root), "func_42_3", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });

    group.finish();
}

criterion_group!(benches, bench_cold_breakdown);
criterion_main!(benches);
