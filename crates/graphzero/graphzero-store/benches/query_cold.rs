//! Cold query-path benchmark covering open, freshness, and query.
//! Budget: p99 < 10ms.

mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_store::store::query::QueryEngine;

fn bench_query_cold(c: &mut Criterion) {
    let fx = common::fixture();
    c.bench_function("query_cold", |b| {
        b.iter(|| {
            let capsule = QueryEngine::cold(
                &fx.store_root,
                Some(&fx.repo_root),
                std::hint::black_box("func_42_3"),
                800,
            )
            .unwrap();
            std::hint::black_box(capsule.matches.len())
        })
    });
}

criterion_group!(benches, bench_query_cold);
criterion_main!(benches);
