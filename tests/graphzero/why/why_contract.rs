mod common;

use graphzero_why::connectors::{ConnectorConfig, ConnectorStatus};
use graphzero_why::evidence::expand_evidence_ref;
use graphzero_why::ingest::{
    CommitFixtureEvent, CommitTouchedEntity, advance_cursor, cursor_digest_for_events,
    ingest_commit_fixture, ingest_commit_metadata_fixture, ingest_pr_issue_fixture,
    ingest_trace_fixture, ingest_unresolved_node_edge, replay_golden,
};
use graphzero_why::schema::{
    ConnectorAvailability, NodeLinkState, ProvenanceSource, ProvenanceSourceKind, RedactionState,
    SCHEMA_VERSION, WhyEdge, WhyRelation,
};
use graphzero_why::store::build_query_manifest;

const FAKE_SECRET: &str = "sk-fake-secret-for-test";

#[test]
fn golden_ingest_covers_all_source_kinds() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    assert!(common::ingest_all_golden(&store, &fx.repo) >= 4);
    let kinds: std::collections::HashSet<_> = store
        .all_edges()
        .unwrap()
        .into_iter()
        .map(|e| e.source.kind)
        .collect();
    for kind in [
        ProvenanceSourceKind::GitCommit,
        ProvenanceSourceKind::AgentTrace,
        ProvenanceSourceKind::PrThread,
        ProvenanceSourceKind::Issue,
    ] {
        assert!(kinds.contains(&kind));
    }
}

#[test]
fn per_connector_ingest_shapes() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let cfg = ConnectorConfig::all_enabled();
    let (commit, trace, pr_issue) = common::load_golden();

    let commit_edge = ingest_commit_fixture(&store, Some(&fx.repo), &cfg, &commit)
        .unwrap()
        .expect("commit");
    assert_eq!(commit_edge.source.kind, ProvenanceSourceKind::GitCommit);
    assert_eq!(commit_edge.node_ref.as_deref(), Some("gz://node/sym:alpha"));

    let trace_edge = ingest_trace_fixture(&store, Some(&fx.repo), &cfg, &trace)
        .unwrap()
        .expect("trace");
    assert_eq!(trace_edge.source.kind, ProvenanceSourceKind::AgentTrace);

    let pr_edges = ingest_pr_issue_fixture(&store, Some(&fx.repo), &cfg, &pr_issue).unwrap();
    assert_eq!(pr_edges.len(), 2);
    assert!(
        pr_edges
            .iter()
            .any(|e| e.source.kind == ProvenanceSourceKind::PrThread)
    );
    assert!(
        pr_edges
            .iter()
            .any(|e| e.source.kind == ProvenanceSourceKind::Issue)
    );
}

#[test]
fn commit_metadata_links_files_and_symbols_to_commit_source() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let cfg = ConnectorConfig::all_enabled();
    let event = CommitFixtureEvent {
        commit_oid: "abc123metadata".into(),
        message: "touch alpha and beta".into(),
        author: Some("Ada <ada@example.invalid>".into()),
        path: "src/a.rs".into(),
        node_ref: "gz://node/sym:alpha".into(),
        freshness: "2026-07-07T00:00:00Z".into(),
        touched: vec![
            CommitTouchedEntity {
                path: "src/a.rs".into(),
                node_ref: Some("gz://node/sym:alpha".into()),
            },
            CommitTouchedEntity {
                path: "src/b.rs".into(),
                node_ref: None,
            },
        ],
    };

    let edges = ingest_commit_metadata_fixture(&store, Some(&fx.repo), &cfg, &event).unwrap();

    assert_eq!(edges.len(), 3);
    assert!(
        edges
            .iter()
            .any(|edge| edge.relation == WhyRelation::Introduced)
    );
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge.relation == WhyRelation::Modified)
            .count(),
        2
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.node_ref.as_deref() == Some("gz://file/src/b.rs"))
    );
    assert!(
        edges
            .iter()
            .all(|edge| edge.source.kind == ProvenanceSourceKind::GitCommit)
    );
    assert!(
        edges
            .iter()
            .all(|edge| edge.source.stable_id == "abc123metadata")
    );
}

