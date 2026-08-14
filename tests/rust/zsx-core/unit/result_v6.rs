//! Unit tests for the V6-R1 session envelope projection (`result_v6`):
//! every session-producible kind is constructible and round-trips through
//! the fail-closed V6 envelope, the context rejects fabricated anchors, and
//! the explicit legacy conversion keeps the pre-V6 shape and codes.

use super::*;
use zero_abi::{
    AuditEventRangeV1, DecisionRequiredV1, ObservationClassV1, SafetyVerdictV1,
    ZeroExecuteErrorV6, ZeroExecuteKindV6, ZeroExecuteResultV6, ZeroExecuteFieldsV6,
};

fn ledger() -> SessionEnvelopeContextV1 {
    SessionEnvelopeContextV1::new("a".repeat(64), AuditEventRangeV1::new(1, 1).unwrap()).unwrap()
}

fn decision_payload() -> DecisionRequiredV1 {
    DecisionRequiredV1 {
        decision_id: "dec:1".into(),
        observation_class: ObservationClassV1 {
            class_id: "branch.test_suite".into(),
        },
        question: "which test strategy?".into(),
        choices: vec!["run_fast".into(), "run_full".into()],
        observed_value: "fast".into(),
    }
}

fn round_trip(envelope: &ZeroExecuteResultV6) -> ZeroExecuteResultV6 {
    let json = serde_json::to_value(envelope).expect("envelope serializes");
    let decoded: ZeroExecuteResultV6 = serde_json::from_value(json).expect("envelope deserializes");
    decoded.validate().expect("decoded envelope validates");
    decoded
}

#[test]
fn context_rejects_fabricated_anchors_fail_closed() {
    let range = AuditEventRangeV1::new(1, 1).unwrap();
    let empty_root = SessionEnvelopeContextV1::new("", range.clone());
    assert!(
        empty_root.is_err(),
        "an empty resource ledger root must be rejected: the session never fabricates an anchor"
    );
    let reversed = SessionEnvelopeContextV1::new(
        "a".repeat(64),
        // Direct struct literal: `new` rejects the reversed range up front,
        // but the context must also fail closed on a struct literal.
        AuditEventRangeV1 { start: 9, end: 3 },
    );
    assert!(
        reversed.is_err(),
        "an invalid audit range must be rejected"
    );
}

#[test]
fn decision_required_envelope_is_kind_correct_and_round_trips() {
    let envelope = decision_required(
        &decision_payload(),
        1,
        7,
        Some("/repo"),
        &ledger(),
    )
    .expect("decision required envelope builds");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::DecisionRequired);
    assert_eq!(envelope.question(), Some("which test strategy?"));
    assert_eq!(
        envelope.choices(),
        &[serde_json::json!("run_fast"), serde_json::json!("run_full")]
    );
    assert_eq!(
        envelope.continuation_handle(),
        Some("zsx://g1-r7/dec:1"),
        "the decision id is bound as the opaque continuation handle"
    );
    assert_eq!(envelope.project_root(), Some("/repo"));
    assert_eq!(envelope.abi_version(), zero_abi::ZERO_EXECUTE_ABI_VERSION_V6);
    let decoded = round_trip(&envelope);
    assert_eq!(decoded, envelope);
}

#[test]
fn decision_required_rejects_missing_payload_fields() {
    let mut payload = decision_payload();
    payload.choices.clear();
    let error = decision_required(&payload, 1, 7, None, &ledger())
        .expect_err("empty choices must fail closed");
    assert_eq!(error, ZeroExecuteErrorV6::EmptyChoices);

    let mut payload = decision_payload();
    payload.question = String::new();
    // A blank question is still Some; the constructor only rejects None, so
    // this must still build -- the honesty boundary is the typed payload,
    // not prose length.
    let envelope = decision_required(&payload, 1, 7, None, &ledger()).expect("blank question builds");
    assert_eq!(envelope.question(), Some(""));
}

#[test]
fn cancelled_envelope_round_trips_and_rejects_successor_root() {
    let envelope = cancelled(Some("/repo"), &ledger()).expect("cancelled envelope builds");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::Cancelled);
    assert_eq!(envelope.project_root(), Some("/repo"));
    assert_eq!(round_trip(&envelope), envelope);

    // Fail-closed adapter outcome: a Cancelled envelope must not carry a
    // successor root, even if injected into the JSON wire shape.
    let mut json = serde_json::to_value(&envelope).expect("serializes");
    json["successor_root"] = serde_json::json!("fz://blob/injected");
    let tampered: ZeroExecuteResultV6 =
        serde_json::from_value(json).expect("wire shape still deserializes");
    assert_eq!(
        tampered.validate().expect_err("successor root must be rejected"),
        ZeroExecuteErrorV6::ForbiddenRoot("successor_root")
    );
}

#[test]
fn failed_no_authority_envelope_round_trips() {
    let envelope =
        failed_no_authority(Some("/repo"), &ledger()).expect("failed no authority envelope builds");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::FailedNoAuthority);
    assert_eq!(round_trip(&envelope), envelope);
}

