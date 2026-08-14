//! W0 conformance: ZeroExecuteResultV6 validates against the canonical V6
//! JSON schema (racc/v6/schemas/zero_execute_result_v6.schema.json) and the
//! D5 continuation forbidden transitions hold at the shared contract level.

use jsonschema::Validator;
use serde_json::{Value, json};
use zero_abi::{
    AuditEventRangeV1, ContinuationStateV1, PremiseV1, SafetyVerdictV1, ZeroExecuteErrorV6,
    ZeroExecuteFieldsV6, ZeroExecuteKindV6, ZeroExecuteResultV6, ZERO_EXECUTE_ABI_VERSION_V6,
};

fn schema() -> Validator {
    let value: Value = serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_result_v6.schema.json"
    ))
    .expect("V6 schema is valid JSON");
    jsonschema::validator_for(&value).expect("V6 schema is a valid JSON schema")
}

fn schema_value() -> Value {
    serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_result_v6.schema.json"
    ))
    .expect("V6 schema is valid JSON")
}

fn range() -> AuditEventRangeV1 {
    AuditEventRangeV1::new(0, 1).expect("valid range")
}

fn root(value: &str) -> String {
    format!("fz://blob/{value}")
}

fn safe_verdict() -> SafetyVerdictV1 {
    SafetyVerdictV1::from_premises(&[
        PremiseV1::new("p1", Some(true)).unwrap(),
        PremiseV1::new("p2", Some(true)).unwrap(),
    ])
}

fn all_envelopes() -> Vec<ZeroExecuteResultV6> {
    let base = ZeroExecuteFieldsV6 {
        continuation_handle: Some("cont:abc".into()),
        project_root: Some("fz://root/project".into()),
        ..ZeroExecuteFieldsV6::default()
    };
    vec![
        ZeroExecuteResultV6::completed(
            safe_verdict(),
            ZeroExecuteFieldsV6 {
                successor_root: Some(root("succ")),
                result_root: Some(root("result")),
                verification_receipt_root: Some(root("verif")),
                ..base.clone()
            },
            root("ledger"),
            range(),
        )
        .unwrap(),
        ZeroExecuteResultV6::decision_required(
            ZeroExecuteFieldsV6 {
                question: Some("which direction?".into()),
                choices: vec![json!("north"), json!("south")],
                ..base.clone()
            },
            root("ledger"),
            range(),
        )
        .unwrap(),
        ZeroExecuteResultV6::evidence_expansion_required(base.clone(), root("ledger"), range())
            .unwrap(),
        ZeroExecuteResultV6::verification_unknown(
            ZeroExecuteFieldsV6 {
                unknown_reasons: vec!["verifier_timeout".into()],
                ..base.clone()
            },
            root("ledger"),
            range(),
        )
        .unwrap(),
        ZeroExecuteResultV6::baseline_fallback_required(
            ZeroExecuteFieldsV6 {
                unknown_reasons: vec!["missing_premise".into()],
                ..base.clone()
            },
            root("ledger"),
            range(),
        )
        .unwrap(),
        ZeroExecuteResultV6::rejected_no_mutation(
            ZeroExecuteFieldsV6 {
                no_mutation_receipt_root: Some(root("no_mutation")),
                ..base.clone()
            },
            root("ledger"),
            range(),
        )
        .unwrap(),
        ZeroExecuteResultV6::cancelled(base.clone(), root("ledger"), range()).unwrap(),
        ZeroExecuteResultV6::failed_no_authority(base, root("ledger"), range()).unwrap(),
    ]
}

#[test]
fn every_base_and_adapter_kind_validates_against_the_v6_schema() {
    let validator = schema();
    for envelope in all_envelopes() {
        let value = serde_json::to_value(&envelope).expect("envelope serializes");
        if envelope.kind().is_v6_base_schema_kind() {
            validator
                .validate(&value)
                .unwrap_or_else(|error| {
                    panic!("schema rejected kind {}: {error}", envelope.kind().kind_name())
                });
        } else {
            // Cancelled / FailedNoAuthority are D5 adapter extensions: the
            // canonical schema enum does not contain them yet, so the
            // schema rejects them loudly -- they must never be laundered as
            // schema-valid base outcomes.
            assert!(
                validator.validate(&value).is_err(),
                "schema accepted D5 extension kind {}",
                envelope.kind().kind_name()
            );
        }
    }
}

