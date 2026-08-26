use std::fs;

use tempfile::tempdir;
use zero_abi::{PROVIDER_USAGE_SCHEMA, ProviderUsageObservation, UsageAmount, ZeroHandle};
use zero_store::{EventLog, EventLogError, ProviderUsageLogRecord, ZeroCas};
use zerostack_test_support::capsule_event;

fn observation(request_id: &str, billed_tokens: u64) -> ProviderUsageObservation {
    ProviderUsageObservation {
        schema: PROVIDER_USAGE_SCHEMA.into(),
        provider: "test-provider".into(),
        model: Some("test-model".into()),
        request_id: request_id.into(),
        route: Some("test-route".into()),
        service_tier: Some("test-tier".into()),
        uncached_input_tokens: UsageAmount::measured(10, "test"),
        cached_read_input_tokens: UsageAmount::measured(20, "test"),
        cached_write_input_tokens: UsageAmount::measured(5, "test"),
        reasoning_tokens: UsageAmount::measured(3, "test"),
        output_tokens: UsageAmount::measured(40, "test"),
        billed_tokens: UsageAmount::measured(billed_tokens, "test"),
        billed_microcredits: UsageAmount::measured(250, "test"),
        credit_microcredits: UsageAmount::measured(1, "test"),
    }
}

fn publish(log: &EventLog, visible: &[u8]) -> ZeroHandle {
    log.publish(&capsule_event(visible), visible).unwrap().event
}

#[test]
fn provider_usage_persists_and_replays() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let observation = observation("req-1", 100);
    let publication = log
        .publish_provider_usage("session", &event_handle, observation)
        .unwrap();
    assert_eq!(publication.kernel_event, event_handle);
    assert_eq!(publication.request_id, "req-1");

    // A fresh EventLog over the same store replays and fully resolves the
    // record: linked event and observation objects must both validate.
    let replayed = EventLog::open(root.path());
    let records = replayed.provider_usage_records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kernel_event, event_handle);
    assert_eq!(records[0].request_id, "req-1");
    assert_eq!(records[0].observation, publication.observation);
    assert_eq!(
        records[0].observation.digest(),
        publication.observation_digest
    );
}

#[test]
fn provider_usage_observation_digest_matches_stored_bytes() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let observation = observation("req-1", 100);
    let publication = log
        .publish_provider_usage("session", &event_handle, observation.clone())
        .unwrap();
    let observation_bytes = serde_json::to_vec(&observation).unwrap();
    assert_eq!(
        publication.observation_digest,
        blake3::hash(&observation_bytes).to_hex().as_str()
    );
    let records = log.provider_usage_records("session").unwrap();
    assert_eq!(
        records[0].observation.digest(),
        publication.observation_digest
    );
}

#[test]
fn provider_usage_publish_is_idempotent_for_same_observation() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let observation = observation("req-1", 100);
    let first = log
        .publish_provider_usage("session", &event_handle, observation.clone())
        .unwrap();
    let second = log
        .publish_provider_usage("session", &event_handle, observation)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(log.provider_usage_records("session").unwrap().len(), 1);
}

#[test]
fn provider_usage_distinct_requests_on_one_event_append() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    log.publish_provider_usage("session", &event_handle, observation("req-1", 100))
        .unwrap();
    log.publish_provider_usage("session", &event_handle, observation("req-2", 200))
        .unwrap();
    let records = log.provider_usage_records("session").unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].request_id, "req-1");
    assert_eq!(records[1].request_id, "req-2");
}

#[test]
fn provider_usage_conflicts_on_different_observation() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    log.publish_provider_usage("session", &event_handle, observation("req-1", 100))
        .unwrap();
    let error = log
        .publish_provider_usage("session", &event_handle, observation("req-1", 999))
        .unwrap_err();
    assert!(matches!(error, EventLogError::UsageConflict(_)));
    // The conflicting observation never reached the log.
    assert_eq!(log.provider_usage_records("session").unwrap().len(), 1);
}