#[test]
fn legacy_conversion_preserves_shape_and_codes_for_known_kinds() {
    let decision = decision_required(&decision_payload(), 1, 7, Some("/repo"), &ledger()).unwrap();
    let legacy = legacy_envelope_value(&decision, 1, 7);
    assert_eq!(legacy["protocol"], serde_json::json!("zerostack.zsx.v1"));
    assert_eq!(legacy["ok"], serde_json::json!(false));
    assert_eq!(legacy["generation"], serde_json::json!(1));
    assert_eq!(legacy["request_id"], serde_json::json!(7));
    assert_eq!(legacy["error"]["code"], serde_json::json!("decision_required"));
    assert_eq!(
        legacy["result"]["question"],
        serde_json::json!("which test strategy?")
    );
    assert_eq!(legacy["result"]["continuation_handle"], serde_json::json!("zsx://g1-r7/dec:1"));

    let cancelled = cancelled(None, &ledger()).unwrap();
    let legacy = legacy_envelope_value(&cancelled, 1, 7);
    assert_eq!(legacy["ok"], serde_json::json!(false));
    assert_eq!(legacy["error"]["code"], serde_json::json!("cancelled"));
    assert!(legacy.get("result").is_none());
}

#[test]
fn legacy_conversion_only_completed_is_ok() {
    let fields = ZeroExecuteFieldsV6 {
        successor_root: Some("fz://blob/successor".into()),
        result_root: Some("fz://blob/result".into()),
        verification_receipt_root: Some("fz://blob/verification".into()),
        project_root: Some("/repo".into()),
        ..Default::default()
    };
    let completed = ZeroExecuteResultV6::completed(
        SafetyVerdictV1::Safe,
        fields,
        "a".repeat(64),
        AuditEventRangeV1::new(1, 1).unwrap(),
    )
    .expect("completed envelope builds with a Safe verdict and roots");
    let legacy = legacy_envelope_value(&completed, 1, 7);
    assert_eq!(legacy["ok"], serde_json::json!(true));
    assert!(legacy.get("error").is_none());

    // The completed envelope round-trips through the fail-closed envelope.
    assert_eq!(round_trip(&completed), completed);
}

#[test]
fn legacy_kind_code_is_snake_case_for_every_kind() {
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::Completed), "completed");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::DecisionRequired), "decision_required");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::EvidenceExpansionRequired), "evidence_expansion_required");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::VerificationUnknown), "verification_unknown");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::BaselineFallbackRequired), "baseline_fallback_required");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::RejectedNoMutation), "rejected_no_mutation");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::Cancelled), "cancelled");
    assert_eq!(legacy_kind_code(ZeroExecuteKindV6::FailedNoAuthority), "failed_no_authority");
}

// V6-R5 (ZS-VIEW-010): a decision-bearing outcome binds the typed decision
// view root when the harness supplied the view context; absent a context the
// envelope leaves `decision_view_root` empty -- a root is never fabricated.

use zero_abi::{CompletenessGradeV6, DecisionViewV6};

fn view_context() -> DecisionViewContextV1 {
    DecisionViewContextV1::new("tc://root/ab12", "cl://lens/34cd", false)
        .expect("view context builds")
        .with_evidence(vec!["fz://blob/evidence-a".into()], vec![])
        .expect("evidence binds")
        .with_expansions(vec!["exp:1".into()])
        .expect("expansions bind")
}

#[test]
fn decision_required_with_view_context_binds_decision_view_root() {
    let context = view_context();
    let ledger = ledger()
        .with_decision_view(context.clone())
        .expect("context attaches");
    let envelope = decision_required(&decision_payload(), 1, 7, Some("/repo"), &ledger)
        .expect("decision required envelope builds with a view");

    let bound_root = envelope
        .decision_view_root()
        .expect("decision view root is bound");
    assert_eq!(bound_root.len(), 64, "root is lowercase hex sha256");

    // The session states only what it can prove: the decision that surfaced,
    // the unresolved question, and an Observed grade. Rebuild the same view
    // from payload + context and verify the envelope bound exactly its
    // digest root.
    let view = DecisionViewV6::new(
        context.task_contract_root,
        "/repo",
        context.causal_lens_root,
        vec!["dec:1".into()],
        context.evidence_refs,
        context.omitted_classes,
        context.expansion_handles,
        CompletenessGradeV6::Observed,
        Some("which test strategy?".into()),
        context.baseline_escape,
        None,
    )
    .expect("rebuilt view builds");
    assert_eq!(bound_root, view.root());
    view.verify_root(bound_root).expect("bound root verifies");

    // The root survives the envelope JSON round trip.
    let decoded = round_trip(&envelope);
    assert_eq!(
        decoded.decision_view_root(),
        Some(bound_root),
        "decision view root survives the round trip"
    );
}

#[test]
fn decision_required_without_view_context_leaves_decision_view_root_absent() {
    let envelope = decision_required(&decision_payload(), 1, 7, Some("/repo"), &ledger())
        .expect("decision required envelope builds");
    assert!(
        envelope.decision_view_root().is_none(),
        "without a harness-supplied view context no root may be fabricated"
    );
}

#[test]
fn view_context_rejects_fabricated_roots_fail_closed() {
    assert!(
        DecisionViewContextV1::new("", "cl://lens/34cd", false).is_err(),
        "an empty task contract root must be rejected"
    );
    assert!(
        DecisionViewContextV1::new("tc://root/ab12", "", false).is_err(),
        "an empty causal lens root must be rejected"
    );

    let mut bad = view_context();
    bad.evidence_refs = vec!["".into()];
    assert!(
        ledger().with_decision_view(bad).is_err(),
        "an empty evidence ref must fail at the envelope context"
    );
}
