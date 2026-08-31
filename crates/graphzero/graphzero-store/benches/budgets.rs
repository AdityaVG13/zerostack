//! Criterion distributions for warm and cold `QueryEngine` calls.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::query::QueryEngine;

fn bench_budgets(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");

    let mut group = c.benchmark_group("budgets");
    group.bench_function("warm_query", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::warm(&snapshot, "func_42_3", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });
    group.bench_function("cold_query", |b| {
        b.iter(|| {
            std::hint::black_box(
                QueryEngine::cold(&fx.store_root, Some(&fx.repo_root), "func_42_3", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });
    group.bench_function("branch_switch", |b| {
        b.iter(|| {
            let snap = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
            std::hint::black_box(
                QueryEngine::warm(&snap, "func_7_1", 800)
                    .unwrap()
                    .matches
                    .len(),
            )
        })
    });
    group.finish();
}

criterion_group!(benches, bench_budgets);
criterion_main!(benches);
