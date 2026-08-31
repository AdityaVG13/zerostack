use graphzero_store::store::query::{
    BudgetLedger, CoverageCertificate, ExportFormat, QueryCapsule, RouteDiagnostics, SnapRoute,
    export_capsule, tokens_for_utf8,
};
use graphzero_store::{ExpandResolver, GzRef, publish_schema_json, validate_batch_json};
use tempfile::tempdir;

#[test]
fn live_schema_requires_z_blob_evidence() {
    let schema = publish_schema_json();
    assert!(schema.contains("^z://blob/"), "{schema}");
    assert!(!schema.contains("^gz://blob/"), "{schema}");
    let z = r#"{"schema_version":"publish/v1","publisher":"ci.flake-detector","edges":[{"src":"a","dst":"b","kind":"calls","evidence_ref":"z://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#B0-1","confidence":0.8}]}"#;
    validate_batch_json(z.as_bytes()).expect("z://blob evidence must validate");
    let gz = z.replace("z://blob/", "gz://blob/");
    assert_eq!(
        validate_batch_json(gz.as_bytes()).unwrap_err().code,
        "E_SCHEMA",
        "gz://blob evidence must fail live schema"
    );
}

#[test]
fn minimal_export_ref_resolves_full_capsule() {
    let root = tempdir().expect("temp store");
    let capsule = QueryCapsule {
        schema_version: 1,
        query: "needle".to_string(),
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
        snapshot_id: 42,
    };
    let output = root.path().join("minimal.json");
    let artifact = export_capsule(&capsule, Some(root.path()), &output, ExportFormat::Minimal)
        .expect("minimal export");
    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).expect("read minimal export"))
            .expect("minimal export JSON");
    let reference = payload["ref"].as_str().expect("minimal ref");
    assert_eq!(reference, artifact.ref_str);

    let parsed = GzRef::parse(reference)
        .ok()
        .or_else(|| GzRef::parse(&format!("query/{}", reference.trim_start_matches("q:"))).ok())
        .expect("parse minimal ref");
    let resolved = ExpandResolver::new(root.path(), None)
        .expect("open resolver")
        .resolve(&parsed, reference)
        .expect("resolve minimal ref");
    let full: serde_json::Value =
        serde_json::from_slice(&resolved.bytes).expect("resolved capsule JSON");
    assert!(
        full.is_object(),
        "minimal ref must resolve the full capsule, got {full}"
    );
    assert_eq!(full["query"], "needle");
    assert_eq!(
        payload["meta"]["full_tokens"].as_u64(),
        Some(tokens_for_utf8(&resolved.bytes) as u64)
    );
    assert!(
        payload["meta"].get("created_ts").is_none(),
        "minimal export must not claim a fabricated timestamp"
    );
}
