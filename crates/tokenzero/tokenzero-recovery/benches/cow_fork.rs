use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use tempfile::{TempDir, tempdir};
use tokenzero_core::{ContentType, sha256_hex};
use tokenzero_recovery::{RecoveryStore, context_view::ContextProjection, cow_fork::CowSession};

fn breakpoint(rendered: String) -> ContextProjection {
    ContextProjection {
        stable_prefix_sha256: sha256_hex(&rendered),
        stable_prefix: rendered.clone(),
        rendered,
        stable_prefix_tokens: 0,
        input_tokens: 0,
        working_set_tokens: 0,
        working_set_ids: Vec::new(),
        hot_tail_ids: Vec::new(),
        evicted_ids: Vec::new(),
        as_of: None,
        cache_breakpoint: true,
    }
}

fn fresh_store() -> (TempDir, RecoveryStore) {
    let dir = tempdir().unwrap();
    let store = RecoveryStore::new(Some(dir.path().join("recovery.json")));
    (dir, store)
}

fn fork_cost_vs_full_replay(c: &mut Criterion) {
    let corpus = (0..1_000)
        .map(|turn| {
            format!(
                "turn-{turn}: {}
",
                "cached-prefix ".repeat(32)
            )
        })
        .collect::<String>();
    let novelty = "branch novelty
";
    let (_measurement_dir, mut measurement_store) = fresh_store();
    let parent = CowSession::from_breakpoint("root", &breakpoint(corpus.clone())).unwrap();
    let mut measured = parent.fork(&mut measurement_store, "measured").unwrap();
    measured.append(&mut measurement_store, novelty).unwrap();
    let cost = measured.cost();
    assert_eq!(cost.novelty_bytes, novelty.len());
    assert!(cost.novelty_bytes * 1_000 < cost.full_replay_bytes);
    eprintln!(
        "cow-fork corpus: shared={} novelty={} full_replay={} novelty_ratio={:.6}",
        cost.shared_prefix_bytes,
        cost.novelty_bytes,
        cost.full_replay_bytes,
        cost.novelty_bytes as f64 / cost.full_replay_bytes as f64,
    );

    c.bench_function("cow_fork_staged_ledger_plus_novelty", |b| {
        b.iter_batched(
            || {
                let (dir, store) = fresh_store();
                let parent =
                    CowSession::from_breakpoint("root", &breakpoint(corpus.clone())).unwrap();
                (dir, store, parent)
            },
            |(_dir, mut store, parent)| {
                let mut branch = parent.fork(&mut store, "branch").unwrap();
                branch.append(&mut store, novelty).unwrap();
                black_box(branch)
            },
            BatchSize::SmallInput,
        )
    });
    c.bench_function("full_replay_durable_materialization", |b| {
        b.iter_batched(
            || {
                let (dir, store) = fresh_store();
                let mut replay = corpus.clone();
                replay.push_str(novelty);
                (dir, store, replay)
            },
            |(_dir, mut store, replay)| {
                black_box(store.store_blob(&replay, ContentType::Unknown).unwrap())
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, fork_cost_vs_full_replay);
criterion_main!(benches);
