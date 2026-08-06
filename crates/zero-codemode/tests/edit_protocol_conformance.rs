//! Zero Edit Protocol v1 conformance: round-trip, rejection, Level-0 fallback.

use zero_codemode::{
    EDIT_PROTOCOL_VERSION, EditErrorClass, EditOp, EditPlan, RefKind, Side, classify_ref,
};

fn every_verb() -> Vec<EditOp> {
    vec![
        EditOp::Read {
            r: "src/lib.rs#L1-L20".into(),
        },
        EditOp::Replace {
            r: "src/lib.rs#L4-L6".into(),
            text: "let x = 1;\n".into(),
        },
        EditOp::Insert {
            at: "src/lib.rs#L9-L9".into(),
            text: "// note\n".into(),
            side: Side::Before,
        },
        EditOp::Delete {
            r: format!("fz://blob/{}#L2-L3", "a".repeat(64)),
        },
        EditOp::Move {
            from: "src/old.rs".into(),
            to: "src/new.rs".into(),
        },
        EditOp::Copy {
            from: "src/a.rs".into(),
            to: "src/b.rs".into(),
        },
        EditOp::Rename {
            sym: "gz://node/parse_ref".into(),
            to: "parse_target_ref".into(),
        },
        EditOp::ApplyPatch {
            base: "src/lib.rs".into(),
            patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
        },
        EditOp::Run {
            cmd: "cargo check -p zero-codemode".into(),
        },
    ]
}

#[test]
fn every_verb_round_trips_through_serde() {
    for op in every_verb() {
        let json = serde_json::to_string(&op).expect("serialize");
        let back: EditOp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(op, back, "round-trip mismatch for {}", op.verb());
        assert!(json.contains(op.verb()), "verb tag missing in {json}");
        op.validate().expect("fixture must validate");
    }
}

#[test]
fn plan_round_trips_and_validates() {
    let plan = EditPlan::new(every_verb());
    let json = serde_json::to_string(&plan).expect("serialize");
    let back = EditPlan::parse(&json).expect("parse");
    assert_eq!(plan, back);
    assert_eq!(back.p, EDIT_PROTOCOL_VERSION);
    assert_eq!(back.ops.len(), 9, "v1 defines exactly nine verbs");
}

#[test]
fn version_tag_defaults_on_input() {
    let plan = EditPlan::parse(r#"{"ops":[{"v":"DELETE","r":"a.rs#L1-L2"}]}"#).expect("parse");
    assert_eq!(plan.p, EDIT_PROTOCOL_VERSION);
}

#[test]
fn every_verb_has_a_level0_fallback() {
    for op in every_verb() {
        let rendered = op.level0();
        assert!(!rendered.is_empty(), "{} has no Level-0 form", op.verb());
        assert!(
            rendered.starts_with(&op.verb().to_lowercase()),
            "{} Level-0 form must name its verb: {rendered}",
            op.verb()
        );
    }
    assert_eq!(
        EditPlan::new(every_verb()).level0().lines().count() >= 9,
        true
    );
}

#[test]
fn ref_grammar_matches_the_existing_ones() {
    assert_eq!(
        classify_ref("src/lib.rs#L1-L20").unwrap(),
        RefKind::FileSpan
    );
    assert_eq!(
        classify_ref(&format!("fz://blob/{}#B0-4", "e".repeat(64))).unwrap(),
        RefKind::BlobSpan
    );
    assert_eq!(
        classify_ref("gz://node/parse_ref").unwrap(),
        RefKind::Symbol
    );
    assert_eq!(
        classify_ref("gz://blob/deadbeef").unwrap(),
        RefKind::BlobSpan
    );
    assert_eq!(classify_ref("src/lib.rs").unwrap(), RefKind::Path);
}

#[test]
fn malformed_refs_are_rejected() {
    for bad in [
        "src/lib.rs#L0-L3",
        "src/lib.rs#L9-L3",
        "src/lib.rs#L1",
        "#L1-L2",
        "gz://node/",
        "gz://mystery/x",
        "fz://object/abc",
        "http://example.com/a",
        "",
    ] {
        let err = classify_ref(bad).unwrap_err();
        assert_eq!(err.class, EditErrorClass::MalformedRef, "{bad} -> {err}");
    }
}

#[test]
fn wrong_ref_kind_in_a_slot_is_rejected() {
    let err = EditOp::Replace {
        r: "gz://node/foo".into(),
        text: "x".into(),
    }
    .validate()
    .unwrap_err();
    assert_eq!(err.class, EditErrorClass::RefKindMismatch);

    let err = EditOp::Rename {
        sym: "src/lib.rs#L1-L2".into(),
        to: "x".into(),
    }
    .validate()
    .unwrap_err();
    assert_eq!(err.class, EditErrorClass::RefKindMismatch);

    let err = EditOp::Move {
        from: "src/a.rs".into(),
        to: "src/b.rs#L1-L2".into(),
    }
    .validate()
    .unwrap_err();
    assert_eq!(err.class, EditErrorClass::RefKindMismatch);
}

#[test]
fn empty_required_fields_are_rejected() {
    let err = EditOp::Run { cmd: String::new() }.validate().unwrap_err();
    assert_eq!(err.class, EditErrorClass::EmptyField);

    let err = EditOp::Rename {
        sym: "gz://node/a".into(),
        to: String::new(),
    }
    .validate()
    .unwrap_err();
    assert_eq!(err.class, EditErrorClass::EmptyField);

    let err = EditOp::ApplyPatch {
        base: "a.rs".into(),
        patch: String::new(),
    }
    .validate()
    .unwrap_err();
    assert_eq!(err.class, EditErrorClass::EmptyField);
}

#[test]
fn unknown_verbs_and_versions_are_rejected() {
    assert!(serde_json::from_str::<EditOp>(r#"{"v":"NUKE","r":"a.rs"}"#).is_err());
    assert!(serde_json::from_str::<EditOp>(r#"{"v":"REPLACE","r":"a.rs#L1-L2"}"#).is_err());
    assert!(EditPlan::parse(r#"{"ops":[{"v":"REPLACE","r":"gz://node/a","text":"x"}]}"#).is_err());

    let err = EditPlan::parse(r#"{"p":"zep/2","ops":[]}"#).unwrap_err();
    assert_eq!(err.class, EditErrorClass::UnsupportedVersion);
}

#[test]
fn default_side_is_omitted_from_the_wire() {
    let op = EditOp::Insert {
        at: "a.rs#L1-L1".into(),
        text: "x\n".into(),
        side: Side::After,
    };
    let json = serde_json::to_string(&op).unwrap();
    assert!(
        !json.contains("side"),
        "default side must not cost tokens: {json}"
    );
    assert_eq!(serde_json::from_str::<EditOp>(&json).unwrap(), op);
}
