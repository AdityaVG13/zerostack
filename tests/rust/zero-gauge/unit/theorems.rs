use std::collections::{BTreeMap, BTreeSet};

use super::*;
use zero_abi::{CoverageGradeV1, ProtectedDimensionV1, ScopeObligationV1};

fn obligation(
    dimension: ProtectedDimensionV1,
    required: bool,
    grade: CoverageGradeV1,
) -> ScopeObligationV1 {
    ScopeObligationV1::new(dimension, required, grade).unwrap()
}

fn port_input<'a>(
    obligations: &'a ProtectedScopeObligationsV1,
    verifier_sound: bool,
    baseline: bool,
) -> PortNonregressionInput<'a> {
    PortNonregressionInput {
        obligations,
        verifier_sound,
        source_baseline_available: baseline,
    }
}

// ---------------------------------------------------------------------------
// Thm 5.1 -- Explanation Evidence Preservation
// ---------------------------------------------------------------------------

#[test]
fn thm51_certifies_known_good_view() {
    let view = CompactExplanationView {
        claims: vec![
            FactualClaim {
                id: "c1".into(),
                support: ClaimSupport::Rooted {
                    artifact_root: "fz://o/1/1".into(),
                },
                omitted_evidence: false,
                expansion_handle: None,
            },
            FactualClaim {
                id: "c2".into(),
                support: ClaimSupport::LabeledInference {
                    label: "inference:author_style".into(),
                },
                omitted_evidence: false,
                expansion_handle: None,
            },
            FactualClaim {
                id: "c3".into(),
                support: ClaimSupport::Rooted {
                    artifact_root: "gz://o/2/3".into(),
                },
                omitted_evidence: true,
                expansion_handle: Some("h1".into()),
            },
        ],
        artifacts: BTreeSet::from(["fz://o/1/1".into(), "gz://o/2/3".into()]),
        expansions: BTreeMap::from([("h1".into(), "gz://o/2/3".into())]),
    };
    let certification = check_explanation_evidence_preservation(&view).unwrap();
    assert_eq!(certification.certified_claims, 3);
    assert_eq!(certification.expandable_omissions, 1);
}

#[test]
fn thm51_refuses_unrooted_factual_claim() {
    // The claim is backed by an artifact root absent from the view: the
    // compact interface dropped the evidence authority.
    let view = CompactExplanationView {
        claims: vec![FactualClaim {
            id: "c1".into(),
            support: ClaimSupport::Rooted {
                artifact_root: "tz://o/9/9".into(),
            },
            omitted_evidence: false,
            expansion_handle: None,
        }],
        artifacts: BTreeSet::from(["fz://o/1/1".into()]),
        expansions: BTreeMap::new(),
    };
    assert_eq!(
        check_explanation_evidence_preservation(&view),
        Err(TheoremViolation::UnrootedClaim {
            id: "c1".into(),
            artifact_root: "tz://o/9/9".into(),
        })
    );
}

#[test]
fn thm51_refuses_omitted_evidence_without_handle() {
    let view = CompactExplanationView {
        claims: vec![FactualClaim {
            id: "c1".into(),
            support: ClaimSupport::Rooted {
                artifact_root: "fz://o/1/1".into(),
            },
            omitted_evidence: true,
            expansion_handle: None,
        }],
        artifacts: BTreeSet::from(["fz://o/1/1".into()]),
        expansions: BTreeMap::new(),
    };
    assert_eq!(
        check_explanation_evidence_preservation(&view),
        Err(TheoremViolation::OmittedEvidenceNotExpandable {
            id: "c1".into()
        })
    );
}

