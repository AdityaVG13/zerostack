//! Canonical shared-CAS GC schema admission.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use zero_store::{GC_SCHEMA_VERSION, current_reachability_snapshot, gc_contract_digest_hex};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn isolated_root(label: &str) -> PathBuf {
    loop {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let candidate = std::env::temp_dir().join(format!(
            "zero-store-gc-{label}-{}-{suffix}",
            std::process::id()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create isolated test root: {error}"),
        }
    }
}

fn record(schema: &str, digest: Option<&str>, project_id: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "schema_version": schema,
            "record_type": "reachability-snapshot",
            "engine": "fszero",
            "project_id": project_id,
            "store_contract_digest": digest,
            "epoch": 7,
            "published_at": "2026-08-17T00:00:00Z",
            "blob_hashes": [],
        })
    )
}

#[test]
fn reads_canonical_reachability_snapshot_with_bound_contract() {
    let root = isolated_root("canonical");
    let project_id = "2".repeat(64);
    let snapshot_dir = root.join("gc/roots/fszero").join(&project_id);
    fs::create_dir_all(&snapshot_dir).unwrap();
    fs::write(
        snapshot_dir.join("current.json"),
        record(
            GC_SCHEMA_VERSION,
            Some(&gc_contract_digest_hex()),
            &project_id,
        ),
    )
    .unwrap();

    let snapshot = current_reachability_snapshot(&root, "fszero", &project_id)
        .unwrap()
        .expect("canonical snapshot");
    assert_eq!(snapshot.epoch, 7);
    assert_eq!(
        snapshot.store_contract_digest.as_deref(),
        Some(gc_contract_digest_hex().as_str())
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_unbound_or_generation_labelled_reachability_snapshot() {
    let root = isolated_root("rejected");
    let project_id = "1".repeat(64);
    let snapshot_dir = root.join("gc/roots/fszero").join(&project_id);
    fs::create_dir_all(&snapshot_dir).unwrap();
    let snapshot_path = snapshot_dir.join("current.json");

    fs::write(&snapshot_path, record(GC_SCHEMA_VERSION, None, &project_id)).unwrap();
    let missing_digest = current_reachability_snapshot(&root, "fszero", &project_id)
        .expect_err("unbound schema must fail closed");
    assert!(
        missing_digest
            .to_string()
            .contains("store_contract_digest mismatch")
    );

    fs::write(
        &snapshot_path,
        record("zerostack.cas-gc.legacy", None, &project_id),
    )
    .unwrap();
    let old_schema = current_reachability_snapshot(&root, "fszero", &project_id)
        .expect_err("generation-labelled schema must fail closed");
    assert!(
        old_schema
            .to_string()
            .contains("unsupported schema_version")
    );

    fs::remove_dir_all(root).unwrap();
}
