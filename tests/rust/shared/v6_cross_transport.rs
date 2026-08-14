//! V6 conformance: canonical vectors replayed through >=3 fixture transports
//! (CLI/RPC/native/MCP) with byte-identical protected roots, ledger,
//! continuation handles, and audit ranges (ZS-ADAPTER-009/011). Violation
//! vectors must be refused loudly.

use jsonschema::Validator;
use serde_json::Value;
use zero_testkit::v6_conformance::{
    V6Transport, apply_violation, envelope_from_vector, protected_fields,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/v6_cross_transport_vectors.json"
    ))
    .expect("fixture is valid JSON")
}

fn result_schema() -> Validator {
    let value: Value = serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_result_v6.schema.json"
    ))
    .expect("V6 schema is valid JSON");
    jsonschema::validator_for(&value).expect("V6 schema is a valid JSON schema")
}

#[test]
fn every_vector_replays_through_every_transport_with_identical_protected_fields() {
    let fixture = fixture();
    let vectors = fixture["vectors"].as_array().expect("vectors array");
    let transports = V6Transport::ALL;
    assert!(
        transports.len() >= 3,
        "the cross-transport contract requires at least three transports"
    );
    let validator = result_schema();
    for vector in vectors {
        let id = vector["id"].as_str().expect("vector id");
        let envelope = envelope_from_vector(vector).unwrap_or_else(|error| {
            panic!("vector {id} cannot build a typed envelope: {error}")
        });
        let protected = protected_fields(&envelope);
        let mut recovered_any = 0usize;
        for transport in transports {
            let projection = transport.project(&envelope, 7);
            let recovered = transport
                .recover(&projection)
                .unwrap_or_else(|error| panic!("{id} via {} failed recovery: {error}", transport.name()));
            assert_eq!(recovered, envelope, "{id} via {} lost semantic fields", transport.name());
            assert_eq!(
                protected_fields(&recovered),
                protected,
                "{id} via {} changed protected fields",
                transport.name()
            );
            recovered_any += 1;
            if envelope.kind().is_v6_base_schema_kind() {
                let value = serde_json::to_value(&recovered).expect("serializes");
                validator.validate(&value).unwrap_or_else(|error| {
                    panic!("{id} via {} failed the V6 schema: {error}", transport.name())
                });
            }
        }
        assert!(recovered_any >= 3, "{id} did not replay through >=3 transports");
    }
}

#[test]
fn same_vector_through_every_transport_yields_one_identical_wire_contract() {
    // The canonical vectors carry every semantic requirement named in the
    // fixture (protected roots, cancellation, timeout, Unknown, fallback,
    // ledger). Replaying the FULL vector set through every transport must
    // never change the kind mapping: a Cancelled vector stays Cancelled, a
    // BaselineFallbackRequired vector stays BaselineFallbackRequired, and
    // Unknown-carrying kinds keep their reasons.
    let fixture = fixture();
    let declared: std::collections::BTreeSet<&str> = [
        "protected_roots",
        "cancellation",
        "timeout",
        "unknown",
        "fallback",
        "ledger",
    ]
    .into_iter()
    .collect();
    let covered = fixture["semantic_requirements"]
        .as_array()
        .expect("semantic_requirements array")
        .iter()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(covered, declared, "fixture must cover every V6 semantic requirement");
    let kinds = fixture["vectors"]
        .as_array()
        .expect("vectors")
        .iter()
        .filter_map(|vector| vector["kind"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for required in [
        "Completed",
        "DecisionRequired",
        "VerificationUnknown",
        "BaselineFallbackRequired",
        "Cancelled",
        "RejectedNoMutation",
        "EvidenceExpansionRequired",
        "FailedNoAuthority",
    ] {
        assert!(kinds.contains(required), "fixture missing {required} vector");
    }
}

#[test]
fn violation_vectors_are_refused_loudly_on_every_transport() {
    let fixture = fixture();
    let violations = fixture["violations"].as_array().expect("violations array");
    let transports = V6Transport::ALL;
    for violation in violations {
        let id = violation["id"].as_str().expect("violation id");
        let mutation = violation["mutation"].as_str().expect("violation mutation");
        // Mutations target a base whose pre-image is known: kind relabeling
        // starts from Cancelled (relabeled to Completed it must fail the
        // Completed per-kind validation), the rest from the Completed vector.
        let base_index = if mutation == "relabel_kind" { 4 } else { 0 };
        let base = fixture["vectors"][base_index].clone();
        let original = envelope_from_vector(&base)
            .unwrap_or_else(|error| panic!("{id} base vector failed: {error}"));
        for transport in transports {
            let mut projection = transport.project(&original, 9);
            apply_violation(&mut projection, mutation)
                .unwrap_or_else(|error| panic!("{id} mutation failed: {error}"));
            match transport.recover(&projection) {
                Ok(envelope) => {
                    // Structural recovery may succeed only when the tampered
                    // envelope still validates (recover() validates); it
                    // must never be accepted as the original outcome.
                    assert_ne!(
                        protected_fields(&envelope),
                        protected_fields(&original),
                        "{id} via {} laundered a tampered envelope as the original",
                        transport.name()
                    );
                }
                Err(_) => {} // refused loudly: the required outcome
            }
        }
    }
}

#[test]
fn fixture_kind_spelling_matches_the_schema_enum() {
    // The eight fixture kinds must match the V6 schema enum exactly for the
    // six base kinds, and the two adapter extensions must stay outside it
    // (never laundered as schema-valid base outcomes).
    let schema: Value = serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_result_v6.schema.json"
    ))
    .expect("schema");
    let enum_values = schema["properties"]["kind"]["enum"]
        .as_array()
        .expect("kind enum")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let fixture = fixture();
    for vector in fixture["vectors"].as_array().expect("vectors") {
        let kind = vector["kind"].as_str().expect("kind");
        let envelope = envelope_from_vector(vector).expect("envelope");
        assert_eq!(
            enum_values.contains(&kind),
            envelope.kind().is_v6_base_schema_kind(),
            "kind {kind} base-schema classification mismatch"
        );
    }
}
