//! filesystem-v1 races.stale-edit / mutations.world / FileEngine apply:
//! mismatched preimages reject without last-writer-wins, overlay drop does
//! not publish, and FileEngine Edit with a wrong handle fails closed.

use fszero_engine::{
    DispatchSurface, FSZeroSession, ZeroFileEngine, classify_detail_to_error_class,
    dispatch_operation,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineErrorKind, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, FileReadRequest, KernelBudget, ReadOptions,
};

const ORIGINAL: &str = "alpha unique omega\n";
const MUTATED: &str = "alpha unique CHANGED\n";
const REPLACEMENT: &str = "UNIQUE";

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

fn assert_stale_preimage(detail: &str) {
    assert_eq!(
        classify_detail_to_error_class(detail),
        "stale_preimage",
        "filesystem-v1 races.stale-edit requires error class stale_preimage, got {detail}"
    );
}

#[test]
fn last_view_unique_replace_rejects_stale_preimage() {
    let (dir, target) = workspace();
    let mut s = FSZeroSession::with_root(dir.path());
    let (_, ok, detail) = s.execute('R', Some("target.txt"));
    assert!(ok, "snapshot read failed: {detail:?}");

    fs::write(&target, MUTATED).unwrap();

    let (_, ok, detail) = s.execute('E', Some("last:unique|UNIQUE"));
    assert!(!ok, "stale unique replace must fail, detail={detail:?}");
    let detail = detail.expect("edit detail");
    assert_stale_preimage(&detail);
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
}

#[test]
fn cas_base_unique_replace_rejects_stale_preimage() {
    let (dir, target) = workspace();
    let mut s = FSZeroSession::with_root(dir.path());
    let base = s.recovery.put_content_ref(ORIGINAL.as_bytes());
    fs::write(&target, MUTATED).unwrap();

    let outcome = dispatch_operation(
        &mut s,
        DispatchSurface::CodeMode,
        "fs.edit",
        &json!({
            "path": "target.txt",
            "find": "unique",
            "replace": REPLACEMENT,
            "base": base,
        }),
    );
    assert!(!outcome.result.ok, "stale CAS base must fail: {outcome:?}");
    assert!(!outcome.result.mutated);
    let class = outcome
        .result
        .error
        .as_ref()
        .map(|e| e.class.as_str())
        .unwrap_or_else(|| classify_detail_to_error_class(outcome.detail.as_deref().unwrap_or("")));
    assert_eq!(class, "stale_preimage", "{outcome:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), MUTATED);
}

#[test]
fn world_overlay_preview_and_drop_leave_base_unchanged() {
    let (dir, target) = workspace();
    {
        let mut s = FSZeroSession::with_root(dir.path());
        let (_, ok, detail) = s.execute('W', Some("new:target.txt:unique|UNIQUE"));
        assert!(ok, "world fork/edit failed: {detail:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);

        let (_, ok, detail) = s.execute('W', Some("preview:W1"));
        assert!(ok, "preview failed: {detail:?}");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            ORIGINAL,
            "preview must stay overlay-only"
        );

        let (_, ok, detail) = s.execute('W', Some("drop:W1"));
        assert!(ok, "drop failed: {detail:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);
    }
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        ORIGINAL,
        "session drop must not publish overlay"
    );
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
