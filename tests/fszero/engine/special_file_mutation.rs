//! filesystem-v1: FIFOs/sockets have no I/O guarantee. FileEngine apply/read
//! and session fs.write/edit/read/multiRead must refuse from metadata (not hang
//! on open). Lookup must list special nodes via dirent/stat only — never File::open.

#![cfg(unix)]

use fszero_engine::{FSZeroSession, ZeroFileEngine};
use serde_json::json;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineError, EngineErrorKind, EngineInvocation,
    FileEffectKind, FileEffectRequest, FileEngine, FileReadRequest, KernelBudget, LookupOptions,
    ReadOptions,
};

const HANG_BUDGET: Duration = Duration::from_millis(1500);
const PAYLOAD: &[u8] = b"must-not-land-on-special-file";

struct NoopCancel;
impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn invocation(root: &Path) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "special-file-mutation".into(),
            cell_id: "cell-1".into(),
            trace_id: "special-file-mutation-cell-1".into(),
            deadline_unix_ms: u64::MAX,
            budget: KernelBudget {
                wall_ms: 1_000,
                cpu_ms: 1_000,
                memory_bytes: 64 * 1024 * 1024,
                call_limit: 1_024,
                task_limit: 8,
                output_byte_limit: 64 * 1024,
            },
        },
        cancellation: Arc::new(NoopCancel),
    }
}

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
        .name("special-file-mutation".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .expect("spawn timeout worker");
    match rx.recv_timeout(HANG_BUDGET) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => {
            panic!(
                "timed out after {:?}: FIFO/socket operation hung instead of failing closed",
                HANG_BUDGET
            )
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("timeout worker panicked before returning a fail-closed result")
        }
    }
}

fn assert_kind_refused(detail: &str, kind: &str) {
    assert!(
        detail.contains("unsupported file kind") && detail.contains(kind),
        "expected unsupported file kind {kind}, got {detail}"
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

fn assert_still_socket(path: &Path) {
    let meta = fs::symlink_metadata(path).expect("socket metadata");
    assert!(
        meta.file_type().is_socket(),
        "{} must remain a socket",
        path.display()
    );
}

fn apply_kind(root: &Path, kind: FileEffectKind, rel: &str) -> Result<(), EngineError> {
    let engine = ZeroFileEngine::open(
        root,
        root.join(".zerostack"),
        "special-file-mutation-contract",
    )
    .unwrap();
    engine
        .apply(
            &invocation(root),
            FileEffectRequest {
                kind,
                path: PathBuf::from(rel),
                content: Some(PAYLOAD.to_vec()),
                patch: None,
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .map(|_| ())
}

fn session_write(root: &Path, spec: &'static str) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('P', Some(spec));
    (ok, detail)
}

fn session_edit(root: &Path, spec: &'static str) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('E', Some(spec));
    (ok, detail)
}

fn session_read(root: &Path, spec: &'static str) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('R', Some(spec));
    (ok, detail)
}

/// Fused `fs.multiRead` capture (`batch_ops::capture_file`) — not opcode `'R'`.
fn session_multi_read(root: &Path, paths: &[&str]) -> Vec<(bool, Option<String>)> {
    let mut s = FSZeroSession::with_root(root);
    let items: Vec<serde_json::Value> = paths.iter().copied().map(|path| json!(path)).collect();
    s.execute_batch_kernel("fs.multiRead", &items, &json!({}))
        .rows
        .into_iter()
        .map(|row| (row.ok, row.detail))
        .collect()
}

fn engine_read(root: &Path, rel: &str) -> Result<zero_abi::FileSnapshot, EngineError> {
    let engine = ZeroFileEngine::open(
        root,
        root.join(".zerostack"),
        "special-file-mutation-contract",
    )
    .unwrap();
    engine.read(
        &invocation(root),
        FileReadRequest {
            path: PathBuf::from(rel),
            options: ReadOptions::default(),
        },
    )
}

fn engine_lookup(
    root: &Path,
    dir: &str,
    options: LookupOptions,
) -> Result<Vec<PathBuf>, EngineError> {
    let engine = ZeroFileEngine::open(
        root,
        root.join(".zerostack"),
        "special-file-mutation-contract",
    )
    .unwrap();
    engine.lookup(&invocation(root), PathBuf::from(dir), options)
}

#[test]
fn file_engine_apply_refuses_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rel = "pipe.fifo";
    let path = root.join(rel);
    mkfifo(&path);

    for kind in [
        FileEffectKind::Write,
        FileEffectKind::Edit,
        FileEffectKind::Remove,
    ] {
        let root = root.clone();
        let err = within_timeout(move || {
            apply_kind(&root, kind, rel).expect_err("FIFO apply must fail closed")
        });
        assert_eq!(err.kind, EngineErrorKind::InvalidInput);
        assert_kind_refused(&err.detail, "fifo");
        assert_still_fifo(&path);
    }
}

#[test]
fn file_engine_apply_refuses_socket_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rel = "sock";
    let path = root.join(rel);
    let _listener = UnixListener::bind(&path).expect("bind unix socket");

    for kind in [
        FileEffectKind::Write,
        FileEffectKind::Edit,
        FileEffectKind::Remove,
    ] {
        let root = root.clone();
        let err = within_timeout(move || {
            apply_kind(&root, kind, rel).expect_err("socket apply must fail closed")
        });
        assert_eq!(err.kind, EngineErrorKind::InvalidInput);
        assert_kind_refused(&err.detail, "socket");
        assert_still_socket(&path);
    }
}

#[test]
fn session_write_edit_refuse_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let root_write = root.clone();
    let (write_ok, write_detail) = within_timeout(move || {
        session_write(&root_write, "pipe.fifo|must-not-land-on-special-file")
    });
    assert!(
        !write_ok,
        "fs.write FIFO must fail, detail={write_detail:?}"
    );
    assert_kind_refused(write_detail.as_deref().unwrap_or(""), "fifo");
    assert_still_fifo(&path);

    let (edit_ok, edit_detail) = within_timeout(move || session_edit(&root, "pipe.fifo:x|y"));
    assert!(!edit_ok, "fs.edit FIFO must fail, detail={edit_detail:?}");
    assert_kind_refused(edit_detail.as_deref().unwrap_or(""), "fifo");
    assert_still_fifo(&path);
}

#[test]
fn session_write_edit_refuse_socket_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("sock");
    let _listener = UnixListener::bind(&path).expect("bind unix socket");

    let root_write = root.clone();
    let (write_ok, write_detail) =
        within_timeout(move || session_write(&root_write, "sock|must-not-land-on-special-file"));
    assert!(
        !write_ok,
        "fs.write socket must fail, detail={write_detail:?}"
    );
    assert_kind_refused(write_detail.as_deref().unwrap_or(""), "socket");
    assert_still_socket(&path);

    let (edit_ok, edit_detail) = within_timeout(move || session_edit(&root, "sock:x|y"));
    assert!(!edit_ok, "fs.edit socket must fail, detail={edit_detail:?}");
    assert_kind_refused(edit_detail.as_deref().unwrap_or(""), "socket");
    assert_still_socket(&path);
}

