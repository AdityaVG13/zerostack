//! NFR-004: branch switch budget (<100ms). Same blobs = same shards, so a
//! branch switch is re-opening the snapshot and answering a query.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::query::QueryEngine;

fn bench_branch_switch(c: &mut Criterion) {
    let fx = common::fixture();
    c.bench_function("branch_switch", |b| {
        b.iter(|| {
            // Re-point: open the published snapshot fresh and answer.
            let snapshot =
                Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open snapshot");
            let capsule = QueryEngine::warm(&snapshot, "func_7_1", 800).unwrap();
            std::hint::black_box(capsule.matches.len())
        })
    });
}

criterion_group!(benches, bench_branch_switch);
criterion_main!(benches);
