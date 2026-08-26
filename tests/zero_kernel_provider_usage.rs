use std::path::Path;

use tempfile::tempdir;
use zero_abi::{
    KernelBudget, PROVIDER_USAGE_SCHEMA, ProviderUsageObservation, UsageAmount, ZeroHandle,
};
use zero_kernel::{HostError, ZeroKernel};
use zero_store::{EventLog, ZeroCas};
use zerostack_test_support::capsule_event;

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
    let expected = observation("req-1");
    let event_handle = EventLog::open(&store_root)
        .publish(&capsule_event(b"model-visible"), b"model-visible")
        .unwrap()
        .event;
    let kernel = kernel(root.path(), &store_root);

    let receipt = kernel
        .record_provider_usage(&event_handle, expected.clone())
        .unwrap();
    assert_eq!(receipt.kernel_event, event_handle);
    assert_eq!(receipt.request_id, "req-1");
    // Exact public outcome: observation is durable in CAS and digest-bound.
    assert_eq!(receipt.observation_digest, receipt.observation.digest());
    let cas = ZeroCas::open(&store_root);
    let stored = cas.get(&receipt.observation).unwrap();
    let decoded: ProviderUsageObservation = serde_json::from_slice(&stored).unwrap();
    assert_eq!(decoded, expected);
    // The kernel's recording landed in the same append-only store sidecar.
    let records = EventLog::open(&store_root)
        .provider_usage_records("session")
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kernel_event, event_handle);
    assert_eq!(records[0].request_id, "req-1");
    assert_eq!(records[0].observation, receipt.observation);
    // Idempotent re-publish returns the same durable publication without duplicating the log.
    let second = kernel
        .record_provider_usage(&event_handle, expected.clone())
        .unwrap();
    assert_eq!(second, receipt);
    let records_after = EventLog::open(&store_root)
        .provider_usage_records("session")
        .unwrap();
    assert_eq!(records_after.len(), 1);
    assert_eq!(records_after[0], records[0]);
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
        .publish(&capsule_event(b"model-visible"), b"model-visible")
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
