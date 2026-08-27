//! filesystem-v1: atomic replacement of one hard link does not mutate sibling
//! links. Path A and B share an inode; write to A must give A new bytes and a
//! new inode, while B keeps the original bytes.

#![cfg(unix)]

use fszero_engine::{FSZeroSession, ZeroFileEngine, atomic_write};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineError, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, KernelBudget,
};

const ORIGINAL: &[u8] = b"original-hardlink-bytes\n";
const REPLACEMENT: &[u8] = b"replacement-via-hardlink\n";

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
            session_id: "hardlink-replace".into(),
            cell_id: "cell-1".into(),
            trace_id: "hardlink-replace-cell-1".into(),
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

struct HardLinkFixture {
    _root_dir: tempfile::TempDir,
    root: PathBuf,
    a: PathBuf,
    b: PathBuf,
    shared_ino: u64,
}

fn setup() -> HardLinkFixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let a = root.join("a");
    let b = root.join("b");
    fs::write(&a, ORIGINAL).unwrap();
    fs::hard_link(&a, &b).expect("unix hard_link");
    let a_meta = fs::metadata(&a).expect("a metadata");
    let b_meta = fs::metadata(&b).expect("b metadata");
    assert_eq!(a_meta.ino(), b_meta.ino(), "A and B must share an inode");
    assert_eq!(a_meta.nlink(), 2, "link count starts at 2");
    HardLinkFixture {
        _root_dir: dir,
        root,
        a,
        b,
        shared_ino: a_meta.ino(),
    }
}

fn assert_sibling_untouched(fx: &HardLinkFixture) {
    assert_eq!(
        fs::read(&fx.a).expect("read A"),
        REPLACEMENT,
        "A must have the replacement bytes"
    );
    assert_eq!(
        fs::read(&fx.b).expect("read B"),
        ORIGINAL,
        "sibling hard link B must keep original bytes"
    );
    let a_meta = fs::metadata(&fx.a).expect("A metadata after write");
    let b_meta = fs::metadata(&fx.b).expect("B metadata after write");
    assert_ne!(
        a_meta.ino(),
        b_meta.ino(),
        "write to A must publish a new inode"
    );
    assert_ne!(
        a_meta.ino(),
        fx.shared_ino,
        "A must not keep the shared inode"
    );
    assert_eq!(
        b_meta.ino(),
        fx.shared_ino,
        "B must keep the original inode"
    );
    assert_eq!(
        b_meta.nlink(),
        1,
        "B is the remaining name of the old inode"
    );
}

fn apply_kind(root: &Path, kind: FileEffectKind, rel: &str) -> Result<(), EngineError> {
    let engine =
        ZeroFileEngine::open(root, root.join(".zerostack"), "hardlink-replace-contract").unwrap();
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
    let (_, ok, detail) = s.execute('P', Some("a|replacement-via-hardlink\n"));
    (ok, detail)
}

fn session_edit(root: &Path) -> (bool, Option<String>) {
    let mut s = FSZeroSession::with_root(root);
    let (_, ok, detail) = s.execute(
        'E',
        Some("a:original-hardlink-bytes|replacement-via-hardlink"),
    );
    (ok, detail)
}

#[test]
fn atomic_write_does_not_mutate_sibling_hard_link() {
    let fx = setup();
    atomic_write(&fx.a, REPLACEMENT).expect("atomic_write on hard-linked A");
    assert_sibling_untouched(&fx);
}

#[test]
fn file_engine_apply_write_does_not_mutate_sibling_hard_link() {
    let fx = setup();
    apply_kind(&fx.root, FileEffectKind::Write, "a")
        .expect("FileEngine Write on hard-linked A must succeed");
    assert_sibling_untouched(&fx);
}

#[test]
fn file_engine_apply_edit_does_not_mutate_sibling_hard_link() {
    let fx = setup();
    apply_kind(&fx.root, FileEffectKind::Edit, "a")
        .expect("FileEngine Edit on hard-linked A must succeed");
    assert_sibling_untouched(&fx);
}

#[test]
fn session_write_does_not_mutate_sibling_hard_link() {
    let fx = setup();
    let (ok, detail) = session_write(&fx.root);
    assert!(ok, "fs.write on hard-linked A must succeed: {detail:?}");
    assert_sibling_untouched(&fx);
}

#[test]
fn session_edit_does_not_mutate_sibling_hard_link() {
    let fx = setup();
    let (ok, detail) = session_edit(&fx.root);
    assert!(ok, "fs.edit on hard-linked A must succeed: {detail:?}");
    assert_sibling_untouched(&fx);
}
