//! Zerokernel request/response fail-closed envelope.

use serde_json::json;
use zero_abi::{
    DecisionRequired, ExactHandles, FiniteBudget, KernelResourceLedger, MAX_WALL_MS,
    ObservationClass, PreflightReport, ReturnKind, ReturnPolicy, RootBindings, RootEvidence,
    RootSnapshot, ZEROKERNEL_ABI_VERSION, ZerokernelExecuteRequest, ZerokernelExecuteResponse,
};

fn sample_budget() -> FiniteBudget {
    FiniteBudget::new(5000, 5000, 64 * 1024 * 1024, 32).unwrap()
}
fn sample_policy() -> ReturnPolicy {
    ReturnPolicy::new(ReturnKind::Inline, 512).unwrap()
}
fn sample_roots() -> RootBindings {
    RootBindings::new(None, "proj_abc123".into(), None, Some("cap_root_123".into()), None)
        .unwrap()
}
fn sample_snapshot() -> RootSnapshot {
    RootSnapshot {
        workspace_root: None,
        project_root: "proj_abc123".into(),
        session_root: Some("sess_1".into()),
    }
}
fn sample_handles() -> ExactHandles {
    ExactHandles {
        session_handle: Some("sess_1".into()),
        continuation_handle: None,
    }
}
fn sample_preflight() -> PreflightReport {
    PreflightReport {
        ok: true,
        checked_roots: vec!["proj_abc123".into()],
        warnings: vec![],
        errors: vec![],
    }
}
fn sample_ledger() -> KernelResourceLedger {
    KernelResourceLedger {
        wall_ms_used: 10,
        cpu_ms_used: 5,
        calls_made: 1,
        bytes_out: 100,
    }
}
fn sample_evidence_unchanged() -> RootEvidence {
    let snap = sample_snapshot();
    RootEvidence {
        before: snap.clone(),
        after: snap,
        unchanged: true,
        successor_root: None,
    }
}
fn sample_evidence_changed() -> RootEvidence {
    RootEvidence {
        before: RootSnapshot {
            workspace_root: None,
            project_root: "proj_abc123".into(),
            session_root: Some("sess_1".into()),
        },
        after: RootSnapshot {
            workspace_root: None,
            project_root: "proj_abc123".into(),
            session_root: Some("sess_2".into()),
        },
        unchanged: false,
        successor_root: Some("succ_1".into()),
    }
}

#[test]
fn round_trip_request_canonical() {
    let req = ZerokernelExecuteRequest::new(
        "return 42;".into(),
        Some("sess_1".into()),
        sample_budget(),
        sample_policy(),
        sample_roots(),
    )
    .unwrap();
    let bytes = req.canonical_bytes();
    let back = ZerokernelExecuteRequest::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(req, back);
    let embedded = ZerokernelExecuteRequest::new(
        "return 42;".into(),
        Some("sess_1".into()),
        sample_budget(),
        sample_policy(),
        sample_roots(),
    )
    .unwrap();
    let oneshot = ZerokernelExecuteRequest::new(
        "return 42;".into(),
        Some("sess_1".into()),
        sample_budget(),
        sample_policy(),
        sample_roots(),
    )
    .unwrap();
    assert_eq!(embedded.canonical_json(), oneshot.canonical_json());
}

#[test]
fn round_trip_response_completed_and_decision() {
    let completed = ZerokernelExecuteResponse::completed(
        sample_handles(),
        sample_preflight(),
        sample_ledger(),
        sample_evidence_changed(),
        json!({"ok":true,"value":42}),
    )
    .unwrap();
    let bytes = completed.canonical_bytes();
    assert_eq!(
        completed,
        ZerokernelExecuteResponse::from_canonical_bytes(&bytes).unwrap()
    );

    let obs = ObservationClass::new("test.class").unwrap();
    let decision = DecisionRequired {
        decision_id: "d1".into(),
        observation_class: obs,
        question: "choose?".into(),
        choices: vec!["a".into(), "b".into()],
        observed_value: "c".into(),
    };
    let dr = ZerokernelExecuteResponse::decision_required(
        sample_handles(),
        sample_preflight(),
        sample_ledger(),
        sample_evidence_unchanged(),
        decision,
    )
    .unwrap();
    let bytes2 = dr.canonical_bytes();
    assert_eq!(
        dr,
        ZerokernelExecuteResponse::from_canonical_bytes(&bytes2).unwrap()
    );
}

