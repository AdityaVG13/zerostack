//! Read compatibility for the pre-cutover v1 GC record identity.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use zero_store::current_reachability_snapshot;

#[test]
fn reads_v1_reachability_snapshot_without_contract_digest() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = loop {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let candidate =
            std::env::temp_dir().join(format!("zero-store-gc-v1-{}-{suffix}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create isolated test root: {error}"),
        }
    };
    let project_id = "1".repeat(64);
    let snapshot_dir = root.join("gc/roots/fszero").join(&project_id);
    fs::create_dir_all(&snapshot_dir).unwrap();
    fs::write(
        snapshot_dir.join("current.json"),
        format!(
            "{{\"schema_version\":\"zerostack.cas-gc.v1\",\"record_type\":\"reachability-snapshot\",\"engine\":\"fszero\",\"project_id\":\"{project_id}\",\"epoch\":1,\"published_at\":\"2026-08-17T00:00:00Z\",\"blob_hashes\":[]}}\n"
        ),
    )
    .unwrap();

    let snapshot = current_reachability_snapshot(&root, "fszero", &project_id)
        .unwrap()
        .expect("v1 snapshot");
    assert_eq!(snapshot.epoch, 1);
    assert!(snapshot.store_contract_digest.is_none());

    fs::remove_dir_all(root).unwrap();
}