#[test]
fn thm51_refuses_expansion_to_wrong_bound_artifact() {
    // The falsifier of Thm 5.1: a factual claim in the compact view cannot
    // expand to its bound artifact.
    let view = CompactExplanationView {
        claims: vec![FactualClaim {
            id: "c1".into(),
            support: ClaimSupport::Rooted {
                artifact_root: "fz://o/1/1".into(),
            },
            omitted_evidence: true,
            expansion_handle: Some("h1".into()),
        }],
        artifacts: BTreeSet::from(["fz://o/1/1".into(), "tz://o/2/2".into()]),
        expansions: BTreeMap::from([("h1".into(), "tz://o/2/2".into())]),
    };
    assert_eq!(
        check_explanation_evidence_preservation(&view),
        Err(TheoremViolation::ExpansionMismatch {
            id: "c1".into(),
            handle: "h1".into(),
            bound: "tz://o/2/2".into(),
            artifact_root: "fz://o/1/1".into(),
        })
    );
}

#[test]
fn thm51_refuses_unresolvable_handle() {
    let view = CompactExplanationView {
        claims: vec![FactualClaim {
            id: "c1".into(),
            support: ClaimSupport::LabeledInference {
                label: "inference:x".into(),
            },
            omitted_evidence: true,
            expansion_handle: Some("missing".into()),
        }],
        artifacts: BTreeSet::from(["fz://o/1/1".into()]),
        expansions: BTreeMap::new(),
    };
    assert_eq!(
        check_explanation_evidence_preservation(&view),
        Err(TheoremViolation::UnresolvableExpansion {
            id: "c1".into(),
            handle: "missing".into(),
        })
    );
}

#[test]
fn thm51_deterministic() {
    let view = CompactExplanationView {
        claims: vec![FactualClaim {
            id: "c1".into(),
            support: ClaimSupport::Rooted {
                artifact_root: "fz://o/1/1".into(),
            },
            omitted_evidence: true,
            expansion_handle: None,
        }],
        artifacts: BTreeSet::from(["fz://o/1/1".into()]),
        expansions: BTreeMap::new(),
    };
    let first = check_explanation_evidence_preservation(&view);
    let second = check_explanation_evidence_preservation(&view);
    assert_eq!(first, second);
    assert!(first.is_err());
}

// ---------------------------------------------------------------------------
// Thm 6.1 -- Decision-Delimited Refactor (d + 1 calls)
// ---------------------------------------------------------------------------

fn refactor_input(handles: Vec<ContinuationHandle>) -> DecisionDelimitedRefactorInput {
    DecisionDelimitedRefactorInput {
        handles,
        other_operations_privately_composable: true,
        other_operations_verifiable: true,
    }
}

#[test]
fn thm61_certifies_exactly_d_plus_one_calls() {
    let input = refactor_input(vec![
        // A fully specified symbol rename: d = 0.
        ContinuationHandle {
            id: "rename".into(),
            declared_unresolved_decisions: 0,
            observed_zero_execute_calls: 1,
        },
        // One public-API design choice: d = 1.
        ContinuationHandle {
            id: "api-choice".into(),
            declared_unresolved_decisions: 1,
            observed_zero_execute_calls: 2,
        },
        // A complex architecture task: d = 3.
        ContinuationHandle {
            id: "architecture".into(),
            declared_unresolved_decisions: 3,
            observed_zero_execute_calls: 4,
        },
    ]);
    let certification = check_decision_delimited_refactor(&input).unwrap();
    assert_eq!(certification.certified_interactions, 3);
    assert_eq!(
        certification.expected_calls,
        vec![
            ("rename".to_owned(), 1),
            ("api-choice".to_owned(), 2),
            ("architecture".to_owned(), 4),
        ]
    );
}

#[test]
fn thm61_refuses_call_count_mismatch() {
    let input = refactor_input(vec![ContinuationHandle {
        id: "api-choice".into(),
        declared_unresolved_decisions: 1,
        observed_zero_execute_calls: 1,
    }]);
    assert_eq!(
        check_decision_delimited_refactor(&input),
        Err(TheoremViolation::CallCountMismatch {
            id: "api-choice".into(),
            expected: 2,
            actual: 1,
        })
    );
}