#[test]
fn schema_validation_cursor_and_manifest() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let stale = WhyEdge {
        schema_version: SCHEMA_VERSION,
        edge_id: String::new(),
        source: ProvenanceSource {
            kind: ProvenanceSourceKind::GitCommit,
            stable_id: "oid1".into(),
        },
        node_ref: Some("gz://node/sym:alpha".into()),
        relation: WhyRelation::Introduced,
        confidence: 0.9,
        source_freshness: Some("t".into()),
        evidence_refs: vec![
            "gz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        ],
        redaction_state: RedactionState::None,
        node_link_state: NodeLinkState::Resolved,
        reserved: serde_json::Value::Null,
    };
    assert!(stale.validate_for_persist().is_err());

    let mut resolved_without_node = stale.clone();
    resolved_without_node.edge_id = "resolved-without-node".into();
    resolved_without_node.node_ref = None;
    resolved_without_node.node_link_state = NodeLinkState::Resolved;
    assert!(resolved_without_node.validate_for_persist().is_err());

    let mut pending_with_node = stale.clone();
    pending_with_node.edge_id = "pending-with-node".into();
    pending_with_node.node_ref = Some("gz://node/sym:alpha".into());
    pending_with_node.node_link_state = NodeLinkState::Pending;
    assert!(pending_with_node.validate_for_persist().is_err());

    common::ingest_all_golden(&store, &fx.repo);
    for e in store.all_edges().unwrap() {
        assert_eq!(e.schema_version, 1);
        assert!(!e.evidence_refs.is_empty());
        assert!(e.node_ref.is_some());
        assert!((0.0..=1.0).contains(&e.confidence));
        assert!(e.source_freshness.is_some());
    }

    let digest = cursor_digest_for_events(&["e1", "e2"]);
    assert_eq!(digest, cursor_digest_for_events(&["e1", "e2"]));
    let source = ProvenanceSource {
        kind: ProvenanceSourceKind::AgentTrace,
        stable_id: "s1".into(),
    };
    advance_cursor(&store, source.clone(), "p", &digest, None).unwrap();
    let key = graphzero_why::store::cursor_key(&source);
    assert_eq!(store.load_ledger().unwrap().cursors[&key].digest, digest);
    advance_cursor(&store, source.clone(), "p", &digest, None).unwrap();
    assert!(advance_cursor(&store, source.clone(), "p", "changed", None).is_err());
    assert!(advance_cursor(&store, source.clone(), "o", &digest, None).is_err());
    let advanced_digest = cursor_digest_for_events(&["e1", "e2", "e3"]);
    advance_cursor(&store, source.clone(), "q", &advanced_digest, Some("e3")).unwrap();
    let cursor = &store.load_ledger().unwrap().cursors[&key];
    assert_eq!(cursor.position, "q");
    assert_eq!(cursor.digest, advanced_digest);

    let git_source = ProvenanceSource {
        kind: ProvenanceSourceKind::GitCommit,
        stable_id: "window".into(),
    };
    let git_digest = cursor_digest_for_events(&["a", "b"]);
    advance_cursor(&store, git_source.clone(), "pos1", &git_digest, Some("e1")).unwrap();
    let git_key = graphzero_why::store::cursor_key(&git_source);
    assert_eq!(
        store.load_ledger().unwrap().cursors[&git_key].digest,
        git_digest
    );

    let ledger = store.load_ledger().unwrap();
    let manifest = build_query_manifest(&ledger);
    assert_eq!(manifest.schema_version, 1);
    assert!(manifest.edge_count >= 3);
    assert!(!manifest.by_node.is_empty());
}

#[test]
fn why_chain_returns_ordered_evidence_refs_for_node() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    common::ingest_all_golden(&store, &fx.repo);

    let chain = store.why_chain_for_node("gz://node/sym:alpha").unwrap();

    assert!(!chain.is_empty());
    assert!(
        chain
            .windows(2)
            .all(|pair| pair[0].source_freshness <= pair[1].source_freshness)
    );
    assert!(chain.iter().all(|entry| !entry.evidence_refs.is_empty()));
    assert!(
        chain
            .iter()
            .any(|entry| entry.source.kind == ProvenanceSourceKind::GitCommit)
    );
}

#[test]
fn evidence_expandability_and_replay_idempotence() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let cfg = ConnectorConfig::all_enabled();
    let (commit, trace, pr_issue) = common::load_golden();
    replay_golden(&store, Some(&fx.repo), &cfg, &commit, &trace, &pr_issue).unwrap();
    let ids1: Vec<_> = store
        .all_edges()
        .unwrap()
        .into_iter()
        .map(|e| e.edge_id)
        .collect();
    let digest1 = store.load_ledger().unwrap().replay_digest;
    replay_golden(&store, Some(&fx.repo), &cfg, &commit, &trace, &pr_issue).unwrap();
    let ids2: Vec<_> = store
        .all_edges()
        .unwrap()
        .into_iter()
        .map(|e| e.edge_id)
        .collect();
    assert_eq!(ids1, ids2);
    assert_eq!(digest1, store.load_ledger().unwrap().replay_digest);

    for edge in store.all_edges().unwrap() {
        for r in &edge.evidence_refs {
            assert!(
                !expand_evidence_ref(&fx.store_root, Some(&fx.repo), r)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    let empty = WhyEdge {
        schema_version: SCHEMA_VERSION,
        edge_id: String::new(),
        source: ProvenanceSource {
            kind: ProvenanceSourceKind::GitCommit,
            stable_id: "x".into(),
        },
        node_ref: Some("gz://node/sym:alpha".into()),
        relation: WhyRelation::Introduced,
        confidence: 0.5,
        source_freshness: None,
        evidence_refs: vec![],
        redaction_state: RedactionState::None,
        node_link_state: NodeLinkState::Resolved,
        reserved: serde_json::Value::Null,
    };
    assert!(store.upsert_edge(empty, Some(&fx.repo)).is_err());
}

#[test]
fn redaction_and_pending_edges() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    common::ingest_all_golden(&store, &fx.repo);

    // Dual-write layout: legacy flat `blobs/<64hex>` plus SharedCas fan-out
    // `blobs/sha256/<hh>/<hash>`. Walk every file under either layout; never
    // `read_dir`+`read` only the top level (sha256 is a directory).
    assert_blob_tree_has_no_secret(&fx.store_root.join("blobs"), FAKE_SECRET);

    let edges = store.all_edges().unwrap();
    assert!(
        edges
            .iter()
            .any(|e| e.redaction_state != RedactionState::None)
    );
    // Expand path must also surface only redacted bytes (resolver may prefer
    // cas-local fan-out over flat).
    for edge in &edges {
        if edge.redaction_state == RedactionState::None {
            continue;
        }
        for r in &edge.evidence_refs {
            let bytes = expand_evidence_ref(&fx.store_root, Some(&fx.repo), r).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(FAKE_SECRET),
                "expanded redacted evidence still contains secret: {r}"
            );
        }
    }

    let edge = ingest_unresolved_node_edge(
        &store,
        Some(&fx.repo),
        "commit-pending",
        "evidence for pending path only",
    )
    .unwrap();
    assert_eq!(edge.node_link_state, NodeLinkState::Pending);
    assert!(edge.node_ref.is_none());
}

