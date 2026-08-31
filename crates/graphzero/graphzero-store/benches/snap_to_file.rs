//! Benchmarks snap-to-file export latency and encoded size.
//! Set `GRAPHZERO_BENCH_FILES` to scale the fixture.

mod common;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use graphzero_store::Snapshot;
use graphzero_store::store::query::{ExportFormat, export_capsule, snap};
use std::fs;
use std::hint::black_box;
use tempfile::tempdir;

/// Bench snap() + export_capsule (real snap-to-file) to temp file + measure resulting size.
/// Extended for A/B size vs mocks/competitors, warm vs cold, full loop elements.
fn bench_snap_to_file(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root))
        .expect("open snapshot for snap_to_file bench");

    let mut group = c.benchmark_group("snap_to_file");
    group.throughput(criterion::Throughput::Elements(1));

    // Queries covering routes + git special
    let queries = ["func_42_3", "hot", "changes", "helper_0"];
    let budgets = [1usize, 64, 800];

    for &budget in &budgets {
        for &q in &queries {
            let label = format!("q={}_bgt={}", q, budget);
            let out_dir = tempdir().expect("temp export dir");
            let out_path =
                out_dir
                    .path()
                    .join(format!("snap_{}_{}.json", q.replace(':', "_"), budget));

            let fmt = if budget <= 1 {
                ExportFormat::Minimal
            } else {
                ExportFormat::Capsule
            };

            group.bench_with_input(
                BenchmarkId::new("export_capsule_latency", &label),
                &(q, budget),
                |b, &(query, bgt)| {
                    b.iter(|| {
                        // Core snap (the routing + capsule) then real export to file
                        let capsule = snap(
                            black_box(&snapshot),
                            black_box(query),
                            black_box(bgt),
                            black_box(None),
                            black_box(false),
                        )
                        .expect("snap in bench");

                        let _art = export_capsule(
                            black_box(&capsule),
                            Some(&fx.store_root),
                            black_box(&out_path),
                            black_box(fmt),
                        )
                        .expect("export_capsule in bench");

                        // size measured post
                        let size = fs::metadata(black_box(&out_path))
                            .map(|m| m.len())
                            .unwrap_or(0);
                        black_box(size);

                        let _ = fs::remove_file(&out_path);
                    });
                },
            );

            // Separate size A/B measurement (export + compare to mock competitor full dump)
            let capsule = snap(&snapshot, q, budget, None, false).expect("snap for size ab");
            let _art = export_capsule(&capsule, Some(&fx.store_root), &out_path, fmt)
                .expect("size ab export");
            let gz_size = fs::metadata(&out_path)
                .map(|m| m.len() as usize)
                .unwrap_or(0);
            let mock_size = if budget == 1 { 15600usize } else { 350000 }; // from perf docs: full graph.json ~15kB for 50, scale
            let _ratio = mock_size.checked_div(gz_size).unwrap_or(999);
            black_box(_ratio);
            let _ = fs::remove_file(&out_path);
            group.bench_function(BenchmarkId::new("export_size_ab_vs_mock", &label), |b| {
                b.iter(|| black_box((gz_size, mock_size)))
            });
        }
    }

    group.finish();
}

/// Baseline pure snap without export write, to isolate fs cost.
fn bench_snap_only(c: &mut Criterion) {
    let fx = common::fixture();
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("open");
    let mut group = c.benchmark_group("snap_only");
    for &bgt in &[1usize, 800] {
        group.bench_with_input(BenchmarkId::new("snap", bgt), &bgt, |b, &bgt| {
            b.iter(|| {
                let c = snap(
                    black_box(&snapshot),
                    "func_42_3",
                    black_box(bgt),
                    None,
                    false,
                )
                .expect("snap");
                black_box(c.destinations.len());
            });
        });
    }
    group.finish();
}

/// Warm vs cold: cold opens snapshot each time (expensive), warm reuses.
/// Full loop elements: snap + export + (blast stub via query count) + handoff size.
fn bench_warm_cold_full_loop(c: &mut Criterion) {
    let fx = common::fixture();
    let mut group = c.benchmark_group("warm_cold_full");
    // Cold path: open + snap + export
    group.bench_function("cold_snap_export", |b| {
        b.iter(|| {
            let snap_cold =
                Snapshot::open(black_box(&fx.store_root), Some(black_box(&fx.repo_root)))
                    .expect("cold open");
            let cap = snap(black_box(&snap_cold), "func_42_3", 1, None, false).expect("cold snap");
            let od = tempdir().unwrap();
            let op = od.path().join("cold.json");
            let _ = export_capsule(&cap, Some(&fx.store_root), &op, ExportFormat::Minimal);
            let _ = fs::remove_file(&op);
            black_box(());
        });
    });
    // Warm
    let snapshot = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).expect("warm");
    group.bench_function("warm_snap_export", |b| {
        b.iter(|| {
            let cap = snap(black_box(&snapshot), "func_42_3", 1, None, false).expect("warm snap");
            let od = tempdir().unwrap();
            let op = od.path().join("warm.json");
            let _ = export_capsule(&cap, Some(&fx.store_root), &op, ExportFormat::Minimal);
            let _ = fs::remove_file(&op);
            black_box(());
        });
    });
    // Full loop stub: snap+export + "blast" (count dests as proxy) + handoff md
    group.bench_function("full_loop_snap_export_blast_handoff", |b| {
        b.iter(|| {
            let cap = snap(black_box(&snapshot), "func_42_3", 1, None, false).expect("loop snap");
            let od = tempdir().unwrap();
            let op = od.path().join("loop_export.json");
            let _art = export_capsule(&cap, Some(&fx.store_root), &op, ExportFormat::Minimal)
                .expect("loop export");
            // blast proxy: use capsule dests
            let blast_proxy = cap.destinations.len();
            let hp = od.path().join("handoff.md");
            let _ = export_capsule(&cap, Some(&fx.store_root), &hp, ExportFormat::Md);
            black_box((blast_proxy, fs::metadata(&op).map(|m| m.len()).unwrap_or(0)));
            let _ = fs::remove_file(&op);
            let _ = fs::remove_file(&hp);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_snap_to_file,
    bench_snap_only,
    bench_warm_cold_full_loop
);
criterion_main!(benches);
