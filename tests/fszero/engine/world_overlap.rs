//! filesystem-v1 races.world-overlap: world commit is preimage-guarded
//! three-way application and rejects overlapping conflicts. No last-write-wins.
//!
//! Two product models are live: (1) two staged edits in one world on the same
//! path with diverging overlays, (2) two worlds whose hunks overlap. Commit
//! rejects and leaves the pre-commit base unchanged.

use fszero_engine::{FSZeroSession, classify_detail_to_error_class};
use std::fs;
use std::path::PathBuf;

const BASE: &str = "l1\nl2\nl3\nl4\n";

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, BASE).unwrap();
    (dir, target)
}

fn world(s: &mut FSZeroSession, arg: &str) -> (bool, String) {
    let (_, ok, detail) = s.execute('W', Some(arg));
    (ok, detail.unwrap_or_default())
}

#[test]
fn intra_world_overlapping_hunks_reject_and_leave_base_unchanged() {
    // Same world, same path, two diverging overlays whose hunks overlap at l3.
    // Unique-replace would still apply (LWW); commit must refuse.
    let (dir, target) = workspace();
    let mut s = FSZeroSession::with_root(dir.path());
    let (ok, detail) = world(
        &mut s,
        "newbatch:target.txt:l2\nl3|L2\nl3;;target.txt:l3|Y3",
    );
    assert!(ok, "stage overlapping overlays: {detail}");
    assert_eq!(fs::read_to_string(&target).unwrap(), BASE);

    let (ok, detail) = world(&mut s, "commit:W1");
    assert!(!ok, "overlapping intra-world commit must fail: {detail}");
    assert!(
        detail.contains("merge conflict"),
        "expected merge conflict, got {detail}"
    );
    assert_eq!(
        classify_detail_to_error_class(&detail),
        "conflict",
        "{detail}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        BASE,
        "rejected overlapping commit must not publish"
    );
}

#[test]
fn cross_world_overlapping_hunks_reject_and_leave_base_unchanged() {
    // W1 claims l2-l3 but only rewrites l2; W2 claims l3. Unique-replace of
    // l3 still succeeds after W1 — that is the last-write-wins hole.
    let (dir, target) = workspace();
    let mut s = FSZeroSession::with_root(dir.path());
    let (ok, detail) = world(&mut s, "new:target.txt:l2\nl3|L2\nl3");
    assert!(ok, "W1 stage: {detail}");
    let (ok, detail) = world(&mut s, "new:target.txt:l3|Y3");
    assert!(ok, "W2 stage: {detail}");
    assert_eq!(fs::read_to_string(&target).unwrap(), BASE);

    let (ok, detail) = world(&mut s, "commit:W1");
    assert!(ok, "first lock-winner must commit: {detail}");
    let after_w1 = fs::read_to_string(&target).unwrap();
    assert_eq!(after_w1, "l1\nL2\nl3\nl4\n");

    let (ok, detail) = world(&mut s, "commit:W2");
    assert!(!ok, "overlapping W2 commit must fail: {detail}");
    assert!(
        detail.contains("merge conflict"),
        "expected merge conflict, got {detail}"
    );
    assert_eq!(
        classify_detail_to_error_class(&detail),
        "conflict",
        "{detail}"
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        after_w1,
        "overlapping W2 must not last-write-wins over W1"
    );
}

#[test]
fn cross_world_disjoint_hunks_auto_merge() {
    let (dir, target) = workspace();
    let mut s = FSZeroSession::with_root(dir.path());
    assert!(world(&mut s, "new:target.txt:l2|L2").0);
    assert!(world(&mut s, "new:target.txt:l4|L4").0);
    assert!(world(&mut s, "commit:W1").0);
    let (ok, detail) = world(&mut s, "commit:W2");
    assert!(ok, "disjoint hunks must auto-merge: {detail}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "l1\nL2\nl3\nL4\n");
}
