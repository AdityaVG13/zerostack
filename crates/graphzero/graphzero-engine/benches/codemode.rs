use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use graphzero_engine::codemode::execute;
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_store::Snapshot;
use graphzero_store::store::indexer;
use serde_json::json;
use std::fs;
use std::hint::black_box;
use std::time::Duration;
use tempfile::tempdir;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let body: String = (0..250)
        .map(|i| {
            format!(
                "fn func_{i}_x() -> u64 {{ helper_{i}_x() }}\nfn helper_{i}_x() -> u64 {{ {i} }}\n"
            )
        })
        .collect();
    fs::write(repo.join("src/lib.rs"), body).unwrap();
    let store = repo.join(".graphzero");
    indexer::index_repo(&repo, &store).unwrap();
    (dir, repo, store)
}

fn bench_per_op_multi_query_equivalent(c: &mut Criterion) {
    let (_dir, repo, store) = fixture();
    let snapshot = Snapshot::open(&store, Some(&repo)).unwrap();
    let targets: Vec<String> = (0..100).map(|_| "func_42_x".to_string()).collect();
    c.bench_function("codemode_per_op_100_queries", |b| {
        b.iter(|| {
            for target in &targets {
                let req = QuerySurfaceRequest {
                    surface: "search".into(),
                    query: Some(target.clone()),
                    budget: Some(1),
                    ..Default::default()
                };
                QuerySurfaceRouter::execute(&snapshot, &req).unwrap();
            }
        });
    });
    let plan = json!({"steps":[{"id":"m","op":"multiQuery","surface":"search","targets":targets}]})
        .to_string();
    c.bench_function("codemode_multi_query_100_logical", |b| {
        b.iter(|| {
            let out = execute(&snapshot, &plan);
            assert_eq!(out.ack, "C");
            assert!(out.telemetry.physical_ops <= 10 + out.telemetry.store_writes);
        });
    });
}

fn padded_json_plan(size: usize) -> String {
    const PREFIX: &str = r#"{"steps":[],"padding":""#;
    const SUFFIX: &str = r#""}"#;
    assert!(size >= PREFIX.len() + SUFFIX.len());
    format!(
        "{PREFIX}{}{SUFFIX}",
        "x".repeat(size - PREFIX.len() - SUFFIX.len())
    )
}

fn bench_plan_input_sizes(c: &mut Criterion) {
    let (_dir, repo, store) = fixture();
    let snapshot = Snapshot::open(&store, Some(&repo)).unwrap();
    let mut group = c.benchmark_group("codemode_plan_input");
    // Public claim-eligible benches need ≥20 measured samples.
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(250));
    group.measurement_time(Duration::from_millis(500));

    // The 10 KiB JSON case exercises plan deserialization. Larger inline
    // JSON plans exercise bounded rejection while retaining exact code refs.
    for (label, size) in [
        ("10KiB", 10 * 1024),
        ("100KiB", 100 * 1024),
        ("1MiB", 1024 * 1024),
    ] {
        let plan = padded_json_plan(size);
        assert_eq!(plan.len(), size);
        let expected_ack = if label == "10KiB" { "C" } else { "X0" };
        assert_eq!(execute(&snapshot, &plan).ack, expected_ack);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &plan, |b, plan| {
            b.iter(|| black_box(execute(&snapshot, black_box(plan.as_str()))));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_per_op_multi_query_equivalent,
    bench_plan_input_sizes
);
criterion_main!(benches);
