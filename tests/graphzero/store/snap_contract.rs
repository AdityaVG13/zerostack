mod common;

use graphzero_store::store::query::QueryEngine;

#[test]
fn snap_budget_one_ref_only() {
    let fx = common::indexed_repo();
    let json = QueryEngine::cold(&fx.store_root, Some(&fx.repo_root), "alpha", 1)
        .unwrap()
        .to_json(Some(&fx.store_root));
    assert!(json.contains("gz://query/"), "{json}");
    assert!(!json.contains("\"matches\":["));
}

#[test]
fn warm_snap_has_coverage_and_refs() {
    let fx = common::indexed_repo();
    let snap = graphzero_store::Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let json = QueryEngine::warm(&snap, "alpha", 800)
        .unwrap()
        .to_json(Some(&fx.store_root));
    assert!(json.contains("coverage"));
    assert!(json.contains("alpha"));
}

#[test]
fn fresh_rust_snap_destinations_expand_to_file_lines() {
    use graphzero_store::store::expand::ExpandResolver;
    use graphzero_store::store::indexer;
    use graphzero_store::store::refs::GzRef;

    let fx = common::make_repo();
    std::fs::write(
        fx.repo_root.join("src/engine.rs"),
        "pub struct ProofEngine {\n    pub enabled: bool,\n}\n\nimpl ProofEngine {\n    pub fn run(&self) -> bool {\n        self.enabled\n    }\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(fx.repo_root.join("benchmarks")).unwrap();
    std::fs::write(
        fx.repo_root.join("benchmarks/gold.json"),
        "{\"ProofEngine\":\"fixture\"}",
    )
    .unwrap();
    indexer::index_repo(&fx.repo_root, &fx.store_root).unwrap();

    let snapshot = graphzero_store::Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let capsule =
        graphzero_store::store::query::snap(&snapshot, "ProofEngine", 800, None, true).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&capsule.to_json(Some(&fx.store_root))).unwrap();
    let destination = value["destinations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["label"] == "ProofEngine")
        .expect("ProofEngine destination");
    assert_eq!(destination["path"], "src/engine.rs");

    for field in ["ref", "evidence_ref"] {
        let reference = destination[field].as_str().unwrap();
        assert!(
            reference.contains("#B"),
            "{field} was not line-addressed: {reference}"
        );
        let expanded = resolver
            .resolve(&GzRef::parse(reference).unwrap(), reference)
            .unwrap();
        let source = std::str::from_utf8(&expanded.bytes).unwrap();
        assert!(
            source.contains("ProofEngine"),
            "{field} expanded to {source:?}"
        );
    }

    for budget in [1, 5] {
        let tiny =
            graphzero_store::store::query::snap(&snapshot, "ProofEngine", budget, None, true)
                .unwrap();
        let wire = tiny.to_json(Some(&fx.store_root));
        let query_ref: String = serde_json::from_str(&wire).expect("tiny snap must be ref-only");
        if budget == 5 {
            assert!(
                query_ref.starts_with("gz://blob/") && query_ref.contains("#B"),
                "{wire}"
            );
            let source = resolver
                .resolve(&GzRef::parse(&query_ref).unwrap(), &query_ref)
                .unwrap();
            assert!(
                std::str::from_utf8(&source.bytes)
                    .unwrap()
                    .contains("ProofEngine")
            );
            continue;
        }
        assert!(query_ref.starts_with("gz://query/"), "{wire}");
        let expanded = resolver
            .resolve(&GzRef::parse(&query_ref).unwrap(), &query_ref)
            .unwrap();
        let full: serde_json::Value = serde_json::from_slice(&expanded.bytes).unwrap();
        assert_eq!(full["ledger"]["requested_budget"], budget);
        let destination = &full["destinations"][0];
        assert_eq!(destination["path"], "src/engine.rs");
        for field in ["ref", "evidence_ref"] {
            let reference = destination[field].as_str().unwrap();
            resolver
                .resolve(&GzRef::parse(reference).unwrap(), reference)
                .unwrap();
        }
    }
}

#[test]
fn snap_to_edit_returns_fszero_compatible_anchor_from_warm_snapshot() {
    let fx = common::indexed_repo();
    let snapshot = graphzero_store::Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let result = graphzero_store::store::query::snap_to_edit(&snapshot, "src/a.rs::beta").unwrap();
    assert_eq!(result.best.path, "src/a.rs");
    assert_eq!(result.best.symbol, "beta");
    assert_eq!(result.best.line, 5);
    assert!(result.best.byte_span.start < result.best.byte_span.end);
    assert!(result.best.enclosing_block_span.start <= result.best.byte_span.start);
    assert!(result.best.enclosing_block_span.end >= result.best.byte_span.end);
    assert!(result.best.evidence_ref.starts_with("gz://blob/"));
    assert!(result.best.confidence >= 0.98);
    assert!(result.alternates.is_empty());

    let warm = snapshot.snap_edit_index().unwrap();
    let started = std::time::Instant::now();
    for _ in 0..1_000 {
        assert_eq!(warm.resolve("widget type").unwrap().best.symbol, "Widget");
    }
    assert!(started.elapsed() < std::time::Duration::from_millis(200));
}
