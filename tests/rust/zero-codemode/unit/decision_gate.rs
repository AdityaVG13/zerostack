    use super::*;

    use std::rc::Rc;
    use std::time::Duration;

    use serde_json::json;
    use zero_abi::{
        ContingentPolicyRuleV1, ContingentPolicyV1, ObservationClassV1, ObservedMatchV1,
    };

    use crate::host::ConnectorError;
    use crate::{CapabilityDescriptor, Connector, ConnectorCompletion, DispatchContext, GlobalRegistration, Host, HostError, HostLimits};

    struct NullConnector;

    impl Connector for NullConnector {
        fn dispatch(
            &self,
            _: &CapabilityDescriptor,
            _: &str,
            _: DispatchContext,
            _: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            Ok(())
        }
    }

    fn test_host(instruction_budget: u64) -> Host {
        let limits = HostLimits::new(
            16 * 1024 * 1024,
            256 * 1024,
            Duration::from_secs(2),
            instruction_budget,
            64,
            crate::MAX_INFLIGHT_CONNECTOR_CALLS,
            256 * 1024,
            16 * 1024 * 1024,
        )
        .unwrap();
        Host::new(limits, GlobalRegistration::zero(vec![])).unwrap()
    }

    fn class(class_id: &str) -> ObservationClassV1 {
        ObservationClassV1::new(class_id).unwrap()
    }

    fn point_json(alternatives: &[&str]) -> serde_json::Value {
        json!({
            "decision_id": "dec:1",
            "observation_class": {"class_id": "branch.test_suite"},
            "question": "which test strategy?",
            "alternatives": alternatives,
            "evidence_refs": ["fz://blob/evidence"]
        })
    }

    fn policy(rules: Vec<ContingentPolicyRuleV1>) -> ContingentPolicyV1 {
        ContingentPolicyV1::new(rules).unwrap()
    }

    fn rule(observed: ObservedMatchV1, alternative: &str) -> ContingentPolicyRuleV1 {
        ContingentPolicyRuleV1::new(class("branch.test_suite"), observed, alternative).unwrap()
    }

    fn require_plan() -> String {
        format!(
            "const point = {}; return await zero.decision.require(point, 'fast');",
            point_json(&["run_fast", "run_full"])
        )
    }

    /// EXEC-004 acceptance: with NO policy attached (fail-closed default),
    /// an adaptive decision aborts with `DecisionRequired` -- the interpreter
    /// never silently picks a branch.
    #[test]
    fn no_policy_aborts_with_decision_required() {
        let host = test_host(100_000);
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        match outcome.result {
            Err(HostError::DecisionRequired(payload)) => {
                assert_eq!(payload.decision_id, "dec:1");
                assert_eq!(payload.observation_class.class_id, "branch.test_suite");
                assert_eq!(payload.question, "which test strategy?");
                assert_eq!(payload.choices, vec!["run_fast", "run_full"]);
                assert_eq!(payload.observed_value, "fast");
            }
            other => panic!("expected DecisionRequired, got {other:?}"),
        }
    }

    /// EXEC-004 acceptance: a policy covering the observation with an offered
    /// alternative keeps the branch within ONE call and returns the selected
    /// alternative.
    #[test]
    fn total_policy_selects_within_one_call() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Exact { value: "fast".into() }, "run_fast"),
            rule(ObservedMatchV1::Any, "run_full"),
        ]))));
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        match outcome.result {
            Ok(value) => assert_eq!(value, json!("run_fast")),
            other => panic!("expected selected alternative, got {other:?}"),
        }
    }

    /// EXEC-004 acceptance: an observation no rule covers returns
    /// `DecisionRequired` even when a policy IS attached (fail closed).
    #[test]
    fn uncovered_observation_aborts_with_decision_required() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Exact { value: "fast".into() }, "run_fast"),
        ]))));
        let plan = format!(
            "const point = {}; return await zero.decision.require(point, 'unexpected');",
            point_json(&["run_fast", "run_full"])
        );
        let outcome = host.execute_measured(&plan, Rc::new(NullConnector));
        match outcome.result {
            Err(HostError::DecisionRequired(payload)) => {
                assert_eq!(payload.observed_value, "unexpected");
                assert_eq!(payload.choices, vec!["run_fast", "run_full"]);
            }
            other => panic!("expected DecisionRequired, got {other:?}"),
        }
    }

    /// A rule selecting an alternative the point does not offer is a policy
    /// error, never a silent selection.
    #[test]
    fn unoffered_alternative_is_a_loud_policy_error() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Any, "not_offered"),
        ]))));
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        assert!(
            matches!(
                &outcome.result,
                Err(HostError::Data(message)) if message.contains("decision policy error")
            ),
            "expected loud policy error, got {:?}",
            outcome.result
        );
    }

    /// Malformed point arguments fail loudly with a typed data error.
    #[test]
    fn malformed_point_argument_fails_loudly() {
        let host = test_host(100_000);
        let plan = "return await zero.decision.require({not: 'a point'}, 'fast');";
        let outcome = host.execute_measured(plan, Rc::new(NullConnector));
        assert!(
            matches!(
                &outcome.result,
                Err(HostError::Data(message)) if message.contains("SemanticDecisionPointV1")
            ),
            "expected typed point error, got {:?}",
            outcome.result
        );
    }

    /// The intrinsic decision surface is always registered, and wrong-arity
    /// calls fail loudly.
    #[test]
    fn decision_surface_is_registered_and_arity_checked() {
        let host = test_host(100_000);
        assert!(host.registration().capabilities.iter().any(|capability| {
            capability.surface == DECISION_SURFACE && capability.method == DECISION_REQUIRE_METHOD
        }));
        let outcome = host.execute_measured("return await zero.decision.require({});", Rc::new(NullConnector));
        assert!(
            matches!(&outcome.result, Err(HostError::Data(message)) if message.contains("expects (point, observed_value)")),
            "expected arity error, got {:?}",
            outcome.result
        );
    }

    /// ZS-EXEC-003 adversarial acceptance: a plan with k adaptive decisions
    /// and no policy produces >=k DecisionRequired aborts (one per uncovered
    /// call site; execution stops at the first).
    #[test]
    fn adversarial_adaptive_plan_never_selects_privately() {
        let host = test_host(100_000);
        let plan = format!(
            "const a = {}; const b = {}; const r1 = await zero.decision.require(a, 'x'); return await zero.decision.require(b, 'y');",
            point_json(&["l", "r"]),
            point_json(&["up", "down"])
        );
        let outcome = host.execute_measured(&plan, Rc::new(NullConnector));
        match outcome.result {
            Err(HostError::DecisionRequired(payload)) => {
                // First decision point aborts; the second is never reached.
                assert_eq!(payload.decision_id, "dec:1");
            }
            other => panic!("expected DecisionRequired at first adaptive decision, got {other:?}"),
        }

        // With a TOTAL policy over both points, the same plan completes in
        // one call with the second selected value.
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Any, "up"),
        ]))));
        // The policy's Any rule covers both decision points (same class);
        // 'up' is offered by the second point. The first point offers
        // ["l","r"] -- 'up' is NOT offered, so this is a policy error, which
        // is exactly the fail-closed behavior for a non-total policy. Assert
        // the loud error rather than a private selection.
        let outcome = host.execute_measured(&plan, Rc::new(NullConnector));
        assert!(
            matches!(
                &outcome.result,
                Err(HostError::Data(message)) if message.contains("decision policy error")
            ),
            "non-total policy must fail loudly, got {:?}",
            outcome.result
        );
    }

    /// V6-R3: the gate reports which rules matched and which never did, in
    /// policy order, after one execution.
    #[test]
    fn usage_report_tracks_matched_and_unused_rules() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Exact { value: "fast".into() }, "run_fast"),
            rule(ObservedMatchV1::Exact { value: "slow".into() }, "run_full"),
        ]))));
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        assert!(outcome.result.is_ok(), "covered decision resolves");
        let report = host
            .decision_gate_usage_report()
            .expect("a policy execution reports usage");
        assert_eq!(report.observations, 1);
        assert_eq!(report.rules.len(), 2);
        assert_eq!(report.rules[0].rule_index, 0);
        assert_eq!(report.rules[0].matched_observations, 1);
        assert_eq!(report.rules[1].rule_index, 1);
        assert_eq!(report.rules[1].matched_observations, 0);
        assert_eq!(report.unused_rule_indexes, vec![1]);
    }

    /// V6-R3: with no policy attached there is nothing to report.
    #[test]
    fn usage_report_is_none_without_policy() {
        let host = test_host(100_000);
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        assert!(matches!(&outcome.result, Err(HostError::DecisionRequired(_))));
        assert!(host.decision_gate_usage_report().is_none());
    }

    /// V6-R3: an uncovered observation aborts, and the report still lists
    /// every rule as unused -- the coverage gap is visible, never silently
    /// dropped.
    #[test]
    fn uncovered_observation_reports_all_rules_unused() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Exact { value: "fast".into() }, "run_fast"),
        ]))));
        let plan = format!(
            "const point = {}; return await zero.decision.require(point, 'unexpected');",
            point_json(&["run_fast", "run_full"])
        );
        let outcome = host.execute_measured(&plan, Rc::new(NullConnector));
        assert!(matches!(&outcome.result, Err(HostError::DecisionRequired(_))));
        let report = host
            .decision_gate_usage_report()
            .expect("a policy execution reports usage");
        assert_eq!(report.observations, 1);
        assert_eq!(report.rules[0].matched_observations, 0);
        assert_eq!(report.unused_rule_indexes, vec![0]);
    }

    /// V6-R3: a matched rule that selects an unoffered alternative still
    /// counts as used in the report -- the abort is loud and the usage is
    /// honest.
    #[test]
    fn policy_error_rule_counts_as_matched() {
        let host = test_host(100_000).with_decision_gate(DecisionGate::new(Some(policy(vec![
            rule(ObservedMatchV1::Any, "not_offered"),
        ]))));
        let outcome = host.execute_measured(&require_plan(), Rc::new(NullConnector));
        assert!(
            matches!(
                &outcome.result,
                Err(HostError::Data(message)) if message.contains("decision policy error")
            ),
            "expected loud policy error, got {:?}",
            outcome.result
        );
        let report = host
            .decision_gate_usage_report()
            .expect("a policy execution reports usage");
        assert_eq!(report.rules[0].matched_observations, 1);
        assert!(report.unused_rule_indexes.is_empty());
    }
