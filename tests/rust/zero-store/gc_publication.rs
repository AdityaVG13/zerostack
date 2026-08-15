use std::fs;
use std::time::SystemTime;

use tempfile::TempDir;
use zero_abi::zbf::{DurableProfileV1, ZbfArtifactKindV1, ZbfObjectV1};
use zero_abi::{ArtifactOwnerV1, DigestV1};
use zero_store::{
    GC_RECORD_TYPE_REACHABILITY, GC_REFS_FORMAT, GC_SCHEMA_VERSION, GC_SCHEMA_VERSION_V1, GcConfig,
    GcError, GcRunState, GcVerdict, LeaseOwner, SharedCas, current_reachability_snapshot,
    gc_contract_digest_hex, gc_contract_manifest, gc_project_id, gc_repair_receipt_digest_hex,
    gc_report_digest_hex, publish_pin_record, publish_reachability_snapshot,
    refs_from_verified_bytes, remove_lease_record, remove_pin_record, repair_object_receipted,
    run_gc, validate_dry_run_report, validate_repair_receipt,
};

fn verdict(report: &zero_store::GcRunReceipt, hash: &str) -> GcVerdict {
    report
        .objects
        .iter()
        .find(|object| object.blob_hash == hash)
        .unwrap()
        .verdict
}
#[test]
fn contract_manifest_freezes_v2_lock_and_compatibility_semantics() {
    let manifest = gc_contract_manifest();
    assert_eq!(manifest["schema_version"], GC_SCHEMA_VERSION);
    assert_eq!(
        manifest["legacy_read_versions"],
        serde_json::json!([GC_SCHEMA_VERSION_V1])
    );
    assert_eq!(
        manifest["legacy_read_record_types"],
        serde_json::json!(["reachability-snapshot", "pin", "lease"])
    );
    assert_eq!(manifest["safety"]["cas_publish_lock"], "shared");
    assert_eq!(manifest["safety"]["metadata_publish_lock"], "exclusive");
    assert_eq!(
        manifest["safety"]["leased_publish"],
        "lease-before-object-under-exclusive-lock"
    );
    assert_eq!(manifest["safety"]["expired_pin"], "does-not-retain");
    assert_eq!(
        manifest["safety"]["lock_namespace"],
        "real-directory-and-regular-file-only"
    );
    assert_eq!(
        manifest["reachability"]["roots"],
        "reachability-snapshot blob_hashes"
    );
    assert_eq!(
        manifest["reachability"]["refs"],
        "content-derived-from-verified-object-bytes"
    );
    assert_eq!(manifest["reachability"]["refs_format"], GC_REFS_FORMAT);
    assert_eq!(
        manifest["reachability"]["closure"],
        "transitive-from-roots-pins-and-leases"
    );
    assert_eq!(
        manifest["reachability"]["fail_closed"],
        "corrupt-or-incomplete-refs-evidence-retains-uncertain-and-never-commits"
    );
    assert_eq!(gc_contract_digest_hex().len(), 64);
}

#[test]
fn producer_snapshot_removal_makes_a_real_blob_collectible() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let blob = cas.put(b"engine-produced object").unwrap();
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&blob),
    )
    .unwrap();
    let retained = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&retained, &blob), GcVerdict::Retain);

    publish_reachability_snapshot(dir.path(), "fixture-engine", &project, 2, &[]).unwrap();
    let snapshot = current_reachability_snapshot(dir.path(), "fixture-engine", &project)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.epoch, 2);
    assert!(snapshot.blob_hashes.is_empty());
    assert_eq!(
        snapshot.store_contract_digest.as_deref(),
        Some(gc_contract_digest_hex().as_str())
    );

    let receipt = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "collect-after-root-removal".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    assert_eq!(receipt.state, GcRunState::Complete);
    assert_eq!(receipt.deleted, vec![blob.clone()]);
    assert_eq!(receipt.planned, vec![blob.clone()]);
    assert_eq!(gc_report_digest_hex(&receipt).unwrap().len(), 64);
    let receipt_value = serde_json::to_value(&receipt).unwrap();
    validate_dry_run_report(&receipt_value).unwrap();
    let mut tampered = receipt_value;
    tampered["planned"] = serde_json::json!([]);
    assert!(validate_dry_run_report(&tampered).is_err());
    assert!(!cas.contains(&blob));
}

