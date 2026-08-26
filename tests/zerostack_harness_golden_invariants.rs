//! Phase 4: load committed goldens and assert Tier-2/3 invariants.
//! Never auto-bless. Fail on drift.

use zerostack_harness::golden::{
    EquivalenceTier, assert_tier2_not_mislabeled, assert_tier3_invariants, load_manifest,
    load_tier3_logical, verify_all, verify_checksums_file, verify_manifest_hashes,
    verify_tier1_byte_equality,
};
use zerostack_harness::repo::repo_root;
use zerostack_harness::spec_oracle::all_verifiers;

#[test]
fn golden_integrity_holds() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    assert_eq!(
        manifest.schema_version, "1.0.0",
        "manifest schema_version is a published compatibility contract"
    );
    assert_eq!(
        zerostack_harness::golden::MANIFEST_SCHEMA_VERSION,
        "1.0.0",
        "golden constant must match manifest"
    );
    let detail = verify_all(&root).expect("three-tier golden integrity");
    assert!(
        detail.contains("schema=1.0.0"),
        "verify_all detail must report schema 1.0.0 via structured result, got: {detail}"
    );
    // Structured verification of parsed manifest, not just substring.
    let parsed: serde_json::Value = serde_json::from_str(
        &zerostack_harness::repo::read_text(&root, zerostack_harness::golden::MANIFEST_REL)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(parsed["schema_version"], "1.0.0");
}

#[test]
fn checksums_and_manifest_agree() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    verify_manifest_hashes(&root, &manifest).expect("manifest hashes");
    let rows = verify_checksums_file(&root).expect("checksums");
    // Independent per-artifact verification, not just count.
    let text = zerostack_harness::repo::read_text(&root, zerostack_harness::golden::CHECKSUMS_REL)
        .expect("checksums file");
    let mut map = std::collections::BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (digest, rel) = line.split_once("  ").expect("checksums format");
        map.insert(rel.to_string(), digest.to_string());
    }
    for artifact in &manifest.artifacts {
        let digest = map
            .get(&artifact.path)
            .unwrap_or_else(|| panic!("{} missing from checksums", artifact.path));
        assert_eq!(
            digest, &artifact.sha256,
            "{} checksum digest must match manifest",
            artifact.fixture_id
        );
        // Verify file identity maps to intended path and digest.
        let actual = zerostack_harness::repo::file_sha256_hex(
            &root,
            &format!("conformance/golden/{}", artifact.path),
        )
        .unwrap();
        assert_eq!(
            actual, artifact.sha256,
            "{} file digest mismatch",
            artifact.fixture_id
        );
    }
    // Any extra checksum entries must be explicitly characterized (manifest itself, tier3 dump).
    let manifest_paths: std::collections::BTreeSet<_> =
        manifest.artifacts.iter().map(|a| a.path.as_str()).collect();
    let mut extras = Vec::new();
    for rel in map.keys() {
        if !manifest_paths.contains(rel.as_str()) {
            extras.push(rel.clone());
        }
    }
    // Expected extras: the manifest file and the tier3 logical dump (not an artifact).
    extras.sort();
    assert_eq!(
        extras,
        vec![
            "manifest.v1.json".to_string(),
            "tier3/logical_dump.json".to_string()
        ],
        "extra checksum entries must be explicitly characterized, got: {extras:?}"
    );
    // Count is not the oracle; ensure rows count matches manifest + extras.
    assert_eq!(rows, manifest.artifacts.len() + extras.len());
}

#[test]
fn tier1_is_byte_equal_to_live_sources() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    let tier1_ids: Vec<String> = manifest
        .artifacts
        .iter()
        .filter(|a| a.tier().unwrap() == EquivalenceTier::Tier1Raw)
        .map(|a| a.fixture_id.clone())
        .collect();
    assert!(!tier1_ids.is_empty(), "must have Tier1Raw artifacts");
    let checked = verify_tier1_byte_equality(&root, &manifest).expect("tier1");
    // Coverage by fixture identity, not hard-coded total.
    assert_eq!(
        checked,
        tier1_ids.len(),
        "every Tier1Raw artifact must be checked: expected {tier1_ids:?}"
    );
    for fid in &tier1_ids {
        assert!(
            manifest.artifacts.iter().any(|a| &a.fixture_id == fid),
            "Tier1Raw fixture {fid} must exist in manifest"
        );
    }
}
fn never_label_tier2_as_tier1() {
    let root = repo_root();
    let manifest = load_manifest(&root).expect("manifest");
    assert_tier2_not_mislabeled(&root, &manifest).expect("tier2 labels");
    // Independent classification: Tier2 is canonical equivalence, never Tier1Raw.
    let tier2_ids: Vec<&str> = manifest
        .artifacts
        .iter()
        .filter(|a| a.tier().unwrap() == EquivalenceTier::Tier2Canonical)
        .map(|a| a.fixture_id.as_str())
        .collect();
    assert!(!tier2_ids.is_empty(), "must have Tier2Canonical artifacts");
    for artifact in &manifest.artifacts {
        let tier = artifact.tier().expect("tier");
        // If canonicalization is present, tier must be Tier2, not Tier1.
        if artifact.canonicalization_fn.is_some() {
            assert_ne!(
                tier,
                EquivalenceTier::Tier1Raw,
                "{} has canonicalization_fn but is labeled Tier1Raw",
                artifact.fixture_id
            );
            assert_eq!(
                tier,
                EquivalenceTier::Tier2Canonical,
                "{} has canonicalization_fn but tier is not Tier2Canonical",
                artifact.fixture_id
            );
        }
        // Conversely, Tier2Canonical must have a canonicalization function.
        if tier == EquivalenceTier::Tier2Canonical {
            assert!(
                artifact.canonicalization_fn.is_some(),
                "{} is Tier2Canonical but missing canonicalization_fn",
                artifact.fixture_id
            );
        }
    }
    // Verify expected Tier2 identities are exactly those declared (no substitution).
    let mut expected: Vec<&str> = tier2_ids.clone();
    expected.sort();
    let mut actual: Vec<&str> = manifest
        .artifacts
        .iter()
        .filter(|a| a.canonicalization_fn.is_some())
        .map(|a| a.fixture_id.as_str())
        .collect();
    actual.sort();
    assert_eq!(
        expected, actual,
        "Tier2 set must match canonicalization-bearing artifacts"
    );
}

