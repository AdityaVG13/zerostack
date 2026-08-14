use super::*;

use std::fs;
use std::path::Path;

use zero_abi::{
    DecisionRequiredV1, DigestV1, ObservationClassV1, ObservedMatchV1, ROOTED_ABI_VERSION_V6,
};

const PROJECT_ROOT: &str = "/repo/session-root";

fn payload(decision_id: &str) -> DecisionRequiredV1 {
    DecisionRequiredV1 {
        decision_id: decision_id.into(),
        observation_class: ObservationClassV1::new("branch.test_suite").unwrap(),
        question: "which test strategy?".into(),
        choices: vec!["run_fast".into(), "run_full".into()],
        observed_value: "fast".into(),
    }
}

fn persist_request(expires_at_unix_ms: u64) -> ContinuationPersistRequestV1 {
    ContinuationPersistRequestV1 {
        generation: 1,
        request_id: 1,
        decision: payload("dec:1"),
        source: "return await zero.decision.require(point, 'fast');".into(),
        project_root: PROJECT_ROOT.into(),
        expires_at_unix_ms,
    }
}

fn now() -> u64 {
    1_700_000_000_000
}

fn open(dir: &Path) -> ContinuationRegistryV1 {
    ContinuationRegistryV1::open(dir).expect("registry opens")
}

/// ZS-ADAPTER-004: persist a typed continuation record, then consume the
/// scoped handle with the model's decision. The returned binding carries
/// the verified record and a one-shot policy selecting the supplied choice.
#[test]
fn persist_then_consume_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    let receipt = registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    assert_eq!(receipt.generation, 1);
    assert_eq!(receipt.request_id, 1);
    assert_eq!(receipt.decision_id, "dec:1");
    assert_eq!(receipt.continuation_handle, "zsx://g1-r1/dec:1");
    assert_eq!(receipt.handle_id_hex.len(), 64);
    assert_eq!(registry.pending_count(), 1);
    assert_eq!(registry.consumed_count(), 0);

    let binding = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect("consume");
    assert_eq!(binding.continuation_handle, "zsx://g1-r1/dec:1");
    assert_eq!(
        binding.record.source,
        "return await zero.decision.require(point, 'fast');"
    );
    assert_eq!(binding.record.decision, payload("dec:1"));
    assert_eq!(binding.record.generation, 1);
    assert_eq!(binding.record.request_id, 1);
    assert_eq!(binding.record.project_root, PROJECT_ROOT);
    assert_eq!(
        binding.record.handle.state(),
        zero_abi::zero_execute::ContinuationStateV1::Bound
    );
    assert_eq!(binding.record.handle.roots().epoch, 1);
    assert_eq!(
        binding.record.handle.roots().project_root,
        DigestV1::from_bytes(sha256(PROJECT_ROOT.as_bytes()))
    );
    binding
        .record
        .handle
        .validate_against(
            ROOTED_ABI_VERSION_V6,
            DigestV1::from_bytes(sha256(PROJECT_ROOT.as_bytes())),
            1,
        )
        .expect("handle validates against the session");
    assert_eq!(binding.policy.rules.len(), 1);
    assert_eq!(binding.policy.rules[0].select_alternative, "run_fast");
    assert_eq!(
        binding.policy.rules[0].observation_class.class_id,
        "branch.test_suite"
    );
    assert_eq!(
        binding.policy.rules[0].observed,
        ObservedMatchV1::Exact { value: "fast".into() }
    );
    assert_eq!(registry.pending_count(), 0);
    assert_eq!(registry.consumed_count(), 1);
}

/// The registry journal is durable: a fresh registry instance replays the
/// pending record and the consumed tombstone from disk.
#[test]
fn replay_rebuilds_pending_and_consumed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wal_path = {
        let mut registry = open(dir.path());
        registry
            .persist(&persist_request(now() + 3600_000))
            .expect("persist");
        registry
            .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
            .expect("consume");
        registry.wal_path()
    };
    assert!(wal_path.exists(), "journal file exists");

    let mut registry = open(dir.path());
    assert_eq!(registry.pending_count(), 0);
    assert_eq!(registry.consumed_count(), 1);
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect_err("consumed handle must refuse after replay");
    assert_eq!(error, ContinuationRegistryErrorV1::AlreadyConsumed);
}

/// An unknown handle refuses loudly and consumes nothing.
#[test]
fn unknown_handle_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    let error = registry
        .consume("zsx://g1-r9/nope", "run_fast", PROJECT_ROOT, 1, now())
        .expect_err("unknown handle must refuse");
    assert_eq!(error, ContinuationRegistryErrorV1::UnknownHandle);
    assert_eq!(registry.pending_count(), 0);
    assert_eq!(registry.consumed_count(), 0);
}

/// A malformed handle string refuses loudly.
#[test]
fn malformed_handle_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    for handle in [
        "garbage",
        "zsx://g1-r9",
        "zsx://gx-r9/dec",
        "zsx://g1-rx/dec",
        "zsx://g1-r9/",
        "http://g1-r1/dec:1",
    ] {
        let error = registry
            .consume(handle, "run_fast", PROJECT_ROOT, 1, now())
            .expect_err("malformed handle must refuse");
        assert!(
            matches!(error, ContinuationRegistryErrorV1::InvalidHandle(_)),
            "handle {handle:?} produced {error:?}"
        );
    }
}

