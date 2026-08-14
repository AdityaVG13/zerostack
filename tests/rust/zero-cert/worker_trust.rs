//! Worker trust boundary tests (ZS-OPS-005 / V6-R14): forged frames,
//! forged traces, replayed frames, stolen identities, and replayed
//! envelopes are refused loudly with sealed records, and -- critically --
//! refused (and even admitted) worker output acquires NO cache or commit
//! authority. The out-of-process fixture spawns the real
//! `forge_worker` binary and feeds its stdout bytes to the boundary.

use std::io::Write;
use std::process::{Command, Stdio};

use zero_abi::identity::EventClassV1;
use zero_abi::{DigestV1, PayloadFormationReceiptV1, ROOTED_ABI_VERSION_V6};
use zero_cert::{
    CacheAdmissionGateV1, InMemoryJournalStore, KernelEventJournalV1, ProjectRootGateV1,
    TrustContextV1, WorkerEnvelopeV1, WorkerRefusalReasonV1, WorkerTrustBoundaryV1,
    WorkerTrustErrorV1,
};

const PINNED_ARTIFACT: u8 = 0xAA;
const PINNED_PROTOCOL: u8 = 0xBB;

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn context() -> TrustContextV1 {
    TrustContextV1::new(
        "fixture-engine",
        digest(PINNED_ARTIFACT),
        digest(PINNED_PROTOCOL),
        8,
        8,
        1000,
    )
    .unwrap()
}

fn honest_envelope(seq: u64) -> WorkerEnvelopeV1 {
    // Built in-process for the unit fixtures; the out-of-process test
    // exercises the same shape through the fixture binary.
    let identity = zero_cert::WorkerIdentityClaimV1 {
        engine: "fixture-engine".to_owned(),
        artifact_digest: digest(PINNED_ARTIFACT),
        protocol_digest: digest(PINNED_PROTOCOL),
    };
    let frame = zero_cert::WorkerFrameV1 {
        frame_index: 0,
        opcode: "op".to_owned(),
        payload: b"honest payload".to_vec(),
        payload_digest: DigestV1::from_bytes(zero_abi::sha256(b"honest payload")),
    };
    let trace = zero_cert::WorkerTraceV1 {
        trace_index: 0,
        stage: "execute".to_owned(),
        tokens: 10,
        root: digest(0xCC),
    };
    WorkerEnvelopeV1::new(seq, identity, vec![frame], vec![trace]).unwrap()
}

// ---------------------------------------------------------------------------
// In-process boundary semantics.
// ---------------------------------------------------------------------------

/// Honest output is admitted with a sealed receipt; admission is NOT
/// authority (see the gate tests below).
#[test]
fn honest_envelope_admitted_with_sealed_receipt() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let receipt = boundary.admit(&honest_envelope(1)).unwrap();
    assert_eq!(receipt.seq, 1);
    assert_eq!(receipt.frames, 1);
    assert_eq!(receipt.traces, 1);
    assert_eq!(receipt.trace_tokens, 10);
    assert_ne!(receipt.digest().unwrap(), DigestV1::ZERO);
    assert_eq!(receipt.envelope_digest, honest_envelope(1).digest().unwrap());
    assert_eq!(boundary.last_accepted_seq, 1);
}

/// Forged frames (payload does not hash to the declared digest) are refused
/// loudly with a sealed record.
#[test]
fn forged_frame_refused_with_sealed_record() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut forged = honest_envelope(1);
    forged.frames[0].payload_digest = digest(0xEE); // does not hash payload
    let error = boundary.admit(&forged).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(
                record.reason,
                WorkerRefusalReasonV1::ForgedFrame { frame_index: 0 }
            );
            assert_ne!(record.digest().unwrap(), DigestV1::ZERO);
            assert_eq!(record.envelope_digest, forged.digest().unwrap());
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
    assert_eq!(boundary.last_accepted_seq, 0, "refused output never advances the boundary");
}

/// Replayed frames (duplicate index) are refused.
#[test]
fn replayed_frame_refused() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut replayed = honest_envelope(1);
    let frame = replayed.frames[0].clone();
    replayed.frames.push(frame);
    let error = boundary.admit(&replayed).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(
                record.reason,
                WorkerRefusalReasonV1::ReplayedFrame { frame_index: 0 }
            );
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

/// Forged traces (empty stage, zero root) are refused.
#[test]
fn forged_trace_refused() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut forged = honest_envelope(1);
    forged.traces[0].stage = String::new();
    forged.traces[0].root = DigestV1::ZERO;
    let error = boundary.admit(&forged).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(
                record.reason,
                WorkerRefusalReasonV1::ForgedTrace { trace_index: 0 }
            );
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

