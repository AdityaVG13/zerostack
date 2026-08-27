//! World `new:` / `edit:` on a FIFO must fail closed from metadata.
//! `fs::read` of a FIFO blocks the worker; refuse before the content open.

#![cfg(unix)]

use fszero_engine::FSZeroSession;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

const HANG_BUDGET: Duration = Duration::from_millis(1500);

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("spawn mkfifo");
    assert!(
        status.success(),
        "mkfifo {} failed: {status}",
        path.display()
    );
}

fn within_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("world-fifo".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .expect("spawn timeout worker");
    match rx.recv_timeout(HANG_BUDGET) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => {
            panic!("timed out after {HANG_BUDGET:?}: world FIFO op hung instead of failing closed")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("timeout worker panicked before returning a fail-closed result")
        }
    }
}

fn assert_kind_refused(detail: &str) {
    assert!(
        detail.contains("unsupported file kind") && detail.contains("fifo"),
        "expected unsupported file kind fifo, got {detail}"
    );
}

fn assert_still_fifo(path: &Path) {
    let meta = fs::symlink_metadata(path).expect("fifo metadata");
    assert!(
        meta.file_type().is_fifo(),
        "{} must remain a FIFO",
        path.display()
    );
}

fn world(s: &mut FSZeroSession, arg: &str) -> (bool, String) {
    let (_, ok, detail) = s.execute('W', Some(arg));
    (ok, detail.unwrap_or_default())
}

#[test]
fn world_new_on_fifo_fails_closed_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let (ok, detail) = within_timeout({
        let root = root.clone();
        move || {
            let mut s = FSZeroSession::with_root(&root);
            world(&mut s, "new:pipe.fifo:x|y")
        }
    });
    assert!(!ok, "world new on FIFO must fail, detail={detail}");
    assert_kind_refused(&detail);
    assert_still_fifo(&path);
}

#[test]
fn world_edit_on_fifo_fails_closed_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let (ok, detail) = within_timeout({
        let root = root.clone();
        move || {
            let mut s = FSZeroSession::with_root(&root);
            let (ok, detail) = world(&mut s, "fork");
            assert!(ok, "fork: {detail}");
            world(&mut s, "edit:W1:pipe.fifo:x|y")
        }
    });
    assert!(!ok, "world edit on FIFO must fail, detail={detail}");
    assert_kind_refused(&detail);
    assert_still_fifo(&path);
}
