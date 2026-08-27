//! FR-008 / NFR-001: warm query path benchmark. Budget: p99 < 1ms.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::query::QueryEngine;

fn bench_query_warm(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");
    c.bench_function("query_warm", |b| {
        b.iter(|| {
            let capsule =
                QueryEngine::warm(&snapshot, std::hint::black_box("func_42_3"), 800).unwrap();
            std::hint::black_box(capsule.matches.len())
        })
    });
}

criterion_group!(benches, bench_query_warm);
criterion_main!(benches);
