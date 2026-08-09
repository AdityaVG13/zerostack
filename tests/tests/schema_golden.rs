//! Golden schema files validate representative contract documents.

use serde_json::json;
use zerostack_codemode_conformance::schema::{validate_against_schema, SchemaName};

fn telemetry() -> serde_json::Value {
    json!({
        "kind": "codemode.execute",
        "status": "ok",
        "logical_ops": 1,
        "physical_ops": 1,
        "batched_ops": 0,
        "internal_actions": 1,
        "cache_hits": 0,
        "cache_misses": 0,
        "store_writes": 4,
        "wall_ms": 1,
        "bytes_materialized": 4
    })
}

#[test]
fn capability_manifest_accepts_normative_defaults() {
    let doc = json!({
        "contract_version": "1.0",
        "ns": "gz",
        "mutation": "readonly",
        "plan_forms": ["recipe", "json", "js"],
        "limits": {
            "max_logical_ops": 1000,
            "max_output_bytes": 65536
        }
    });
    validate_against_schema(SchemaName::CapabilityManifest, &doc).expect("capabilities");

    let missing_js = json!({
        "contract_version": "1.0",
        "ns": "gz",
        "mutation": "readonly",
        "plan_forms": ["recipe", "json"],
        "limits": {}
    });
    assert!(validate_against_schema(SchemaName::CapabilityManifest, &missing_js).is_err());
}

#[test]
fn limits_echo_accepts_subset_of_normative_limits() {
    let doc = json!({
        "max_logical_ops": 1000,
        "max_physical_ops": 256,
        "max_output_bytes": 65536,
        "max_code_bytes": 65536
    });
    validate_against_schema(SchemaName::LimitsEcho, &doc).expect("limits echo");

    let bad = json!({ "dead_limit": 1 });
    assert!(validate_against_schema(SchemaName::LimitsEcho, &bad).is_err());
}

#[test]
fn execution_record_requires_cm_execution_id_and_refs_object() {
    let ok = json!({
        "execution_id": "cm://exec/1719859200123-abcdef012345",
        "ns": "gz",
        "status": "ok",
        "refs": {
            "code": "gz://codemode/execution/1719859200123-abcdef012345/code",
            "steps": "gz://codemode/execution/1719859200123-abcdef012345/steps",
            "telemetry": "gz://codemode/execution/1719859200123-abcdef012345/telemetry",
            "result": "gz://codemode/execution/1719859200123-abcdef012345/result"
        },
        "telemetry": telemetry()
    });
    validate_against_schema(SchemaName::ExecutionRecord, &ok).expect("execution record");

    let bad_id = json!({
        "execution_id": "cm://exec/not-a-valid-id",
        "ns": "gz",
        "status": "ok",
        "refs": {
            "code": "gz://codemode/execution/x/code",
            "steps": "gz://codemode/execution/x/steps",
            "telemetry": "gz://codemode/execution/x/telemetry"
        },
        "telemetry": telemetry()
    });
    assert!(validate_against_schema(SchemaName::ExecutionRecord, &bad_id).is_err());
}

#[test]
fn racc_certificate_schema_is_golden_pinned() {
    use zerostack_codemode_conformance::fake_substrate::RaccFakeSubstrate;
    use zerostack_codemode_conformance::racc::{
        immutable_query_fixtures, validate_racc_schema, RaccSubstrate, RACC_CERTIFICATE_SCHEMA,
    };
    let certificate = RaccFakeSubstrate::default().certified_query(&immutable_query_fixtures()[1]);
    validate_racc_schema(
        RACC_CERTIFICATE_SCHEMA,
        &serde_json::to_value(certificate).unwrap(),
    )
    .unwrap();
}

#[test]
fn racc_receipt_schema_is_golden_pinned() {
    use zerostack_codemode_conformance::fake_substrate::RaccFakeSubstrate;
    use zerostack_codemode_conformance::racc::{
        validate_racc_schema, RaccSubstrate, RACC_RECEIPT_SCHEMA,
    };
    let receipt = RaccFakeSubstrate::default().dominance_receipt();
    validate_racc_schema(RACC_RECEIPT_SCHEMA, &serde_json::to_value(receipt).unwrap()).unwrap();
}