#[test]
fn thm61_refuses_unmet_premises() {
    let input = DecisionDelimitedRefactorInput {
        handles: vec![ContinuationHandle {
            id: "h".into(),
            declared_unresolved_decisions: 0,
            observed_zero_execute_calls: 1,
        }],
        other_operations_privately_composable: false,
        other_operations_verifiable: true,
    };
    assert_eq!(
        check_decision_delimited_refactor(&input),
        Err(TheoremViolation::NotPrivatelyComposable)
    );
    let input = DecisionDelimitedRefactorInput {
        handles: vec![ContinuationHandle {
            id: "h".into(),
            declared_unresolved_decisions: 0,
            observed_zero_execute_calls: 1,
        }],
        other_operations_privately_composable: true,
        other_operations_verifiable: false,
    };
    assert_eq!(
        check_decision_delimited_refactor(&input),
        Err(TheoremViolation::NotVerifiable)
    );
}

#[test]
fn thm61_refuses_decision_count_overflow() {
    let input = refactor_input(vec![ContinuationHandle {
        id: "huge".into(),
        declared_unresolved_decisions: u64::MAX,
        observed_zero_execute_calls: 0,
    }]);
    assert_eq!(
        check_decision_delimited_refactor(&input),
        Err(TheoremViolation::DecisionCountOverflow { id: "huge".into() })
    );
}

#[test]
fn thm61_deterministic() {
    let input = refactor_input(vec![ContinuationHandle {
        id: "rename".into(),
        declared_unresolved_decisions: 0,
        observed_zero_execute_calls: 1,
    }]);
    let first = check_decision_delimited_refactor(&input);
    let second = check_decision_delimited_refactor(&input);
    assert_eq!(first, second);
    assert!(first.is_ok());
}

// ---------------------------------------------------------------------------
// Thm 7.1 -- Port Nonregression under Complete Observational Coverage
// ---------------------------------------------------------------------------

#[test]
fn thm71_certifies_complete_observational_coverage() {
    let obligations = ProtectedScopeObligationsV1::new(vec![
        obligation(ProtectedDimensionV1::Tests, true, CoverageGradeV1::Proved),
        obligation(ProtectedDimensionV1::Behavior, true, CoverageGradeV1::BoundedComplete),
        obligation(
            ProtectedDimensionV1::Performance,
            false,
            CoverageGradeV1::Observed,
        ),
    ])
    .unwrap();
    let certification =
        check_port_nonregression_coverage(&port_input(&obligations, true, true)).unwrap();
    assert_eq!(certification.declared_obligations, 3);
    assert_eq!(certification.verified_obligations, 3);
    assert_eq!(certification.dimensions.len(), 3);
}

#[test]
fn thm71_refuses_incomplete_coverage() {
    // V != B: an uncovered obligation stays Unknown and no equivalence claim
    // may be published.
    let obligations = ProtectedScopeObligationsV1::new(vec![
        obligation(ProtectedDimensionV1::Tests, true, CoverageGradeV1::Proved),
        obligation(ProtectedDimensionV1::Security, true, CoverageGradeV1::Unknown),
    ])
    .unwrap();
    assert_eq!(
        check_port_nonregression_coverage(&port_input(&obligations, true, true)),
        Err(TheoremViolation::IncompleteCoverage {
            uncovered: vec![ProtectedDimensionV1::Security],
        })
    );
}

#[test]
fn thm71_refuses_required_obligation_only_observed() {
    let obligations = ProtectedScopeObligationsV1::new(vec![obligation(
        ProtectedDimensionV1::Api,
        true,
        CoverageGradeV1::Observed,
    )])
    .unwrap();
    assert_eq!(
        check_port_nonregression_coverage(&port_input(&obligations, true, true)),
        Err(TheoremViolation::WeakRequiredObligation {
            dimension: ProtectedDimensionV1::Api,
        })
    );
}

#[test]
fn thm71_refuses_unsound_verifier_and_missing_baseline() {
    let obligations = ProtectedScopeObligationsV1::new(vec![obligation(
        ProtectedDimensionV1::Tests,
        true,
        CoverageGradeV1::Proved,
    )])
    .unwrap();
    assert_eq!(
        check_port_nonregression_coverage(&port_input(&obligations, false, true)),
        Err(TheoremViolation::UnsoundVerifier)
    );
    assert_eq!(
        check_port_nonregression_coverage(&port_input(&obligations, true, false)),
        Err(TheoremViolation::BaselineUnavailable)
    );
}

