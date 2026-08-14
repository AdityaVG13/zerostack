//! Scrubber tests (ZS-STORE-007 / V6-R9): a scrub pass verifies content
//! digests against CAS records, quarantines corrupt entries fail-loud, and
//! emits a receipt with counts.

use super::*;
use std::fs;
use tempfile::tempdir;

fn setup() -> (tempfile::TempDir, SharedCas) {
    let directory = tempdir().unwrap();
    let cas = SharedCas::open(directory.path().to_path_buf());
    (directory, cas)
}

fn corrupt_blob(cas: &SharedCas, identity: &str) {
    let path = cas.object_path(identity);
    fs::write(&path, b"tampered bytes that do not hash to the recorded identity").unwrap();
}

#[test]
fn scrub_clean_store_reports_all_verified_and_persists_receipt() {
    let (directory, cas) = setup();
    let payloads = [b"alpha".as_slice(), b"beta", b"gamma"];
    let mut identities = Vec::new();
    for payload in payloads {
        identities.push(cas.put(payload).unwrap());
    }

    let receipt = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1::default(),
        "session-alpha",
        "scrub-op-1",
    )
    .unwrap();
    assert_eq!(receipt.objects_scanned, 3);
    assert_eq!(receipt.objects_verified, 3);
    assert_eq!(receipt.objects_corrupt_quarantined, 0);
    assert_eq!(receipt.objects_unavailable, 0);
    assert_eq!(receipt.objects_io_error, 0);
    assert_eq!(receipt.objects_skipped_idle, 0);
    assert!(receipt.findings.is_empty());
    receipt.validate().unwrap();
    assert_ne!(receipt.digest().unwrap(), DigestV1::ZERO);

    // Receipt read-back with canonical validation.
    let read_back = read_scrub_receipt_v1(directory.path(), "session-alpha", "scrub-op-1").unwrap();
    assert_eq!(read_back, receipt);

    // A second pass is also clean (idempotent).
    let again = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1::default(),
        "session-alpha",
        "scrub-op-2",
    )
    .unwrap();
    assert_eq!(again.objects_verified, 3);
}

#[test]
fn scrub_catches_intentionally_corrupted_blob_and_quarantines_fail_loud() {
    let (directory, cas) = setup();
    let good = cas.put(b"good object bytes").unwrap();
    let bad = cas.put(b"doomed object").unwrap();
    corrupt_blob(&cas, &bad);

    let receipt = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1::default(),
        "session-alpha",
        "scrub-op-1",
    )
    .unwrap();
    assert_eq!(receipt.objects_scanned, 2);
    assert_eq!(receipt.objects_verified, 1);
    assert_eq!(receipt.objects_corrupt_quarantined, 1);
    assert_eq!(receipt.findings.len(), 1);
    assert_eq!(receipt.findings[0].identity, bad);
    assert_eq!(receipt.findings[0].kind, ScrubFindingKindV1::CorruptQuarantined);

    // The corrupt body is quarantined, never silently repaired, never deleted.
    // The exact body found at the identity (the tampered bytes) is preserved
    // so a wrong verdict stays recoverable.
    let quarantine_path = directory
        .path()
        .join(crate::gc_lock::GC_DIR)
        .join(crate::cas::CAS_QUARANTINE_DIR)
        .join(&bad);
    assert!(quarantine_path.is_file());
    assert_eq!(
        fs::read(&quarantine_path).unwrap(),
        b"tampered bytes that do not hash to the recorded identity",
        "quarantine preserves the exact corrupt body for recovery"
    );
    // The object tree no longer serves the corrupt identity.
    assert_eq!(cas.get_verified(&bad).unwrap_err().class(), "missing");
    // The good object is untouched and still verifies.
    assert_eq!(cas.get_verified(&good).unwrap(), b"good object bytes");

    // A subsequent pass is clean: the corruption was caught once, loudly.
    let again = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1::default(),
        "session-alpha",
        "scrub-op-2",
    )
    .unwrap();
    assert_eq!(again.objects_corrupt_quarantined, 0);
    assert_eq!(again.objects_scanned, 1);
}

#[test]
fn scrub_idle_filter_skips_fresh_objects_and_bounds_the_pass() {
    let (directory, cas) = setup();
    cas.put(b"fresh object").unwrap();
    cas.put(b"another fresh object").unwrap();

    // Nothing is idle enough, so a background pass scans nothing.
    let receipt = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1 {
            idle_older_than: Some(std::time::Duration::from_secs(3600)),
            ..ScrubConfigV1::default()
        },
        "session-alpha",
        "scrub-op-1",
    )
    .unwrap();
    assert_eq!(receipt.objects_scanned, 0);
    assert_eq!(receipt.objects_skipped_idle, 2);
    assert_eq!(receipt.objects_verified, 0);

    // A bounded pass completes when the bound admits the whole store.
    let bounded = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1 {
            max_objects: Some(2),
            ..ScrubConfigV1::default()
        },
        "session-alpha",
        "scrub-op-2",
    )
    .unwrap();
    assert_eq!(bounded.objects_scanned, 2);

    // A bound below the store size refuses loudly instead of silently
    // scanning a subset.
    let error = run_scrub_v1(
        directory.path(),
        &ScrubConfigV1 {
            max_objects: Some(1),
            ..ScrubConfigV1::default()
        },
        "session-alpha",
        "scrub-op-3",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ScrubErrorV1::EnumerationExceedsBound { max: 1 }
    ));
}

#[test]
fn scrub_receipt_tamper_is_detected_on_read_back() {
    let (directory, cas) = setup();
    cas.put(b"some object").unwrap();
    run_scrub_v1(
        directory.path(),
        &ScrubConfigV1::default(),
        "session-alpha",
        "scrub-op-1",
    )
    .unwrap();

    let path = directory
        .path()
        .join(crate::gc_lock::GC_DIR)
        .join("scrubs")
        .join("session-alpha")
        .join("scrub-op-1.json");

    // Non-canonical encoding: read-back must refuse.
    let mut non_canonical = fs::read(&path).unwrap();
    non_canonical.insert(1, b' ');
    fs::write(&path, &non_canonical).unwrap();
    assert!(
        read_scrub_receipt_v1(directory.path(), "session-alpha", "scrub-op-1").is_err(),
        "a non-canonically encoded receipt must fail loud"
    );

    // Structurally inconsistent counts: read-back must refuse.
    fs::write(&path, &fs::read(&path).unwrap()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["objects_scanned"] = serde_json::json!(0);
    fs::write(&path, canonical_json(&value).into_bytes()).unwrap();
    assert!(
        read_scrub_receipt_v1(directory.path(), "session-alpha", "scrub-op-1").is_err(),
        "a receipt with inconsistent counts must fail loud"
    );
}
