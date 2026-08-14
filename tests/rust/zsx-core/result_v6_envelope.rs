#![cfg(feature = "fixture-adapters")]

//! V6-R1 end-to-end: the session execute path emits the kind-tagged
//! `ZeroExecuteResultV6` envelope (ZS-ADAPTER-003, ZS-EXEC-003). An
//! uncovered decision point surfaces as `DecisionRequired` with the typed
//! question/choices/continuation handle, a cancelled request as `Cancelled`,
//! and an approval rejection as `FailedNoAuthority`; a plain successful
//! execution never claims a kind it cannot prove.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zero_abi::{
    AuditEventRangeV1, EffectClass, EngineIdentity, ZeroExecuteKindV6, ZERO_EXECUTE_ABI_VERSION_V6,
};
use zsx_core::{
    SessionApprovalGrantV1, SessionEnvelopeContextV1, ZsxSession, ZsxSessionFailureCode,
    fixture::fixture_adapters, legacy_envelope_value,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn fixture_session() -> (tempfile::TempDir, ZsxSession) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, "result-v6");
    let session = ZsxSession::builder(&root_path)
        .with_session_id("result-v6")
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session");
    (root, session)
}

fn ledger() -> SessionEnvelopeContextV1 {
    // Synthetic anchor: the harness supplies a real ledger root (e.g. from a
    // finalized dominance receipt) in production; the session never
    // fabricates one, and this test only proves envelope emission.
    SessionEnvelopeContextV1::new("a".repeat(64), AuditEventRangeV1::new(1, 1).unwrap()).unwrap()
}

const POINT: &str = r#"{"decision_id":"dec:1","observation_class":{"class_id":"branch.test_suite"},
    "question":"which test strategy?","alternatives":["run_fast","run_full"],
    "evidence_refs":["fz://blob/evidence"]}"#;

#[test]
fn execute_v6_surfaces_decision_required_envelope_end_to_end() {
    let (root, session) = fixture_session();
    let root_path = root.path().canonicalize().unwrap();
    let plan = format!(
        "const point = {POINT}; return await zero.decision.require(point, 'fast');"
    );
    let result = session
        .execute_v6(1, 1, plan, Duration::from_secs(5), ledger())
        .expect("decision boundary returns a V6 result, not a bare error");

    let envelope = result
        .envelope
        .expect("an uncovered decision point must emit a V6 envelope");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::DecisionRequired);
    assert_eq!(envelope.kind().kind_name(), "DecisionRequired");
    assert_eq!(envelope.abi_version(), ZERO_EXECUTE_ABI_VERSION_V6);
    assert_eq!(envelope.question(), Some("which test strategy?"));
    assert_eq!(
        envelope.choices(),
        &[serde_json::json!("run_fast"), serde_json::json!("run_full")]
    );
    assert_eq!(
        envelope.continuation_handle(),
        Some("zsx://g1-r1/dec:1"),
        "the decision id is bound as the scoped continuation handle"
    );
    assert_eq!(
        envelope.project_root(),
        Some(root_path.to_str().expect("root is utf-8")),
        "the envelope carries the authorized session root"
    );
    envelope.validate().expect("emitted envelope validates");
    let round_tripped: zero_abi::ZeroExecuteResultV6 =
        serde_json::from_value(serde_json::to_value(&envelope).unwrap()).unwrap();
    assert_eq!(round_tripped, envelope, "envelope round-trips through JSON");

    // Legacy contract preserved: the typed error is still visible and the
    // legacy conversion keeps the pre-V6 shape and code.
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some(ZsxSessionFailureCode::DecisionRequired)
    );
    assert!(result.value.is_none());
    let legacy = legacy_envelope_value(&envelope, 1, 1);
    assert_eq!(legacy["protocol"], serde_json::json!("zerostack.zsx.v1"));
    assert_eq!(legacy["ok"], serde_json::json!(false));
    assert_eq!(legacy["error"]["code"], serde_json::json!("decision_required"));
    assert_eq!(
        legacy["result"]["question"],
        serde_json::json!("which test strategy?")
    );

    session.shutdown().expect("shutdown");
}