#[test]
fn tier3_logical_invariants() {
    let root = repo_root();
    let dump = load_tier3_logical(&root).expect("tier3");
    assert_tier3_invariants(&root, &dump).expect("tier3 invariants");
    // Identity-based verifier wiring, not just count.
    let wired_tags: Vec<String> = dump["spec_verifiers"]["wired_tags"]
        .as_array()
        .expect("wired_tags")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let wired_count = dump["spec_verifiers"]["wired_count"].as_u64().unwrap();
    assert_eq!(
        wired_count as usize,
        wired_tags.len(),
        "wired_count must match wired_tags length"
    );
    let mut live: Vec<String> = all_verifiers().iter().map(|v| v.tag.to_string()).collect();
    let mut wired_sorted = wired_tags.clone();
    wired_sorted.sort();
    live.sort();
    assert_eq!(
        wired_sorted, live,
        "wired verifier identities must exactly match live verifier set; missing/extra/duplicate"
    );
    // No duplicates.
    let uniq: std::collections::BTreeSet<_> = wired_tags.iter().collect();
    assert_eq!(
        uniq.len(),
        wired_tags.len(),
        "wired_tags must have no duplicates"
    );
}

#[test]
fn insta_pins_logical_counts_without_autobless() {
    let root = repo_root();
    let dump = load_tier3_logical(&root).expect("tier3");
    // Semantic invariants, not aggregate snapshot.
    // Feature universe must be complete and weights sum to 1.0 (checked by assert_tier3_invariants),
    // but we also assert stable identities.
    assert_tier3_invariants(&root, &dump).expect("tier3 invariants");
    // Required contract sections by identity.
    let headings = dump["contract_md"]["required_headings"]
        .as_array()
        .expect("required_headings")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    for required in [
        "## 1. Composition",
        "## 2. Surfaces",
        "## 3. Result shape",
        "## 4. Refs",
        "## 5. Honesty",
        "## 6. Settlement",
        "## 7. What this folder holds",
        "## 8. What is not claimed",
    ] {
        assert!(
            headings.contains(&required),
            "contract required_heading {required} missing"
        );
    }
    assert_eq!(
        dump["contract_md"]["section_count"].as_u64().unwrap(),
        8,
        "contract section_count is a published invariant"
    );
    // Fixture entries by stable identity.
    let kinds = dump["fixture_raw_worker_v2"]["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    for kind in ["handshake", "call", "cancel", "shutdown"] {
        assert!(kinds.contains(&kind), "fixture kind {kind} missing");
    }
    assert_eq!(
        dump["fixture_raw_worker_v2"]["entry_count"]
            .as_u64()
            .unwrap(),
        4,
        "fixture entry_count is a published invariant"
    );
    let required_keys = dump["fixture_raw_worker_v2"]["required_entry_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(required_keys.contains(&"kind"));
    assert!(required_keys.contains(&"request"));
    // Spec verifier wiring by identity (complements tier3_logical_invariants).
    let wired_tags = dump["spec_verifiers"]["wired_tags"].as_array().unwrap();
    assert_eq!(wired_tags.len(), all_verifiers().len());
    // Feature counts are live-derived, not snapshot-pinned; verify they equal live.
    let live_features = dump["feature_universe"]["feature_count"].as_u64().unwrap();
    let hist = &dump["feature_universe"]["status_histogram"];
    let sum = hist["present"].as_u64().unwrap()
        + hist["partial"].as_u64().unwrap()
        + hist["missing"].as_u64().unwrap()
        + hist["excluded"].as_u64().unwrap();
    assert_eq!(sum, live_features, "histogram must sum to feature_count");
    // Published totals asserted explicitly with rationale, not via snapshot file.
    assert_eq!(
        live_features, 77,
        "feature_count 77 is a published contract; bump requires explicit update"
    );
    assert_eq!(
        wired_tags.len(),
        48,
        "wired_count 48 is published; adding verifier requires contract update"
    );
}
