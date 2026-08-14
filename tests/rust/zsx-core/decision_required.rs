#![cfg(feature = "fixture-adapters")]

//! W0 end-to-end: an uncovered semantic decision point aborts the session
//! with the typed `DecisionRequired` failure code instead of silently
//! selecting a branch (ZS-EXEC-003/004/007, V6-C03/H03).

use std::sync::Arc;
use std::time::Duration;

use zsx_core::fixture::fixture_adapters;
use zsx_core::{ZsxSession, ZsxSessionFailureCode};

fn fixture_session() -> (tempfile::TempDir, ZsxSession) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, "decision-required");
    let session = ZsxSession::builder(&root_path)
        .with_session_id("decision-required")
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session");
    (root, session)
}

const POINT: &str = r#"{"decision_id":"dec:1","observation_class":{"class_id":"branch.test_suite"},
    "question":"which test strategy?","alternatives":["run_fast","run_full"],
    "evidence_refs":["fz://blob/evidence"]}"#;

#[test]
fn uncovered_semantic_decision_aborts_with_decision_required_code() {
    let (_root, session) = fixture_session();
    let plan = format!(
        "const point = {POINT}; return await zero.decision.require(point, 'fast');"
    );
    let error = session
        .execute(
            1,
            1,
            plan,
            Duration::from_secs(5),
        )
        .expect_err("uncovered decision must abort");
    assert_eq!(error.code, ZsxSessionFailureCode::DecisionRequired);
    session.shutdown().expect("shutdown");
}

#[test]
fn policy_error_is_a_loud_backend_failure_never_a_selection() {
    let (_root, session) = fixture_session();
    // The policy-less session gate resolves everything to DecisionRequired;
    // with a malformed point the interpreter fails loudly as a data error.
    let error = session
        .execute(
            1,
            1,
            "return await zero.decision.require({not: 'a point'}, 'fast');".to_string(),
            Duration::from_secs(5),
        )
        .expect_err("malformed point must fail");
    assert!(
        matches!(
            error.code,
            ZsxSessionFailureCode::BackendExecution
        ),
        "malformed point maps to backend execution failure, got {:?}",
        error.code
    );
    session.shutdown().expect("shutdown");
}
