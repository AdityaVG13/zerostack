//! Phase 4: load committed goldens and assert Tier-2/3 invariants.
//! Never auto-bless. Fail on drift.

use serde::Serialize;
use zerostack_harness::golden::{
    EquivalenceTier, assert_tier2_not_mislabeled, assert_tier3_invariants, load_manifest,
    load_tier3_logical, verify_all, verify_checksums_file, verify_manifest_hashes,
    verify_tier1_byte_equality,
};
use zerostack_harness::repo::repo_root;
use zerostack_harness::spec_oracle::all_verifiers;

#[derive(Serialize)]
struct LogicalPin {
    feature_count: u64,
    present: u64,
    partial: u64,
    missing: u64,
    excluded: u64,
    wired_count: u64,
    catalog_tag_count: u64,
    unverified_count: u64,
    contract_sections: u64,
    fixture_entries: u64,
}

#[test]
fn golden_integrity_holds() {
    let root = repo_root();
    let detail = verify_all(&root).expect("three-tier golden integrity");
    assert!(detail.contains("schema=1.0.0"), "{detail}");
}

#[test]
fn checksums_and_manifest_agree() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    verify_manifest_hashes(&root, &manifest).expect("manifest hashes");
    let rows = verify_checksums_file(&root).expect("checksums");
    assert!(rows >= manifest.artifacts.len());
}

#[test]
fn tier1_is_byte_equal_to_live_sources() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    let checked = verify_tier1_byte_equality(&root, &manifest).expect("tier1");
    assert_eq!(checked, 5);
}

#[test]
fn never_label_tier2_as_tier1() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    assert_tier2_not_mislabeled(&root, &manifest).expect("tier2 labels");
    for artifact in &manifest.artifacts {
        let tier = artifact.tier().expect("tier");
        if artifact.canonicalization_fn.is_some() {
            assert_ne!(
                tier,
                EquivalenceTier::Tier1Raw,
                "{} has canonicalization_fn but is labeled Tier1Raw",
                artifact.fixture_id
            );
        }
    }
}

#[test]
fn tier3_logical_invariants() {
    let root = repo_root();
    let dump = load_tier3_logical(&root).expect("tier3");
    assert_tier3_invariants(&root, &dump).expect("tier3 invariants");
    let wired = dump["spec_verifiers"]["wired_count"]
        .as_u64()
        .expect("wired_count");
    assert_eq!(wired, all_verifiers().len() as u64);
}

#[test]
fn insta_pins_logical_counts_without_autobless() {
    let root = repo_root();
    let dump = load_tier3_logical(&root).expect("tier3");
    let pin = LogicalPin {
        feature_count: dump["feature_universe"]["feature_count"].as_u64().unwrap(),
        present: dump["feature_universe"]["status_histogram"]["present"]
            .as_u64()
            .unwrap(),
        partial: dump["feature_universe"]["status_histogram"]["partial"]
            .as_u64()
            .unwrap(),
        missing: dump["feature_universe"]["status_histogram"]["missing"]
            .as_u64()
            .unwrap(),
        excluded: dump["feature_universe"]["status_histogram"]["excluded"]
            .as_u64()
            .unwrap(),
        wired_count: dump["spec_verifiers"]["wired_count"].as_u64().unwrap(),
        catalog_tag_count: dump["spec_verifiers"]["catalog_tag_count"]
            .as_u64()
            .unwrap(),
        unverified_count: dump["spec_verifiers"]["unverified_count"].as_u64().unwrap(),
        contract_sections: dump["contract_md"]["section_count"].as_u64().unwrap(),
        fixture_entries: dump["fixture_raw_worker_v2"]["entry_count"]
            .as_u64()
            .unwrap(),
    };
    insta::assert_json_snapshot!("phase4_logical_counts", pin);
}
