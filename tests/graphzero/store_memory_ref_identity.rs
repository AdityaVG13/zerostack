use graphzero_store::store::query::{
    BudgetLedger, CoverageCertificate, ExportFormat, QueryCapsule, RouteDiagnostics, SnapRoute,
    export_capsule,
};
use graphzero_store::{ExpandResolver, GzRef};
use graphzero_store::{
    MemoryFact, MemoryKind, RememberInput, Snapshot, format_recall_budget_one, mem_ref,
    remember_fact,
};

#[test]
fn recall_budget_emits_unprefixed_mem_ref() {
    let fact = MemoryFact {
        id: "abc123".into(),
        ts: 1,
        kind: MemoryKind::Note,
        text: "keep this".into(),
        anchors: vec![],
        anchor_resolutions: vec![],
        supersedes: vec![],
    };
    let line = format_recall_budget_one("src/a.rs", &[&fact]);
    assert!(line.contains("(mem/abc123)"), "{line}");
    assert!(!line.contains("gz://"), "{line}");
    assert_eq!(mem_ref("abc123"), "mem/abc123");
}

#[test]
fn remember_rejects_retired_supersedes_ref() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let store = repo.join(".graphzero");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn anchor() {}\n").unwrap();
    graphzero_store::store::indexer::index_repo(&repo, &store).unwrap();
    let snapshot = Snapshot::open(&store, Some(&repo)).unwrap();
    let input = RememberInput {
        text: "keep this".into(),
        anchors: vec![],
        kind: None,
        supersedes: vec!["gz://mem/abc123".into()],
    };
    assert!(remember_fact(&snapshot, input).is_err());
}

#[test]
fn nonminimal_exports_return_resolvable_capsule_refs() {
    let root = tempfile::tempdir().unwrap();
    let capsule = QueryCapsule {
        schema_version: 1,
        query: "exported needle".to_string(),
        budget: 1,
        route: SnapRoute::Symbol,
        destinations: Vec::new(),
        coverage: CoverageCertificate {
            tier_a: 1.0,
            tier_b: 0.0,
            tier_c: 0.0,
            semantic_tier_percent: 0.0,
            freshness_verified: true,
        },
        diagnostics: RouteDiagnostics::default(),
        ledger: BudgetLedger {
            requested_budget: 1,
            used_budget: 1,
            remaining_budget: 0,
            truncated: true,
            omitted_count: 0,
        },
        snapshot_id: 43,
    };

    let mut expected_ref = None;
    for format in [ExportFormat::Capsule, ExportFormat::Md, ExportFormat::Zst] {
        let output = root.path().join(format!("export.{}", format.as_str()));
        let artifact =
            export_capsule(&capsule, Some(root.path()), &output, format).expect("export capsule");
        if let Some(expected) = &expected_ref {
            assert_eq!(&artifact.ref_str, expected);
        } else {
            expected_ref = Some(artifact.ref_str.clone());
        }
        let parsed = GzRef::parse(&artifact.ref_str)
            .or_else(|_| {
                GzRef::parse(&format!(
                    "query/{}",
                    artifact.ref_str.trim_start_matches("q:")
                ))
            })
            .expect("parse export ref");
        let resolved = ExpandResolver::new(root.path(), None)
            .expect("open resolver")
            .resolve(&parsed, &artifact.ref_str)
            .expect("export ref must resolve");
        let full: serde_json::Value =
            serde_json::from_slice(&resolved.bytes).expect("resolved full capsule JSON");
        assert_eq!(full["query"], "exported needle");
    }
}
