//! Micro-benchmark: decompose query_warm into its constituent costs.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::csr::CsrAdjacency;
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::symbol_table::SymbolTable;

fn bench_breakdown(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");

    let mut group = c.benchmark_group("warm_breakdown");

    group.bench_function("view_parse", |b| {
        b.iter(|| {
            let view = snapshot.global_view().unwrap();
            std::hint::black_box(&view);
        })
    });

    group.bench_function("symbol_lookup", |b| {
        let view = snapshot.global_view().unwrap();
        let table = SymbolTable::from_view(&view).unwrap();
        b.iter(|| std::hint::black_box(table.get("func_42_3")))
    });

    group.bench_function("full_warm_query", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::warm(&snapshot, "func_42_3", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });

    group.bench_function("view_plus_table", |b| {
        b.iter(|| {
            let view = snapshot.global_view().unwrap();
            let table = SymbolTable::from_view(&view).unwrap();
            std::hint::black_box(table.get("func_42_3"))
        })
    });

    group.bench_function("view_all_sections", |b| {
        b.iter(|| {
            let view = snapshot.global_view().unwrap();
            let _t = SymbolTable::from_view(&view).unwrap();
            let _s = view.spans().unwrap();
            let _c = CsrAdjacency::new(view.edges().unwrap());
            let _e = view.edge_evidence().unwrap();
            let _cv = view.coverage().unwrap();
            std::hint::black_box(())
        })
    });

    group.bench_function("tier_counts_only", |b| {
        let view = snapshot.global_view().unwrap();
        let cov = view.coverage().unwrap();
        b.iter(|| {
            std::hint::black_box(graphzero_store::CoverageBitmap::tier_counts_packed(
                cov.bits,
                cov.blob_hashes.len(),
            ))
        })
    });

    group.bench_function("warm_query_no_freshness", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::warm(&snapshot, "func_42_3", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });

    group.bench_function("warm_miss_prefix", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::warm(&snapshot, "nonexistent_xyz", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });

    group.finish();
}

criterion_group!(benches, bench_breakdown);
criterion_main!(benches);
