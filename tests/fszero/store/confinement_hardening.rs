//! V6-F1 (ZS-SEC-001): confinement hardening red tests.
//!
//! A written path outside the session root must be impossible: symlink
//! escapes (tail AND mid-path, on create), `..` traversal, and absolute-path
//! smuggling are refused loud with a receipted effect record, while confined
//! writes (including through symlinks that stay inside the root) keep
//! working unchanged.

use fs_zero::core::path::{guard_write_target_parent, validate_rollback_path};
use fs_zero::{DispatchSurface, FSZeroSession, dispatch_operation};
use serde_json::json;
use std::fs;
use std::path::Path;

fn dispatch(s: &mut FSZeroSession, op: &str, args: serde_json::Value) -> (bool, String) {
    let outcome = dispatch_operation(s, DispatchSurface::CodeMode, op, &args);
    (
        outcome.result.ok,
        outcome
            .detail
            .unwrap_or_else(|| outcome.result.ack.unwrap_or_default()),
    )
}

fn expand_effect_json(s: &FSZeroSession, detail: &str) -> serde_json::Value {
    let r = detail
        .split_whitespace()
        .find_map(|tok| {
            tok.strip_prefix("effects=")
                .map(|r| r.trim_end_matches(')').to_string())
        })
        .unwrap_or_else(|| panic!("detail carries no effects= token: {detail}"));
    let bytes = s.expand(&r).unwrap_or_else(|| panic!("expand {r}"));
    serde_json::from_slice(&bytes).expect("effect record JSON")
}

/// THE missing case (per ZS-SEC-001): a symlink INSIDE the root pointing
/// OUTSIDE, used as a mid-path component of a write to a NOT-YET-EXISTING
/// file. The write must be refused (nothing lands outside), loud, and
/// receipted.
#[cfg(unix)]
#[test]
fn write_through_midpath_symlink_escape_is_refused_with_receipt() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("sub")).unwrap();
    // `sub/esc` is a symlink inside the root pointing at an outside dir.
    symlink(outside.path(), root.path().join("sub/esc")).unwrap();

    let mut s = FSZeroSession::with_root(root.path());
    let (ok, detail) = dispatch(
        &mut s,
        "fs.write",
        json!({"path": "sub/esc/new.txt", "content": "HACKED"}),
    );
    assert!(!ok, "mid-path symlink escape must fail: {detail}");
    assert!(detail.contains("outside root"), "{detail}");
    assert!(
        detail.contains("effects="),
        "escape must fail loud WITH a receipt: {detail}"
    );
    assert!(
        !outside.path().join("new.txt").exists(),
        "nothing may be written outside the root"
    );
    let rec = expand_effect_json(&s, &detail);
    assert!(rec["paths"].as_array().unwrap().is_empty(), "{rec}");
    assert_eq!(
        rec["refused"].as_array().unwrap(),
        &json!(["sub/esc/new.txt"]).as_array().unwrap().clone(),
        "{rec}"
    );
}

/// Same hole at the validator level (unit surface of the write-side jail):
/// `validate_rollback_path` must refuse a mid-path symlink escape on a
/// missing target.
#[cfg(unix)]
#[test]
fn validate_rollback_path_rejects_midpath_symlink_escape() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("sub")).unwrap();
    symlink(outside.path(), root.path().join("sub/esc")).unwrap();

    let err = validate_rollback_path(root.path(), Path::new("sub/esc/new.txt"))
        .expect_err("must refuse the mid-path symlink escape");
    assert!(err.contains("outside root"), "{err}");
    assert!(!outside.path().join("new.txt").exists());
}

/// A mid-path symlink that stays INSIDE the root is not a false positive:
/// the write resolves through it and lands at the real target inside root.
#[cfg(unix)]
#[test]
fn write_through_midpath_symlink_inside_root_still_works() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("real")).unwrap();
    fs::create_dir_all(root.path().join("sub")).unwrap();
    // `sub/link` -> `real` (both inside root).
    symlink(root.path().join("real"), root.path().join("sub/link")).unwrap();

    let mut s = FSZeroSession::with_root(root.path());
    let (ok, detail) = dispatch(
        &mut s,
        "fs.write",
        json!({"path": "sub/link/new.txt", "content": "ok\n"}),
    );
    assert!(ok, "in-root mid-path symlink must keep working: {detail}");
    assert_eq!(
        fs::read_to_string(root.path().join("real/new.txt")).unwrap(),
        "ok\n",
        "write lands at the real target inside the root"
    );
}

