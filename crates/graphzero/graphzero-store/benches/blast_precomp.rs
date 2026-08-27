//! Blast scaling bench: measures blast_radius via cached Snapshot indexes.
//! Snapshot caches the blast-filtered ReverseIndex and silent-risk scan;
//! this bench exercises the cached path, not a per-blast rebuild.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use graphzero_engine::blast::blast_radius;
use graphzero_store::Snapshot;
use std::hint::black_box;
mod common;

fn bench_blast_current(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open");
    let mut group = c.benchmark_group("blast_current");
    for intent in ["change_signature of func_42_3", "refactor helper_5"] {
        group.bench_with_input(BenchmarkId::new("blast", intent), intent, |b, intent| {
            b.iter(|| {
                let cap =
                    blast_radius(black_box(&snapshot), black_box(intent), 800).expect("blast");
                black_box(cap.break_sites.len() + cap.covering_tests.len());
            });
        });
    }
    group.finish();
}

/// Multi-size blast sweep: primary hot path across documented N set
/// (`BENCH_FILE_SWEEP`) so skill scaling law is derivable from criterion
/// alone (graphzero-ijf2c). Intent stays fixed; `BenchmarkId` is file count.
fn bench_blast_by_file_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("blast_by_files");
    // Keep sample count modest: each N indexes a fresh synthetic repo once.
    group.sample_size(20);
    for &n in common::BENCH_FILE_SWEEP {
        let fx = common::fixture_n(n);
        let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open");
        // Pick a symbol that exists at every N (file_0 always present).
        let intent = "change_signature of func_0_0";
        group.bench_with_input(BenchmarkId::new("files", n), &n, |b, _| {
            b.iter(|| {
                let cap =
                    blast_radius(black_box(&snapshot), black_box(intent), 800).expect("blast");
                black_box(cap.break_sites.len() + cap.covering_tests.len());
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_blast_current, bench_blast_by_file_count);
criterion_main!(benches);
