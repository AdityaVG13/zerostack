#![cfg(feature = "fixture-adapters")]

//! V6-R2 end-to-end (ZS-ADAPTER-004, ZS-SESSION-001/005): an uncovered
//! decision point emits a `DecisionRequired` envelope carrying the scoped
//! continuation handle; the harness persists the typed continuation record;
//! `resume_continuation_v6` validates the binding, durably consumes the
//! handle (single-use), and re-executes the recorded plan with the model's
//! decision supplied. Unknown, expired, tampered, and replayed handles
//! refuse loudly at the session API; a stored handle resumes across a
//! session restart (kill/restart acceptance).

use std::fs;
use std::time::Duration;

use zero_abi::{
    AuditEventRangeV1, DecisionRequiredV1, ObservationClassV1, ZeroExecuteKindV6,
};
use zsx_core::{
    SessionEnvelopeContextV1, ZsxSession, ZsxSessionFailureCode, fixture::fixture_adapters,
};

fn ledger() -> SessionEnvelopeContextV1 {
    // Synthetic anchor, same convention as result_v6_envelope.rs: the
    // harness supplies a real ledger root in production; the session never
    // fabricates one, and these tests only prove continuation behavior.
    SessionEnvelopeContextV1::new("a".repeat(64), AuditEventRangeV1::new(1, 1).unwrap()).unwrap()
}

fn decision_payload() -> DecisionRequiredV1 {
    DecisionRequiredV1 {
        decision_id: "dec:1".into(),
        observation_class: ObservationClassV1::new("branch.test_suite").unwrap(),
        question: "which test strategy?".into(),
        choices: vec!["run_fast".into(), "run_full".into()],
        observed_value: "fast".into(),
    }
}

const POINT: &str = r#"{"decision_id":"dec:1","observation_class":{"class_id":"branch.test_suite"},
    "question":"which test strategy?","alternatives":["run_fast","run_full"],
    "evidence_refs":["fz://blob/evidence"]}"#;

/// One session over a fresh or restarted root. A restart builds a new
/// session (and a new executor) over the same root; the continuation
/// registry journal under the state root survives it.
fn session_over(root: &std::path::Path) -> ZsxSession {
    let (fs, graph, token) = fixture_adapters(root, "continuation-resume");
    ZsxSession::builder(root)
        .with_session_id("continuation-resume")
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session")
}

/// Persist the envelope's decision point and return the scoped handle.
fn persist_after_decision(
    session: &ZsxSession,
    plan: &str,
    ttl: Duration,
) -> String {
    let result = session
        .execute_v6(1, 1, plan.to_owned(), Duration::from_secs(5), ledger())
        .expect("decision boundary returns a V6 result, not a bare error");
    let envelope = result
        .envelope
        .expect("an uncovered decision point must emit a V6 envelope");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::DecisionRequired);
    let handle = envelope
        .continuation_handle()
        .expect("scoped continuation handle")
        .to_owned();
    assert_eq!(handle, "zsx://g1-r1/dec:1");
    let receipt = session
        .persist_continuation(1, 1, &decision_payload(), plan.to_owned(), ttl)
        .expect("persist the typed continuation record");
    assert_eq!(receipt.continuation_handle, handle);
    handle
}