#[test]
fn schema_rejects_tampered_envelopes_and_serde_agrees() {
    let validator = schema();
    let envelope = ZeroExecuteResultV6::completed(
        safe_verdict(),
        ZeroExecuteFieldsV6 {
            successor_root: Some(root("succ")),
            result_root: Some(root("result")),
            verification_receipt_root: Some(root("verif")),
            ..ZeroExecuteFieldsV6::default()
        },
        root("ledger"),
        range(),
    )
    .unwrap();

    // Missing schema-required resource_ledger_root.
    let mut no_ledger = serde_json::to_value(&envelope).unwrap();
    no_ledger.as_object_mut().unwrap().remove("resource_ledger_root");
    assert!(validator.validate(&no_ledger).is_err(), "schema accepted missing ledger");
    assert!(
        serde_json::from_value::<ZeroExecuteResultV6>(no_ledger).is_err(),
        "serde accepted missing ledger"
    );

    // Wrong abi_version is rejected by schema const.
    let mut wrong_abi = serde_json::to_value(&envelope).unwrap();
    wrong_abi["abi_version"] = json!("zerostack.racc.v5");
    assert!(validator.validate(&wrong_abi).is_err(), "schema accepted wrong abi_version");
    assert!(
        serde_json::from_value::<ZeroExecuteResultV6>(wrong_abi)
            .unwrap()
            .validate()
            .is_err()
    );

    // Unknown kind string rejected by schema enum.
    let mut bad_kind = serde_json::to_value(&envelope).unwrap();
    bad_kind["kind"] = json!("PrivatelyCompleted");
    assert!(validator.validate(&bad_kind).is_err(), "schema accepted unknown kind");
    assert!(
        serde_json::from_value::<ZeroExecuteResultV6>(bad_kind).is_err(),
        "serde accepted unknown kind"
    );

    // Extra unknown field rejected by schema additionalProperties:false and
    // by serde deny_unknown_fields.
    let mut extra = serde_json::to_value(&envelope).unwrap();
    extra["future_field"] = json!(1);
    assert!(validator.validate(&extra).is_err(), "schema accepted extra field");
    assert!(
        serde_json::from_value::<ZeroExecuteResultV6>(extra).is_err(),
        "serde accepted extra field"
    );
}

#[test]
fn kind_enum_spelling_matches_the_schema_enum() {
    let _validator = schema();
    for kind in [
        ZeroExecuteKindV6::Completed,
        ZeroExecuteKindV6::DecisionRequired,
        ZeroExecuteKindV6::EvidenceExpansionRequired,
        ZeroExecuteKindV6::VerificationUnknown,
        ZeroExecuteKindV6::BaselineFallbackRequired,
        ZeroExecuteKindV6::RejectedNoMutation,
        ZeroExecuteKindV6::Cancelled,
        ZeroExecuteKindV6::FailedNoAuthority,
    ] {
        // The serialized kind string must be a valid value for the schema's
        // kind field; the six base kinds are in the schema enum, the two
        // adapter extensions are not.
        let property = json!({"kind": kind.kind_name()});
        let kind_schema = &schema_value()["properties"]["kind"];
        let enum_values = kind_schema["enum"].as_array().expect("kind has enum");
        let contained = enum_values
            .iter()
            .any(|value| value.as_str() == Some(kind.kind_name()));
        assert_eq!(
            contained,
            kind.is_v6_base_schema_kind(),
            "kind {} base-schema classification mismatch",
            kind.kind_name()
        );
        let _ = property;
    }
    assert_eq!(ZERO_EXECUTE_ABI_VERSION_V6, "zerostack.racc.v6");
}

#[test]
fn d5_forbidden_continuation_transitions_hold() {
    use ContinuationStateV1::{Authorized, Committed, Executing, Unknown, WaitingDecision};
    for policy in [false, true] {
        assert!(!ContinuationStateV1::allowed_transition(Unknown, Authorized, policy));
        assert!(!ContinuationStateV1::allowed_transition(Executing, Committed, policy));
        assert!(!ContinuationStateV1::allowed_transition(WaitingDecision, Executing, policy));
    }
    assert!(!ContinuationStateV1::allowed_transition(WaitingDecision, Committed, true));
    assert_eq!(
        ZeroExecuteErrorV6::InvalidAuditRange { start: 5, end: 2 },
        AuditEventRangeV1::new(5, 2).unwrap_err()
    );
}
