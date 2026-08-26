use std::path::PathBuf;

use serde_json::json;
use zero_abi::{
    CapsuleEventRoots, CapsulePublication, EngineError, EngineErrorKind, KernelBudget,
    KernelContext, KernelLedger, StateEvidence, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent,
    ZeroKernelOutcome, ZeroKernelRequest, ZeroKernelResponse, ZeroOperationStatus,
    ZeroOperationTrace,
};

fn budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 1_000,
        cpu_ms: 500,
        memory_bytes: 64 * 1024 * 1024,
        call_limit: 32,
        task_limit: 8,
        output_byte_limit: 64 * 1024,
    }
}

fn context() -> KernelContext {
    KernelContext {
        workspace_root: PathBuf::from("/workspace"),
        project_root: PathBuf::from("/workspace/project"),
        session_id: "session".into(),
        expected_state_root: None,
        contract_digest: "contract".into(),
    }
}

fn root(fill: char) -> String {
    fill.to_string().repeat(64)
}

fn handle(fill: char) -> ZeroHandle {
    ZeroHandle::from_digest(&fill.to_string().repeat(64)).unwrap()
}

fn sample_publication() -> CapsulePublication {
    CapsulePublication {
        capsule_root: root('a'),
        object: handle('b'),
        created: true,
    }
}

fn sample_capsule_roots() -> CapsuleEventRoots {
    CapsuleEventRoots {
        capsule_root: root('a'),
        capsule_object: handle('b'),
        provider_root: root('c'),
        cache_root: root('d'),
        speculation_root: root('e'),
        effect_root: root('f'),
        quality_root: root('1'),
        occurrence_root: root('2'),
    }
}

fn sample_event() -> ZeroKernelEvent {
    ZeroKernelEvent {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        session_id: "session".into(),
        cell_id: "cell".into(),
        source_digest: "source".into(),
        contract_digest: "contract".into(),
        policy_digest: "policy".into(),
        state_root_before: None,
        state_root_after: None,
        input_handles: vec![],
        output_handles: vec![],
        outcome: ZeroKernelOutcome::Completed,
        ledger: KernelLedger::default(),
        model_visible_digest: "visible".into(),
        turn: None,
        capsule: None,
    }
}

fn trace(sequence: u64, occurrence: u64, capsule_root: &str) -> ZeroOperationTrace {
    ZeroOperationTrace {
        sequence,
        method: "read".into(),
        status: ZeroOperationStatus::Completed,
        capsule_root: capsule_root.into(),
        occurrence,
        parallel_group: None,
        target: None,
        detail: None,
        result_count: None,
        changed_files: None,
        duration_ns: 1,
    }
}

fn completed_response(operations: Vec<ZeroOperationTrace>) -> ZeroKernelResponse {
    ZeroKernelResponse {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        outcome: ZeroKernelOutcome::Completed,
        value: Some(json!({"ok": true})),
        error: None,
        operations,
        operations_truncated: false,
        handles: vec![],
        event: handle('e'),
        state: StateEvidence {
            before: None,
            after: None,
            unchanged: true,
        },
        ledger: KernelLedger::default(),
        turn: None,
    }
}