#[test]
fn leased_publish_is_atomic_and_release_enables_collection() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    publish_reachability_snapshot(dir.path(), "adapter-fixture", &project, 1, &[]).unwrap();
    let owner = || LeaseOwner {
        pid: u64::from(std::process::id()),
        host: "test-host".into(),
    };
    let outcome = cas
        .put_leased(
            b"leased object",
            "adapter-fixture",
            &project,
            "operation-1",
            1,
            owner(),
            300,
        )
        .unwrap();
    let retained = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&retained, &outcome.hash), GcVerdict::Retain);
    assert!(matches!(
        cas.put_leased(
            b"leased object",
            "adapter-fixture",
            &project,
            "operation-1",
            1,
            owner(),
            300,
        ),
        Err(GcError::SchemaViolation(_))
    ));

    remove_lease_record(dir.path(), "adapter-fixture", &project, "operation-1").unwrap();
    let collected = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "collect-after-lease-release".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    assert_eq!(collected.deleted, vec![outcome.hash.clone()]);
    assert!(!cas.contains(&outcome.hash));
}

#[test]
fn legacy_snapshot_is_read_only_compatible_then_upgraded() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let blob = cas.put(b"legacy reachable object").unwrap();
    let pinned = cas.put(b"legacy pinned object").unwrap();
    let leased = cas.put(b"legacy leased object").unwrap();
    let snapshot_path = dir
        .path()
        .join("gc/roots/legacy-producer")
        .join(&project)
        .join("current.json");
    fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
    fs::write(
        &snapshot_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION_V1,
            "record_type": GC_RECORD_TYPE_REACHABILITY,
            "engine": "legacy-producer",
            "project_id": project,
            "epoch": 1,
            "published_at": "2026-01-01T00:00:00Z",
            "blob_hashes": [blob]
        }))
        .unwrap(),
    )
    .unwrap();
    let pin_path = dir
        .path()
        .join("gc/pins/legacy-producer")
        .join(&project)
        .join("pin-old.json");
    fs::create_dir_all(pin_path.parent().unwrap()).unwrap();
    fs::write(
        &pin_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION_V1,
            "record_type": "pin",
            "engine": "legacy-producer",
            "project_id": project,
            "pin_id": "pin-old",
            "created_at": "2026-01-01T00:00:00Z",
            "expires_at": null,
            "blob_hash": pinned
        }))
        .unwrap(),
    )
    .unwrap();
    let lease_path = dir
        .path()
        .join("gc/leases/legacy-producer")
        .join(&project)
        .join("operation-old.json");
    fs::create_dir_all(lease_path.parent().unwrap()).unwrap();
    fs::write(
        &lease_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION_V1,
            "record_type": "lease",
            "engine": "legacy-producer",
            "project_id": project,
            "operation_id": "operation-old",
            "epoch": 1,
            "owner": {"pid": u64::from(std::process::id()), "host": "test-host"},
            "started_at": "2026-01-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "grace_seconds": 60,
            "blob_hashes": [leased]
        }))
        .unwrap(),
    )
    .unwrap();

    let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&report, &blob), GcVerdict::Retain);
    assert_eq!(verdict(&report, &pinned), GcVerdict::Retain);
    assert_eq!(verdict(&report, &leased), GcVerdict::Retain);
    publish_reachability_snapshot(dir.path(), "legacy-producer", &project, 2, &[]).unwrap();
    let upgraded = current_reachability_snapshot(dir.path(), "legacy-producer", &project)
        .unwrap()
        .unwrap();
    assert_eq!(upgraded.schema_version, GC_SCHEMA_VERSION);
    assert_eq!(upgraded.epoch, 2);
    assert!(publish_reachability_snapshot(dir.path(), "../escape", &project, 3, &[]).is_err());
}