/// Write-time TOCTOU guard: a target whose parent is a symlink pointing
/// outside is refused AT WRITE TIME even if validation happened earlier.
#[cfg(unix)]
#[test]
fn guard_write_target_parent_refuses_symlinked_parent() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("leak")).unwrap();

    let err = guard_write_target_parent(root.path(), &root.path().join("leak/evil.txt"))
        .expect_err("write-time guard must refuse");
    assert!(err.contains("outside root"), "{err}");
    // Guard is not a false positive inside the root.
    fs::create_dir_all(root.path().join("ok")).unwrap();
    guard_write_target_parent(root.path(), &root.path().join("ok/evil.txt"))
        .expect("in-root parent passes the write-time guard");
}

/// Absolute-path smuggling: an absolute path outside the root is refused
/// for fs.write, loud and receipted; nothing lands outside.
#[test]
fn absolute_path_outside_root_write_refused() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let absolute = outside
        .path()
        .join("evil.txt")
        .to_string_lossy()
        .into_owned();

    let mut s = FSZeroSession::with_root(root.path());
    let (ok, detail) = dispatch(
        &mut s,
        "fs.write",
        json!({"path": absolute, "content": "HACKED"}),
    );
    assert!(!ok, "absolute escape must fail: {detail}");
    assert!(detail.contains("outside root"), "{detail}");
    assert!(
        detail.contains("effects="),
        "refusal must be receipted: {detail}"
    );
    assert!(!outside.path().join("evil.txt").exists());
}

/// `..` traversal on a write is refused loud, with a receipt, and the
/// traversal target never materializes.
#[test]
fn dotdot_traversal_write_refused() {
    let root = tempfile::tempdir().unwrap();
    let mut s = FSZeroSession::with_root(root.path());

    let (ok, detail) = dispatch(
        &mut s,
        "fs.write",
        json!({"path": "a/../../evil.txt", "content": "HACKED"}),
    );
    assert!(!ok, "dotdot escape must fail: {detail}");
    assert!(
        detail.contains("outside root") || detail.contains("parent"),
        "{detail}"
    );
    assert!(detail.contains("effects="), "refusal receipted: {detail}");
    let rec = expand_effect_json(&s, &detail);
    assert!(rec["paths"].as_array().unwrap().is_empty(), "{rec}");
    assert_eq!(
        rec["refused"].as_array().unwrap(),
        &json!(["a/../../evil.txt"]).as_array().unwrap().clone()
    );
    assert!(
        !root.path().parent().unwrap().join("evil.txt").exists(),
        "nothing may be written outside the root"
    );
}

/// No false positives: the full confined mutation cycle (write, edit, undo,
/// world commit) keeps working under the root, each op sealing its effect
/// record.
#[test]
fn confined_mutation_cycle_still_works() {
    let root = tempfile::tempdir().unwrap();
    let mut s = FSZeroSession::with_root(root.path());

    // write
    let (ok, detail) = dispatch(
        &mut s,
        "fs.write",
        json!({"path": "src/a.txt", "content": "one\n"}),
    );
    assert!(ok, "write: {detail}");
    assert!(detail.contains("effects="), "{detail}");
    // edit
    let (ok, detail) = dispatch(&mut s, "fs.edit", json!({"spec": "src/a.txt:one\n|TWO\n"}));
    assert!(ok, "edit: {detail}");
    assert!(detail.contains("effects="), "{detail}");
    assert_eq!(
        fs::read_to_string(root.path().join("src/a.txt")).unwrap(),
        "TWO\n"
    );
    // world commit
    let (ok, detail) = dispatch(
        &mut s,
        "fs.world",
        json!({"arg": "new:src/a.txt:TWO\n|THREE\n"}),
    );
    assert!(ok, "stage: {detail}");
    let (ok, detail) = dispatch(&mut s, "fs.world", json!({"arg": "commit:W1"}));
    assert!(ok, "commit: {detail}");
    assert!(detail.contains("effects="), "{detail}");
    assert_eq!(
        fs::read_to_string(root.path().join("src/a.txt")).unwrap(),
        "THREE\n"
    );
    // undo the world commit's mutation
    let (ok, detail) = dispatch(&mut s, "fs.undo", json!({"path": "src/a.txt"}));
    assert!(ok, "undo: {detail}");
    assert!(detail.contains("effects="), "{detail}");
    assert_eq!(
        fs::read_to_string(root.path().join("src/a.txt")).unwrap(),
        "TWO\n"
    );
}
