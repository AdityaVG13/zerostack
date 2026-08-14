//! Unit tests for the typed `DecisionViewV6` (ZS-VIEW-010): schema-golden
//! wire shape, canonical round-trip with stable digest root, tamper
//! detection, completeness certification (removing a needed evidence class
//! yields `Unknown` or a failed `Proved` certificate), and exact expansion
//! reproducing the bound object.

use std::collections::BTreeSet;

use serde_json::json;

use super::*;

fn view() -> DecisionViewV6 {
    DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec!["fz://blob/evidence-a".into()],
        vec![],
        vec!["exp:1".into()],
        CompletenessGradeV6::Observed,
        Some("which test strategy?".into()),
        false,
        None,
    )
    .expect("golden view builds")
}

fn present(classes: &[&str]) -> BTreeSet<String> {
    classes.iter().map(|c| c.to_string()).collect()
}

#[test]
fn golden_wire_shape_matches_schema_and_renders_canonically() {
    let view = view();
    // Schema-golden: the canonical bounded rendering is byte-exact, sorted
    // keys, field-for-field per decision_view_v6.schema.json. Tautological
    // construction would defeat the golden, so the expected string is
    // written out by hand.
    assert_eq!(
        view.canonical_render_json(),
        "{\"baseline_escape\":false,\"canonical_render_root\":null,\
         \"causal_lens_root\":\"cl://lens/34cd\",\"completeness_grade\":\"Observed\",\
         \"evidence_refs\":[\"fz://blob/evidence-a\"],\"expansion_handles\":[\"exp:1\"],\
         \"omitted_classes\":[],\"project_root\":\"/repo\",\
         \"supported_decisions\":[\"dec:1\"],\"task_contract_root\":\"tc://root/ab12\",\
         \"unresolved_question\":\"which test strategy?\"}"
    );

    // The render is exactly the schema's property set: no field outside the
    // contract and every required field present.
    let schema: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../racc/v6/schemas/decision_view_v6.schema.json"
    )))
    .expect("schema parses");
    let properties = schema["properties"].as_object().expect("properties object");
    let rendered = view.canonical_render();
    let rendered = rendered.as_object().expect("render is an object");
    for key in rendered.keys() {
        assert!(
            properties.contains_key(key),
            "rendered field {key} is not in the schema"
        );
    }
    let required = schema["required"]
        .as_array()
        .expect("required array")
        .iter()
        .map(|value| value.as_str().expect("required name"))
        .collect::<Vec<_>>();
    for key in required {
        assert!(rendered.contains_key(key), "required field {key} is missing");
    }
}

#[test]
fn canonical_round_trip_preserves_render_and_digest_root() {
    let built = view();
    let root = built.root();
    assert_eq!(root.len(), 64, "root is lowercase hex sha256");

    let json = serde_json::to_value(&built).expect("view serializes");
    let decoded: DecisionViewV6 = serde_json::from_value(json).expect("view deserializes");
    decoded.validate().expect("decoded view validates");
    assert_eq!(decoded, built);
    assert_eq!(
        decoded.canonical_render_json(),
        built.canonical_render_json()
    );
    assert_eq!(decoded.root(), root, "digest root survives the round trip");

    // Digest stability: a second build with identical inputs yields the same
    // root; the digest is a pure function of the canonical rendering.
    let rebuilt = view();
    assert_eq!(rebuilt.root(), root);
}

#[test]
fn tampered_view_root_is_detected() {
    let view = view();
    let root = view.root();

    // Flip a valid field: the tampered view still deserializes and validates
    // (the mutation is structurally legal) but the root no longer matches.
    let mut json = serde_json::to_value(&view).expect("view serializes");
    json["baseline_escape"] = json!(true);
    let tampered: DecisionViewV6 = serde_json::from_value(json).expect("tampered view deserializes");
    tampered.validate().expect("tampered view is structurally valid");
    assert_eq!(
        tampered.verify_root(&root).expect_err("root must not match"),
        DecisionViewErrorV6::RootMismatch
    );
    assert_ne!(tampered.root(), root);

    // An injected unknown field violates the schema's
    // `additionalProperties: false` and must be rejected on deserialization.
    let mut json = serde_json::to_value(&view).expect("view serializes");
    json["hacked"] = json!(true);
    assert!(
        serde_json::from_value::<DecisionViewV6>(json).is_err(),
        "unknown fields must be rejected exactly like the schema"
    );
}

#[test]
fn removing_needed_evidence_class_degrades_grade_to_unknown() {
    let view = view();
    let needed = present(&["branch.test_suite", "api.breaking_change"]);

    // All needed classes present: the claimed grade is returned unchanged.
    assert_eq!(
        view.certificate(&needed, &present(&["branch.test_suite", "api.breaking_change"]))
            .expect("full evidence certifies"),
        CompletenessGradeV6::Observed
    );

    // Removing a needed evidence class: no silent grade retention -- the
    // verified grade degrades to Unknown.
    assert_eq!(
        view.certificate(&needed, &present(&["branch.test_suite"]))
            .expect("degraded certificate is still a certificate"),
        CompletenessGradeV6::Unknown
    );
}

