use criterion::{Criterion, criterion_group, criterion_main};
use std::fs;
use std::hint::black_box;
use tempfile::tempdir;

use graphzero_reserve::schema::IntentOperation;
use graphzero_reserve::service::{DeclareRequest, ReserveService};

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let body: String = (0..500)
        .map(|i| format!("fn func_{i}(x: u64) -> u64 {{ x + {i} }}\n"))
        .collect();
    fs::write(repo.join("src/main.rs"), &body).unwrap();
    let store = repo.join(".graphzero");
    graphzero_store::store::indexer::index_repo(&repo, &store).unwrap();
    (dir, repo, store)
}

fn bench_declare(c: &mut Criterion) {
    let (_dir, repo, store) = setup();
    let svc = ReserveService::new(&store, &repo);
    let mut i = 0u64;
    c.bench_function("reserve_check_declare", |b| {
        b.iter(|| {
            i += 1;
            let req = DeclareRequest {
                agent_id: format!("agent_{i}"),
                intent_ops: vec![IntentOperation {
                    kind: "change_signature".into(),
                    target_symbol: Some("func_42".into()),
                    intent_text: None,
                }],
                ttl_seconds: 300,
            };
            black_box(svc.declare(req).unwrap())
        })
    });
}

fn bench_check_clear(c: &mut Criterion) {
    let (_dir, repo, store) = setup();
    let svc = ReserveService::new(&store, &repo);
    let ops = vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("func_100".into()),
        intent_text: None,
    }];
    c.bench_function("reserve_check_clear", |b| {
        b.iter(|| black_box(svc.check("bench_agent", &ops, false).unwrap()))
    });
}

fn bench_check_conflict(c: &mut Criterion) {
    let (_dir, repo, store) = setup();
    let svc = ReserveService::new(&store, &repo);
    let ops = vec![IntentOperation {
        kind: "change_signature".into(),
        target_symbol: Some("func_50".into()),
        intent_text: None,
    }];
    // Pre-acquire with different caller
    let _ = svc.check_with_ttl("blocker_agent", &ops, true, None);
    c.bench_function("reserve_check_conflict", |b| {
        b.iter(|| black_box(svc.check("victim_agent", &ops, false).unwrap()))
    });
}

criterion_group!(
    benches,
    bench_declare,
    bench_check_clear,
    bench_check_conflict
);
criterion_main!(benches);
