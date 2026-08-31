//! Replacing a symlink through FileEngine replaces the link entry, not its referent.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineError, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, KernelBudget,
};
use zero_fs::ZeroFileEngine;

const ORIGINAL: &[u8] = b"original-target-bytes\n";
const REPLACEMENT: &[u8] = b"replacement-via-link\n";

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
            session_id: "symlink-replace".into(),
            cell_id: "cell-1".into(),
            trace_id: "symlink-replace-cell-1".into(),
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

struct LinkFixture {
    _root_dir: tempfile::TempDir,
    _outside_dir: Option<tempfile::TempDir>,
    root: PathBuf,
    link: PathBuf,
    target: PathBuf,
}

fn setup_in_root() -> LinkFixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("target.txt");
    let link = root.join("link");
    fs::write(&target, ORIGINAL).unwrap();
    symlink("target.txt", &link).unwrap();
    LinkFixture {
        _root_dir: dir,
        _outside_dir: None,
        root,
        link,
        target,
    }
}

fn setup_outside_root() -> LinkFixture {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = outside.path().join("target.txt");
    let link = root.join("link");
    fs::write(&target, ORIGINAL).unwrap();
    symlink(&target, &link).unwrap();
    LinkFixture {
        _root_dir: dir,
        _outside_dir: Some(outside),
        root,
        link,
        target,
    }
}

fn assert_no_write_through(fx: &LinkFixture, write_ok: bool) {
    assert_eq!(
        fs::read(&fx.target).expect("read target"),
        ORIGINAL,
        "write-through is forbidden: referent bytes must stay original"
    );
    let meta = fs::symlink_metadata(&fx.link).expect("link metadata");
    if write_ok {
        assert!(
            meta.file_type().is_file(),
            "successful write must replace the symlink inode with a regular file"
        );
        assert!(
            !meta.file_type().is_symlink(),
            "successful write must not leave a symlink at the write path"
        );
        assert_eq!(
            fs::read(&fx.link).expect("read replaced link"),
            REPLACEMENT,
            "replacement bytes land on the directory entry, not the referent"
        );
    } else {
        assert!(
            meta.file_type().is_symlink(),
            "refused write must leave the symlink inode in place"
        );
    }
}

fn apply_kind(root: &Path, kind: FileEffectKind, rel: &str) -> Result<(), EngineError> {
    let engine =
        ZeroFileEngine::open(root, root.join(".zerostack"), "symlink-replace-contract").unwrap();
    engine
        .apply(
            &invocation(root),
            FileEffectRequest {
                kind,
                path: PathBuf::from(rel),
                content: Some(REPLACEMENT.to_vec()),
                patch: None,
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .map(|_| ())
}

#[test]
fn file_engine_apply_write_replaces_symlink_not_target() {
    let fx = setup_in_root();
    let result = apply_kind(&fx.root, FileEffectKind::Write, "link");
    assert_no_write_through(&fx, result.is_ok());
    result.expect("FileEngine Write on an in-root symlink must succeed by replacing the link");
}

#[test]
fn file_engine_apply_edit_replaces_symlink_not_target() {
    let fx = setup_in_root();
    let result = apply_kind(&fx.root, FileEffectKind::Edit, "link");
    assert_no_write_through(&fx, result.is_ok());
    result.expect("FileEngine Edit on an in-root symlink must succeed by replacing the link");
}

#[test]
fn file_engine_restore_recreates_replaced_symlink() {
    let fx = setup_in_root();
    let engine = ZeroFileEngine::open(
        &fx.root,
        fx.root.join(".zerostack"),
        "symlink-restore-contract",
    )
    .unwrap();
    let receipt = engine
        .apply(
            &invocation(&fx.root),
            FileEffectRequest {
                kind: FileEffectKind::Write,
                path: PathBuf::from("link"),
                content: Some(REPLACEMENT.to_vec()),
                patch: None,
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .expect("replace symlink entry");

    engine
        .restore(&invocation(&fx.root), &receipt)
        .expect("restore symlink entry");

    assert!(
        fs::symlink_metadata(&fx.link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "rollback must restore the original symlink inode"
    );
    assert_eq!(
        fs::read_link(&fx.link).unwrap(),
        PathBuf::from("target.txt")
    );
    assert_eq!(fs::read(&fx.target).unwrap(), ORIGINAL);
}

#[test]
fn write_to_symlink_pointing_outside_root_does_not_touch_referent() {
    let fx = setup_outside_root();
    let apply = apply_kind(&fx.root, FileEffectKind::Write, "link");
    assert_eq!(fs::read(&fx.target).unwrap(), ORIGINAL);
    if apply.is_ok() {
        let meta = fs::symlink_metadata(&fx.link).unwrap();
        assert!(
            meta.file_type().is_file(),
            "if the write is accepted, it must replace the in-root link inode"
        );
        assert_eq!(fs::read(&fx.link).unwrap(), REPLACEMENT);
    } else {
        assert!(
            fs::symlink_metadata(&fx.link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "refused outside-link write must leave the symlink"
        );
    }
}

#[test]
fn file_engine_rejects_absolute_writes_outside_workspace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("created.txt");
    let engine = ZeroFileEngine::open(
        root.path(),
        root.path().join(".zerostack"),
        "workspace-jail-contract",
    )
    .unwrap();

    let error = engine
        .apply(
            &invocation(root.path()),
            FileEffectRequest {
                kind: FileEffectKind::Write,
                path: target.clone(),
                content: Some(REPLACEMENT.to_vec()),
                patch: None,
                expected_preimage: None,
                expect_absent: true,
            },
        )
        .expect_err("absolute writes outside the workspace must fail closed");

    assert_eq!(error.kind, zero_abi::EngineErrorKind::OutsideWorkspace);
    assert!(
        !target.exists(),
        "a rejected write must have no side effect"
    );
}

#[test]
fn file_engine_rejects_symlink_parent_before_creating_outside_directories() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let engine = ZeroFileEngine::open(
        root.path(),
        root.path().join(".zerostack"),
        "workspace-jail-contract",
    )
    .unwrap();

    let error = engine
        .apply(
            &invocation(root.path()),
            FileEffectRequest {
                kind: FileEffectKind::Write,
                path: PathBuf::from("escape/new/created.txt"),
                content: Some(REPLACEMENT.to_vec()),
                patch: None,
                expected_preimage: None,
                expect_absent: true,
            },
        )
        .expect_err("a symlinked parent outside the workspace must fail closed");

    assert_eq!(error.kind, zero_abi::EngineErrorKind::OutsideWorkspace);
    assert!(
        !outside.path().join("new").exists(),
        "validation must happen before create_dir_all"
    );
}

#[test]
fn file_engine_remove_missing_path_does_not_create_its_parent() {
    let root = tempfile::tempdir().unwrap();
    let engine = ZeroFileEngine::open(
        root.path(),
        root.path().join(".zerostack"),
        "workspace-jail-contract",
    )
    .unwrap();

    engine
        .apply(
            &invocation(root.path()),
            FileEffectRequest {
                kind: FileEffectKind::Remove,
                path: PathBuf::from("missing/created-by-validation.txt"),
                content: None,
                patch: None,
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .expect_err("removing a missing path must fail");

    assert!(
        !root.path().join("missing").exists(),
        "a failed remove must not create directories"
    );
}
