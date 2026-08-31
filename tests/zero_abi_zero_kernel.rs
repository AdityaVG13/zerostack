use std::path::PathBuf;

use serde_json::json;
use zero_abi::{
    AsgrepOptions, CapsuleEventRoots, CapsulePublication, EngineError, EngineErrorKind,
    FileMetadata, KernelBudget, KernelContext, KernelLedger, PARALLEL_TASK_LIMIT, SharedCapability,
    StateEvidence, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent, ZeroKernelOutcome,
    ZeroKernelRequest, ZeroKernelResponse, ZeroOperationStatus, ZeroOperationTrace,
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
        capsule: Some(sample_capsule_roots()),
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
        effects: Vec::new(),
    }
}

#[test]
fn handle_requires_canonical_blake3_shape() {
    let valid = handle('a');
    assert_eq!(valid.as_str(), format!("z://blob/{}", "a".repeat(64)));
    assert_eq!(valid.digest(), "a".repeat(64));

    for invalid in [
        "fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
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
    // Boundary-valid budgets must be accepted
    let mut good = request.clone();
    good.budget.task_limit = PARALLEL_TASK_LIMIT as u32;
    good.validate()
        .expect("maximum task_limit must be accepted");
    // Each malformed component must be rejected independently
    let mut bad_task = request.clone();
    bad_task.budget.task_limit = 0;
    assert!(
        bad_task.validate().is_err(),
        "zero task_limit must be rejected"
    );
    let mut bad_protocol = request.clone();
    bad_protocol.protocol = "wrong.protocol".into();
    assert!(
        bad_protocol.validate().is_err(),
        "wrong protocol must be rejected"
    );
    let mut bad_context = request.clone();
    bad_context.context.session_id = "".into();
    assert!(
        bad_context.validate().is_err(),
        "empty session_id must be rejected"
    );
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
        effects: Vec::new(),
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
fn event_requires_and_validates_capsule_tuple() {
    sample_event().validate().expect("event valid");

    let mut missing = serde_json::to_value(sample_event()).unwrap();
    missing.as_object_mut().unwrap().remove("capsule");
    assert!(serde_json::from_value::<ZeroKernelEvent>(missing).is_err());

    let mut broken = sample_event();
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

    let repeated = completed_response(vec![trace(1, 2, &root('a')), trace(2, 2, &root('a'))]);
    assert!(repeated.validate().is_err());

    let decreasing = completed_response(vec![trace(1, 2, &root('a')), trace(2, 1, &root('a'))]);
    assert!(decreasing.validate().is_err());
}

#[test]
fn file_metadata_timestamp_is_json_safe_and_exact() {
    let modified_unix_ns = 1_780_000_000_000_000_123u128;
    let metadata = FileMetadata {
        mode: 0o644,
        modified_unix_ns,
        symlink_target: None,
        symlink_target_is_dir: false,
    };
    let encoded = serde_json::to_value(&metadata).unwrap();
    assert_eq!(
        encoded["modifiedUnixNs"],
        json!(modified_unix_ns.to_string())
    );
    let decoded: FileMetadata = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.modified_unix_ns, modified_unix_ns);
}

#[test]
fn find_mode_wire_rejects_noncanonical_aliases() {
    for mode in ["defs", "call-path"] {
        let options: AsgrepOptions = serde_json::from_value(json!({"mode": mode})).unwrap();
        assert_eq!(serde_json::to_value(options).unwrap()["mode"], json!(mode));
    }
    for alias in ["definition", "call_path"] {
        assert!(
            serde_json::from_value::<AsgrepOptions>(json!({"mode": alias})).is_err(),
            "accepted retired mode alias: {alias}"
        );
    }
}

#[test]
fn shared_capability_rejects_retired_field_aliases() {
    let canonical = json!({
        "schema": "zeroref-capability",
        "hash": {"algorithm": "sha256"},
        "shared_cas": {
            "layout": "blobs/sha256/<hh>/<hash>",
            "layout_version": 1
        },
        "fragments": {
            "byte": "strict",
            "line_start": "strict",
            "line_end": "clamp_end"
        }
    });
    serde_json::from_value::<SharedCapability>(canonical).expect("canonical capability");

    let retired_hash = json!({
        "schema": "zeroref-capability",
        "hash": {"algo": "sha256"},
        "shared_cas": {
            "layout": "blobs/sha256/<hh>/<hash>",
            "layout_version": 1
        },
        "fragments": {
            "byte": "strict",
            "line_start": "strict",
            "line_end": "clamp_end"
        }
    });
    assert!(serde_json::from_value::<SharedCapability>(retired_hash).is_err());

    let retired_layout = json!({
        "schema": "zeroref-capability",
        "hash": {"algorithm": "sha256"},
        "shared_cas": {
            "layout": "blobs/sha256/<hh>/<hash>",
            "version": 1
        },
        "fragments": {
            "byte": "strict",
            "line_start": "strict",
            "line_end": "clamp_end"
        }
    });
    assert!(serde_json::from_value::<SharedCapability>(retired_layout).is_err());
}
