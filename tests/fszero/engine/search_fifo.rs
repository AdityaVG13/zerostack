//! Session search over a workspace that contains a FIFO must not hang
//! on content open. Product policy: skip the FIFO, still hit regular files.

#![cfg(unix)]

use fszero_engine::FSZeroSession;
use fszero_engine::search_prefilter_eval::{
    LazyBigramIndex, scan_bigram_memmem, scan_contains_literal,
};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

const HANG_BUDGET: Duration = Duration::from_millis(1500);
const NEEDLE: &str = "search_fifo_needle_unique";

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
        .name("search-fifo".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .expect("spawn timeout worker");
    match rx.recv_timeout(HANG_BUDGET) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => {
            panic!("timed out after {HANG_BUDGET:?}: search FIFO op hung instead of skipping")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("timeout worker panicked before returning a skip result")
        }
    }
}

fn assert_still_fifo(path: &Path) {
    let meta = fs::symlink_metadata(path).expect("fifo metadata");
    assert!(
        meta.file_type().is_fifo(),
        "{} must remain a FIFO",
        path.display()
    );
}

fn session_search(root: &Path, query: &str) -> (bool, String, String) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('S', Some(query));
    let payload = s
        .expand("search")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    (ok, detail.unwrap_or_default(), payload)
}

#[test]
fn session_search_skips_fifo_and_hits_regular_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("hit.rs"), format!("fn {NEEDLE}() {{}}\n")).unwrap();
    let fifo = root.join("hang.rs");
    mkfifo(&fifo);

    let (ok, detail, payload) = within_timeout({
        let root = root.clone();
        move || session_search(&root, NEEDLE)
    });
    assert!(
        ok,
        "search must succeed (skip FIFO, not fail), detail={detail}"
    );
    assert!(
        payload.contains(NEEDLE),
        "search must still hit the regular file, payload={payload}"
    );
    assert!(
        !payload.contains("hang.rs"),
        "search must skip the FIFO, payload={payload}"
    );
    assert_still_fifo(&fifo);
}

#[test]
fn literal_scan_skips_fifo_keys_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("hit.rs"), format!("fn {NEEDLE}() {{}}\n")).unwrap();
    let fifo = root.join("hang.rs");
    mkfifo(&fifo);

    let mut keys = HashSet::new();
    keys.insert("hit.rs".to_string());
    keys.insert("hang.rs".to_string());
    let terms = vec![NEEDLE.to_string()];

    let contains = within_timeout({
        let root = root.clone();
        let keys = keys.clone();
        let terms = terms.clone();
        move || scan_contains_literal(&root, &keys, &terms, 16)
    });
    assert!(
        contains
            .iter()
            .any(|h| h.file_key.as_ref() == "hit.rs" && h.text.contains(NEEDLE)),
        "contains scan must hit the regular file, got {contains:?}"
    );
    assert!(
        contains.iter().all(|h| h.file_key.as_ref() != "hang.rs"),
        "contains scan must skip the FIFO, got {contains:?}"
    );

    let bigram = within_timeout({
        let root = root.clone();
        let keys = keys.clone();
        let terms = terms.clone();
        move || {
            let mut index = LazyBigramIndex::new();
            scan_bigram_memmem(&root, &keys, &terms, 16, &mut index)
        }
    });
    assert!(
        bigram
            .iter()
            .any(|h| h.file_key.as_ref() == "hit.rs" && h.text.contains(NEEDLE)),
        "bigram scan must hit the regular file, got {bigram:?}"
    );
    assert!(
        bigram.iter().all(|h| h.file_key.as_ref() != "hang.rs"),
        "bigram scan must skip the FIFO, got {bigram:?}"
    );
    assert_still_fifo(&fifo);
}
