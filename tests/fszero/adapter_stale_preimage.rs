//! FileEngine edits reject stale preimages without mutating current bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineErrorKind, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, FileReadRequest, KernelBudget, ReadOptions,
};
use zero_fs::ZeroFileEngine;

const ORIGINAL: &str = "alpha unique omega\n";
const MUTATED: &str = "alpha unique CHANGED\n";

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
            session_id: "stale-preimage".into(),
            cell_id: "cell-1".into(),
            trace_id: "stale-preimage-cell-1".into(),
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

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    (dir, target)
}

#[test]
fn file_engine_apply_edit_rejects_wrong_preimage() {
    let (dir, target) = workspace();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "stale-preimage-contract",
    )
    .unwrap();
    let inv = invocation(dir.path());
    let snapshot = engine
        .read(
            &inv,
            FileReadRequest {
                path: PathBuf::from("target.txt"),
                options: ReadOptions::default(),
            },
        )
        .unwrap();

    fs::write(&target, MUTATED).unwrap();

    let err = engine
        .apply(
            &inv,
            FileEffectRequest {
                kind: FileEffectKind::Edit,
                path: PathBuf::from("target.txt"),
                content: Some(b"alpha UNIQUE omega\n".to_vec()),
                patch: None,
                expected_preimage: Some(snapshot.content),
                expect_absent: false,
            },
        )
        .expect_err("wrong preimage must fail closed");
    assert_eq!(err.kind, EngineErrorKind::Conflict);
    assert!(
        err.detail.contains("stale preimage"),
        "FileEngine Edit must name stale preimage, got {}",
        err.detail
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
}
