//! Cancel-before-publish: an expired request guard rejects without mutating.

#[path = "../common/mod.rs"]
mod common;

use common::TestRoot;
use fs_zero::{FSZeroSession, classify_detail_to_error_class};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ORIGINAL: &str = "original\n";

fn session_with_target() -> (TestRoot, FSZeroSession) {
    let root = TestRoot::new("req_exp");
    root.write("target.txt", ORIGINAL);
    let sess = FSZeroSession::with_root(root.path());
    (root, sess)
}

fn assert_target_unchanged(root: &TestRoot) {
    assert_eq!(
        fs::read_to_string(root.join("target.txt")).unwrap(),
        ORIGINAL
    );
}

#[test]
fn cancelled_write_leaves_target_unchanged() {
    let (root, mut s) = session_with_target();
    let cancel = Arc::new(AtomicBool::new(true));
    s.install_request_guard(
        Arc::clone(&cancel),
        Instant::now() + Duration::from_secs(60),
    );

    let (_, ok, detail) = s.execute('P', Some("target.txt|mutated"));
    assert!(!ok, "cancelled write must fail closed, detail={detail:?}");
    let detail = detail.expect("detail");
    assert_eq!(classify_detail_to_error_class(&detail), "cancelled");
    assert_target_unchanged(&root);
    assert!(cancel.load(Ordering::SeqCst));
}

#[test]
fn deadline_exceeded_write_leaves_target_unchanged() {
    let (root, mut s) = session_with_target();
    s.install_request_guard(
        Arc::new(AtomicBool::new(false)),
        Instant::now() - Duration::from_secs(1),
    );

    let (_, ok, detail) = s.execute('P', Some("target.txt|mutated"));
    assert!(!ok, "expired write must fail closed, detail={detail:?}");
    let detail = detail.expect("detail");
    assert_eq!(classify_detail_to_error_class(&detail), "deadline_exceeded");
    assert_target_unchanged(&root);
}

#[test]
fn cancelled_edit_parts_and_transact_leave_target_unchanged() {
    let (root, mut s) = session_with_target();
    s.install_request_guard(
        Arc::new(AtomicBool::new(true)),
        Instant::now() + Duration::from_secs(60),
    );

    let (_, edit_ok, edit_detail) = s.execute_edit_parts("target.txt", "original", "mutated");
    assert!(
        !edit_ok,
        "cancelled edit_parts must fail, detail={edit_detail:?}"
    );
    assert_eq!(
        classify_detail_to_error_class(edit_detail.as_deref().unwrap_or("")),
        "cancelled"
    );

    let steps = [json!({"op": "write", "path": "target.txt", "content": "mutated"})];
    let (_, tx_ok, tx_detail) = s.execute_transact_kernel(&steps);
    assert!(!tx_ok, "cancelled transact must fail, detail={tx_detail:?}");
    assert_eq!(
        classify_detail_to_error_class(tx_detail.as_deref().unwrap_or("")),
        "cancelled"
    );
    assert_target_unchanged(&root);
}