#[test]
fn execute_v6_plain_success_never_claims_a_kind() {
    let (_root, session) = fixture_session();
    let result = session
        .execute_v6(
            1,
            1,
            r#"return await zero.fs.compound('list', {path: '.'});"#,
            Duration::from_secs(5),
            ledger(),
        )
        .expect("plain success settles");

    assert!(
        result.envelope.is_none(),
        "a plain successful execution has no provable V6 kind at the session boundary: \
         no safety verdict, no content roots -- Completed must never be claimed"
    );
    assert!(result.error.is_none());
    let value = result.value.expect("legacy-visible value is preserved");
    assert!(
        value.get("content").is_some() && value.get("ack").is_some(),
        "the normalized public result is preserved for legacy consumers: {value}"
    );
    assert!(result.metrics.is_some(), "legacy metrics are preserved");

    session.shutdown().expect("shutdown");
}

#[test]
fn execute_v6_cancelled_request_yields_cancelled_envelope() {
    let (_root, session) = fixture_session();
    let worker_session = Arc::new(session);
    let slow = {
        let worker_session = Arc::clone(&worker_session);
        std::thread::spawn(move || {
            worker_session.execute_v6(
                1,
                1,
                r#"await zero.fs.compound('search', {query: 'x', __fixture_delay_ms: 900});"#,
                Duration::from_secs(30),
                ledger(),
            )
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        worker_session.cancellation().cancel_request(1, 1),
        "in-flight request must be actively cancelled"
    );

    let result = slow.join().expect("worker thread").expect("cancelled settle");
    let envelope = result
        .envelope
        .expect("a cancelled request must emit a Cancelled envelope");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::Cancelled);
    assert_eq!(envelope.kind().kind_name(), "Cancelled");
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some(ZsxSessionFailureCode::Cancelled),
        "the legacy error keeps the pre-V6 cancellation code"
    );
    assert!(result.value.is_none());
    let legacy = legacy_envelope_value(&envelope, 1, 1);
    assert_eq!(legacy["error"]["code"], serde_json::json!("cancelled"));

    worker_session.shutdown().expect("shutdown");
}

#[test]
fn execute_with_approvals_v6_rejection_yields_failed_no_authority_envelope() {
    let (root, session) = fixture_session();
    let root_path = root.path().canonicalize().unwrap();
    let now = now_ms();
    let expired = SessionApprovalGrantV1 {
        schema: "zerostack.session.approval_grant.v1".into(),
        grant_id: "grant-1".into(),
        engine: EngineIdentity::FsZero,
        root: root_path.to_str().expect("root is utf-8").into(),
        generation: 1,
        request_id: 1,
        operation: "fs.write".into(),
        effect: EffectClass::ApprovalRequiredMutation,
        authority_digest: "a".repeat(64),
        policy_digest: "b".repeat(64),
        issued_at_unix_ms: now.saturating_sub(2),
        expires_at_unix_ms: now.saturating_sub(1),
    };
    let result = session
        .execute_with_approvals_v6(
            1,
            1,
            "return null;".to_string(),
            Duration::from_secs(5),
            vec![expired],
            ledger(),
        )
        .expect("an approval rejection returns the FailedNoAuthority envelope");

    let envelope = result
        .envelope
        .expect("an approval rejection must emit a FailedNoAuthority envelope");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::FailedNoAuthority);
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some(ZsxSessionFailureCode::InvalidApproval),
        "the legacy error keeps the pre-V6 approval code"
    );
    let legacy = legacy_envelope_value(&envelope, 1, 1);
    assert_eq!(legacy["ok"], serde_json::json!(false));
    assert_eq!(
        legacy["error"]["code"],
        serde_json::json!("failed_no_authority")
    );

    session.shutdown().expect("shutdown");
}
