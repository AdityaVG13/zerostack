    use super::*;

    fn class(class_id: &str) -> ObservationClassV1 {
        ObservationClassV1::new(class_id).expect("valid class fixture")
    }

    fn point(alternatives: &[&str]) -> SemanticDecisionPointV1 {
        SemanticDecisionPointV1::new(
            "dec:1",
            class("branch.test_suite"),
            "which test strategy?",
            alternatives.iter().map(|value| value.to_string()).collect(),
            vec!["fz://blob/evidence".into()],
        )
        .expect("valid point fixture")
    }

    fn rule(
        class_id: &str,
        observed: ObservedMatchV1,
        alternative: &str,
    ) -> ContingentPolicyRuleV1 {
        ContingentPolicyRuleV1::new(class(class_id), observed, alternative)
            .expect("valid rule fixture")
    }

    #[test]
    fn covered_observation_selects_within_offered_alternatives() {
        let policy = ContingentPolicyV1::new(vec![
            rule(
                "branch.test_suite",
                ObservedMatchV1::Exact {
                    value: "fast".into(),
                },
                "run_fast",
            ),
            rule(
                "branch.test_suite",
                ObservedMatchV1::Any,
                "run_full",
            ),
        ])
        .expect("valid policy");

        let point = point(&["run_fast", "run_full", "skip"]);
        // Exact match wins over Any by rule order.
        assert_eq!(
            policy.resolve(&point, "fast"),
            PolicyResolutionV1::Selected {
                alternative: "run_fast".into(),
                rule_index: 0
            }
        );
        // Any matches everything else offered.
        assert_eq!(
            policy.resolve(&point, "slow"),
            PolicyResolutionV1::Selected {
                alternative: "run_full".into(),
                rule_index: 1
            }
        );
        // Different observation class never matches.
        let other_policy = ContingentPolicyV1::new(vec![rule(
            "other.class",
            ObservedMatchV1::Any,
            "run_full",
        )])
        .unwrap();
        assert!(matches!(
            other_policy.resolve(&point, "fast"),
            PolicyResolutionV1::Uncovered { .. }
        ));
    }

    #[test]
    fn uncovered_observation_returns_decision_required_payload() {
        let policy = ContingentPolicyV1::new(vec![rule(
            "branch.test_suite",
            ObservedMatchV1::Exact {
                value: "fast".into(),
            },
            "run_fast",
        )])
        .expect("valid policy");
        let point = point(&["run_fast", "skip"]);

        match policy.resolve(&point, "unexpected") {
            PolicyResolutionV1::Uncovered { decision_required } => {
                assert_eq!(decision_required.decision_id, "dec:1");
                assert_eq!(
                    decision_required.observation_class.class_id,
                    "branch.test_suite"
                );
                assert_eq!(decision_required.question, "which test strategy?");
                assert_eq!(
                    decision_required.choices,
                    vec!["run_fast".to_string(), "skip".to_string()]
                );
                assert_eq!(decision_required.observed_value, "unexpected");
            }
            other => panic!("expected Uncovered, got {other:?}"),
        }

        // An empty policy covers nothing: everything is Uncovered (fail
        // closed, never a silent selection).
        let empty = ContingentPolicyV1::new(vec![]).unwrap();
        assert!(empty.is_empty());
        assert!(matches!(
            empty.resolve(&point, "fast"),
            PolicyResolutionV1::Uncovered { .. }
        ));
    }

    #[test]
    fn rule_selecting_unoffered_alternative_fails_closed() {
        let policy = ContingentPolicyV1::new(vec![rule(
            "branch.test_suite",
            ObservedMatchV1::Any,
            "not_offered",
        )])
        .expect("valid policy");
        let point = point(&["run_fast", "skip"]);

        match policy.resolve(&point, "whatever") {
            PolicyResolutionV1::PolicyError(DecisionErrorV1::AlternativeNotOffered {
                decision_id,
                alternative,
                rule_index,
            }) => {
                assert_eq!(decision_id, "dec:1");
                assert_eq!(alternative, "not_offered");
                assert_eq!(rule_index, 0);
            }
            other => panic!("expected AlternativeNotOffered, got {other:?}"),
        }
        // Nothing may be silently selected.
        assert!(!matches!(policy.resolve(&point, "whatever"), PolicyResolutionV1::Selected { .. }));
    }

    #[test]
    fn observation_class_grammar_rejects_bad_ids() {
        for bad in [
            "", "UPPER", "has space", "has/slash", "has\\backslash", "tab\t", "a".repeat(129).as_str(),
        ] {
            assert!(
                ObservationClassV1::new(bad).is_err(),
                "class {bad:?} must be rejected"
            );
        }
        for good in ["a", "branch.test_suite", "api.breaking-change", "x_1.2-3"] {
            assert!(
                ObservationClassV1::new(good).is_ok(),
                "class {good:?} must be accepted"
            );
        }
    }

    #[test]
    fn decision_point_and_policy_validation_fail_closed() {
        // Empty alternatives rejected (constructed directly; the fixture
        // itself fails closed on this input).
        let empty_alternatives = SemanticDecisionPointV1 {
            decision_id: "dec:1".into(),
            observation_class: class("branch.test_suite"),
            question: "which test strategy?".into(),
            alternatives: vec![],
            evidence_refs: vec![],
        };
        assert!(empty_alternatives.validate().is_err());
        // Duplicate alternatives rejected.
        let mut duplicate = point(&["a", "b"]);
        duplicate.alternatives.push("a".into());
        assert!(duplicate.validate().is_err());
        // Empty decision id rejected.
        let mut no_id = point(&["a"]);
        no_id.decision_id.clear();
        assert!(no_id.validate().is_err());
        // Empty question rejected.
        let mut no_question = point(&["a"]);
        no_question.question.clear();
        assert!(no_question.validate().is_err());
        // Empty exact observed value rejected.
        assert!(ContingentPolicyRuleV1::new(
            class("branch.test_suite"),
            ObservedMatchV1::Exact {
                value: String::new()
            },
            "run_fast",
        )
        .is_err());
        // Empty select_alternative rejected.
        assert!(ContingentPolicyRuleV1::new(
            class("branch.test_suite"),
            ObservedMatchV1::Any,
            "",
        )
        .is_err());
    }

    #[test]
    fn verdict_bridge_never_permits_selection_outside_safe() {
        assert!(verdict_permits_selection(&SafetyVerdictV1::Safe));
        assert!(!verdict_permits_selection(&SafetyVerdictV1::Unknown {
            reasons: vec!["missing".into()]
        }));
        assert!(!verdict_permits_selection(&SafetyVerdictV1::Unsafe {
            reasons: vec!["bad".into()]
        }));
    }