/// ZS-ADAPTER-004: persist -> resume round trip. The resumed plan continues
/// past the decision point with the supplied choice and completes.
#[test]
fn persist_then_resume_round_trip_completes_with_supplied_decision() {
    let (_root, session) = {
        let root = tempfile::tempdir().expect("root");
        let root_path = root.path().canonicalize().expect("canonical root");
        let session = session_over(&root_path);
        (root, session)
    };
    let plan = format!(
        "const point = {POINT}; const choice = await zero.decision.require(point, 'fast'); \
         return 'chose:' + choice;"
    );
    let handle = persist_after_decision(&session, &plan, Duration::from_secs(3600));

    let resumed = session
        .resume_continuation_v6(
            1,
            2,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect("resume completes with the supplied decision");
    assert_eq!(
        resumed.value,
        Some(serde_json::json!("chose:run_fast")),
        "the resumed plan continues with the model's choice"
    );
    assert!(
        resumed.envelope.is_none(),
        "a completed plain resume has no provable V6 kind at the session boundary"
    );
    assert!(resumed.error.is_none());

    // The handle is single-use: a replayed resume refuses loudly even though
    // the execution identity is fresh.
    let error = session
        .resume_continuation_v6(
            1,
            3,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("replayed resume of a consumed handle must refuse");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("already consumed"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("shutdown");
}

/// An unknown handle refuses loudly at the session API.
#[test]
fn unknown_handle_is_refused_loudly() {
    let (_root, session) = fixture_session();
    let error = session
        .resume_continuation_v6(
            1,
            1,
            "zsx://g1-r1/dec:nope",
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("unknown handle must refuse");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("unknown continuation handle"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("shutdown");
}

/// An expired handle refuses loudly at the session API.
#[test]
fn expired_handle_is_refused_loudly() {
    let (_root, session) = fixture_session();
    let plan = format!(
        "const point = {POINT}; return await zero.decision.require(point, 'fast');"
    );
    let handle = persist_after_decision(&session, &plan, Duration::ZERO);
    let error = session
        .resume_continuation_v6(
            1,
            2,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("expired handle must refuse");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("expired"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("shutdown");
}

/// A handle persisted in a revoked epoch (the session was replaced) refuses
/// loudly at the session API.
#[test]
fn revoked_epoch_handle_is_refused_after_replacement() {
    let (_root, session) = fixture_session();
    let plan = format!(
        "const point = {POINT}; return await zero.decision.require(point, 'fast');"
    );
    let handle = persist_after_decision(&session, &plan, Duration::from_secs(3600));
    session
        .replace(1, zsx_core::SessionReplacementReason::Manual)
        .expect("replacement advances the epoch");
    let error = session
        .resume_continuation_v6(
            2,
            2,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("handle of the revoked epoch must refuse");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("revoked"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("shutdown");
}

/// Kill/restart acceptance: a stored handle resumes after a session restart
/// (new session, new executor, same state root) without retransmitting
/// evidence -- the registry journal replays from disk.
#[test]
fn stored_handle_resumes_after_session_restart() {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let plan = format!(
        "const point = {POINT}; const choice = await zero.decision.require(point, 'fast'); \
         return 'chose:' + choice;"
    );
    let handle = {
        let session = session_over(&root_path);
        let handle = persist_after_decision(&session, &plan, Duration::from_secs(3600));
        session.shutdown().expect("first session shuts down");
        handle
    };
    let session = session_over(&root_path);
    let resumed = session
        .resume_continuation_v6(
            1,
            2,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect("restarted session resumes the stored handle");
    assert_eq!(resumed.value, Some(serde_json::json!("chose:run_fast")));
    session.shutdown().expect("second session shuts down");
}

/// A tampered journaled record refuses loudly at the session API after the
/// restart that replays it.
#[test]
fn tampered_record_is_refused_after_restart() {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let plan = format!(
        "const point = {POINT}; return await zero.decision.require(point, 'fast');"
    );
    let handle = {
        let session = session_over(&root_path);
        let handle = persist_after_decision(&session, &plan, Duration::from_secs(3600));
        session.shutdown().expect("first session shuts down");
        handle
    };
    // Flip one byte of the plan source inside the journal frame, keeping the
    // frame and the JSON parseable so only the record digest can catch it.
    let wal = root_path.join("continuations.snapshot.wal");
    let bytes = fs::read(&wal).expect("read continuation journal");
    let needle = b"decision.require";
    let position = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("plan source is journaled");
    let mut tampered = bytes.clone();
    tampered[position] = b'X';
    fs::write(&wal, &tampered).expect("rewrite continuation journal");

    let session = session_over(&root_path);
    let error = session
        .resume_continuation_v6(
            1,
            2,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("tampered record must refuse");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("tampered"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("second session shuts down");
}

fn fixture_session() -> (tempfile::TempDir, ZsxSession) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    (root, session_over(&root_path))
}

/// A persisted handle survives a restart and its consumption is durable:
/// after restart, the consumed tombstone replays and the replayed resume
/// refuses.
#[test]
fn consumption_survives_restart() {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let plan = format!(
        "const point = {POINT}; const choice = await zero.decision.require(point, 'fast'); \
         return 'chose:' + choice;"
    );
    let handle = {
        let session = session_over(&root_path);
        let handle = persist_after_decision(&session, &plan, Duration::from_secs(3600));
        session
            .resume_continuation_v6(
                1,
                2,
                &handle,
                "run_fast",
                Duration::from_secs(5),
                Vec::new(),
                ledger(),
            )
            .expect("resume consumes the handle");
        session.shutdown().expect("first session shuts down");
        handle
    };
    let session = session_over(&root_path);
    let error = session
        .resume_continuation_v6(
            1,
            3,
            &handle,
            "run_fast",
            Duration::from_secs(5),
            Vec::new(),
            ledger(),
        )
        .expect_err("consumed tombstone replays across restart");
    assert_eq!(error.code, ZsxSessionFailureCode::ContinuationRefused);
    assert!(
        error.detail.contains("already consumed"),
        "refusal detail: {}",
        error.detail
    );
    session.shutdown().expect("second session shuts down");
}
