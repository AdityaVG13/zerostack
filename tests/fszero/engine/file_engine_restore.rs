//! FileEngine Restore is CAS replay, not another Write.
//!
//! `apply(Restore)` with no content must materialize `expected_preimage` (or a
//! prior receipt's `before` handle) from CAS. Missing/wrong handles fail
//! closed; a cancelled invocation must not mutate.

use fszero_engine::ZeroFileEngine;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineErrorKind, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, FileReadRequest, KernelBudget, ReadOptions, ZeroHandle,
};

const ORIGINAL: &str = "alpha unique omega\n";
const MUTATED: &str = "alpha unique CHANGED\n";

struct NoopCancel;
impl CancellationProbe for NoopCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct AlwaysCancel;
impl CancellationProbe for AlwaysCancel {
    fn is_cancelled(&self) -> bool {
        true
    }
}

fn invocation(root: &Path) -> EngineInvocation {
    invocation_with_cancel(root, Arc::new(NoopCancel))
}

fn invocation_with_cancel(
    root: &Path,
    cancellation: Arc<dyn CancellationProbe>,
) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "file-engine-restore".into(),
            cell_id: "cell-1".into(),
            trace_id: "file-engine-restore-cell-1".into(),
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
        cancellation,
    }
}

fn workspace() -> (tempfile::TempDir, PathBuf, ZeroFileEngine) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "file-engine-restore-contract",
    )
    .unwrap();
    (dir, target, engine)
}

fn read_snapshot(engine: &ZeroFileEngine, root: &Path) -> zero_abi::FileSnapshot {
    engine
        .read(
            &invocation(root),
            FileReadRequest {
                path: PathBuf::from("target.txt"),
                options: ReadOptions::default(),
            },
        )
        .unwrap()
}

fn restore_request(preimage: Option<ZeroHandle>) -> FileEffectRequest {
    FileEffectRequest {
        kind: FileEffectKind::Restore,
        path: PathBuf::from("target.txt"),
        content: None,
        patch: None,
        expected_preimage: preimage,
        expect_absent: false,
    }
}

fn absent_cas_handle() -> ZeroHandle {
    ZeroHandle::parse(format!("z://blob/{}", "ab".repeat(32))).unwrap()
}

#[test]
fn restore_replays_cas_preimage_when_content_absent() {
    let (dir, target, engine) = workspace();
    let original = read_snapshot(&engine, dir.path());
    fs::write(&target, MUTATED).unwrap();

    let receipt = engine
        .apply(
            &invocation(dir.path()),
            restore_request(Some(original.content.clone())),
        )
        .expect("restore from CAS preimage must succeed");
    assert_eq!(receipt.kind, FileEffectKind::Restore);
    assert_eq!(receipt.after.as_ref(), Some(&original.content));
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);
}

#[test]
fn restore_replays_receipt_before_handle() {
    let (dir, target, engine) = workspace();
    let original = read_snapshot(&engine, dir.path());
    let write = engine
        .apply(
            &invocation(dir.path()),
            FileEffectRequest {
                kind: FileEffectKind::Write,
                path: PathBuf::from("target.txt"),
                content: Some(MUTATED.as_bytes().to_vec()),
                patch: None,
                expected_preimage: Some(original.content.clone()),
                expect_absent: false,
            },
        )
        .expect("write mutated bytes");
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
    let before = write
        .before
        .expect("write receipt must carry before handle");

    let receipt = engine
        .apply(
            &invocation(dir.path()),
            restore_request(Some(before.clone())),
        )
        .expect("restore from receipt before-handle must succeed");
    assert_eq!(receipt.kind, FileEffectKind::Restore);
    assert_eq!(receipt.after.as_ref(), Some(&before));
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);
}

#[test]
fn restore_fails_closed_when_preimage_missing_or_absent_from_cas() {
    let (dir, target, engine) = workspace();
    let _ = read_snapshot(&engine, dir.path());
    fs::write(&target, MUTATED).unwrap();

    let missing = engine
        .apply(&invocation(dir.path()), restore_request(None))
        .expect_err("restore without preimage or content must fail");
    assert_eq!(missing.kind, EngineErrorKind::InvalidInput);
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);

    let wrong = engine
        .apply(
            &invocation(dir.path()),
            restore_request(Some(absent_cas_handle())),
        )
        .expect_err("restore of a handle not in CAS must fail");
    assert_eq!(wrong.kind, EngineErrorKind::NotFound);
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
}

#[test]
fn restore_does_not_apply_when_cancelled() {
    let (dir, target, engine) = workspace();
    let original = read_snapshot(&engine, dir.path());
    fs::write(&target, MUTATED).unwrap();

    let err = engine
        .apply(
            &invocation_with_cancel(dir.path(), Arc::new(AlwaysCancel)),
            restore_request(Some(original.content)),
        )
        .expect_err("cancelled restore must fail closed");
    assert_eq!(err.kind, EngineErrorKind::Cancelled);
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
}

#[test]
fn read_accepts_explicit_parent_relative_external_path() {
    let constellation = tempfile::tempdir().unwrap();
    let workspace = constellation.path().join("ZeroStack");
    let sibling = constellation.path().join("TokenZero");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("contract.txt"), "sibling contract").unwrap();
    let engine = ZeroFileEngine::open(
        &workspace,
        workspace.join(".zerostack"),
        "constellation-read-contract",
    )
    .unwrap();

    let snapshot = engine
        .read(
            &invocation(&workspace),
            FileReadRequest {
                path: PathBuf::from("../TokenZero/contract.txt"),
                options: ReadOptions::default(),
            },
        )
        .expect("explicit parent-relative path is an external read");
    assert_eq!(snapshot.inline_utf8.as_deref(), Some("sibling contract"));
}

#[test]
fn same_length_edit_invalidates_read_cache() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("target.txt"), "BEFORE\n").unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "read-cache-contract",
    )
    .unwrap();
    let before = read_snapshot(&engine, dir.path());

    engine
        .apply(
            &invocation(dir.path()),
            FileEffectRequest {
                kind: FileEffectKind::Edit,
                path: PathBuf::from("target.txt"),
                content: None,
                patch: Some(r#"{"find":"BEFORE","replacement":"AFTER!"}"#.into()),
                expected_preimage: Some(before.content),
                expect_absent: false,
            },
        )
        .unwrap();

    let after = read_snapshot(&engine, dir.path());
    assert_eq!(after.inline_utf8.as_deref(), Some("AFTER!\n"));
}