#[test]
fn tampered_resume_plan_fails_closed() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    publish_reachability_snapshot(dir.path(), "fixture-engine", &project, 1, &[]).unwrap();
    cas.put(b"first orphan").unwrap();
    cas.put(b"second orphan").unwrap();
    let config = GcConfig {
        run_id: "tamper-resume".into(),
        apply: true,
        now: SystemTime::now(),
        fault_after_deletes: Some(1),
        ..GcConfig::default()
    };
    assert!(matches!(
        run_gc(dir.path(), &config),
        Err(GcError::FaultInjected)
    ));
    let progress_path = dir.path().join("gc/reports/tamper-resume.progress.json");
    let mut progress: serde_json::Value =
        serde_json::from_slice(&fs::read(&progress_path).unwrap()).unwrap();
    progress["plan_digest"] = serde_json::Value::String("0".repeat(64));
    fs::write(&progress_path, serde_json::to_vec(&progress).unwrap()).unwrap();
    assert!(matches!(
        run_gc(dir.path(), &config),
        Err(GcError::CorruptMetadata { .. })
    ));
}

#[test]
fn repair_quarantines_corruption_and_persists_receipt() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let bytes = b"authoritative bytes";
    let blob = cas.put(bytes).unwrap();
    fs::write(cas.object_path(&blob), b"corrupt bytes").unwrap();
    let receipt = repair_object_receipted(
        dir.path(),
        "fixture-engine",
        &project,
        "repair-1",
        &blob,
        bytes,
    )
    .unwrap();
    assert!(receipt.repaired);
    assert!(receipt.quarantined);
    assert_eq!(receipt.store_contract_digest, gc_contract_digest_hex());
    let receipt_value = serde_json::to_value(&receipt).unwrap();
    validate_repair_receipt(&receipt_value).unwrap();
    assert_eq!(gc_repair_receipt_digest_hex(&receipt).unwrap().len(), 64);
    let mut tampered = receipt_value;
    tampered["store_contract_digest"] = serde_json::Value::String("0".repeat(64));
    assert!(validate_repair_receipt(&tampered).is_err());
    assert_eq!(cas.get_verified(&blob).unwrap(), bytes);
    assert_eq!(
        fs::read(
            dir.path()
                .join("gc/quarantine")
                .join(format!("{blob}.corrupt-0"))
        )
        .unwrap(),
        b"corrupt bytes"
    );
    assert!(
        dir.path()
            .join("gc/repairs/fixture-engine")
            .join(&project)
            .join("repair-1.json")
            .is_file()
    );
    assert!(
        repair_object_receipted(
            dir.path(),
            "fixture-engine",
            &project,
            "repair-1",
            &blob,
            bytes,
        )
        .is_err()
    );
}

fn zbf_profile() -> DurableProfileV1 {
    DurableProfileV1::portable_strict()
}

fn zbf_leaf(payload: &[u8]) -> ZbfObjectV1 {
    ZbfObjectV1::new_leaf(
        ZbfArtifactKindV1::Snapshot,
        ArtifactOwnerV1::ZeroStack,
        DigestV1::from_bytes([1; 32]),
        zbf_profile(),
        DigestV1::from_bytes([2; 32]),
        DigestV1::from_bytes([3; 32]),
        payload.to_vec(),
    )
    .unwrap()
}

fn zbf_container(children: Vec<ZbfObjectV1>) -> ZbfObjectV1 {
    ZbfObjectV1::new_container(
        ZbfArtifactKindV1::Snapshot,
        ArtifactOwnerV1::ZeroStack,
        DigestV1::from_bytes([1; 32]),
        zbf_profile(),
        DigestV1::from_bytes([2; 32]),
        DigestV1::from_bytes([3; 32]),
        children,
    )
    .unwrap()
}

fn ref_entry<'a>(report: &'a zero_store::GcRunReceipt, hash: &str) -> &'a zero_store::GcCandidate {
    report
        .objects
        .iter()
        .find(|object| object.blob_hash == hash)
        .unwrap()
}

