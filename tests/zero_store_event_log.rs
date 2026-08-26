use std::fs;

use tempfile::tempdir;
use zero_abi::{CapsuleEventRoots, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelEvent};
use zero_store::{EventLog, EventLogError, EventLogRecord, ZeroCas};
use zerostack_test_support::{capsule_event, capsule_roots};

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
    let publication = log.publish(&capsule_event(visible), visible).unwrap();
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
    assert!(
        log.publish(&capsule_event(b"expected"), b"different")
            .is_err()
    );
    assert!(log.records("session").unwrap().is_empty());
}

#[test]
fn publish_rejects_event_without_capsule_tuple() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let mut bare = capsule_event(b"model-visible");
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
        .publish(&capsule_event(b"model-visible"), b"model-visible")
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
    let mut tampered = capsule_event(b"model-visible");
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
    let tampered = capsule_event(b"model-visible");
    let json = serde_json::to_string(&tampered).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["capsule"]["capsuleObject"] = serde_json::Value::String("z://blob/NOTHEX".into());
    let tampered_json = serde_json::to_string(&value).unwrap();
    let handle = ZeroCas::open(root.path())
        .put(tampered_json.as_bytes())
        .unwrap();
    write_raw_record(root.path(), &handle, &tampered.model_visible_digest);

    let error = log.records("session").unwrap_err();
    assert!(matches!(error, EventLogError::Invalid(_)));
}

#[test]
fn legacy_record_without_capsule_replays_read_only() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    // Fixed legacy fixture that genuinely predates capsule fields: hand-written
    // JSON with no "capsule" member, independent of the current serializer.
    let visible = b"model-visible";
    let digest = blake3::hash(visible).to_hex().to_string();
    let legacy_value = serde_json::json!({
        "protocol": ZERO_KERNEL_PROTOCOL,
        "sessionId": "session",
        "cellId": "cell",
        "sourceDigest": "source",
        "contractDigest": "contract",
        "policyDigest": "policy",
        "inputHandles": [],
        "outputHandles": [],
        "outcome": "Completed",
        "ledger": {
            "wallNs": 0,
            "cpuNsUpperBound": 0,
            "calls": 0,
            "tasks": 0,
            "bytesRead": 0,
            "bytesWritten": 0,
            "bytesVisible": 0
        },
        "modelVisibleDigest": digest.clone()
    });
    assert!(!legacy_value.to_string().contains("capsule"));
    let bytes = serde_json::to_vec(&legacy_value).unwrap();
    let handle = ZeroCas::open(root.path()).put(&bytes).unwrap();
    let log_path = write_raw_record(root.path(), &handle, &digest);
    let before = fs::read(&log_path).unwrap();

    let records = log.records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, handle);
    // Verify via public replay that the event is read-only and capsule-free:
    // a second replay returns the same handle and leaves the log bytes unchanged.
    let second = log.records("session").unwrap();
    assert_eq!(second, records);
    assert_eq!(fs::read(&log_path).unwrap(), before);
    // Directly decoding the stored bytes confirms capsule absence without relying
    // on string-shape checks.
    let stored: serde_json::Value =
        serde_json::from_slice(&ZeroCas::open(root.path()).get(&handle).unwrap()).unwrap();
    assert!(
        stored.get("capsule").is_none(),
        "legacy fixture must omit capsule"
    );
}