/// Recursively assert no secret marker appears in any blob file under `blobs_root`.
fn assert_blob_tree_has_no_secret(blobs_root: &std::path::Path, secret: &str) {
    assert!(
        blobs_root.is_dir(),
        "expected blob root {}",
        blobs_root.display()
    );
    let mut stack = vec![blobs_root.to_path_buf()];
    let mut files_seen = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let data = std::fs::read(&path).unwrap();
            files_seen += 1;
            assert!(
                !String::from_utf8_lossy(&data).contains(secret),
                "secret leaked in blob file {}",
                path.display()
            );
        }
    }
    assert!(
        files_seen > 0,
        "expected at least one blob file under {}",
        blobs_root.display()
    );
}

#[test]
fn ledger_load_rejects_invalid_node_link_pairs() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    common::ingest_all_golden(&store, &fx.repo);
    let mut edge = store.all_edges().unwrap().into_iter().next().unwrap();
    edge.node_ref = None;
    edge.node_link_state = NodeLinkState::Resolved;
    let why_dir = fx.store_root.join("why");
    std::fs::create_dir_all(&why_dir).unwrap();
    std::fs::write(
        why_dir.join("edges.jsonl"),
        format!("{}\n", serde_json::to_string(&edge).unwrap()),
    )
    .unwrap();
    assert!(store.load_ledger().is_err());
}

#[test]
fn disabled_connector_is_unknown_not_absence() {
    let cfg = ConnectorConfig {
        git_commit: true,
        pr_thread: true,
        issue: false,
        agent_trace: true,
    };
    assert_eq!(
        cfg.availability(ProvenanceSourceKind::Issue),
        ConnectorAvailability::Unknown
    );
    let status = ConnectorStatus::from_config(&cfg);
    assert_eq!(status.issue, ConnectorAvailability::Unknown);
    assert!(status.absence_certificate.is_none());
}

#[test]
fn missing_evidence_ref_is_rejected_with_context() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let edge = WhyEdge {
        schema_version: SCHEMA_VERSION,
        edge_id: "missing-evidence".into(),
        source: ProvenanceSource {
            kind: ProvenanceSourceKind::GitCommit,
            stable_id: "missing-evidence-source".into(),
        },
        node_ref: Some("gz://node/sym:alpha".into()),
        relation: WhyRelation::Introduced,
        confidence: 0.5,
        source_freshness: Some("fixture".into()),
        evidence_refs: vec![
            "gz://blob/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into(),
        ],
        redaction_state: RedactionState::None,
        node_link_state: NodeLinkState::Resolved,
        reserved: serde_json::Value::Null,
    };

    let err = store.upsert_edge(edge, Some(&fx.repo)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("evidence not expandable"), "{msg}");
    assert!(msg.contains("gz://blob/ffffffff"), "{msg}");
}

#[test]
fn disabled_issue_connector_does_not_ingest_edges() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let (_, _, pr_issue) = common::load_golden();
    let cfg = ConnectorConfig {
        git_commit: true,
        pr_thread: false,
        issue: false,
        agent_trace: true,
    };

    let edges = ingest_pr_issue_fixture(&store, Some(&fx.repo), &cfg, &pr_issue).unwrap();
    assert!(edges.is_empty());
    let status = ConnectorStatus::from_config(&cfg);
    assert_eq!(status.pr_thread, ConnectorAvailability::Unknown);
    assert_eq!(status.issue, ConnectorAvailability::Unknown);
    assert!(status.absence_certificate.is_none());
}

#[test]
fn malformed_why_ledger_json_is_rejected() {
    let fx = common::indexed_repo();
    let store = common::open_why(&fx.store_root);
    let why_dir = fx.store_root.join("why");
    std::fs::create_dir_all(&why_dir).unwrap();
    std::fs::write(why_dir.join("edges.jsonl"), "{not-json}\n").unwrap();

    let err = store.load_ledger().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("key") || msg.contains("expected"), "{msg}");
}
