use criterion::{Criterion, criterion_group, criterion_main};
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_store::Snapshot;
use graphzero_store::store::indexer;
use std::fs;
use tempfile::tempdir;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let body: String = (0..2000)
        .map(|i| format!("fn func_{i}_x() {{}}\n"))
        .collect();
    fs::write(repo.join("src/big.rs"), body).unwrap();
    let store = repo.join(".graphzero");
    indexer::index_repo(&repo, &store).unwrap();
    (dir, repo, store)
}

fn bench_symbol(c: &mut Criterion) {
    let (_dir, repo, store) = fixture();
    let snapshot = Snapshot::open(&store, Some(&repo)).unwrap();
    c.bench_function("query_surface_symbol_warm", |b| {
        b.iter(|| {
            let req = QuerySurfaceRequest {
                surface: "symbol".into(),
                name: Some("func_42_x".into()),
                budget: Some(800),
                ..Default::default()
            };
            QuerySurfaceRouter::execute(&snapshot, &req).unwrap();
        });
    });
}

fn bench_search_scan_vs_bigram(c: &mut Criterion) {
    let (_dir, repo, store) = fixture();
    let snapshot = Snapshot::open(&store, Some(&repo)).unwrap();
    let _ = snapshot.name_bigram_index().unwrap();
    let mut env = zerostack_test_support::ScopedEnvVars::new();

    let mut group = c.benchmark_group("query_surface_search");
    group.bench_function("scan", |b| {
        env.remove("GRAPHZERO_SEARCH_BIGRAM");
        b.iter(|| {
            let req = QuerySurfaceRequest {
                surface: "search".into(),
                query: Some("func_42".into()),
                budget: Some(80),
                ..Default::default()
            };
            QuerySurfaceRouter::execute(&snapshot, &req).unwrap();
        });
    });
    group.bench_function("bigram", |b| {
        env.set("GRAPHZERO_SEARCH_BIGRAM", "1");
        b.iter(|| {
            let req = QuerySurfaceRequest {
                surface: "search".into(),
                query: Some("func_42".into()),
                budget: Some(80),
                ..Default::default()
            };
            QuerySurfaceRouter::execute(&snapshot, &req).unwrap();
        });
    });
    group.finish();
}

criterion_group!(benches, bench_symbol, bench_search_scan_vs_bigram);
criterion_main!(benches);
