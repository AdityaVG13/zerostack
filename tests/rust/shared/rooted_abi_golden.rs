//! ZS-KERNEL-001/007 conformance: the rooted-ABI object-class registry is
//! pinned as a cross-release golden fixture (every class roots through the
//! one canonical byte path), and incompatible-version migration is explicit
//! and receipted -- never a silent reinterpretation of old bytes.

use serde_json::{Value, json};
use zero_abi::{
    DigestV1, IdentityErrorV1, ObjectClassV1, ROOTED_ABI_VERSION_V6,
    RootedAbiMigrationReceiptV1, canonical_object_bytes, object_root, root_preimage,
    verify_object_root,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/rooted_abi_golden_v6.json"
    ))
    .expect("fixture is valid JSON")
}

fn class_from_name(name: &str) -> ObjectClassV1 {
    match name {
        "task_contract" => ObjectClassV1::TaskContract,
        "protected_scope" => ObjectClassV1::ProtectedScope,
        "formation_receipt" => ObjectClassV1::FormationReceipt,
        "event_record" => ObjectClassV1::EventRecord,
        "successor_record" => ObjectClassV1::SuccessorRecord,
        "execute_result" => ObjectClassV1::ExecuteResult,
        "continuation_handle" => ObjectClassV1::ContinuationHandle,
        "continuation_compact_record" => ObjectClassV1::ContinuationCompactRecord,
        "decision_view" => ObjectClassV1::DecisionView,
        "delta" => ObjectClassV1::Delta,
        "authority_object" => ObjectClassV1::AuthorityObject,
        "migration_receipt" => ObjectClassV1::MigrationReceipt,
        other => panic!("fixture names unknown class {other}"),
    }
}

fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn every_object_class_roots_through_the_golden_byte_path() {
    let fixture = fixture();
    let objects = fixture["objects"].as_array().expect("objects array");
    assert!(
        objects.len() >= 12,
        "the registry must cover every rooted class"
    );
    let mut seen = std::collections::BTreeSet::new();
    for object in objects {
        let class = class_from_name(object["class"].as_str().expect("class"));
        let payload = object["payload"].clone();
        let expected_bytes = hex_decode(object["canonical_bytes_hex"].as_str().expect("bytes"));
        let expected_root =
            DigestV1::from_hex(object["root_hex"].as_str().expect("root")).expect("root hex");
        // One canonical byte path per class.
        let canonical = canonical_object_bytes(class, ROOTED_ABI_VERSION_V6, &payload)
            .expect("canonical bytes");
        assert_eq!(canonical, expected_bytes, "{} canonical bytes drifted", object["class"]);
        // The root binds class + ABI version + algorithm tag.
        let root = object_root(class, ROOTED_ABI_VERSION_V6, &canonical).expect("root");
        assert_eq!(root, expected_root, "{} root drifted", object["class"]);
        assert!(verify_object_root(
            class,
            ROOTED_ABI_VERSION_V6,
            &canonical,
            expected_root,
        ));
        assert!(!verify_object_root(
            class,
            ROOTED_ABI_VERSION_V6,
            &canonical,
            DigestV1::from_bytes([0xabu8; 32]),
        ));
        assert!(seen.insert(object["class"].as_str().expect("class")), "duplicate class entry");
    }
    assert_eq!(seen.len(), 12, "the registry must cover all twelve classes");
}

#[test]
fn registry_covers_views_deltas_and_authority_objects() {
    // ZS-KERNEL-001: TokenZero decision views, FSZero/GraphZero deltas, and
    // zero-gate authority objects are first-class rooted classes.
    for (name, class, domain) in [
        ("decision_view", ObjectClassV1::DecisionView, "zerostack.object.decision_view.v1"),
        ("delta", ObjectClassV1::Delta, "zerostack.object.delta.v1"),
        ("authority_object", ObjectClassV1::AuthorityObject, "zerostack.object.authority_object.v1"),
        ("migration_receipt", ObjectClassV1::MigrationReceipt, "zerostack.object.migration_receipt.v1"),
    ] {
        assert_eq!(class.domain(), domain);
        let payload = json!({"class": name});
        let canonical = canonical_object_bytes(class, ROOTED_ABI_VERSION_V6, &payload).expect("bytes");
        let root = object_root(class, ROOTED_ABI_VERSION_V6, &canonical).expect("root");
        assert!(verify_object_root(class, ROOTED_ABI_VERSION_V6, &canonical, root));
        // Same payload under another class: a different root (class-bound).
        let other = canonical_object_bytes(
            ObjectClassV1::TaskContract,
            ROOTED_ABI_VERSION_V6,
            &payload,
        )
        .expect("bytes");
        assert_ne!(
            object_root(ObjectClassV1::TaskContract, ROOTED_ABI_VERSION_V6, &other).expect("root"),
            root,
            "roots must be class-bound"
        );
    }
}

