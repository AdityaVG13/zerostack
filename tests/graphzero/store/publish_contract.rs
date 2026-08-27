mod common;

use graphzero_store::store::csr::EdgeKind;
use graphzero_store::store::publish::{PublishOptions, publish_batch, wal_edge_count};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::{
    SCHEMA, Snapshot, confidence_to_u8, map_publish_kind, schema_v1_json, validate_batch_json,
};

fn opts(fx: &common::Fixture) -> PublishOptions<'_> {
    PublishOptions {
        store_root: &fx.store_root,
        repo_root: Some(&fx.repo_root),
        capability: Some("test-publish-token"),
        allow_anonymous: false,
    }
}

#[test]
fn schema_auth_and_wal() {
    let fx = common::indexed_repo();
    let ev = common::evidence_for_file(&fx.repo_root, "src/a.rs", 0, 20);
    let raw = common::minimal_batch(&format!(
        r#"{{"src":"alpha","dst":"beta","kind":"calls","evidence_ref":"{ev}","confidence":0.7}}"#
    ));
    validate_batch_json(raw.as_bytes()).unwrap();
    let missing_conf = r#"{"schema_version":"publish/v1","publisher":"ci.flake-detector","edges":[{"src":"a","dst":"b","kind":"calls","evidence_ref":"gz://blob/0000000000000000000000000000000000000000000000000000000000000000#B0-1"}]}"#;
    assert_eq!(
        validate_batch_json(missing_conf.as_bytes())
            .unwrap_err()
            .code,
        "E_SCHEMA"
    );
    let valid = common::minimal_batch(
        r#"{"src":"alpha","dst":"beta","kind":"calls","evidence_ref":"gz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#B0-1","confidence":0.8}"#,
    );
    validate_batch_json(valid.as_bytes()).unwrap();
    assert_eq!(SCHEMA, "publish/v1");
    assert!(schema_v1_json().contains("publish/v1"));
    assert_eq!(
        map_publish_kind("runtime_called"),
        Some(EdgeKind::RUNTIME_CALLED)
    );
    let u = confidence_to_u8(0.42).unwrap();
    assert!((u as f64 / 255.0 - 0.42).abs() < 0.01);

    let no_auth = PublishOptions {
        store_root: &fx.store_root,
        repo_root: Some(&fx.repo_root),
        capability: None,
        allow_anonymous: false,
    };
    assert_eq!(
        publish_batch(raw.as_bytes(), &no_auth).unwrap_err().code,
        "E_AUTH"
    );

    common::publish_token(&fx.store_root);
    let before = wal_edge_count(&fx.store_root.join("wal")).unwrap();
    publish_batch(raw.as_bytes(), &opts(&fx)).expect("publish");
    assert_eq!(
        wal_edge_count(&fx.store_root.join("wal")).unwrap(),
        before + 1
    );

    let bad = "gz://blob/deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef#B0-4";
    let mixed = format!(
        r#"{{"schema_version":"publish/v1","publisher":"ci.flake-detector","edges":[
        {{"src":"alpha","dst":"beta","kind":"calls","evidence_ref":"{ev}","confidence":0.7}},
        {{"src":"gamma","dst":"alpha","kind":"calls","evidence_ref":"{bad}","confidence":0.7}}]}}"#
    );
    let wal = wal_edge_count(&fx.store_root.join("wal")).unwrap();
    assert_eq!(
        publish_batch(mixed.as_bytes(), &opts(&fx))
            .unwrap_err()
            .code,
        "E_EVIDENCE"
    );
    assert_eq!(wal_edge_count(&fx.store_root.join("wal")).unwrap(), wal);
}

#[test]
fn evidence_span_and_snap_merge() {
    let fx = common::indexed_repo();
    common::publish_token(&fx.store_root);
    let o = opts(&fx);
    let dead = common::minimal_batch(
        r#"{"src":"alpha","dst":"beta","kind":"calls","evidence_ref":"gz://blob/deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef#B0-4","confidence":0.5}"#,
    );
    assert_eq!(
        publish_batch(dead.as_bytes(), &o).unwrap_err().code,
        "E_EVIDENCE"
    );

    let good = common::evidence_for_file(&fx.repo_root, "src/a.rs", 3, 12);
    let ok = common::minimal_batch(&format!(
        r#"{{"src":"alpha","dst":"beta","kind":"calls","evidence_ref":"{good}","confidence":0.5}}"#
    ));
    assert_eq!(publish_batch(ok.as_bytes(), &o).unwrap().edges_accepted, 1);

    let ev = common::evidence_for_file(&fx.repo_root, "src/a.rs", 0, 20);
    publish_batch(
        common::minimal_batch(&format!(
            r#"{{"src":"alpha","dst":"beta","kind":"linter_smell","evidence_ref":"{ev}","confidence":0.6}}"#
        ))
        .as_bytes(),
        &o,
    )
    .unwrap();
    let snap = Snapshot::open(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let json = QueryEngine::warm(&snap, "alpha", 8000)
        .unwrap()
        .to_json(Some(&fx.store_root));
    assert!(json.contains("beta") && json.contains("linter_smell"));
}