#[test]
fn proved_claim_with_missing_class_fails_certificate() {
    let view = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec!["fz://blob/evidence-a".into()],
        vec![],
        vec![],
        CompletenessGradeV6::Proved,
        None,
        false,
        None,
    )
    .expect("proved view builds");
    let needed = present(&["branch.test_suite", "api.breaking_change"]);
    let error = view
        .certificate(&needed, &present(&["branch.test_suite"]))
        .expect_err("a Proved claim with missing evidence must fail the certificate");
    assert_eq!(
        error,
        DecisionViewErrorV6::MissingEvidenceClass {
            class: "api.breaking_change".into()
        }
    );
}

#[test]
fn proved_claim_with_omissions_or_without_evidence_fails_certificate() {
    let omitted = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec!["fz://blob/evidence-a".into()],
        vec!["branch.test_suite".into()],
        vec![],
        CompletenessGradeV6::Proved,
        None,
        false,
        None,
    )
    .expect("view with declared omission builds");
    let needed = present(&["branch.test_suite"]);
    assert_eq!(
        omitted
            .certificate(&needed, &needed)
            .expect_err("Proved with omissions must fail"),
        DecisionViewErrorV6::ProvedClaimWithOmissions
    );

    let no_evidence = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec![],
        CompletenessGradeV6::Proved,
        None,
        false,
        None,
    )
    .expect("view without evidence builds");
    assert_eq!(
        no_evidence
            .certificate(&needed, &needed)
            .expect_err("Proved without evidence must fail"),
        DecisionViewErrorV6::ProvedClaimWithoutEvidence
    );
}

#[test]
fn exact_expansion_reproduces_the_bound_object() {
    let view = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec!["exp:1".into()],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect("view with expansion handle builds");
    let bound = json!({"plan": "run_full", "cost": 2, "refs": ["fz://blob/expansion"]});
    let binding = DecisionViewBindingV6::new(view, vec![("exp:1".into(), bound.clone())])
        .expect("binding binds the listed handle");

    let expanded = binding.expand_exact("exp:1").expect("exact expansion hits");
    assert_eq!(
        canonical_json(&expanded),
        canonical_json(&bound),
        "exact expansion reproduces the bound object byte-exactly"
    );
    assert_eq!(binding.root(), binding.view().root());
}

#[test]
fn unknown_expansion_handle_is_a_typed_miss() {
    let view = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec!["exp:1".into()],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect("view builds");
    let binding = DecisionViewBindingV6::new(view, vec![("exp:1".into(), json!({"plan": "run_full"}))])
        .expect("binding builds");
    assert_eq!(
        binding
            .expand_exact("exp:missing")
            .expect_err("unbound handle must be a typed miss"),
        DecisionViewErrorV6::UnknownExpansionHandle("exp:missing".into())
    );
}

#[test]
fn binding_rejects_unlisted_and_unbound_handles() {
    let view = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec!["exp:1".into(), "exp:2".into()],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect("view with two handles builds");

    let unlisted = DecisionViewBindingV6::new(
        view.clone(),
        vec![("exp:1".into(), json!({"a": 1})), ("exp:9".into(), json!({"b": 2}))],
    );
    assert_eq!(
        unlisted.expect_err("unlisted handle must be rejected"),
        DecisionViewErrorV6::ExpansionHandleNotListed("exp:9".into())
    );

    let dangling = DecisionViewBindingV6::new(view, vec![("exp:1".into(), json!({"a": 1}))]);
    assert_eq!(
        dangling.expect_err("listed but unbound handle is a dangling claim"),
        DecisionViewErrorV6::UnboundExpansionHandle("exp:2".into())
    );
}

#[test]
fn validation_rejects_fabricated_roots_and_degenerate_views() {
    let error = DecisionViewV6::new(
        "",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec![],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect_err("empty task contract root must be rejected");
    assert_eq!(error, DecisionViewErrorV6::EmptyRoot("task_contract_root"));

    let error = DecisionViewV6::new(
        "tc://root/ab12",
        "",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec![],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect_err("empty project root must be rejected");
    assert_eq!(error, DecisionViewErrorV6::EmptyRoot("project_root"));

    let error = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "",
        vec!["dec:1".into()],
        vec![],
        vec![],
        vec![],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect_err("empty causal lens root must be rejected");
    assert_eq!(error, DecisionViewErrorV6::EmptyRoot("causal_lens_root"));

    let error = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec![],
        vec![],
        vec![],
        vec![],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect_err("a view supporting no decisions must be rejected");
    assert_eq!(error, DecisionViewErrorV6::EmptySupportedDecisions);

    let error = DecisionViewV6::new(
        "tc://root/ab12",
        "/repo",
        "cl://lens/34cd",
        vec!["dec:1".into()],
        vec!["".into()],
        vec![],
        vec![],
        CompletenessGradeV6::Observed,
        None,
        false,
        None,
    )
    .expect_err("an empty evidence ref must be rejected");
    assert_eq!(error, DecisionViewErrorV6::EmptyListEntry("evidence_refs"));
}

#[test]
fn grade_names_match_the_schema_enum() {
    assert_eq!(CompletenessGradeV6::Proved.grade_name(), "Proved");
    assert_eq!(
        CompletenessGradeV6::BoundedComplete.grade_name(),
        "BoundedComplete"
    );
    assert_eq!(CompletenessGradeV6::Observed.grade_name(), "Observed");
    assert_eq!(CompletenessGradeV6::Unknown.grade_name(), "Unknown");
}
