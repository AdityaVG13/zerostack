use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tempfile::tempdir;
use tokenzero_recovery::{
    RecoveryStore,
    context_view::{ContextView, ContextViewConfig},
};

fn replay_context_view(c: &mut Criterion) {
    const TURNS: u64 = 1_000;
    const W: usize = 512;
    let dir = tempdir().unwrap();
    let mut store = RecoveryStore::new(Some(dir.path().join("recovery-cache.json")));
    let mut view = ContextView::new(
        "SYSTEM tools manifest=v1\n",
        ContextViewConfig {
            working_set_tokens: W,
            hot_tail_tokens: W / 2,
        },
    )
    .expect("benchmark ContextView config is valid");
    let mut max_input_tokens = 0;
    let mut stable_prefix = None;
    for turn in 1..=TURNS {
        view.append(
            &mut store,
            turn,
            turn * 1_000,
            format!("turn-{turn} {}", "payload ".repeat(64)),
        )
        .unwrap();
        let projection = if turn % 100 == 0 {
            view.reproject_at_cache_breakpoint(None)
        } else {
            view.project(None)
        };
        max_input_tokens = max_input_tokens.max(projection.input_tokens);
        assert!(projection.working_set_tokens <= W);
        assert_eq!(
            stable_prefix.get_or_insert_with(|| projection.stable_prefix_sha256.clone()),
            &projection.stable_prefix_sha256
        );
    }
    eprintln!(
        "context-view replay: turns={TURNS} W={W} max_input_tokens={max_input_tokens} stable_prefix=true"
    );
    c.bench_function("context_view_project_1k_as_of", |b| {
        b.iter(|| {
            black_box(view.project(Some(tokenzero_recovery::context_view::AsOf::Turn(5_000))))
        })
    });
}

criterion_group!(benches, replay_context_view);
criterion_main!(benches);