#[test]
fn refs_closure_defines_reachability_and_apply_commits_only_unreachable() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let profile = zbf_profile();
    let leaf = zbf_leaf(b"child one");
    let child = zbf_leaf(b"child two");
    let container = zbf_container(vec![leaf.clone(), child.clone()]);
    let container_hash = cas.put_zbf(&container, profile).unwrap().hash;
    let leaf_hash = cas.put_zbf(&leaf, profile).unwrap().hash;
    let child_hash = cas.put_zbf(&child, profile).unwrap().hash;
    let orphan = cas.put(b"orphan").unwrap();
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&container_hash),
    )
    .unwrap();

    let dry = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&dry, &container_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &leaf_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &child_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &orphan), GcVerdict::Collect);
    let leaf_entry = ref_entry(&dry, &leaf_hash);
    assert!(leaf_entry.reason_codes.iter().any(|r| r == "ref-child"));
    assert!(leaf_entry.evidence.iter().any(|e| e.contains("ref from")));
    let value = serde_json::to_value(&dry).unwrap();
    validate_dry_run_report(&value).unwrap();
    assert_eq!(gc_report_digest_hex(&dry).unwrap().len(), 64);

    let applied = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "refs-apply".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    assert_eq!(applied.deleted, vec![orphan.clone()]);
    assert!(cas.contains(&container_hash));
    assert!(cas.contains(&leaf_hash));
    assert!(cas.contains(&child_hash));
    assert!(!cas.contains(&orphan));
}

#[test]
fn nested_refs_closure_is_transitive() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let profile = zbf_profile();
    let leaf = zbf_leaf(b"deep child");
    let inner = zbf_container(vec![leaf.clone()]);
    let outer = zbf_container(vec![inner.clone()]);
    let leaf_hash = cas.put_zbf(&leaf, profile).unwrap().hash;
    let inner_hash = cas.put_zbf(&inner, profile).unwrap().hash;
    let outer_hash = cas.put_zbf(&outer, profile).unwrap().hash;
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&outer_hash),
    )
    .unwrap();

    let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&report, &outer_hash), GcVerdict::Retain);
    assert_eq!(verdict(&report, &inner_hash), GcVerdict::Retain);
    assert_eq!(verdict(&report, &leaf_hash), GcVerdict::Retain);
    assert!(
        ref_entry(&report, &inner_hash)
            .reason_codes
            .iter()
            .any(|r| r == "ref-child")
    );
    assert!(
        ref_entry(&report, &leaf_hash)
            .reason_codes
            .iter()
            .any(|r| r == "ref-child")
    );
}

#[test]
fn corrupt_container_evidence_fails_closed_and_never_commits() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let profile = zbf_profile();
    let child = zbf_leaf(b"child");
    let container = zbf_container(vec![child.clone()]);
    let container_hash = cas.put_zbf(&container, profile).unwrap().hash;
    let child_hash = cas.put_zbf(&child, profile).unwrap().hash;
    let orphan = cas.put(b"orphan").unwrap();
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&container_hash),
    )
    .unwrap();
    // Shrink the container below the ZBF header size: a corrupt file must
    // still fail closed, never be mistaken for a leaf.
    fs::write(cas.object_path(&container_hash), b"corrupt").unwrap();

    let dry = run_gc(dir.path(), &GcConfig::default()).unwrap();
    // The corrupt root is itself never collected; its unknown subtree and
    // every other object fail closed as retain-uncertain.
    assert_eq!(verdict(&dry, &container_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &child_hash), GcVerdict::RetainUncertain);
    assert_eq!(verdict(&dry, &orphan), GcVerdict::RetainUncertain);
    assert!(dry.planned.is_empty());

    let applied = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "refs-corrupt".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    assert!(applied.deleted.is_empty());
    assert!(cas.contains(&child_hash));
    assert!(cas.contains(&orphan));
}

