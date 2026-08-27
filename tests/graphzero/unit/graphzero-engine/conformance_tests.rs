use super::*;

#[test]
fn corpus_versioned_and_covers_ops() {
    let corpus = generate_corpus();
    assert_eq!(corpus.corpus_version, CONFORMANCE_CORPUS_VERSION);
    assert_eq!(corpus.semantic_contract_digest.len(), 64);
    assert!(corpus.vectors.len() > 20);
    corpus_covers_registry_ops(&corpus).expect("coverage");
    // Machine-readable.
    let raw = serde_json::to_string(&corpus).unwrap();
    assert!(raw.contains("corpus_version"));
    let back: ConformanceCorpus = serde_json::from_str(&raw).unwrap();
    assert_eq!(back.vectors.len(), corpus.vectors.len());
}

#[test]
fn deliberate_mutation_breaks_agreement() {
    let report = DifferentialReport {
        vector_id: "t".into(),
        op: "search".into(),
        class: "positive".into(),
        fastmcp: NormalizedOutcome::Ok {
            body: json!({"op": "search", "value": {"x": 1}, "refs": []}),
        },
        codemode: NormalizedOutcome::Ok {
            body: json!({"op": "search", "value": {"x": 1}, "refs": []}),
        },
        private_worker: NormalizedOutcome::Ok {
            body: json!({"op": "search", "value": {"x": 1}, "refs": []}),
        },
        agree: true,
        store_fp_fastmcp: None,
        store_fp_codemode: None,
        store_fp_private_worker: None,
        store_state_agree: true,
    };
    assert!(report.agree);
    let mut_report = deliberate_adapter_semantic_mutation(report);
    assert!(!mut_report.agree, "adapter-only mutation must fail suite");
}

#[test]
fn store_fingerprint_stable_for_same_tree() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("a"), b"hello").unwrap();
    let a = store_state_fingerprint(&p);
    let b = store_state_fingerprint(&p);
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
}

#[test]
fn cancelled_preflight_kind_is_cancelled() {
    use crate::operation_abi::DomainErrorKind;
    let mut ctx = EngineContext::for_paths(
        PathBuf::from("."),
        PathBuf::from(".graphzero"),
        AdapterKind::FastMcp,
    );
    ctx.cancelled = true;
    let err = dispatch(&ctx, "search", &json!({"query": "x"})).unwrap_err();
    assert_eq!(err.kind, DomainErrorKind::Cancelled);
}