#[test]
fn migration_receipt_pins_legacy_root_and_v6_target_and_fails_closed_on_tamper() {
    // A legacy v5 object with its own root: the receipt must re-derive that
    // root from the legacy preimage, root the v6 replacement through the
    // canonical path, and pin both.
    let legacy_payload = json!({"legacy": "v5-task"});
    let legacy_canonical = zero_abi::canonical_json(&legacy_payload);
    let legacy_bytes = legacy_canonical.as_bytes().to_vec();
    let legacy_root = DigestV1::from_bytes(zero_abi::sha256(&root_preimage(
        ObjectClassV1::TaskContract,
        "zerostack.racc.v5",
        &legacy_bytes,
    )));
    let target_payload = json!({"task_kind": "port"});
    let receipt = RootedAbiMigrationReceiptV1::new(
        ObjectClassV1::TaskContract,
        "zerostack.racc.v5",
        &legacy_bytes,
        legacy_root,
        ObjectClassV1::TaskContract,
        &target_payload,
        "v6 contract completion: structured task contract fields",
    )
    .expect("migration receipt mints");
    receipt.validate().expect("receipt validates");
    let canonical = receipt.canonical_bytes().expect("receipt canonicalizes");
    let receipt_root = object_root(
        ObjectClassV1::MigrationReceipt,
        ROOTED_ABI_VERSION_V6,
        &canonical,
    )
    .expect("receipt root");
    assert_eq!(
        receipt.receipt_root().expect("receipt root"),
        receipt_root,
        "receipt roots through the golden byte path"
    );
    let round_tripped =
        RootedAbiMigrationReceiptV1::from_canonical_bytes(&canonical).expect("durable round trip");
    assert_eq!(round_tripped, receipt);

    // Tamper 1: forged source root is refused.
    let forged = RootedAbiMigrationReceiptV1::new(
        ObjectClassV1::TaskContract,
        "zerostack.racc.v5",
        &legacy_bytes,
        DigestV1::from_bytes([0x42u8; 32]),
        ObjectClassV1::TaskContract,
        &target_payload,
        "forged",
    );
    assert!(matches!(forged, Err(IdentityErrorV1::SourceRootMismatch)));

    // Tamper 2: a mutated stored receipt can never verify against the
    // original sealed root (any field change flips the receipt root).
    let mut mutated = receipt.clone();
    mutated.migration_reason = "relabeled".into();
    assert_ne!(
        mutated.receipt_root().unwrap(),
        receipt.receipt_root().unwrap()
    );
    assert!(!verify_object_root(
        ObjectClassV1::MigrationReceipt,
        ROOTED_ABI_VERSION_V6,
        &mutated.canonical_bytes().unwrap(),
        receipt.receipt_root().unwrap(),
    ));
    let mut mutated_bytes = receipt.clone();
    mutated_bytes.source_canonical_bytes_hex =
        hex_decode(receipt.target_canonical_bytes_hex.as_str())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
    assert!(matches!(
        mutated_bytes.validate(),
        Err(IdentityErrorV1::SourceRootMismatch)
    ));

    // Tamper 3: no ABI change is not a migration.
    assert!(matches!(
        RootedAbiMigrationReceiptV1::new(
            ObjectClassV1::TaskContract,
            ROOTED_ABI_VERSION_V6,
            &legacy_bytes,
            legacy_root,
            ObjectClassV1::TaskContract,
            &target_payload,
            "no-op",
        ),
        Err(IdentityErrorV1::MigrationWithoutAbiChange)
    ));

    // Tamper 4: wrong ABI on the target side fails closed.
    let mut wrong_target = receipt.clone();
    wrong_target.target_abi_version = "zerostack.racc.v5".into();
    assert!(matches!(
        wrong_target.validate(),
        Err(IdentityErrorV1::LegacyTargetAbi(_))
    ));
}

#[test]
fn golden_fixture_itself_round_trips_through_the_registry() {
    // The golden fixture's own entries must re-parse from their pinned
    // canonical bytes through the same canonical path (cross-release
    // stability: decode-identically across releases).
    let fixture = fixture();
    for object in fixture["objects"].as_array().expect("objects") {
        let class = class_from_name(object["class"].as_str().expect("class"));
        let expected_bytes = hex_decode(object["canonical_bytes_hex"].as_str().expect("bytes"));
        let reparsed: Value =
            serde_json::from_slice(&expected_bytes).expect("pinned bytes are valid JSON");
        let recanonicalized =
            canonical_object_bytes(class, ROOTED_ABI_VERSION_V6, &reparsed).expect("bytes");
        assert_eq!(recanonicalized, expected_bytes, "{} not decode-identical", object["class"]);
    }
}
