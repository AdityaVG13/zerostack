use std::path::Path;

use tempfile::tempdir;
use zero_abi::{
    CapsuleEventRoots, KernelBudget, KernelLedger, PROVIDER_USAGE_SCHEMA, ProviderUsageObservation,
    UsageAmount, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent, ZeroKernelOutcome,
};
use zero_kernel::{HostError, ZeroKernel};
use zero_store::EventLog;

fn root64(hex: char) -> String {
    std::iter::repeat_n(hex, 64).collect()
}

fn capsule_roots() -> CapsuleEventRoots {
    CapsuleEventRoots {
        capsule_root: root64('1'),
        capsule_object: ZeroHandle::from_digest(&root64('a')).unwrap(),
        provider_root: root64('2'),
        cache_root: root64('3'),
        speculation_root: root64('4'),
        effect_root: root64('5'),
        quality_root: root64('6'),
        occurrence_root: root64('7'),
    }
}

fn event(visible: &[u8]) -> ZeroKernelEvent {
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
        model_visible_digest: blake3::hash(visible).to_hex().to_string(),
        turn: None,
        capsule: Some(capsule_roots()),
    }
}

fn observation(request_id: &str) -> ProviderUsageObservation {
    ProviderUsageObservation {
        schema: PROVIDER_USAGE_SCHEMA.into(),
        provider: "test-provider".into(),
        model: Some("test-model".into()),
        request_id: request_id.into(),
        route: None,
        service_tier: None,
        uncached_input_tokens: UsageAmount::measured(10, "test"),
        cached_read_input_tokens: UsageAmount::measured(20, "test"),
        cached_write_input_tokens: UsageAmount::measured(5, "test"),
        reasoning_tokens: UsageAmount::measured(3, "test"),
        output_tokens: UsageAmount::measured(40, "test"),
        billed_tokens: UsageAmount::measured(78, "test"),
        billed_microcredits: UsageAmount::measured(250, "test"),
        credit_microcredits: UsageAmount::measured(1, "test"),
    }
}

fn kernel(project_root: &Path, store_root: &Path) -> ZeroKernel {
    ZeroKernel::canonical_with_tokenizer(
        project_root,
        store_root,
        "session",
        KernelBudget {
            wall_ms: 1_000,
            cpu_ms: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        None,
    )
    .unwrap()
}

#[test]
fn record_provider_usage_delegates_to_event_log() {
    let root = tempdir().unwrap();
    let store_root = root.path().join(".zerostack");
    let event_handle = EventLog::open(&store_root)
        .publish(&event(b"model-visible"), b"model-visible")
        .unwrap()
        .event;
    let kernel = kernel(root.path(), &store_root);

    let receipt = kernel
        .record_provider_usage(&event_handle, observation("req-1"))
        .unwrap();
    assert_eq!(receipt.kernel_event, event_handle);
    assert_eq!(receipt.request_id, "req-1");

    // The kernel's recording landed in the same append-only store sidecar.
    let records = EventLog::open(&store_root)
        .provider_usage_records("session")
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kernel_event, event_handle);
    assert_eq!(records[0].request_id, "req-1");
    assert_eq!(records[0].observation, receipt.observation);
}

#[test]
fn record_provider_usage_rejects_unresolvable_event() {
    let root = tempdir().unwrap();
    let store_root = root.path().join(".zerostack");
    let kernel = kernel(root.path(), &store_root);
    let missing = ZeroHandle::from_digest(&"0".repeat(64)).unwrap();
    let error = kernel
        .record_provider_usage(&missing, observation("req-1"))
        .unwrap_err();
    assert!(matches!(error, HostError::Event(_)));
    assert!(
        EventLog::open(&store_root)
            .provider_usage_records("session")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn record_provider_usage_rejects_invalid_observation() {
    let root = tempdir().unwrap();
    let store_root = root.path().join(".zerostack");
    let event_handle = EventLog::open(&store_root)
        .publish(&event(b"model-visible"), b"model-visible")
        .unwrap()
        .event;
    let kernel = kernel(root.path(), &store_root);
    let mut invalid = observation("req-1");
    invalid.billed_tokens = UsageAmount::measured(5, "");
    let error = kernel
        .record_provider_usage(&event_handle, invalid)
        .unwrap_err();
    assert!(matches!(error, HostError::Event(_)));
    assert!(
        EventLog::open(&store_root)
            .provider_usage_records("session")
            .unwrap()
            .is_empty()
    );
}
