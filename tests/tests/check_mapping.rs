//! G1-G10 canonical plan IDs and RW1-RW10 raw-worker IDs are pinned here,
//! along with the invariant that the two vocabularies never overlap.

use zerostack_shared_tests::checks::{
    CheckId, GATE_MAPPINGS, RAW_GATE_MAPPINGS, RawCheckId,
};
use std::collections::HashSet;

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

#[test]
fn raw_mapping_table_is_complete_distinct_and_disjoint_from_plan() {
    let raw_ids: Vec<_> = RAW_GATE_MAPPINGS.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        raw_ids,
        (1..=10).map(|n| format!("RW{n}")).collect::<Vec<_>>()
    );
    let plan_ids: HashSet<&str> = GATE_MAPPINGS.iter().map(|m| m.id.as_str()).collect();
    let raw_id_set: HashSet<&str> = raw_ids.iter().copied().collect();
    assert_eq!(raw_id_set.len(), 10);
    assert!(
        plan_ids.is_disjoint(&raw_id_set),
        "plan and raw id sets must be disjoint"
    );
    let labels: HashSet<&str> = RAW_GATE_MAPPINGS.iter().map(|m| m.semantic_label).collect();
    assert_eq!(labels.len(), 10);
    assert_eq!(RawCheckId::Rw8DomainMutation.semantic_label(), "domain_mutation");
}

#[test]
fn raw_checkid_serde_emits_rw_form_and_rejects_plan_ids() {
    assert_eq!(
        serde_json::to_string(&RawCheckId::Rw1ArtifactExposure).unwrap(),
        "\"RW1\""
    );
    assert_eq!(
        serde_json::from_str::<RawCheckId>("\"RW10\"").unwrap(),
        RawCheckId::Rw10PlannerRefusal
    );
    assert!(serde_json::from_str::<RawCheckId>("\"G1\"").is_err());
    assert!(serde_json::from_str::<RawCheckId>("\"RW11\"").is_err());
}