#[test]
fn malformed_magic_prefixed_evidence_fails_closed() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    // ZBF magic with an unsupported schema and a payload length mismatch.
    let mut malformed = vec![0u8; 192];
    malformed[..8].copy_from_slice(b"ZEROZBF1");
    let malformed_hash = cas.put(&malformed).unwrap();
    let orphan = cas.put(b"orphan").unwrap();
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&malformed_hash),
    )
    .unwrap();

    let dry = run_gc(dir.path(), &GcConfig::default()).unwrap();
    // The malformed root is never collected; its unknown subtree and every
    // other object fail closed.
    assert_eq!(verdict(&dry, &malformed_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &orphan), GcVerdict::RetainUncertain);
    assert!(dry.planned.is_empty());
    assert!(refs_from_verified_bytes(&malformed).is_err());
}

#[test]
fn absent_root_object_is_incomplete_evidence_and_fails_closed() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let absent = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string();
    let orphan = cas.put(b"orphan").unwrap();
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&absent),
    )
    .unwrap();

    let dry = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&dry, &orphan), GcVerdict::RetainUncertain);
    assert!(dry.planned.is_empty());
}

#[test]
fn pinned_container_refs_are_traced_and_released_with_the_pin() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let profile = zbf_profile();
    let child = zbf_leaf(b"pinned child");
    let container = zbf_container(vec![child.clone()]);
    let container_hash = cas.put_zbf(&container, profile).unwrap().hash;
    let child_hash = cas.put_zbf(&child, profile).unwrap().hash;
    let orphan = cas.put(b"orphan").unwrap();
    publish_reachability_snapshot(dir.path(), "fixture-engine", &project, 1, &[]).unwrap();
    publish_pin_record(
        dir.path(),
        &zero_store::PinRecord {
            schema_version: GC_SCHEMA_VERSION.into(),
            record_type: "pin".into(),
            engine: "fixture-engine".into(),
            project_id: project.clone(),
            store_contract_digest: Some(gc_contract_digest_hex()),
            pin_id: "pinned-container".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
            blob_hash: container_hash.clone(),
        },
    )
    .unwrap();

    let retained = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&retained, &container_hash), GcVerdict::Retain);
    assert_eq!(verdict(&retained, &child_hash), GcVerdict::Retain);
    assert!(
        ref_entry(&retained, &child_hash)
            .reason_codes
            .iter()
            .any(|r| r == "ref-child")
    );
    assert_eq!(verdict(&retained, &orphan), GcVerdict::Collect);

    remove_pin_record(dir.path(), "fixture-engine", &project, "pinned-container").unwrap();
    let collected = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "collect-after-pin-release".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    let mut expected = vec![container_hash.clone(), child_hash.clone(), orphan.clone()];
    expected.sort();
    assert_eq!(collected.deleted, expected);
    assert!(!cas.contains(&container_hash));
    assert!(!cas.contains(&child_hash));
    assert!(!cas.contains(&orphan));
}

#[test]
fn corrupt_separate_copy_of_referenced_child_is_never_collected() {
    let dir = TempDir::new().unwrap();
    let cas = SharedCas::open(dir.path());
    let project = gc_project_id(dir.path()).unwrap();
    let profile = zbf_profile();
    let child = zbf_leaf(b"child");
    let container = zbf_container(vec![child.clone()]);
    let container_hash = cas.put_zbf(&container, profile).unwrap().hash;
    let child_hash = cas.put_zbf(&child, profile).unwrap().hash;
    publish_reachability_snapshot(
        dir.path(),
        "fixture-engine",
        &project,
        1,
        std::slice::from_ref(&container_hash),
    )
    .unwrap();
    // The separate copy is corrupt, but the verified container still names the
    // child: refs evidence is complete, so the child is retained, not uncertain.
    fs::write(cas.object_path(&child_hash), b"corrupt child").unwrap();

    let dry = run_gc(dir.path(), &GcConfig::default()).unwrap();
    assert_eq!(verdict(&dry, &container_hash), GcVerdict::Retain);
    assert_eq!(verdict(&dry, &child_hash), GcVerdict::Retain);
    assert!(dry.planned.is_empty());

    let applied = run_gc(
        dir.path(),
        &GcConfig {
            run_id: "refs-child-corrupt".into(),
            apply: true,
            ..GcConfig::default()
        },
    )
    .unwrap();
    assert!(applied.deleted.is_empty());
    assert!(cas.contains(&child_hash));
}
