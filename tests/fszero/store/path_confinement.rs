//! Path confinement red tests (fszero-w2g.48 / .23).
//!
//! Missing-file rollback must not accept embedded `..` after a Normal component.

use fs_zero::core::path::{canonical_path_within_root, lexical_normalize, validate_rollback_path};
use fs_zero::{DispatchSurface, FSZeroSession, dispatch_operation};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn rollback_rejects_embedded_dotdot_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().parent().unwrap().join("OUTSIDE_OF_ROOT");
    // Do not create outside under root — sibling of root.
    let attack = PathBuf::from("nested/../../").join(outside.file_name().unwrap());
    // attack relative to root: nested/../../OUTSIDE_OF_ROOT
    let err = validate_rollback_path(root.path(), &attack).expect_err("must reject escape");
    assert!(
        err.contains("outside root") || err.contains("parent"),
        "unexpected err: {err}"
    );
}

#[test]
fn rollback_rejects_sub_dotdot_dotdot_secret() {
    let root = tempfile::tempdir().unwrap();
    // Classic CE: first component Normal("sub") fooled the old strip_prefix guard.
    let attack = Path::new("sub/../../secret");
    let err = validate_rollback_path(root.path(), attack).expect_err("must reject");
    assert!(err.contains("outside root"), "unexpected err: {err}");
}

#[test]
fn rollback_accepts_missing_nested_path_under_root() {
    let root = tempfile::tempdir().unwrap();
    let missing = Path::new("a/b/c.txt");
    let got = validate_rollback_path(root.path(), missing).expect("in-root missing path ok");
    let expect = fs::canonicalize(root.path()).unwrap().join("a/b/c.txt");
    // lexical join under root
    assert!(got.ends_with("a/b/c.txt") || got == expect);
    assert!(canonical_path_within_root(
        &fs::canonicalize(root.path()).unwrap(),
        &lexical_normalize(&got)
    ));
}

#[test]
fn lexical_normalize_collapses_dotdot() {
    let p = Path::new("/proj/nested/../../secret");
    let n = lexical_normalize(p);
    assert_eq!(n, Path::new("/secret"));
}

#[test]
fn within_root_rejects_parent_dir_in_rest() {
    let root = Path::new("/workspace/root");
    // Non-normalized join shape (what the bug accepted).
    let bad = Path::new("/workspace/root/sub/../../secret");
    assert!(!canonical_path_within_root(root, bad));
    let good = Path::new("/workspace/root/a/b");
    assert!(canonical_path_within_root(root, good));
}

use fs_zero::{negotiate_shared_interop, validate_peer_descriptor};
use serde_json::json;

/// Minimal peer descriptor matching production shape (`zeroref` contract,
/// string `interop.shared_interop`, bool attached/writable).
fn base_cap(shared_interop: &str, attached: bool, writable: bool) -> serde_json::Value {
    json!({
        "contract": {
            "name": "zeroref",
            "major": 1,
            "minor": 0
        },
        "hash": { "algo": "sha256", "hex_len": 64 },
        "shared_cas": {
            "attached": attached,
            "writable": writable,
            "layout": "blobs/sha256/<hh>/<hash>",
            "version": 1
        },
        "interop": { "shared_interop": shared_interop }
    })
}

#[test]
fn peer_validation_ok_does_not_imply_shared_interop() {
    let a = base_cap("disabled", false, false);
    let b = base_cap("disabled", false, false);
    validate_peer_descriptor(&a, &b).expect("identity match");
    assert!(!negotiate_shared_interop(&a, &b).unwrap());
}

#[test]
fn shared_interop_requires_both_attached_writable_and_enabled() {
    let live = base_cap("enabled", true, true);
    let dead = base_cap("enabled", true, false); // attached but not writable
    let read_only = base_cap("read_only", true, false);
    assert!(!negotiate_shared_interop(&live, &dead).unwrap());
    assert!(!negotiate_shared_interop(&live, &read_only).unwrap());
    assert!(negotiate_shared_interop(&live, &live).unwrap());
}

/// fszero-r2ia: an absolute `ls` path must say paths are root-relative, echo the
/// active root, and offer a corrected example — "absolute path rejected" alone
/// left the caller no way to self-correct.
#[test]
fn absolute_ls_path_names_the_root_relative_rule() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    let mut session = FSZeroSession::with_root(root.path());

    let absolute = root.path().join("src").to_string_lossy().into_owned();
    let outcome = dispatch_operation(
        &mut session,
        DispatchSurface::CodeMode,
        "fs.ls",
        &json!({ "path": absolute }),
    );

    let message = format!(
        "{:?} {:?} {:?}",
        outcome.result.error, outcome.result.ack, outcome.detail
    );

    assert!(!outcome.result.ok, "absolute ls path must fail: {message}");
    assert!(
        message.contains("relative to the session root"),
        "must state the root-relative rule: {message}"
    );
    assert!(
        message.contains(&root.path().display().to_string()),
        "must echo the active root: {message}"
    );
    assert!(
        message.contains("path:'.'"),
        "must give a corrected example: {message}"
    );
}