#[test]
fn thm71_refuses_empty_declared_scope() {
    let obligations = ProtectedScopeObligationsV1::new(vec![]).unwrap();
    assert_eq!(
        check_port_nonregression_coverage(&port_input(&obligations, true, true)),
        Err(TheoremViolation::NoDeclaredObligations)
    );
}

#[test]
fn thm71_deterministic() {
    let obligations = ProtectedScopeObligationsV1::new(vec![obligation(
        ProtectedDimensionV1::Tests,
        true,
        CoverageGradeV1::Proved,
    )])
    .unwrap();
    let input = port_input(&obligations, true, true);
    let first = check_port_nonregression_coverage(&input);
    let second = check_port_nonregression_coverage(&input);
    assert_eq!(first, second);
    assert!(first.is_ok());
}

// ---------------------------------------------------------------------------
// Thm 8.1 -- Greenfield Strategy Preservation (mandatory-gate audit)
// ---------------------------------------------------------------------------

fn capability(id: &str, optional: bool) -> BackendCapability {
    BackendCapability {
        id: id.into(),
        kind: CapabilityKind::Suggestion,
        optional,
        requires_native_tool: None,
    }
}

fn greenfield_input(capabilities: Vec<BackendCapability>) -> GreenfieldStrategyInput {
    GreenfieldStrategyInput {
        capabilities,
        native_tools_available: BTreeSet::from(["git".to_owned()]),
        evidence_expandable: true,
        subjective_decisions_with_model_user: true,
    }
}

#[test]
fn thm81_certifies_optional_capability_set() {
    let input = greenfield_input(vec![
        capability("milestone-root", true),
        BackendCapability {
            id: "structure".into(),
            kind: CapabilityKind::Plan,
            optional: true,
            requires_native_tool: Some("git".into()),
        },
        BackendCapability {
            id: "tests".into(),
            kind: CapabilityKind::Capability,
            optional: true,
            requires_native_tool: None,
        },
    ]);
    let certification = check_greenfield_strategy_preservation(&input).unwrap();
    assert_eq!(certification.audited_capabilities, 3);
}

#[test]
fn thm81_refuses_mandatory_gate() {
    let input = greenfield_input(vec![capability("force-format", false)]);
    assert_eq!(
        check_greenfield_strategy_preservation(&input),
        Err(TheoremViolation::MandatoryGate {
            id: "force-format".into()
        })
    );
}

#[test]
fn thm81_refuses_unavailable_native_tool() {
    let input = greenfield_input(vec![BackendCapability {
        id: "plan".into(),
        kind: CapabilityKind::Plan,
        optional: true,
        requires_native_tool: Some("rustup".into()),
    }]);
    assert_eq!(
        check_greenfield_strategy_preservation(&input),
        Err(TheoremViolation::NativeToolUnavailable {
            id: "plan".into(),
            tool: "rustup".into(),
        })
    );
}

#[test]
fn thm81_refuses_unmet_premises() {
    let input = GreenfieldStrategyInput {
        capabilities: vec![],
        native_tools_available: BTreeSet::new(),
        evidence_expandable: false,
        subjective_decisions_with_model_user: true,
    };
    assert_eq!(
        check_greenfield_strategy_preservation(&input),
        Err(TheoremViolation::EvidenceNotExpandable)
    );
    let input = GreenfieldStrategyInput {
        capabilities: vec![],
        native_tools_available: BTreeSet::new(),
        evidence_expandable: true,
        subjective_decisions_with_model_user: false,
    };
    assert_eq!(
        check_greenfield_strategy_preservation(&input),
        Err(TheoremViolation::SubjectiveDecisionAutoResolved)
    );
}

#[test]
fn thm81_deterministic() {
    let input = greenfield_input(vec![capability("milestone-root", true)]);
    let first = check_greenfield_strategy_preservation(&input);
    let second = check_greenfield_strategy_preservation(&input);
    assert_eq!(first, second);
    assert!(first.is_ok());
}