#[test]
fn racc_invalidation_freshness_schema_is_golden_pinned() {
    use zerostack_codemode_conformance::racc::{
        validate_racc_schema, RACC_INVALIDATION_FRESHNESS_SCHEMA,
    };
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/invalidation-freshness-v1.json")).unwrap();
    let certificate = vectors["canonical_fresh_certificate"].clone();
    let digest = certificate["certificate_digest"].clone();
    let document = json!({
        "schema_version": 1,
        "model_version": "zerostack.invalidation-freshness.v1",
        "certificate": certificate,
        "result": {
            "schema_version": 1,
            "status": "fresh",
            "trusted": true,
            "failure_code": null,
            "detail": "exact identity closure",
            "indexed_certificate_digest": digest
        }
    });
    validate_racc_schema(RACC_INVALIDATION_FRESHNESS_SCHEMA, &document).unwrap();
    let mut partial_trust = document;
    partial_trust["result"]["status"] = json!("unknown");
    assert!(validate_racc_schema(RACC_INVALIDATION_FRESHNESS_SCHEMA, &partial_trust).is_err());
}

#[test]
fn racc_golden_schemas_reject_shape_drift() {
    use zerostack_codemode_conformance::racc::{
        validate_racc_schema, RACC_CERTIFICATE_SCHEMA, RACC_RECEIPT_SCHEMA,
    };
    assert!(validate_racc_schema(RACC_CERTIFICATE_SCHEMA, &json!({"schema_version": 1})).is_err());
    assert!(validate_racc_schema(RACC_RECEIPT_SCHEMA, &json!({"schema_version": 1})).is_err());
}

#[test]
fn task_acceptance_receipt_schema_is_golden_pinned() {
    use zerostack_codemode_conformance::racc::{
        digest_hex, validate_racc_schema, RACC_TASK_ACCEPTANCE_RECEIPT_SCHEMA,
    };
    let artifact = digest_hex(b"artifact");
    let receipt = json!({
        "schema_version": 1,
        "task_id": "task-7",
        "verifier_command_id": 41,
        "verifier_environment_digest": digest_hex(b"env"),
        "outcome": "passed",
        "exit_code": 0,
        "expected_artifact_digests": [artifact.clone()],
        "observed_artifact_digests": [artifact],
        "journal_id": digest_hex(b"journal"),
        "attempt_cost": 13
    });
    validate_racc_schema(RACC_TASK_ACCEPTANCE_RECEIPT_SCHEMA, &receipt).unwrap();
    let mut nonzero = receipt.clone();
    nonzero["exit_code"] = json!(1);
    assert!(validate_racc_schema(RACC_TASK_ACCEPTANCE_RECEIPT_SCHEMA, &nonzero).is_err());
    let mut free = receipt;
    free["attempt_cost"] = json!(0);
    assert!(validate_racc_schema(RACC_TASK_ACCEPTANCE_RECEIPT_SCHEMA, &free).is_err());
}

fn harness_report_validator() -> jsonschema::Validator {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/harness_report.schema.json")).unwrap();
    jsonschema::Validator::new(&schema).unwrap()
}

fn full_harness_report() -> serde_json::Value {
    json!({
        "ns": "gz",
        "bin": "fake-graphzero",
        "contract_version": "1.0",
        "surface": "codemode",
        "completion_status": "complete",
        "passed": true,
        "checks": (1..=10).map(|gate| json!({
            "id": format!("G{gate}"),
            "name": format!("semantic-{gate}"),
            "passed": true,
            "status": "pass",
            "details": []
        })).collect::<Vec<_>>()
    })
}

#[test]
fn harness_report_schema_pair_is_deterministic_and_accepts_full_shape() {
    let source: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/harness-report.schema.json")).unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/harness_report.schema.json")).unwrap();
    assert_eq!(
        snapshot, source,
        "underscore snapshot must equal the resolved SSOT schema"
    );
    assert!(harness_report_validator().is_valid(&full_harness_report()));
}

#[test]
fn harness_report_schema_rejects_duplicate_and_unknown_gate_ids() {
    let validator = harness_report_validator();
    let mut duplicate = full_harness_report();
    duplicate["checks"][1]["id"] = json!("G1");
    duplicate["checks"][1]["name"] = json!("different semantics");
    assert!(!validator.is_valid(&duplicate));

    let mut unknown = full_harness_report();
    unknown["checks"][9]["id"] = json!("G11");
    assert!(!validator.is_valid(&unknown));
}

#[test]
fn canonical_report_serde_rejects_unknown_and_duplicate_fields() {
    let mut unknown = full_harness_report();
    unknown["unexpected"] = json!(true);
    assert!(
        serde_json::from_value::<zerostack_codemode_conformance::ConformanceReport>(unknown)
            .is_err()
    );

    let duplicate = r#"{"ns":"gz","ns":"fz","bin":"fake","contract_version":"1.0","surface":"mcp","completion_status":"partial","passed":false,"checks":[]}"#;
    assert!(
        serde_json::from_str::<zerostack_codemode_conformance::ConformanceReport>(duplicate)
            .is_err()
    );
}