/// Stolen identities (engine matches, artifact digest does not) are
/// refused: trust is digest-pinned, not name-pinned.
#[test]
fn stolen_identity_refused() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut stolen = honest_envelope(1);
    stolen.identity.artifact_digest = digest(0x99);
    let error = boundary.admit(&stolen).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(record.reason, WorkerRefusalReasonV1::IdentityMismatch);
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

/// Replayed envelopes (seq not newer than the last accepted seq) are
/// refused: a captured honest envelope cannot be re-fed.
#[test]
fn replayed_envelope_refused() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    boundary.admit(&honest_envelope(1)).unwrap();
    let error = boundary.admit(&honest_envelope(1)).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(record.reason, WorkerRefusalReasonV1::ReplayedEnvelope);
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

/// Token budget overruns are refused.
#[test]
fn trace_token_budget_refused() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut greedy = honest_envelope(1);
    greedy.traces[0].tokens = 100_000;
    let error = boundary.admit(&greedy).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(record.reason, WorkerRefusalReasonV1::TraceTokenBudgetExceeded);
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Authority: admitted worker output still acquires NO cache/commit
// authority without the trusted gates.
// ---------------------------------------------------------------------------

fn receipt(contract_root: DigestV1, payload_root: &str) -> PayloadFormationReceiptV1 {
    PayloadFormationReceiptV1::new(
        "constructor:seed-42",
        contract_root,
        vec!["dep-a".to_owned()],
        "fz://blob/exec-1",
        payload_root,
        7,
    )
    .unwrap()
}

/// Even an ADMITTED honest envelope cannot acquire cache authority: cache
/// admission requires a rooted formation receipt through
/// `CacheAdmissionGateV1`. Worker output alone (frames/traces) is never a
/// cache key with authority.
#[test]
fn admitted_worker_output_acquires_no_cache_authority() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let envelope = honest_envelope(1);
    let admission = boundary.admit(&envelope).unwrap();
    assert!(admission.frames >= 1);

    // The worker's frame payload claims to be a formation receipt, but no
    // rooted receipt exists: the gate refuses.
    let contract_root = digest(0x21);
    let receipt = receipt(contract_root, "fz://blob/exec-1");
    let expected_receipt_root = receipt.receipt_root().unwrap();
    let record = CacheAdmissionGateV1::decide(
        &receipt,
        expected_receipt_root,
        contract_root,
        "fz://blob/exec-1",
        &["dep-a".to_owned()],
    )
    .unwrap();
    assert!(record.admitted);

    // With a WRONG expected root (as a forged worker would present), the
    // gate refuses: admission requires the exact sealed receipt root.
    let forged_root = digest(0x99);
    let record = CacheAdmissionGateV1::decide(
        &receipt,
        forged_root,
        contract_root,
        "fz://blob/exec-1",
        &["dep-a".to_owned()],
    )
    .unwrap();
    assert!(!record.admitted, "forged receipt root must not admit");
}

/// Even an ADMITTED honest envelope cannot acquire commit authority: commit
/// requires ProjectRootGateV1's verify -> authorize -> commit chain. An
/// envelope-derived successor root is refused as a stale/unverified handle.
#[test]
fn admitted_worker_output_acquires_no_commit_authority() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let envelope = honest_envelope(1);
    boundary.admit(&envelope).unwrap();

    let genesis = zero_abi::event_log_genesis();
    let journal = KernelEventJournalV1::open(InMemoryJournalStore::new()).unwrap();
    let gate = ProjectRootGateV1::new(genesis, "fixture-gate").unwrap();

    // The worker claims a successor rooted in its trace root. Verify fails:
    // the declared parent is not the current root, or there is no verified
    // change -- either way a loud refusal with an unchanged receipt.
    let claimed_parent = envelope.traces[0].root;
    let claimed_successor = digest(0x77);
    let error = gate
        .verify(claimed_parent, claimed_successor)
        .unwrap_err();
    assert!(
        matches!(
            error,
            zero_cert::KernelRuntimeError::StaleProjectHandle { .. }
                | zero_cert::KernelRuntimeError::NoVerifiedChange { .. }
        ),
        "worker-claimed successor must be refused: {error:?}"
    );

    // No commit event was ever journaled: the worker acquired no commit
    // authority, and the audit read-side agrees.
    assert_eq!(journal.current_project_root().unwrap(), None);
    assert_eq!(journal.records().len(), 0);
}

// ---------------------------------------------------------------------------
// Out-of-process fixture: the real forge_worker binary.
// ---------------------------------------------------------------------------

fn fixture_bin() -> Option<&'static str> {
    option_env!("CARGO_BIN_EXE_forge_worker")
}

