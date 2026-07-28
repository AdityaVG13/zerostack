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
    use zerostack_codemode_conformance::racc::{immutable_query_fixtures, validate_racc_schema, RaccSubstrate, RACC_CERTIFICATE_SCHEMA};
    let certificate = RaccFakeSubstrate::default().certified_query(&immutable_query_fixtures()[1]);
    validate_racc_schema(RACC_CERTIFICATE_SCHEMA, &serde_json::to_value(certificate).unwrap()).unwrap();
}

#[test]
fn racc_receipt_schema_is_golden_pinned() {
    use zerostack_codemode_conformance::fake_substrate::RaccFakeSubstrate;
    use zerostack_codemode_conformance::racc::{validate_racc_schema, RaccSubstrate, RACC_RECEIPT_SCHEMA};
    let receipt = RaccFakeSubstrate::default().dominance_receipt();
    validate_racc_schema(RACC_RECEIPT_SCHEMA, &serde_json::to_value(receipt).unwrap()).unwrap();
}

#[test]
fn racc_golden_schemas_reject_shape_drift() {
    use zerostack_codemode_conformance::racc::{validate_racc_schema, RACC_CERTIFICATE_SCHEMA, RACC_RECEIPT_SCHEMA};
    assert!(validate_racc_schema(RACC_CERTIFICATE_SCHEMA, &json!({"schema_version": 1})).is_err());
    assert!(validate_racc_schema(RACC_RECEIPT_SCHEMA, &json!({"schema_version": 1})).is_err());
}
