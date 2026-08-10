use std::fs;
use std::time::SystemTime;

use tempfile::TempDir;
use zero_store::{
    GC_RECORD_TYPE_REACHABILITY, GC_SCHEMA_VERSION, GC_SCHEMA_VERSION_V1, GcConfig, GcError,
    GcRunState, GcVerdict, LeaseOwner, SharedCas, current_reachability_snapshot,
    gc_contract_digest_hex, gc_contract_manifest, gc_project_id, gc_repair_receipt_digest_hex,
    gc_report_digest_hex, publish_reachability_snapshot, remove_lease_record,
    repair_object_receipted, run_gc, validate_dry_run_report, validate_repair_receipt,
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
        fs::read(dir.path().join("gc/quarantine").join(&blob)).unwrap(),
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
