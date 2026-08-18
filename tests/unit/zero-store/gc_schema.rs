//! Read compatibility for pre-cutover v1 and v2 GC record identities.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use zero_store::current_reachability_snapshot;

const V2_CONTRACT_DIGEST: &str =
    "ad931317c574795866b794c67dc4067415decc91113dae699e692c69d64aea0e";
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

#[test]
fn reads_v1_reachability_snapshot_without_contract_digest() {
    let root = isolated_root("v1");
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

#[test]
fn reads_v2_reachability_snapshot_only_with_its_frozen_contract_digest() {
    let root = isolated_root("v2");
    let project_id = "2".repeat(64);
    let snapshot_dir = root.join("gc/roots/fszero").join(&project_id);
    fs::create_dir_all(&snapshot_dir).unwrap();
    let snapshot_path = snapshot_dir.join("current.json");
    let record = |digest: &str| {
        format!(
            "{{\"schema_version\":\"zerostack.cas-gc.v2\",\"record_type\":\"reachability-snapshot\",\"engine\":\"fszero\",\"project_id\":\"{project_id}\",\"store_contract_digest\":\"{digest}\",\"epoch\":7,\"published_at\":\"2026-08-17T00:00:00Z\",\"blob_hashes\":[]}}\n"
        )
    };
    fs::write(&snapshot_path, record(V2_CONTRACT_DIGEST)).unwrap();

    let snapshot = current_reachability_snapshot(&root, "fszero", &project_id)
        .unwrap()
        .expect("v2 snapshot");
    assert_eq!(snapshot.epoch, 7);
    assert_eq!(
        snapshot.store_contract_digest.as_deref(),
        Some(V2_CONTRACT_DIGEST)
    );

    fs::write(&snapshot_path, record(&"0".repeat(64))).unwrap();
    let error = current_reachability_snapshot(&root, "fszero", &project_id)
        .expect_err("unknown v2 contract digest must fail closed");
    assert!(error.to_string().contains("store_contract_digest mismatch"));

    fs::remove_dir_all(root).unwrap();
}
