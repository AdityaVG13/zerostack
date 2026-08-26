use std::fs;

use tempfile::tempdir;
use zero_abi::{
    CapsuleEventRoots, KernelLedger, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent,
    ZeroKernelOutcome,
};
use zero_store::{EventLog, EventLogError, EventLogRecord, ZeroCas};

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

/// Append a hand-written event log record over a caller-provided event object,
/// simulating legacy or tampered on-disk state that bypasses publish.
fn write_raw_record(
    root: &std::path::Path,
    event_handle: &ZeroHandle,
    model_visible_digest: &str,
) -> std::path::PathBuf {
    let session_digest = blake3::hash(b"session").to_hex().to_string();
    let log_path = root.join("events").join(format!("{session_digest}.jsonl"));
    fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    let record = EventLogRecord {
        event: event_handle.clone(),
        model_visible_digest: model_visible_digest.into(),
        cell_id: "cell".into(),
    };
    let mut line = serde_json::to_vec(&record).unwrap();
    line.push(b'\n');
    fs::write(&log_path, line).unwrap();
    log_path
}

#[test]
fn visible_bytes_are_bound_before_event_is_returned() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let visible = b"model-visible";
    let publication = log.publish(&event(visible), visible).unwrap();
    assert_eq!(
        publication.model_visible_digest,
        blake3::hash(visible).to_hex().as_str()
    );
    let records = log.records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, publication.event);
}

#[test]
fn digest_mismatch_exposes_no_log_record() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    assert!(log.publish(&event(b"expected"), b"different").is_err());
    assert!(log.records("session").unwrap().is_empty());
}

#[test]
fn publish_rejects_event_without_capsule_tuple() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let mut bare = event(b"model-visible");
    bare.capsule = None;
    let error = log.publish(&bare, b"model-visible").unwrap_err();
    assert!(matches!(error, EventLogError::Invalid(_)));
    // Nothing reached the durable log or the CAS.
    assert!(log.records("session").unwrap().is_empty());
}

#[test]
fn capsule_tuple_replays_exactly() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let publication = log
        .publish(&event(b"model-visible"), b"model-visible")
        .unwrap();
    let records = log.records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, publication.event);

    // Replay reconstructs the exact tuple from the stored event object:
    // capsule/provider/cache/speculation/effect/quality/occurrence roots and
    // the capsule object handle all round-trip unchanged.
    let bytes = ZeroCas::open(root.path()).get(&publication.event).unwrap();
    let replayed: ZeroKernelEvent = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(replayed.capsule, Some(capsule_roots()));
}

#[test]
fn tampered_capsule_root_blocks_replay() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let mut tampered = event(b"model-visible");
    tampered.capsule = Some(CapsuleEventRoots {
        capsule_root: "A".repeat(64), // uppercase is not canonical
        ..capsule_roots()
    });
    let bytes = serde_json::to_vec(&tampered).unwrap();
    let handle = ZeroCas::open(root.path()).put(&bytes).unwrap();
    write_raw_record(root.path(), &handle, &tampered.model_visible_digest);

    let error = log.records("session").unwrap_err();
    assert!(matches!(error, EventLogError::Invalid(_)));
}

#[test]
fn tampered_capsule_object_blocks_replay() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let tampered = event(b"model-visible");
    let mut json = serde_json::to_string(&tampered).unwrap();
    let object = format!("z://blob/{}", root64('a'));
    json = json.replace(
        &format!("\"capsuleObject\":\"{object}\""),
        "\"capsuleObject\":\"z://blob/NOTHEX\"",
    );
    let handle = ZeroCas::open(root.path()).put(json.as_bytes()).unwrap();
    write_raw_record(root.path(), &handle, &tampered.model_visible_digest);

    let error = log.records("session").unwrap_err();
    assert!(matches!(error, EventLogError::Invalid(_)));
}

#[test]
fn legacy_record_without_capsule_replays_read_only() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    // A pre-capsule event object carries no tuple at all; serde default
    // reconstructs capsule: None and replay must accept it as-is.
    let mut legacy = event(b"model-visible");
    legacy.capsule = None;
    let bytes = serde_json::to_vec(&legacy).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("capsule"));
    let handle = ZeroCas::open(root.path()).put(&bytes).unwrap();
    let log_path = write_raw_record(root.path(), &handle, &legacy.model_visible_digest);
    let before = fs::read(&log_path).unwrap();

    let records = log.records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, handle);
    let stored: ZeroKernelEvent =
        serde_json::from_slice(&ZeroCas::open(root.path()).get(&handle).unwrap()).unwrap();
    assert_eq!(stored.capsule, None);
    // Replay is read-only: the on-disk log was not rewritten.
    assert_eq!(fs::read(&log_path).unwrap(), before);
}
