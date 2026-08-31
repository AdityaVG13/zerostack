//! FileEngine rejects FIFO and socket I/O without opening special nodes.
//! Lookup still lists them through directory metadata.

#![cfg(unix)]

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
use zero_fs::ZeroFileEngine;

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
