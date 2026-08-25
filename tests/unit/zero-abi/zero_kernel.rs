use std::path::PathBuf;

use serde_json::json;
use zero_abi::{
    EngineError, EngineErrorKind, KernelBudget, KernelContext, KernelLedger, StateEvidence,
    ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelOutcome, ZeroKernelRequest, ZeroKernelResponse,
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

fn handle(fill: char) -> ZeroHandle {
    ZeroHandle::from_digest(&fill.to_string().repeat(64)).unwrap()
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
        decision: None,
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
fn completed_response_rejects_error_or_decision() {
    let mut response = ZeroKernelResponse {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        outcome: ZeroKernelOutcome::Completed,
        value: Some(json!({"ok": true})),
        decision: None,
        error: None,
        operations: vec![],
        operations_truncated: false,
        handles: vec![],
        event: handle('c'),
        state: StateEvidence {
            before: None,
            after: None,
            unchanged: true,
        },
        ledger: KernelLedger::default(),
        turn: None,
    };
    response.validate().expect("valid completion");
    response.decision = Some(json!({"choose": true}));
    assert!(response.validate().is_err());
}
