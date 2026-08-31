use std::fs;
use std::path::{Path, PathBuf};

use fszero_store::recovery::{store_gc_apply, store_gc_plan};
use zerostack_test_support::TempWorkspace;

fn snapshot(parent: &Path, kind: &str, stamp: u128, bytes: usize) -> PathBuf {
    let path = parent.join(format!("store.db.{kind}-{stamp}-1-0"));
    fs::create_dir(&path).expect("create snapshot directory");
    fs::write(path.join("store.db"), vec![0_u8; bytes]).expect("write snapshot payload");
    path
}

#[test]
fn gc_plan_is_read_only_and_deletes_oldest_first() {
    let workspace = TempWorkspace::new("fszero-store-gc-plan-").expect("temp workspace");
    let db = workspace.root().join("store.db");
    let oldest = snapshot(workspace.root(), "forensic", 1, 4096);
    let middle = snapshot(workspace.root(), "salvage", 2, 4096);
    let newest = snapshot(workspace.root(), "forensic", 3, 4096);

    let plan = store_gc_plan(&db, 8192).expect("plan store GC");

    assert_eq!(plan.store, "store.db");
    assert_eq!(plan.scanned, 3);
    assert_eq!(plan.delete.len(), 1);
    assert_eq!(plan.delete[0].name, "store.db.forensic-1-1-0");
    assert_eq!(
        plan.retained
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["store.db.forensic-3-1-0", "store.db.salvage-2-1-0"]
    );
    assert!(oldest.is_dir() && middle.is_dir() && newest.is_dir());
}

#[test]
fn gc_apply_enforces_byte_budget_without_deleting_newest_evidence() {
    let workspace = TempWorkspace::new("fszero-store-gc-apply-").expect("temp workspace");
    let db = workspace.root().join("store.db");
    let oldest = snapshot(workspace.root(), "forensic", 1, 4096);
    let newest = snapshot(workspace.root(), "salvage", 2, 4096);

    let applied = store_gc_apply(&db, 0).expect("apply store GC");

    assert_eq!(applied.delete.len(), 1);
    assert_eq!(applied.retained.len(), 1);
    assert!(!oldest.exists(), "oldest evidence must be pruned first");
    assert!(newest.is_dir(), "newest evidence must survive any byte cap");
}

#[test]
fn gc_plan_enforces_snapshot_count_cap_even_when_bytes_fit() {
    let workspace = TempWorkspace::new("fszero-store-gc-count-").expect("temp workspace");
    let db = workspace.root().join("store.db");
    for stamp in 1..=4 {
        snapshot(workspace.root(), "forensic", stamp, 1);
    }

    let plan = store_gc_plan(&db, u64::MAX).expect("plan count-bounded store GC");

    assert_eq!(plan.count_cap, 3);
    assert_eq!(plan.delete.len(), 1);
    assert_eq!(plan.delete[0].stamp, 1);
    assert_eq!(plan.retained.len(), 3);
}

#[test]
fn gc_ignores_unrelated_sibling_directories() {
    let workspace = TempWorkspace::new("fszero-store-gc-scope-").expect("temp workspace");
    let db = workspace.root().join("store.db");
    let unrelated = workspace.root().join("store.db.backup-1");
    fs::create_dir(&unrelated).expect("create unrelated directory");
    fs::write(unrelated.join("store.db"), b"operator backup").expect("write unrelated payload");
    snapshot(workspace.root(), "forensic", 1, 8);

    let applied = store_gc_apply(&db, 0).expect("apply scoped store GC");

    assert_eq!(applied.scanned, 1);
    assert!(
        unrelated.is_dir(),
        "store GC must not touch unrecognized siblings"
    );
}
