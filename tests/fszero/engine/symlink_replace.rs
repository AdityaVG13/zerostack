//! filesystem-v1: replacing a symlink replaces that link entry rather than
//! writing through it. FileEngine apply Write/Edit and session fs.write/edit
//! must not mutate the referent.

#![cfg(unix)]

use fszero_engine::{FSZeroSession, ZeroFileEngine, atomic_write};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineError, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, KernelBudget,
};

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

fn session_write(root: &Path) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('P', Some("link|replacement-via-link\n"));
    (ok, detail)
}

fn session_edit(root: &Path) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute('E', Some("link:original-target-bytes|replacement-via-link"));
    (ok, detail)
}

#[test]
fn atomic_write_replaces_symlink_directory_entry() {
    let fx = setup_in_root();
    atomic_write(&fx.link, REPLACEMENT).expect("atomic_write on symlink");
    assert_no_write_through(&fx, true);
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
fn session_write_replaces_symlink_not_target() {
    let fx = setup_in_root();
    let (ok, detail) = session_write(&fx.root);
    assert_no_write_through(&fx, ok);
    assert!(
        ok,
        "fs.write on an in-root symlink must succeed: {detail:?}"
    );
}

#[test]
fn session_edit_replaces_symlink_not_target() {
    let fx = setup_in_root();
    let (ok, detail) = session_edit(&fx.root);
    assert_no_write_through(&fx, ok);
    assert!(ok, "fs.edit on an in-root symlink must succeed: {detail:?}");
}

#[test]
fn write_to_symlink_pointing_outside_root_does_not_touch_referent() {
    let fx = setup_outside_root();
    let apply = apply_kind(&fx.root, FileEffectKind::Write, "link");
    assert_eq!(fs::read(&fx.target).unwrap(), ORIGINAL);
    let (write_ok, write_detail) = session_write(&fx.root);
    assert_eq!(
        fs::read(&fx.target).unwrap(),
        ORIGINAL,
        "session write must not mutate an outside-root referent: {write_detail:?}"
    );
    if apply.is_ok() || write_ok {
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