#[test]
fn file_engine_read_snapshot_refuses_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rel = "pipe.fifo";
    let path = root.join(rel);
    mkfifo(&path);

    let root_read = root.clone();
    let err = within_timeout(move || {
        engine_read(&root_read, rel).expect_err("FileEngine::read FIFO must fail closed")
    });
    assert_eq!(err.kind, EngineErrorKind::InvalidInput);
    assert_kind_refused(&err.detail, "fifo");
    assert_still_fifo(&path);
}

#[test]
fn file_engine_read_snapshot_refuses_socket_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let rel = "sock";
    let path = root.join(rel);
    let _listener = UnixListener::bind(&path).expect("bind unix socket");

    let root_read = root.clone();
    let err = within_timeout(move || {
        engine_read(&root_read, rel).expect_err("FileEngine::read socket must fail closed")
    });
    assert_eq!(err.kind, EngineErrorKind::InvalidInput);
    assert_kind_refused(&err.detail, "socket");
    assert_still_socket(&path);
}

#[test]
fn session_read_refuses_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let root_read = root.clone();
    let (ok, detail) = within_timeout(move || session_read(&root_read, "pipe.fifo"));
    assert!(!ok, "fs.read FIFO must fail, detail={detail:?}");
    assert_kind_refused(detail.as_deref().unwrap_or(""), "fifo");

    let (range_ok, range_detail) = within_timeout(move || session_read(&root, "pipe.fifo#B0-16"));
    assert!(
        !range_ok,
        "fs.read FIFO byte-range must fail, detail={range_detail:?}"
    );
    assert_kind_refused(range_detail.as_deref().unwrap_or(""), "fifo");
    assert_still_fifo(&path);
}