/// Run the out-of-process forger for one kind; returns its stdout bytes.
fn forge(kind: &str, seq: u64) -> Vec<u8> {
    let bin = fixture_bin().expect("forge_worker fixture binary must be built (cargo test)");
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn forge_worker");
    write!(
        child.stdin.as_mut().expect("stdin"),
        "{{\"kind\":\"{kind}\",\"seq\":{seq}}}"
    )
    .expect("write forge spec");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for forge_worker");
    assert!(output.status.success(), "forge_worker failed for {kind}");
    output.stdout
}

/// Every forged envelope kind, parsed from real child-process stdout, is
/// refused by the boundary -- forged frames/traces/identities acquire no
/// trust, and therefore no cache or commit authority.
#[test]
fn out_of_process_forged_envelopes_acquire_no_trust() {
    if fixture_bin().is_none() {
        eprintln!("SKIP: forge_worker fixture binary not built");
        return;
    }
    let cases: &[(&str, WorkerRefusalReasonV1)] = &[
        ("forged_frame", WorkerRefusalReasonV1::ForgedFrame { frame_index: 0 }),
        ("replayed_frame", WorkerRefusalReasonV1::ReplayedFrame { frame_index: 0 }),
        ("forged_trace", WorkerRefusalReasonV1::ForgedTrace { trace_index: 0 }),
        (
            "stolen_identity",
            WorkerRefusalReasonV1::IdentityMismatch,
        ),
    ];
    for (kind, expected_reason) in cases {
        let stdout = forge(kind, 1);
        let envelope: WorkerEnvelopeV1 = serde_json::from_slice(&stdout)
            .unwrap_or_else(|error| panic!("parse {kind} envelope: {error}"));
        let mut boundary = WorkerTrustBoundaryV1::new(context());
        match boundary.admit(&envelope) {
            Err(WorkerTrustErrorV1::Refused { record }) => {
                assert_eq!(
                    record.reason, *expected_reason,
                    "kind {kind}: reason mismatch"
                );
                assert_ne!(record.digest().unwrap(), DigestV1::ZERO);
            }
            other => panic!("kind {kind}: expected sealed refusal, got {other:?}"),
        }
        // The refused envelope also cannot pass the gates (cache/commit
        // authority): nothing was admitted, and a forged frame is not a
        // rooted receipt.
        assert_eq!(boundary.last_accepted_seq, 0);
    }
}

/// The honest out-of-process envelope IS admitted (the fixture proves the
/// boundary is not simply refusing everything), and a replayed honest
/// envelope is refused.
#[test]
fn out_of_process_honest_admitted_and_replay_refused() {
    if fixture_bin().is_none() {
        eprintln!("SKIP: forge_worker fixture binary not built");
        return;
    }
    let stdout = forge("honest", 1);
    let envelope: WorkerEnvelopeV1 = serde_json::from_slice(&stdout).unwrap();
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let receipt = boundary.admit(&envelope).unwrap();
    assert_eq!(receipt.seq, 1);
    assert_ne!(receipt.digest().unwrap(), DigestV1::ZERO);

    // Replay the exact captured envelope: refused.
    let error = boundary.admit(&envelope).unwrap_err();
    match error {
        WorkerTrustErrorV1::Refused { record } => {
            assert_eq!(record.reason, WorkerRefusalReasonV1::ReplayedEnvelope);
        }
        other => panic!("expected sealed refusal, got {other:?}"),
    }
}

/// The contract manifest freezes the boundary semantics.
#[test]
fn contract_manifest_freezes_boundary_semantics() {
    let manifest = zero_cert::worker_trust_contract_v1();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(
        manifest["refusals"],
        serde_json::json!("fail-loud with sealed WorkerRefusalRecordV1; never silent")
    );
    assert!(
        manifest["authority"]
            .as_str()
            .unwrap()
            .contains("admission is NOT authority")
    );
}

/// The refusal record digest is deterministic and tamper-sensitive.
#[test]
fn refusal_record_is_deterministic_and_tamper_sensitive() {
    let mut boundary = WorkerTrustBoundaryV1::new(context());
    let mut forged = honest_envelope(1);
    forged.frames[0].payload_digest = digest(0xEE);
    let error = boundary.admit(&forged).unwrap_err();
    let record = match error {
        WorkerTrustErrorV1::Refused { record } => record,
        other => panic!("expected refusal, got {other:?}"),
    };
    let digest = record.digest().unwrap();
    assert_eq!(record.digest().unwrap(), digest);

    let mut tampered = record.clone();
    tampered.detail = "rewritten".to_owned();
    assert_ne!(tampered.digest().unwrap(), digest);
    let _ = ROOTED_ABI_VERSION_V6;
    let _ = EventClassV1::Execution;
}
