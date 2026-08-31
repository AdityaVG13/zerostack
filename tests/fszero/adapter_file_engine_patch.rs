//! FileEngine Edit honors FileEffectRequest.patch as unique find/replace.
//! A present patch must not be a silent no-op: unique JSON
//! find/replacement is applied against current bytes, and any other shape fails closed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zero_abi::{
    CancellationProbe, EngineCallContext, EngineErrorKind, EngineInvocation, FileEffectKind,
    FileEffectRequest, FileEngine, FileReadRequest, KernelBudget, ReadOptions,
};
use zero_fs::ZeroFileEngine;

const ORIGINAL: &str = "alpha unique omega\n";
const PATCHED: &str = "alpha UNIQUE omega\n";

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
            session_id: "file-engine-patch".into(),
            cell_id: "cell-1".into(),
            trace_id: "file-engine-patch-cell-1".into(),
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

fn workspace() -> (tempfile::TempDir, PathBuf, ZeroFileEngine) {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, ORIGINAL).unwrap();
    let engine = ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "file-engine-patch-contract",
    )
    .unwrap();
    (dir, target, engine)
}

fn snapshot_handle(engine: &ZeroFileEngine, root: &Path) -> zero_abi::ZeroHandle {
    engine
        .read(
            &invocation(root),
            FileReadRequest {
                path: PathBuf::from("target.txt"),
                options: ReadOptions::default(),
            },
        )
        .unwrap()
        .content
}

fn edit_request(
    patch: Option<String>,
    content: Option<Vec<u8>>,
    expected_preimage: Option<zero_abi::ZeroHandle>,
) -> FileEffectRequest {
    FileEffectRequest {
        kind: FileEffectKind::Edit,
        path: PathBuf::from("target.txt"),
        content,
        patch,
        expected_preimage,
        expect_absent: false,
    }
}

#[test]
fn canonical_patch_shapes_replace_once() {
    let cases = [
        serde_json::json!({"find": "unique", "replacement": "UNIQUE"}),
        serde_json::json!({
            "kind": "replace_exact",
            "old": "unique",
            "replacement": "UNIQUE",
            "expectedCount": 1,
        }),
    ];

    for patch in cases {
        let (dir, target, engine) = workspace();
        let preimage = snapshot_handle(&engine, dir.path());
        let receipt = engine
            .apply(
                &invocation(dir.path()),
                edit_request(Some(patch.to_string()), None, Some(preimage.clone())),
            )
            .expect("canonical patch must apply");
        assert_eq!(receipt.kind, FileEffectKind::Edit);
        assert_eq!(receipt.before.as_ref(), Some(&preimage));
        assert_eq!(fs::read_to_string(&target).unwrap(), PATCHED);
    }
}

#[test]
fn noncanonical_patch_fields_are_rejected_without_mutation() {
    let cases = [
        (
            "old/new",
            serde_json::json!({"old": "unique", "new": "UNIQUE"}),
        ),
        (
            "pattern/replace",
            serde_json::json!({"pattern": "unique", "replace": "UNIQUE"}),
        ),
        (
            "find/replace",
            serde_json::json!({"find": "unique", "replace": "UNIQUE"}),
        ),
        (
            "old/replacement",
            serde_json::json!({"old": "unique", "replacement": "UNIQUE"}),
        ),
        (
            "find/new",
            serde_json::json!({"find": "unique", "new": "UNIQUE"}),
        ),
        (
            "expected_count",
            serde_json::json!({
                "kind": "replace_exact",
                "find": "unique",
                "replacement": "UNIQUE",
                "expected_count": 1,
            }),
        ),
        (
            "unknown field",
            serde_json::json!({
                "find": "unique",
                "replacement": "UNIQUE",
                "unexpected": true,
            }),
        ),
    ];

    for (label, patch) in cases {
        let (dir, target, engine) = workspace();
        let preimage = snapshot_handle(&engine, dir.path());
        let error = engine
            .apply(
                &invocation(dir.path()),
                edit_request(
                    Some(patch.to_string()),
                    Some(PATCHED.as_bytes().to_vec()),
                    Some(preimage),
                ),
            )
            .expect_err("retired patch keys must fail closed");
        assert_eq!(error.kind, EngineErrorKind::InvalidInput, "{label}");
        assert_eq!(error.detail, "patch not supported", "{label}");
        assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL, "{label}");
    }
}

#[test]
fn unique_patch_fails_closed_when_find_is_missing_or_ambiguous() {
    let (dir, target, engine) = workspace();
    let missing = serde_json::json!({"find": "absent", "replacement": "UNIQUE"}).to_string();
    let missing_err = engine
        .apply(
            &invocation(dir.path()),
            edit_request(Some(missing), None, None),
        )
        .expect_err("missing find must fail closed");
    assert_eq!(missing_err.kind, EngineErrorKind::InvalidInput);
    assert!(
        missing_err.detail.contains("unique match"),
        "missing find must name unique match, got {}",
        missing_err.detail
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);

    fs::write(&target, "unique unique\n").unwrap();
    let ambiguous = serde_json::json!({"find": "unique", "replacement": "UNIQUE"}).to_string();
    let ambiguous_err = engine
        .apply(
            &invocation(dir.path()),
            edit_request(Some(ambiguous), None, None),
        )
        .expect_err("ambiguous find must fail closed");
    assert_eq!(ambiguous_err.kind, EngineErrorKind::InvalidInput);
    assert!(
        ambiguous_err.detail.contains("unique match"),
        "ambiguous find must name unique match, got {}",
        ambiguous_err.detail
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "unique unique\n");
}

#[test]
fn unsupported_or_non_edit_patch_is_rejected() {
    let (dir, target, engine) = workspace();
    let bare = engine
        .apply(
            &invocation(dir.path()),
            edit_request(Some("unique|UNIQUE".into()), None, None),
        )
        .expect_err("bare patch string must fail closed");
    assert_eq!(bare.kind, EngineErrorKind::InvalidInput);
    assert_eq!(bare.detail, "patch not supported");
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);

    let write_err = engine
        .apply(
            &invocation(dir.path()),
            FileEffectRequest {
                kind: FileEffectKind::Write,
                path: PathBuf::from("target.txt"),
                content: Some(PATCHED.as_bytes().to_vec()),
                patch: Some(
                    serde_json::json!({"find": "unique", "replacement": "UNIQUE"}).to_string(),
                ),
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .expect_err("Write must not silently ignore patch");
    assert_eq!(write_err.kind, EngineErrorKind::InvalidInput);
    assert_eq!(write_err.detail, "patch not supported");
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);

    let restore_err = engine
        .apply(
            &invocation(dir.path()),
            FileEffectRequest {
                kind: FileEffectKind::Restore,
                path: PathBuf::from("target.txt"),
                content: Some(PATCHED.as_bytes().to_vec()),
                patch: Some(
                    serde_json::json!({"find": "unique", "replacement": "UNIQUE"}).to_string(),
                ),
                expected_preimage: None,
                expect_absent: false,
            },
        )
        .expect_err("Restore must not silently ignore patch");
    assert_eq!(restore_err.kind, EngineErrorKind::InvalidInput);
    assert_eq!(restore_err.detail, "patch not supported");
    assert_eq!(fs::read_to_string(&target).unwrap(), ORIGINAL);
}