#[test]
fn handle_requires_canonical_blake3_shape() {
    let valid = handle('a');
    assert_eq!(valid.as_str(), format!("z://blob/{}", "a".repeat(64)));
    assert_eq!(valid.digest(), "a".repeat(64));

    for invalid in [
        "tz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "z://blob/ABCDEF",
        "z://blob/short",
    ] {
        assert!(ZeroHandle::parse(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn request_is_versionless_finite_and_rooted() {
    let request = ZeroKernelRequest::new("return await z.read('a');".into(), context(), budget())
        .expect("request");
    assert_eq!(request.protocol, ZERO_KERNEL_PROTOCOL);
    request.validate().expect("valid request");

    let mut invalid = request.clone();
    invalid.budget.task_limit = 17;
    assert!(invalid.validate().is_err());
}

#[test]
fn failed_response_must_carry_error_and_unchanged_state() {
    let response = ZeroKernelResponse {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        outcome: ZeroKernelOutcome::Failed,
        value: None,
        error: Some(EngineError::new(EngineErrorKind::Io, "failed", false)),
        operations: vec![],
        operations_truncated: false,
        handles: vec![],
        event: handle('b'),
        state: StateEvidence {
            before: Some("root".into()),
            after: Some("root".into()),
            unchanged: true,
        },
        ledger: KernelLedger::default(),
        turn: None,
    };
    response.validate().expect("valid failure");

    let mut dirty = response.clone();
    dirty.state.after = Some("other".into());
    dirty.state.unchanged = false;
    assert!(dirty.validate().is_err());

    let mut missing = response;
    missing.error = None;
    assert!(missing.validate().is_err());
}

#[test]
fn completed_response_rejects_error() {
    let mut response = completed_response(vec![]);
    response.validate().expect("valid completion");

    response.error = Some(EngineError::new(EngineErrorKind::Io, "late", false));
    assert!(response.validate().is_err());
}

#[test]
fn publication_requires_canonical_root_and_object() {
    sample_publication().validate().expect("valid publication");

    let mut bad_root = sample_publication();
    bad_root.capsule_root = "not-a-root".into();
    assert!(bad_root.validate().is_err());

    // ZeroHandle is validated on construction, but serde bypasses that:
    // validation must still reject a non-canonical handle from the wire.
    let mut bad_object = sample_publication();
    bad_object.object = serde_json::from_value(json!("z://blob/ABCDEF")).unwrap();
    assert!(bad_object.validate().is_err());

    let mut bad_object = sample_publication();
    bad_object.object = serde_json::from_value(json!("z://blob/not-a-real-digest")).unwrap();
    assert!(bad_object.validate().is_err());
}

#[test]
fn capsule_event_roots_validate_every_coordinate() {
    sample_capsule_roots().validate().expect("valid tuple");

    let mut bad_root = sample_capsule_roots();
    bad_root.capsule_root = "z".repeat(64);
    assert!(bad_root.validate().is_err());

    let mut bad_root = sample_capsule_roots();
    bad_root.provider_root = "ABCDEF".into();
    assert!(bad_root.validate().is_err());

    let mut bad_root = sample_capsule_roots();
    bad_root.occurrence_root = "x".repeat(63);
    assert!(bad_root.validate().is_err());

    let mut bad_object = sample_capsule_roots();
    bad_object.capsule_object = serde_json::from_value(json!("z://blob/deadbeef")).unwrap();
    assert!(bad_object.validate().is_err());
}

#[test]
fn event_validates_optional_capsule_tuple() {
    // Legacy events without capsule roots still validate and replay.
    sample_event().validate().expect("legacy event valid");

    let mut with_capsule = sample_event();
    with_capsule.capsule = Some(sample_capsule_roots());
    with_capsule.validate().expect("new event valid");

    let mut broken = sample_event();
    broken.capsule = Some(sample_capsule_roots());
    broken.capsule.as_mut().unwrap().occurrence_root = "x".repeat(63);
    assert!(broken.validate().is_err());
}

#[test]
fn trace_requires_canonical_capsule_root_and_positive_occurrence() {
    let response = completed_response(vec![trace(1, 1, &root('a'))]);
    response.validate().expect("valid trace");

    let mut bad_root = response.clone();
    bad_root.operations = vec![trace(1, 1, "not-a-root")];
    assert!(bad_root.validate().is_err());

    let mut bad_root = response.clone();
    bad_root.operations = vec![trace(1, 1, &"a".repeat(63))];
    assert!(bad_root.validate().is_err());

    let mut zero_occurrence = response;
    zero_occurrence.operations = vec![trace(1, 0, &root('a'))];
    assert!(zero_occurrence.validate().is_err());
}

#[test]
fn response_requires_strictly_increasing_occurrence() {
    let ok = completed_response(vec![trace(1, 1, &root('a')), trace(2, 2, &root('a'))]);
    ok.validate().expect("increasing occurrence");

    let mut repeated = completed_response(vec![trace(1, 2, &root('a')), trace(2, 2, &root('a'))]);
    assert!(repeated.validate().is_err());

    let mut decreasing = completed_response(vec![trace(1, 2, &root('a')), trace(2, 1, &root('a'))]);
    assert!(decreasing.validate().is_err());
}