#[test]
fn provider_usage_sidecar_links_to_capsule_rooted_event() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let publication = log
        .publish_provider_usage("session", &event_handle, observation("req-1", 100))
        .unwrap();

    // Public replay must durably link the usage record to the accepted kernel event.
    // Exercise this through the supported EventLog replay APIs, not by re-reading
    // setup data directly from ZeroCas.
    let fresh = EventLog::open(root.path());
    let event_records = fresh.records("session").unwrap();
    assert_eq!(event_records.len(), 1);
    assert_eq!(event_records[0].event, event_handle);
    assert_eq!(event_records[0].event, publication.kernel_event);

    let usage_records = fresh.provider_usage_records("session").unwrap();
    assert_eq!(usage_records.len(), 1);
    assert_eq!(usage_records[0].kernel_event, event_handle);
    assert_eq!(usage_records[0].kernel_event, event_records[0].event);
    assert_eq!(usage_records[0].request_id, "req-1");
    assert_eq!(usage_records[0].observation, publication.observation);
    // Capsule and session linkage is validated implicitly: provider_usage_records
    // replays and validates the linked event's capsule tuple and session_id, so
    // success proves the sidecar is bound to a capsule-rooted event for "session".
}

#[test]
fn provider_usage_rejects_wrong_session() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let error = log
        .publish_provider_usage("other-session", &event_handle, observation("req-1", 100))
        .unwrap_err();
    assert!(matches!(error, EventLogError::UsageInvalid(_)));
    assert!(
        log.provider_usage_records("other-session")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn provider_usage_rejects_unresolvable_event_handle() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let missing = ZeroHandle::from_digest(&"0".repeat(64)).unwrap();
    let err = log
        .publish_provider_usage("session", &missing, observation("req-1", 100))
        .unwrap_err();
    assert!(matches!(err, EventLogError::Cas(_)));
    assert!(log.provider_usage_records("session").unwrap().is_empty());
}

#[test]
fn provider_usage_rejects_event_missing_from_durable_log() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_bytes = serde_json::to_vec(&capsule_event(b"model-visible")).unwrap();
    let cas_only_event = ZeroCas::open(root.path()).put(&event_bytes).unwrap();

    let error = log
        .publish_provider_usage("session", &cas_only_event, observation("req-1", 100))
        .unwrap_err();
    assert!(matches!(error, EventLogError::UsageInvalid(_)));
    assert!(log.provider_usage_records("session").unwrap().is_empty());
}

#[test]
fn provider_usage_rejects_invalid_observation() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    let mut invalid = observation("req-1", 100);
    invalid.uncached_input_tokens = UsageAmount::measured(5, "");
    let error = log
        .publish_provider_usage("session", &event_handle, invalid)
        .unwrap_err();
    assert!(matches!(error, EventLogError::UsageInvalid(_)));
    assert!(log.provider_usage_records("session").unwrap().is_empty());
}

#[test]
fn provider_usage_replay_rejects_tampered_record() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let event_handle = publish(&log, b"model-visible");
    log.publish_provider_usage("session", &event_handle, observation("req-1", 100))
        .unwrap();

    // Rewrite the usage log record so its request_id no longer matches the
    // observation object it references; replay must reject the tampered log.
    let session_digest = blake3::hash(b"session").to_hex().to_string();
    let log_path = root
        .path()
        .join("usage")
        .join(format!("{session_digest}.jsonl"));
    let mut line: ProviderUsageLogRecord =
        serde_json::from_str(fs::read_to_string(&log_path).unwrap().trim()).unwrap();
    line.request_id = "tampered".into();
    fs::write(&log_path, serde_json::to_string(&line).unwrap()).unwrap();

    let error = log.provider_usage_records("session").unwrap_err();
    assert!(matches!(error, EventLogError::UsageInvalid(_)));
}
