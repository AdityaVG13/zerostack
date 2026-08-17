//! After a successful `ensure_layout`, `blobs/` and `gc/` exist.

use std::time::{SystemTime, UNIX_EPOCH};

use zero_store::{ensure_layout, Engine, ResolvedStore, StoreEnv};

fn unique_tmp() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "zerostack-ensure-layout-{}-{}",
        std::process::id(),
        nanos
    ))
}

#[test]
fn ensure_layout_creates_blobs_and_gc_under_cas_host() {
    let tmp = unique_tmp();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    let env = StoreEnv::default();
    let resolved = ResolvedStore::resolve(&repo, Engine::FsZero, &env);
    ensure_layout(&resolved).expect("layout");
    assert!(
        resolved.engine_dir().is_dir(),
        "engine_dir missing: {}",
        resolved.engine_dir().display()
    );
    assert!(
        resolved.blobs_dir().is_dir(),
        "blobs/ missing after ensure_layout: {}",
        resolved.blobs_dir().display()
    );
    assert!(
        resolved.gc_dir().is_dir(),
        "gc/ missing after ensure_layout: {}",
        resolved.gc_dir().display()
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ensure_layout_creates_blobs_and_gc_for_local_unified() {
    let tmp = unique_tmp();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(repo.join(zero_store::LOCAL_STORE_DIR)).expect("marker");
    let resolved = ResolvedStore::resolve(&repo, Engine::GraphZero, &StoreEnv::default());
    assert!(resolved.unified_root().is_some());
    ensure_layout(&resolved).expect("layout");
    assert!(resolved.blobs_dir().is_dir());
    assert!(resolved.gc_dir().is_dir());
    assert_eq!(
        resolved.blobs_dir().parent(),
        resolved.gc_dir().parent(),
        "blobs/ and gc/ must share cas_host"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