/// A handle bound to a revoked epoch (a different session generation)
/// refuses loudly (ZS-SESSION-005 revocation via epoch).
#[test]
fn revoked_epoch_handle_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 2, now())
        .expect_err("revoked epoch must refuse");
    assert_eq!(
        error,
        ContinuationRegistryErrorV1::RevokedEpoch { expected: 2, actual: 1 }
    );
    assert_eq!(registry.pending_count(), 1, "refusal consumes nothing");
}

/// A handle whose roots belong to a different project scope refuses loudly.
#[test]
fn cross_project_handle_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", "/other/repo", 1, now())
        .expect_err("cross-project handle must refuse");
    assert_eq!(error, ContinuationRegistryErrorV1::CrossProjectScope);
}

/// An expired record refuses loudly with the expiry facts.
#[test]
fn expired_handle_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() - 1000))
        .expect("persist");
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect_err("expired handle must refuse");
    assert_eq!(
        error,
        ContinuationRegistryErrorV1::Expired {
            expires_at_unix_ms: now() - 1000,
            now_unix_ms: now(),
        }
    );
    assert_eq!(registry.pending_count(), 1, "expiry consumes nothing");
}

/// A replayed resume of an already-consumed handle refuses loudly.
#[test]
fn double_resume_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect("first consume");
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect_err("second consume must refuse");
    assert_eq!(error, ContinuationRegistryErrorV1::AlreadyConsumed);
}

/// A decision the recorded choices do not offer refuses loudly; nothing is
/// consumed and no policy is formed.
#[test]
fn unoffered_decision_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_sideways", PROJECT_ROOT, 1, now())
        .expect_err("unoffered decision must refuse");
    assert_eq!(
        error,
        ContinuationRegistryErrorV1::DecisionNotOffered {
            decision: "run_sideways".into()
        }
    );
    assert_eq!(registry.pending_count(), 1, "refusal consumes nothing");
}

/// A tampered journal record (valid JSON, stale record digest) loads, but
/// its consume refuses loudly as tampered; the same protection covers a
/// forged handle id, whose digest is covered by the record digest.
#[test]
fn tampered_record_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wal_path = {
        let mut registry = open(dir.path());
        registry
            .persist(&persist_request(now() + 3600_000))
            .expect("persist");
        registry.wal_path()
    };
    let bytes = fs::read(&wal_path).expect("read journal");
    let needle = b"decision.require";
    let position = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("plan source is journaled");
    let mut tampered = bytes.clone();
    tampered[position] = b'X';
    fs::write(&wal_path, &tampered).expect("rewrite journal");

    let mut registry = open(dir.path());
    assert_eq!(
        registry.pending_count(),
        1,
        "tampered record still occupies its key"
    );
    let error = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect_err("tampered record must refuse");
    assert_eq!(error, ContinuationRegistryErrorV1::TamperedRecord);
    assert_eq!(registry.pending_count(), 1, "refusal consumes nothing");
}

/// A forged handle id inside a record (with a recomputed record digest)
/// refuses loudly as a tampered handle: the self-verifying id check runs at
/// every use, so a handle id that does not recompute from its own fields is
/// rejected before any mutation.
#[test]
fn forged_handle_id_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let mut registry = open(dir.path());
        registry
            .persist(&persist_request(now() + 3600_000))
            .expect("persist");
    }
    // Read the journaled record back through the public API, flip the
    // handle id inside the record, and recompute the record digest -- the
    // maximum a writer with full journal access could do. The handle's own
    // self-verifying id then fails the resume.
    let mut registry = open(dir.path());
    let binding = registry
        .consume("zsx://g1-r1/dec:1", "run_fast", PROJECT_ROOT, 1, now())
        .expect("consume");
    let mut record = binding.record;
    let mut bytes = serde_json::to_vec(&record).expect("serialize record");
    // DigestV1 serializes as a 64-char lowercase hex string.
    let key = b"\"handle_id\":\"";
    let position = bytes
        .windows(key.len())
        .position(|window| window == key)
        .expect("handle id hex string is serialized");
    let hex_byte = bytes[position + key.len()];
    bytes[position + key.len()] = if hex_byte == b'f' { b'0' } else { b'f' };
    record = serde_json::from_slice(&bytes).expect("tampered record still parses");
    record.record_digest = record
        .compute_digest()
        .expect("attacker recomputes the record digest");
    assert!(record.verify().is_err(), "self-verifying id check must fail");
    assert_eq!(
        record.verify().expect_err("verify refuses"),
        ContinuationRegistryErrorV1::TamperedHandle
    );
}

/// Persisting the same execution identity and decision point twice refuses
/// loudly.
#[test]
fn duplicate_persist_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut registry = open(dir.path());
    registry
        .persist(&persist_request(now() + 3600_000))
        .expect("persist");
    let error = registry
        .persist(&persist_request(now() + 3600_000))
        .expect_err("duplicate persist must refuse");
    assert_eq!(error, ContinuationRegistryErrorV1::DuplicatePersist);
}