#[test]
fn round_trip_failed_proves_unchanged() {
    let failed = ZerokernelExecuteResponse::failed(
        sample_handles(),
        sample_preflight(),
        sample_ledger(),
        sample_evidence_unchanged(),
    )
    .unwrap();
    let bytes = failed.canonical_bytes();
    let back = ZerokernelExecuteResponse::from_canonical_bytes(&bytes).unwrap();
    assert!(back.root_evidence.unchanged);
    assert!(back.root_evidence.successor_root.is_none());
    assert_eq!(failed, back);
}

#[test]
fn unknown_fields_fail_closed() {
    let req = ZerokernelExecuteRequest::new(
        "p".into(),
        None,
        sample_budget(),
        sample_policy(),
        sample_roots(),
    )
    .unwrap();
    let mut v = serde_json::to_value(req).unwrap();
    v["daemon"] = json!(true);
    assert!(serde_json::from_value::<ZerokernelExecuteRequest>(v).is_err());

    let v2 = json!({"abi_version": ZEROKERNEL_ABI_VERSION, "kind":"Failed", "handles":{}, "preflight":{"ok":true,"checked_roots":[],"warnings":[],"errors":[]}, "ledger":{"wall_ms_used":1,"cpu_ms_used":1,"calls_made":1,"bytes_out":0}, "root_evidence":{"before":{"project_root":"p"},"after":{"project_root":"p"},"unchanged":true}, "unknown":1});
    assert!(serde_json::from_value::<ZerokernelExecuteResponse>(v2).is_err());
}

#[test]
fn zero_and_unbounded_budgets_fail() {
    assert!(FiniteBudget::new(0, 100, 1024, 1).is_err());
    assert!(FiniteBudget::new(100, 0, 1024, 1).is_err());
    assert!(FiniteBudget::new(1, 1, 0, 1).is_err());
    assert!(FiniteBudget::new(1, 1, 1024, 0).is_err());
    assert!(FiniteBudget::new(MAX_WALL_MS + 1, 100, 1024, 1).is_err());
    let v = json!({"wall_ms":"unbounded","cpu_ms":100,"memory_bytes":1024,"max_calls":1});
    assert!(serde_json::from_value::<FiniteBudget>(v).is_err());
    let v2 = json!({"wall_ms":0,"cpu_ms":100,"memory_bytes":1024,"max_calls":1});
    let b: Result<FiniteBudget, _> = serde_json::from_value(v2);
    assert!(b.is_ok());
    assert!(b.unwrap().validate().is_err());
}

#[test]
fn mutation_effect_daemon_fields_rejected() {
    for field in [
        "mutation",
        "effect",
        "daemon",
        "pool",
        "background_pool",
        "write_authority",
    ] {
        let v = json!({
            "abi_version": ZEROKERNEL_ABI_VERSION,
            "program": "return 1",
            "budget": {"wall_ms":100,"cpu_ms":100,"memory_bytes":1024,"max_calls":1},
            "return_policy": {"kind":"inline","max_preview_chars":100},
            "roots": {"project_root":"p"},
            field: true
        });
        assert!(
            serde_json::from_value::<ZerokernelExecuteRequest>(v).is_err(),
            "field {field} should be rejected"
        );
    }
}

#[test]
fn invalid_root_combinations_fail() {
    let roots = RootBindings::new(None, "p".into(), None, None, Some("sess_root".into())).unwrap();
    let req = ZerokernelExecuteRequest::new(
        "prog".into(),
        None,
        sample_budget(),
        sample_policy(),
        roots,
    );
    assert!(req.is_err());
}

#[test]
fn failure_with_successor_root_rejected() {
    let mut evidence = sample_evidence_unchanged();
    evidence.successor_root = Some("succ".into());
    let res = ZerokernelExecuteResponse::failed(
        sample_handles(),
        sample_preflight(),
        sample_ledger(),
        evidence,
    );
    assert!(res.is_err());
    let mut evidence2 = sample_evidence_changed();
    evidence2.unchanged = false;
    let res2 = ZerokernelExecuteResponse::failed(
        sample_handles(),
        sample_preflight(),
        sample_ledger(),
        evidence2,
    );
    assert!(res2.is_err());
}

#[test]
fn wrong_abi_version_rejected() {
    let mut req = ZerokernelExecuteRequest::new(
        "p".into(),
        None,
        sample_budget(),
        sample_policy(),
        sample_roots(),
    )
    .unwrap();
    req.abi_version = "wrong/v1".into();
    assert!(req.validate().is_err());
}
