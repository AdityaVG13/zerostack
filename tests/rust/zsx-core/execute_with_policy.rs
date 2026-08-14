#![cfg(feature = "fixture-adapters")]

//! V6-R3 end-to-end (ZS-EXEC-004/007): a typed `ContingentPolicyV1` rides an
//! ORDINARY execute request into the host decision gate. Covered decision
//! points resolve within one call; uncovered ones still abort with the
//! typed `DecisionRequired` (never a private selection); invalid policies
//! are refused before any execution begins; rules that never matched are
//! reported honestly in the result (unused-rule report), never silently
//! dropped; and the gate is restored after settle so a later plain execute
//! is unaffected.

use std::time::Duration;

use zero_abi::{
    AuditEventRangeV1, ContingentPolicyRuleV1, ContingentPolicyV1, ObservationClassV1,
    ObservedMatchV1, ZeroExecuteKindV6,
};
use zsx_core::{
    GateRuleUsageV1, SessionEnvelopeContextV1, ZsxSession, ZsxSessionFailureCode,
    fixture::fixture_adapters,
};

fn ledger() -> SessionEnvelopeContextV1 {
    // Synthetic anchor, same convention as continuation_resume.rs: the
    // harness supplies a real ledger root in production; the session never
    // fabricates one, and these tests only prove policy-gate behavior.
    SessionEnvelopeContextV1::new("a".repeat(64), AuditEventRangeV1::new(1, 1).unwrap()).unwrap()
}

fn fixture_session() -> (tempfile::TempDir, ZsxSession) {
    let root = tempfile::tempdir().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    let (fs, graph, token) = fixture_adapters(&root_path, "execute-with-policy");
    let session = ZsxSession::builder(&root_path)
        .with_session_id("execute-with-policy")
        .fszero(fs.clone())
        .graphzero(graph.clone())
        .tokenzero(token.clone())
        .build()
        .expect("session");
    (root, session)
}

fn class(class_id: &str) -> ObservationClassV1 {
    ObservationClassV1::new(class_id).unwrap()
}

fn rule(observed: ObservedMatchV1, alternative: &str) -> ContingentPolicyRuleV1 {
    ContingentPolicyRuleV1::new(class("branch.test_suite"), observed, alternative).unwrap()
}

fn policy(rules: Vec<ContingentPolicyRuleV1>) -> ContingentPolicyV1 {
    ContingentPolicyV1::new(rules).unwrap()
}

const POINT: &str = r#"{"decision_id":"dec:1","observation_class":{"class_id":"branch.test_suite"},
    "question":"which test strategy?","alternatives":["run_fast","run_full"],
    "evidence_refs":["fz://blob/evidence"]}"#;

fn choose_plan() -> String {
    format!(
        "const point = {POINT}; const choice = await zero.decision.require(point, 'fast'); \
         return 'chose:' + choice;"
    )
}

