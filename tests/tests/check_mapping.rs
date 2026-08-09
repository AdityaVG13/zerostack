//! G1-G10 canonical IDs and semantic labels are pinned here.

use zerostack_shared_tests::checks::{CheckId, GATE_MAPPINGS};

#[test]
fn mapping_table_is_complete_distinct_and_canonical() {
    let ids: Vec<_> = GATE_MAPPINGS
        .iter()
        .map(|mapping| mapping.id.as_str())
        .collect();
    let labels: Vec<_> = GATE_MAPPINGS
        .iter()
        .map(|mapping| mapping.semantic_label)
        .collect();
    assert_eq!(
        ids,
        (1..=10).map(|gate| format!("G{gate}")).collect::<Vec<_>>()
    );
    assert_eq!(
        ids.iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        10
    );
    assert_eq!(
        labels
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        10
    );
    assert_eq!(CheckId::G4LeakProof.semantic_label(), "leak_proof");
}

#[test]
fn serde_emits_only_canonical_ids_and_accepts_only_the_legacy_g4_alias() {
    for mapping in GATE_MAPPINGS {
        assert_eq!(
            serde_json::to_string(&mapping.id).unwrap(),
            format!("\"{}\"", mapping.id.as_str())
        );
    }
    assert_eq!(
        serde_json::from_str::<CheckId>("\"G4LEAKPROOF\"").unwrap(),
        CheckId::G4LeakProof
    );
    assert_eq!(
        serde_json::to_string(&CheckId::G4LeakProof).unwrap(),
        "\"G4\""
    );
    assert!(serde_json::from_str::<CheckId>("\"G4_leak_proof\"").is_err());
    assert!(serde_json::from_str::<CheckId>("\"G11\"").is_err());
}
