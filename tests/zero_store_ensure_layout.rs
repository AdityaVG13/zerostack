//! After a successful `ensure_layout`, `blobs/` and `gc/` exist.

use std::time::{SystemTime, UNIX_EPOCH};

use zero_store::{Engine, ResolvedStore, StoreEnv, ensure_layout};

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
    assert_eq!(
        resolved.mode(),
        zero_store::StoreMode::LocalUnified,
        "new repositories must default to the unified store"
    );
    assert_eq!(
        resolved.engine_dir(),
        resolved.repo_root().join(".zerostack/fszero"),
        "FSZero must not create a new .fszero directory"
    );
    ensure_layout(&resolved).expect("layout");
    // Explicit expected paths rooted at <repo>/.zerostack, not via resolver accessors.
    let engine_dir = repo.join(".zerostack/fszero");
    let blobs_dir = repo.join(".zerostack/blobs");
    let gc_dir = repo.join(".zerostack/gc");
    assert!(
        engine_dir.is_dir(),
        "engine_dir missing: {}",
        engine_dir.display()
    );
    assert!(
        blobs_dir.is_dir(),
        "blobs/ missing after ensure_layout: {}",
        blobs_dir.display()
    );
    assert!(
        gc_dir.is_dir(),
        "gc/ missing after ensure_layout: {}",
        gc_dir.display()
    );
    assert!(
        !repo.join(".fszero").exists(),
        "legacy .fszero must not be created"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn new_repository_uses_one_zerostack_root_for_all_engines() {
    let tmp = unique_tmp();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    for engine in [Engine::FsZero, Engine::GraphZero, Engine::TokenZero] {
        let resolved = ResolvedStore::resolve(&repo, engine, &StoreEnv::default());
        let expected_root = resolved.repo_root().join(".zerostack");
        assert_eq!(resolved.mode(), zero_store::StoreMode::LocalUnified);
        assert_eq!(resolved.unified_root(), Some(expected_root.as_path()));
        assert_eq!(resolved.engine_dir(), expected_root.join(engine.dir_name()));
        ensure_layout(&resolved).expect("layout");
    }
    assert!(repo.join(".zerostack/fszero").is_dir());
    assert!(repo.join(".zerostack/graphzero").is_dir());
    assert!(repo.join(".zerostack/tokenzero").is_dir());
    assert!(!repo.join(".fszero").exists());
    assert!(!repo.join(".graphzero").exists());
    assert!(!repo.join(".tokenzero").exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn existing_legacy_store_remains_authoritative_until_migration() {
    let tmp = unique_tmp();
    let repo = tmp.join("repo");
    std::fs::create_dir_all(repo.join(".graphzero")).expect("legacy marker");
    let resolved = ResolvedStore::resolve(&repo, Engine::GraphZero, &StoreEnv::default());
    assert_eq!(resolved.mode(), zero_store::StoreMode::Legacy);
    assert_eq!(
        resolved.engine_dir(),
        resolved.repo_root().join(".graphzero")
    );
    assert!(resolved.unified_root().is_none());
    let report = resolved.report(&StoreEnv::default());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("migrate")),
        "legacy resolution must explain that deletion is unsafe"
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
    // Explicit expected paths rooted at <repo>/.zerostack, assert co-location.
    let blobs_dir = repo.join(".zerostack/blobs");
    let gc_dir = repo.join(".zerostack/gc");
    let engine_dir = repo.join(".zerostack/graphzero");
    assert!(
        blobs_dir.is_dir(),
        "blobs missing at {}",
        blobs_dir.display()
    );
    assert!(gc_dir.is_dir(), "gc missing at {}", gc_dir.display());
    assert!(
        engine_dir.is_dir(),
        "engine dir missing at {}",
        engine_dir.display()
    );
    assert_eq!(
        blobs_dir.parent().unwrap(),
        gc_dir.parent().unwrap(),
        "blobs/ and gc/ must share cas_host"
    );
    assert_eq!(
        blobs_dir.parent().unwrap(),
        repo.join(".zerostack").as_path()
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