/// ZS-EXEC-004/007: a covered decision point resolves within one call -- no
/// extra roundtrip -- and the result reports which policy rule matched.
#[test]
fn covered_decision_point_resolves_within_one_call() {
    let (_root, session) = fixture_session();
    let result = session
        .execute_with_policy_v6(
            1,
            1,
            choose_plan(),
            &policy(vec![rule(
                ObservedMatchV1::Exact {
                    value: "fast".into(),
                },
                "run_fast",
            )]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("a covered decision resolves, not an error");
    assert_eq!(result.value, Some(serde_json::json!("chose:run_fast")));
    assert!(result.error.is_none());
    assert!(
        result.envelope.is_none(),
        "plain success has no provable V6 kind at the session boundary"
    );
    let report = result
        .policy_report
        .expect("a policy execution reports usage");
    assert_eq!(report.observations, 1);
    assert_eq!(report.rules.len(), 1);
    assert_eq!(report.rules[0].rule_index, 0);
    assert_eq!(report.rules[0].matched_observations, 1);
    assert!(
        report.unused_rule_indexes.is_empty(),
        "the single rule matched; nothing is unused"
    );
    session.shutdown().expect("shutdown");
}

/// ZS-EXEC-004/007: an observation no policy rule covers still aborts with
/// the typed `DecisionRequired` -- the interpreter never selects privately
/// -- and the coverage gap is reported: the rule never matched.
#[test]
fn uncovered_observation_still_aborts_with_decision_required() {
    let (_root, session) = fixture_session();
    // The policy covers only 'slow'; the plan observes 'fast' -> uncovered.
    let result = session
        .execute_with_policy_v6(
            1,
            1,
            choose_plan(),
            &policy(vec![rule(
                ObservedMatchV1::Exact {
                    value: "slow".into(),
                },
                "run_full",
            )]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("an uncovered abort returns a V6 result, not a bare error");
    assert!(result.value.is_none());
    let error = result
        .error
        .as_ref()
        .expect("uncovered abort keeps the legacy typed error");
    assert_eq!(error.code, ZsxSessionFailureCode::DecisionRequired);
    let envelope = result
        .envelope
        .as_ref()
        .expect("uncovered abort emits the typed DecisionRequired envelope");
    assert_eq!(envelope.kind(), ZeroExecuteKindV6::DecisionRequired);
    let report = result
        .policy_report
        .expect("a policy execution reports usage");
    assert_eq!(report.observations, 1);
    assert_eq!(report.rules[0].matched_observations, 0);
    assert_eq!(
        report.unused_rule_indexes,
        vec![0],
        "the never-matched rule is reported, not silently dropped"
    );
    session.shutdown().expect("shutdown");
}

/// ZS-EXEC-004/007 fail-closed: an invalid policy (empty select_alternative)
/// is refused with `InvalidPolicy` before any execution begins -- the
/// request id is not consumed and the session keeps working.
#[test]
fn invalid_policy_is_refused_before_execution() {
    let (_root, session) = fixture_session();
    // Bypass the validating constructors on purpose: a rule whose selected
    // alternative is empty is invalid, and the policy must never reach the
    // decision gate.
    let invalid = ContingentPolicyV1 {
        rules: vec![ContingentPolicyRuleV1 {
            observation_class: class("branch.test_suite"),
            observed: ObservedMatchV1::Exact {
                value: "fast".into(),
            },
            select_alternative: String::new(),
        }],
    };
    let error = session
        .execute_with_policy_v6(
            1,
            1,
            "return 'ran';".to_string(),
            &invalid,
            Duration::from_secs(5),
            ledger(),
        )
        .expect_err("invalid policy must be refused");
    assert_eq!(error.code, ZsxSessionFailureCode::InvalidPolicy);
    assert!(
        error.detail.contains("invalid contingent policy"),
        "refusal detail: {}",
        error.detail
    );
    // Refusal happened before admission: the same request id executes fine
    // afterwards, proving nothing was consumed and nothing ran.
    let result = session
        .execute_with_policy_v6(
            1,
            1,
            "return 'ran';".to_string(),
            &policy(vec![rule(ObservedMatchV1::Any, "run_fast")]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("the same request id is reusable after an invalid-policy refusal");
    assert_eq!(result.value, Some(serde_json::json!("ran")));
    session.shutdown().expect("shutdown");
}

/// ZS-EXEC-004/007: rules that never matched are reported explicitly, one
/// entry per policy rule with its own count -- nothing is dropped.
#[test]
fn unused_rules_are_reported_not_dropped() {
    let (_root, session) = fixture_session();
    let result = session
        .execute_with_policy_v6(
            1,
            1,
            choose_plan(),
            &policy(vec![
                rule(
                    ObservedMatchV1::Exact {
                        value: "fast".into(),
                    },
                    "run_fast",
                ),
                rule(
                    ObservedMatchV1::Exact {
                        value: "slow".into(),
                    },
                    "run_full",
                ),
                rule(ObservedMatchV1::Any, "run_full"),
            ]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("covered decision resolves");
    let report = result
        .policy_report
        .expect("a policy execution reports usage");
    assert_eq!(report.observations, 1);
    assert_eq!(report.rules.len(), 3);
    let by_index = |index: usize| -> &GateRuleUsageV1 {
        report
            .rules
            .iter()
            .find(|entry| entry.rule_index == index)
            .expect("one entry per policy rule")
    };
    assert_eq!(by_index(0).matched_observations, 1);
    assert_eq!(by_index(1).matched_observations, 0);
    assert_eq!(by_index(2).matched_observations, 0);
    assert_eq!(
        report.unused_rule_indexes,
        vec![1, 2],
        "both never-matched rules are listed"
    );
    session.shutdown().expect("shutdown");
}

/// ZS-EXEC-004/007: the gate is restored after settle -- a later plain
/// execute (no policy) hits the policy-less fail-closed gate again and
/// aborts with `DecisionRequired`, and carries no policy report.
#[test]
fn decision_gate_restored_after_policy_execution() {
    let (_root, session) = fixture_session();
    let plan = format!("const point = {POINT}; return await zero.decision.require(point, 'fast');");
    let first = session
        .execute_with_policy_v6(
            1,
            1,
            plan.clone(),
            &policy(vec![rule(
                ObservedMatchV1::Exact {
                    value: "fast".into(),
                },
                "run_fast",
            )]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("covered decision resolves");
    assert_eq!(first.value, Some(serde_json::json!("run_fast")));
    // Second execution: plain execute_v6 with no policy. The restored
    // policy-less gate must fail the same decision point closed.
    let second = session
        .execute_v6(1, 2, plan, Duration::from_secs(5), ledger())
        .expect("uncovered abort returns a V6 result, not a bare error");
    assert!(second.value.is_none());
    let error = second
        .error
        .as_ref()
        .expect("legacy typed error");
    assert_eq!(error.code, ZsxSessionFailureCode::DecisionRequired);
    assert_eq!(
        second
            .envelope
            .as_ref()
            .expect("typed DecisionRequired envelope")
            .kind(),
        ZeroExecuteKindV6::DecisionRequired
    );
    assert!(
        second.policy_report.is_none(),
        "a plain execute carries no policy report"
    );
    session.shutdown().expect("shutdown");
}

/// ZS-EXEC-004/007: a policy rule selecting an alternative the point does
/// not offer aborts loudly (a backend data error, never a selection), and
/// the matching rule still counts as used -- usage stays honest in failure.
#[test]
fn policy_error_selecting_unoffered_alternative_aborts_loudly() {
    let (_root, session) = fixture_session();
    // The point offers only run_full; the policy selects run_fast.
    let point = r#"{"decision_id":"dec:1","observation_class":{"class_id":"branch.test_suite"},
        "question":"which test strategy?","alternatives":["run_full"],
        "evidence_refs":["fz://blob/evidence"]}"#;
    let plan = format!(
        "const point = {point}; const choice = await zero.decision.require(point, 'fast'); \
         return 'chose:' + choice;"
    );
    let result = session
        .execute_with_policy_v6(
            1,
            1,
            plan,
            &policy(vec![rule(
                ObservedMatchV1::Exact {
                    value: "fast".into(),
                },
                "run_fast",
            )]),
            Duration::from_secs(5),
            ledger(),
        )
        .expect("a policy error returns a V6 result, not a bare error");
    assert!(result.value.is_none());
    assert!(
        result.envelope.is_none(),
        "policy errors have no provable V6 kind"
    );
    let error = result
        .error
        .as_ref()
        .expect("legacy typed error");
    assert!(
        error.detail.contains("decision policy error"),
        "refusal detail: {}",
        error.detail
    );
    let report = result
        .policy_report
        .expect("a policy execution reports usage");
    assert_eq!(
        report.rules[0].matched_observations, 1,
        "the matching rule counts as used even though the policy was defective"
    );
    session.shutdown().expect("shutdown");
}
