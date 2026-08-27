mod common;

use graphzero_store::store::indexer::index_repo;
use graphzero_store::store::query::QueryEngine;
use serde_json::Value;

fn query_json(store_root: &std::path::Path, repo_root: &std::path::Path, needle: &str) -> String {
    QueryEngine::cold(store_root, Some(repo_root), needle, 800)
        .unwrap()
        .to_json(Some(store_root))
}

#[test]
fn query_result_survives_unrelated_file_insertion() {
    let fx = common::indexed_repo_no_git();

    let before = query_json(&fx.store_root, &fx.repo_root, "alpha");
    assert!(before.contains("\"symbol\":\"alpha\""), "{before}");

    std::fs::write(
        fx.repo_root.join("src/noise.rs"),
        "fn unrelated_noise() {}\n",
    )
    .unwrap();
    index_repo(&fx.repo_root, &fx.store_root).unwrap();

    let after = query_json(&fx.store_root, &fx.repo_root, "alpha");
    assert!(after.contains("\"symbol\":\"alpha\""), "{after}");

    let budget_ref = QueryEngine::cold(&fx.store_root, Some(&fx.repo_root), "alpha", 1)
        .unwrap()
        .to_json(Some(&fx.store_root));
    assert!(budget_ref.contains("gz://query/"), "{budget_ref}");
}

#[test]
fn store_reindex_keeps_query_envelope_parseable_and_fresh_checked() {
    let fx = common::indexed_repo_no_git();

    let before: Value =
        serde_json::from_str(&query_json(&fx.store_root, &fx.repo_root, "alpha")).unwrap();
    std::fs::write(
        fx.repo_root.join("src/noise.rs"),
        "fn unrelated_noise() {}\n",
    )
    .unwrap();
    index_repo(&fx.repo_root, &fx.store_root).unwrap();
    let after: Value =
        serde_json::from_str(&query_json(&fx.store_root, &fx.repo_root, "alpha")).unwrap();

    assert_eq!(before["query"], "alpha");
    assert_eq!(after["query"], "alpha");
    assert!(before["matches"].is_array());
    assert!(after["matches"].is_array());
    assert!(after.to_string().contains("alpha"));
}

#[test]
fn metamorphic_corpus_covers_query_and_store_domains() {
    let corpus = include_str!("../../benchmarks/metamorphic/cases.jsonl");
    assert!(corpus.contains("\"domain\":\"query\""));
    assert!(corpus.contains("\"domain\":\"store\""));
    assert!(corpus.contains("\"expected_envelope\""));
}
