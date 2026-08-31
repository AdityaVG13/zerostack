use fszero_core::edit_spec::{EditTarget, parse_edit_spec};

#[test]
fn product_schemes_fail_closed_on_live_edit_parse() {
    for scheme in ["fz://blob/aa", "gz://blob/aa", "tz://blob/aa"] {
        let spec = format!("{scheme}:old|new");
        let err = parse_edit_spec(&spec).expect_err(scheme);
        assert!(err.contains("retired product scheme"), "{scheme} err={err}");
    }
}

#[test]
fn z_blob_classifies_as_content_ref() {
    let hash = "a".repeat(64);
    let spec = parse_edit_spec(&format!("z://blob/{hash}:old|new")).unwrap();
    match spec.target {
        EditTarget::ContentRef(r) => assert!(r.starts_with("z://blob/")),
        other => panic!("expected ContentRef, got {other:?}"),
    }
}

#[test]
fn unprefixed_path_still_parses() {
    let spec = parse_edit_spec("src/lib.rs:old|new").unwrap();
    match spec.target {
        EditTarget::Path(p) => assert_eq!(p, "src/lib.rs"),
        other => panic!("expected Path, got {other:?}"),
    }
}
