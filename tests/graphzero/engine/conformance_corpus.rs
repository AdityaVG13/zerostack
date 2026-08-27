//! Full differential conformance corpus (graphzero-o2uq.7).

use graphzero_engine::conformance::{
    ConformanceVector, corpus_covers_registry_ops, deliberate_adapter_semantic_mutation,
    generate_corpus, mutation_vector_store_markers_agree, run_corpus_differential,
    run_differential,
};
use graphzero_store::store::indexer;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("src/lib.rs"),
        "fn alpha(x: u64) -> u64 { beta(x) }\nfn beta(v: u64) -> u64 { v }\n",
    )
    .unwrap();
    let store = repo.join(".graphzero");
    indexer::index_repo(&repo, &store).unwrap();
    (dir, repo, store)
}

#[test]
fn corpus_covers_every_registry_op_classes() {
    let corpus = generate_corpus();
    corpus_covers_registry_ops(&corpus).expect("coverage");
    // Versioned + digest pinned.
    assert!(!corpus.corpus_version.is_empty());
    assert_eq!(corpus.semantic_contract_digest.len(), 64);
}

#[test]
fn differential_all_vectors_agree_across_surfaces() {
    let (_d, repo, store) = fixture();
    let reports = run_corpus_differential(repo, store);
    assert!(!reports.is_empty());
    let mut disagreements = Vec::new();
    for r in &reports {
        if !r.agree {
            disagreements.push(format!(
                "{} op={} class={} fm={:?} cm={:?} pw={:?}",
                r.vector_id, r.op, r.class, r.fastmcp, r.codemode, r.private_worker
            ));
        }
        if r.op == "remember" || r.op == "index" || r.vector_id.contains("mutation") {
            // Mutation vectors must compare final store fingerprints.
            if r.store_fp_fastmcp.is_some() {
                assert!(
                    mutation_vector_store_markers_agree(r),
                    "store fingerprints must match for mutation vector {}: fm={:?} cm={:?} pw={:?}",
                    r.vector_id,
                    r.store_fp_fastmcp,
                    r.store_fp_codemode,
                    r.store_fp_private_worker
                );
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "surface disagreements:\n{}",
        disagreements.join("\n")
    );
}

#[test]
fn deliberate_adapter_mutation_detected() {
    let (_d, repo, store) = fixture();
    let v = ConformanceVector {
        id: "kill".into(),
        op: "search".into(),
        class: "positive".into(),
        args: json!({"query": "alpha", "budget": 1}),
        mutation: false,
    };
    let report = run_differential(repo, store, &v);
    assert!(report.agree, "baseline must agree: {:?}", report);
    let killed = deliberate_adapter_semantic_mutation(report);
    assert!(!killed.agree, "deliberate adapter drift must fail");
}

#[test]
fn preflight_cancel_and_deadline_agree() {
    let (_d, repo, store) = fixture();
    let corpus = generate_corpus();
    for id in ["preflight_cancelled_search", "preflight_deadline_search"] {
        let v = corpus.vectors.iter().find(|v| v.id == id).expect(id);
        let r = run_differential(repo.clone(), store.clone(), v);
        assert!(r.agree, "{id} must agree: {:?}", r);
    }
}
