//! CodeMode plan rollback `restore_file_for_rollback` used to `fs::write`
//! then `OpenOptions::write(true).open` a FIFO and hang. Refuse from
//! metadata before the content open.

#![cfg(unix)]

use fszero_engine::FSZeroSession;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, SystemTime};

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
        .name("rollback-fifo".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .expect("spawn timeout worker");
    match rx.recv_timeout(HANG_BUDGET) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => {
            panic!(
                "timed out after {HANG_BUDGET:?}: restore_file_for_rollback hung on FIFO instead of failing closed"
            )
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

#[test]
fn restore_file_for_rollback_on_fifo_fails_closed_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let err = within_timeout({
        let root = root.clone();
        move || {
            let mut s = FSZeroSession::with_root(&root);
            // Relative arg: validate_rollback_path joins under the canonical
            // root (macOS `/var` vs `/private/var`).
            s.restore_file_for_rollback(
                Path::new("pipe.fifo"),
                b"must-not-land-on-fifo",
                Some(SystemTime::now()),
                None,
                None,
            )
            .expect_err("restore_file_for_rollback FIFO must fail closed")
        }
    });
    assert_kind_refused(&err);
    assert_still_fifo(&path);
}
