//! Late cancel after a successful publish must not rewrite the outcome.
//!
//! Session execute and FileEngine apply that already committed bytes stay Ok
//! even if the request guard flips afterwards. Restore still polls cancel
//! after restore_bytes and before commit_file_bytes.

use fszero_engine::{FSZeroSession, ZeroFileEngine};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineInvocation, FileEffectKind, FileEffectRequest,
    FileEngine, KernelBudget,
};

const ORIGINAL: &[u8] = b"original\n";
const MUTATED: &[u8] = b"mutated-after-publish\n";

struct FlagCancel(Arc<AtomicBool>);

impl CancellationProbe for FlagCancel {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// True only after the target already contains the published postimage.
struct CancelOncePublished {
    path: PathBuf,
    published: Vec<u8>,
}

impl CancellationProbe for CancelOncePublished {
    fn is_cancelled(&self) -> bool {
        fs::read(&self.path)
            .ok()
            .is_some_and(|bytes| bytes == self.published)
    }
}

fn invocation_with(root: &Path, cancellation: Arc<dyn CancellationProbe>) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "cancel-after-publish".into(),
            cell_id: "cell-1".into(),
            trace_id: "cancel-after-publish-cell-1".into(),
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

fn write_request() -> FileEffectRequest {
    FileEffectRequest {
        kind: FileEffectKind::Write,
        path: PathBuf::from("target.txt"),
        content: Some(MUTATED.to_vec()),
        patch: None,
        expected_preimage: None,
        expect_absent: false,
    }
}

#[test]
fn session_write_stays_ok_when_guard_flips_after_return() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    let mut sess = FSZeroSession::with_root(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    sess.install_request_guard(
        Arc::clone(&cancel),
        Instant::now() + Duration::from_secs(60),
    );

    let (_, ok, detail) = sess.execute('P', Some("target.txt|mutated-after-publish\n"));
    assert!(ok, "write must publish, detail={detail:?}");
    cancel.store(true, Ordering::SeqCst);
    assert!(ok, "late cancel must not rewrite a published write");
    assert_eq!(fs::read(&target).unwrap(), MUTATED);
}

#[test]
fn file_engine_apply_receipt_stays_ok_when_cancel_flips_after_return() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "cancel-after-publish-contract",
    )
    .unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let receipt = engine
        .apply(
            &invocation_with(dir.path(), Arc::new(FlagCancel(Arc::clone(&flag)))),
            write_request(),
        )
        .expect("apply must publish");
    flag.store(true, Ordering::SeqCst);
    assert_eq!(receipt.kind, FileEffectKind::Write);
    assert!(
        receipt.after.is_some(),
        "published receipt must keep after handle"
    );
    assert_eq!(fs::read(&target).unwrap(), MUTATED);
}

#[test]
fn file_engine_apply_stays_ok_when_cancel_arrives_after_commit() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "cancel-after-publish-contract",
    )
    .unwrap();
    let receipt = engine
        .apply(
            &invocation_with(
                dir.path(),
                Arc::new(CancelOncePublished {
                    path: target.clone(),
                    published: MUTATED.to_vec(),
                }),
            ),
            write_request(),
        )
        .expect("post-commit cancel must not rewrite a published apply");
    assert_eq!(receipt.kind, FileEffectKind::Write);
    assert!(receipt.after.is_some());
    assert_eq!(fs::read(&target).unwrap(), MUTATED);
}
