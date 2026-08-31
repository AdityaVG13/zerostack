use proptest::prelude::*;

use graphzero_engine::blast::parse_intent;

fn arb_identifier() -> impl Strategy<Value = String> {
    "[a-zA-Z_][a-zA-Z0-9_]{0,40}".prop_map(|s| s)
}

fn arb_intent_prefix() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("change signature of ".to_string()),
        Just("change ".to_string()),
        Just("".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests cannot use SourceParallel (no lib.rs above tests/),
        // so pin persistence to the committed crate-local layout explicitly.
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/proptest-regressions/tests/blast_proptest.txt"
            )),
        )),
        ..ProptestConfig::with_cases(256)
    })]

    #[test]
    fn parse_intent_never_panics(input in "\\PC{0,200}") {
        let _ = parse_intent(&input);
    }

    #[test]
    fn valid_ident_always_resolves(
        prefix in arb_intent_prefix(),
        ident in arb_identifier(),
    ) {
        let intent = format!("{prefix}{ident}");
        let parsed = parse_intent(&intent);
        prop_assert!(
            parsed.target_symbol.is_some(),
            "failed to extract symbol from: {:?}",
            intent
        );
        let sym = parsed.target_symbol.unwrap();
        prop_assert!(
            sym.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "symbol contains invalid chars: {:?}",
            sym
        );
    }

    #[test]
    fn parse_preserves_case(ident in arb_identifier()) {
        let intent = format!("change signature of {ident}");
        let parsed = parse_intent(&intent);
        if let Some(sym) = &parsed.target_symbol {
            prop_assert_eq!(sym, &ident, "case was not preserved");
        }
    }

    #[test]
    fn successful_parse_has_target_ref(
        prefix in arb_intent_prefix(),
        ident in arb_identifier(),
    ) {
        let intent = format!("{prefix}{ident}");
        let parsed = parse_intent(&intent);
        if parsed.error.is_none() {
            prop_assert!(parsed.target_ref.is_some());
            let r = parsed.target_ref.unwrap();
            prop_assert!(r.starts_with("node/"));
        }
    }

    #[test]
    fn parse_intent_idempotent_on_symbol(
        prefix in arb_intent_prefix(),
        ident in arb_identifier(),
    ) {
        let intent = format!("{prefix}{ident}");
        let p1 = parse_intent(&intent);
        let p2 = parse_intent(&intent);
        prop_assert_eq!(p1, p2);
    }
}

#[test]
fn retired_graph_aliases_fail_closed() {
    assert!(graphzero_engine::world_envelope::canonicalize_world_ref("fz://world/W1").is_err());

    let dir = tempfile::tempdir().unwrap();
    let queries = dir.path().join("queries");
    std::fs::create_dir_all(&queries).unwrap();
    std::fs::write(
        queries.join("a.json"),
        r#"{"schema":"graphzero.page","kind":"test","payload":{}}"#,
    )
    .unwrap();
    assert!(graphzero_engine::query_surface::load_page(dir.path(), "query/a").is_some());
    assert!(graphzero_engine::query_surface::load_page(dir.path(), "gz://query/a").is_none());
}

#[test]
fn retired_witness_aliases_fail_closed() {
    use graphzero_engine::witness_cache::{CacheRoot, RootResolution, RootResolver, SnapshotRoots};

    let fixture = zerostack_test_support::basic_indexed_repo();
    let snapshot = graphzero_store::Snapshot::open(&fixture.store_root, None).unwrap();
    let roots = SnapshotRoots::from_snapshot(&snapshot);
    let canonical = roots.toolchain_root().unwrap();
    assert_eq!(roots.resolve(&canonical), RootResolution::Unchanged);

    let retired = CacheRoot::new(format!("gz://{}", canonical.as_str())).unwrap();
    assert_eq!(roots.resolve(&retired), RootResolution::Unresolvable);
}
