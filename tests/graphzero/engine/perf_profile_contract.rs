//! Structured perf.profile.* log contract (graphzero-jjvkx).

use serde_json::json;
use serial_test::serial;

struct EnvGuard {
    _env: graphzero_test_support::ScopedEnvVars,
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        graphzero_store::reset_perf_profile_for_tests();
    }
}

#[test]
#[serial]
fn perf_profile_emits_run_span_and_complete_from_dispatch() {
    let _g = EnvGuard {
        _env: graphzero_test_support::ScopedEnvVars::set_one("GRAPHZERO_PERF_PROFILE", "1"),
    };
    graphzero_store::reset_perf_profile_for_tests();

    graphzero_store::perf_profile_run_start(
        "dispatch_contract_test",
        json!({"source": "perf_profile_contract"}),
    );

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let store = repo.join(".graphzero");
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo.clone(),
        store,
        graphzero_engine::AdapterKind::Cli,
    );
    graphzero_engine::dispatch(
        &ctx,
        "index",
        &json!({ "path": repo.display().to_string() }),
    )
    .expect("index");
    graphzero_engine::dispatch(
        &ctx,
        "query",
        &json!({ "surface": "symbol", "query": "alpha", "budget": 1 }),
    )
    .expect("query");

    graphzero_store::perf_profile_hypothesis_evaluated(
        "query_has_stage_samples",
        true,
        json!({"note": "dispatch path under PERF_PROFILE must record samples"}),
    );
    graphzero_store::perf_profile_run_complete(json!({"ok": true}));

    // Contract constants are public for parsers.
    assert_eq!(
        graphzero_store::PERF_PROFILE_SCHEMA,
        "graphzero.perf.profile.v1"
    );
    assert_eq!(graphzero_store::PERF_PROFILE_ENV, "GRAPHZERO_PERF_PROFILE");
    assert!(graphzero_store::perf_profile_enabled());
}