#[test]
fn session_read_refuses_socket_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("sock");
    let _listener = UnixListener::bind(&path).expect("bind unix socket");

    let root_read = root.clone();
    let (ok, detail) = within_timeout(move || session_read(&root_read, "sock"));
    assert!(!ok, "fs.read socket must fail, detail={detail:?}");
    assert_kind_refused(detail.as_deref().unwrap_or(""), "socket");
    assert_still_socket(&path);
}

#[test]
fn session_multi_read_refuses_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let fifo = root.join("pipe.fifo");
    mkfifo(&fifo);
    fs::write(root.join("plain.txt"), b"ok").unwrap();

    let root_read = root.clone();
    let rows = within_timeout(move || session_multi_read(&root_read, &["pipe.fifo", "plain.txt"]));
    assert_eq!(rows.len(), 2, "multiRead must return one row per path");
    let (fifo_ok, fifo_detail) = &rows[0];
    assert!(
        !fifo_ok,
        "fs.multiRead FIFO capture must fail, detail={fifo_detail:?}"
    );
    assert_kind_refused(fifo_detail.as_deref().unwrap_or(""), "fifo");
    let (plain_ok, plain_detail) = &rows[1];
    assert!(
        *plain_ok,
        "fs.multiRead regular file must still succeed, detail={plain_detail:?}"
    );
    assert_still_fifo(&fifo);
}

#[test]
fn session_multi_read_refuses_socket_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let sock = root.join("sock");
    let _listener = UnixListener::bind(&sock).expect("bind unix socket");
    fs::write(root.join("plain.txt"), b"ok").unwrap();

    let root_read = root.clone();
    let rows = within_timeout(move || session_multi_read(&root_read, &["sock", "plain.txt"]));
    assert_eq!(rows.len(), 2, "multiRead must return one row per path");
    let (sock_ok, sock_detail) = &rows[0];
    assert!(
        !sock_ok,
        "fs.multiRead socket capture must fail, detail={sock_detail:?}"
    );
    assert_kind_refused(sock_detail.as_deref().unwrap_or(""), "socket");
    let (plain_ok, plain_detail) = &rows[1];
    assert!(
        *plain_ok,
        "fs.multiRead regular file must still succeed, detail={plain_detail:?}"
    );
    assert_still_socket(&sock);
}

#[test]
fn file_engine_lookup_lists_fifo_without_opening() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);
    fs::write(root.join("plain.txt"), b"ok").unwrap();

    let root_lookup = root.clone();
    let listed = within_timeout(move || {
        engine_lookup(
            &root_lookup,
            ".",
            LookupOptions {
                filter: None,
                limit: Some(32),
                recursive: false,
            },
        )
        .expect("lookup of a dir that contains a FIFO must not hang")
    });
    assert!(
        listed.iter().any(|p| p.ends_with("pipe.fifo")),
        "lookup must name the FIFO via dirent, got {listed:?}"
    );
    assert!(
        listed.iter().any(|p| p.ends_with("plain.txt")),
        "lookup must still list regular files, got {listed:?}"
    );
    assert_still_fifo(&path);
}

#[test]
fn file_engine_lookup_fifo_root_fails_closed_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("pipe.fifo");
    mkfifo(&path);

    let root_lookup = root.clone();
    let err = within_timeout(move || {
        engine_lookup(
            &root_lookup,
            "pipe.fifo",
            LookupOptions {
                filter: None,
                limit: Some(32),
                recursive: false,
            },
        )
        .expect_err("lookup of a FIFO root must fail closed")
    });
    assert_eq!(err.kind, EngineErrorKind::InvalidInput);
    assert!(
        err.detail.contains("not a directory"),
        "lookup FIFO root must refuse as non-directory, got {}",
        err.detail
    );
    assert_still_fifo(&path);
}

#[test]
fn ast_ingest_skips_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(root.join("ok.rs"), b"pub fn ok() {}\n").unwrap();
    let fifo = root.join("hang.rs");
    mkfifo(&fifo);

    let root_ingest = root.clone();
    let ingested = within_timeout(move || {
        let mut s = FSZeroSession::with_root(&root_ingest);
        s.ingest_file(&root_ingest, &root_ingest.join("hang.rs"))
    });
    assert!(
        ingested.is_none(),
        "ingest_file must skip FIFO rather than hang"
    );

    let root_reindex = root.clone();
    within_timeout(move || {
        let mut s = FSZeroSession::with_root(&root_reindex);
        s.reindex_path(&root_reindex.join("hang.rs"));
    });
    assert_still_fifo(&fifo);
}
