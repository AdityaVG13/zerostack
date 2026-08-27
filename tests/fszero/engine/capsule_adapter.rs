//! FileEngine capsule adapter: typed put/get publication roundtrip through
//! the ZeroFileEngine FileEngine implementation.
//!
//! put_capsule stores the WorkCapsule in the CAS and returns an exact
//! CapsulePublication; get_capsule re-verifies the object hash and the
//! capsule root before any byte escapes. Wrong expected roots, tampered
//! objects, and malformed publications fail closed.

use std::path::Path;
use std::sync::Arc;

use fszero_kernel::ZeroFileEngine;
use fszero_store::CasStore;
use zero_abi::{
    CancellationProbe, CapsulePublication, CapsuleRoots, CapsuleState, EngineCallContext,
    EngineErrorKind, EngineInvocation, FileEngine, KernelBudget, WorkCapsule, ZeroHandle,
};

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
            session_id: "capsule-adapter".into(),
            cell_id: "cell-1".into(),
            trace_id: "capsule-adapter-cell-1".into(),
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

fn capsule(epoch: u64) -> WorkCapsule {
    let digest = |byte: char| std::iter::repeat_n(byte, 64).collect();
    WorkCapsule {
        version: 1,
        roots: CapsuleRoots {
            project: digest('a'),
            task: digest('b'),
            protected_scope: digest('c'),
            obligations: digest('d'),
            evidence: digest('e'),
            policy: digest('f'),
            execution: digest('1'),
            verifier: digest('2'),
            fallback: digest('3'),
            ledger: digest('4'),
        },
        state: CapsuleState::Draft,
        epoch,
        provider_usage_budget: 10,
        complete_work_budget: 20,
    }
}

fn engine(dir: &tempfile::TempDir) -> ZeroFileEngine {
    ZeroFileEngine::open(
        dir.path(),
        dir.path().join(".zerostack"),
        "capsule-adapter-contract",
    )
    .unwrap()
}

#[test]
fn put_and_get_capsule_roundtrip_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir);
    let invocation = invocation(dir.path());
    let capsule = capsule(1);

    let publication = engine.put_capsule(&invocation, &capsule).unwrap();
    assert!(publication.created);
    assert_eq!(publication.capsule_root, capsule.root().unwrap());
    assert_eq!(publication.object.digest().len(), 64);
    assert!(publication.object.as_str().starts_with("z://blob/"));

    let fetched = engine.get_capsule(&invocation, &publication).unwrap();
    assert_eq!(fetched, capsule);
}

#[test]
fn put_capsule_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir);
    let invocation = invocation(dir.path());
    let capsule = capsule(1);

    let first = engine.put_capsule(&invocation, &capsule).unwrap();
    let second = engine.put_capsule(&invocation, &capsule).unwrap();
    assert!(!second.created, "identical capsule must not be re-created");
    assert_eq!(second.object, first.object);
    assert_eq!(second.capsule_root, first.capsule_root);
}

#[test]
fn get_capsule_refuses_expected_root_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir);
    let invocation = invocation(dir.path());

    let publication = engine.put_capsule(&invocation, &capsule(1)).unwrap();
    let wrong = CapsulePublication {
        capsule_root: capsule(2).root().unwrap(),
        object: publication.object.clone(),
        created: false,
    };
    let error = engine.get_capsule(&invocation, &wrong).unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Corrupt);
    assert!(
        error.detail.contains("capsule root mismatch"),
        "unexpected detail: {}",
        error.detail
    );
}

#[test]
fn get_capsule_refuses_tampered_object() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir);
    let invocation = invocation(dir.path());

    let publication = engine.put_capsule(&invocation, &capsule(1)).unwrap();
    let object = CasStore::at_blobs_root(dir.path().join(".zerostack").join("blobs"))
        .object_path(publication.object.digest())
        .unwrap();
    std::fs::write(&object, b"tampered").unwrap();

    let error = engine.get_capsule(&invocation, &publication).unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::Corrupt);
}

#[test]
fn get_capsule_refuses_malformed_publication() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(&dir);
    let invocation = invocation(dir.path());

    let bad = CapsulePublication {
        capsule_root: "not-a-capsule-root".into(),
        object: ZeroHandle::parse(format!("z://blob/{}", "a".repeat(64))).unwrap(),
        created: false,
    };
    let error = engine.get_capsule(&invocation, &bad).unwrap_err();
    assert_eq!(error.kind, EngineErrorKind::InvalidInput);
}
