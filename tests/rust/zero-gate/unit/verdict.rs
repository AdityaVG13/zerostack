    use super::*;

    fn digest_hex_ok() -> String {
        "ab".repeat(32)
    }

    fn evaluator(id: &str) -> EvaluatorIdentityV1 {
        EvaluatorIdentityV1 {
            evaluator_id: id.into(),
            declaration_digest_hex: digest_hex_ok(),
        }
    }

    fn verdict_equivalent() -> VerifierVerdictV1 {
        VerifierVerdictV1::Equivalent
    }

    fn verdict_dominates() -> VerifierVerdictV1 {
        VerifierVerdictV1::Dominates
    }

    fn verdict_reject(reason: &str) -> VerifierVerdictV1 {
        VerifierVerdictV1::reject(vec![reason.into()]).expect("nonempty reason")
    }

    fn verdict_unknown(reason: &str) -> VerifierVerdictV1 {
        VerifierVerdictV1::unknown(vec![reason.into()]).expect("nonempty reason")
    }

    fn all_kinds() -> Vec<VerifierVerdictV1> {
        vec![
            verdict_equivalent(),
            verdict_dominates(),
            verdict_reject("r1"),
            verdict_unknown("u1"),
        ]
    }

    #[test]
    fn quality_evidence_maps_distributional_and_unidentified_to_unknown() {
        let cases = [
            (
                QualityEvidenceClassV1::ExactNeutral,
                VerifierVerdictV1::Equivalent,
                true,
            ),
            (
                QualityEvidenceClassV1::PointwiseDominance,
                VerifierVerdictV1::Dominates,
                true,
            ),
            (
                QualityEvidenceClassV1::ScopedClassDominance,
                VerifierVerdictV1::Dominates,
                true,
            ),
            (
                QualityEvidenceClassV1::Distributional,
                VerifierVerdictV1::Unknown {
                    reasons: vec!["distributional_evidence_is_not_pointwise_proof".into()],
                },
                false,
            ),
            (
                QualityEvidenceClassV1::Unidentified,
                VerifierVerdictV1::Unknown {
                    reasons: vec!["unidentified_quality_evidence".into()],
                },
                false,
            ),
        ];
        for (class, expected, grants) in cases {
            let verdict = VerifierVerdictV1::from_quality_evidence(class);
            assert_eq!(verdict, expected, "class {class:?}");
            assert_eq!(
                verdict.grants_candidate_authority(),
                grants,
                "authority for class {class:?}"
            );
        }
    }

    #[test]
    fn timeout_disagreement_and_uncovered_dimension_are_unknown_never_promotable() {
        // Verifier timeout is Unknown and carries the verifier id.
        let timed_out = VerifierVerdictV1::from_verifier_timeout("verifier-a");
        assert_eq!(timed_out, verdict_unknown("verifier_timeout:verifier-a"));
        assert!(!timed_out.grants_candidate_authority());

        // Uncovered protected dimension is Unknown and carries the dimension.
        let uncovered = VerifierVerdictV1::from_uncovered_dimension("response_latency");
        assert_eq!(
            uncovered,
            verdict_unknown("uncovered_protected_dimension:response_latency")
        );
        assert!(!uncovered.grants_candidate_authority());

        // Full ordered-pair disagreement matrix: agreeing pairs keep their
        // kind, Reject+Reject merges, and every mixed pair resolves to
        // Unknown -- including Equivalent vs Dominates and Reject vs passing.
        let kinds = all_kinds();
        for a in &kinds {
            for b in &kinds {
                let merged = VerifierVerdictV1::from_verifier_disagreement(a, b);
                if a == b {
                    match a {
                        VerifierVerdictV1::Unknown { .. } => {
                            assert!(
                                matches!(&merged, VerifierVerdictV1::Unknown { .. }),
                                "Unknown+Unknown must stay Unknown"
                            );
                        }
                        VerifierVerdictV1::Reject { .. } => {
                            assert!(
                                matches!(&merged, VerifierVerdictV1::Reject { .. }),
                                "Reject+Reject must stay Reject"
                            );
                        }
                        _ => assert_eq!(&merged, a, "agreeing pair must keep its kind"),
                    }
                    continue;
                }
                match (a, b) {
                    (
                        VerifierVerdictV1::Reject { .. },
                        VerifierVerdictV1::Reject { .. },
                    ) => {
                        // Unreachable for a != b with normalized single-reason
                        // fixtures, but assert the law anyway.
                        assert!(matches!(&merged, VerifierVerdictV1::Reject { .. }));
                    }
                    _ => {
                        assert!(
                            matches!(&merged, VerifierVerdictV1::Unknown { .. }),
                            "mixed pair {a:?} vs {b:?} must resolve to Unknown, got {merged:?}"
                        );
                        assert!(
                            !merged.grants_candidate_authority(),
                            "mixed pair must never grant authority"
                        );
                    }
                }
            }
        }

        // Unknown never upgrades: any disagreement involving Unknown on either
        // side is Unknown (the never-promotable law across the only module
        // function that accepts VerifierVerdictV1 inputs).
        for other in &kinds {
            let left = VerifierVerdictV1::from_verifier_disagreement(&verdict_unknown("u"), other);
            let right = VerifierVerdictV1::from_verifier_disagreement(other, &verdict_unknown("u"));
            for result in [left, right] {
                assert!(
                    matches!(result, VerifierVerdictV1::Unknown { .. }),
                    "Unknown input must never promote, got {result:?}"
                );
                assert!(!result.grants_candidate_authority());
            }
        }
    }

    #[test]
    fn subjective_dimension_without_declared_evaluator_forces_decision_required() {
        // Dominates cannot pass an unattested subjective dimension.
        let verdict = verdict_dominates();
        let unattested = vec![SubjectiveDimensionV1 {
            name: "ergonomics".into(),
            declared_evaluator: None,
        }];
        assert_eq!(
            admit_with_subjective_gate(verdict.clone(), &unattested),
            GateAdmissionV1::DecisionRequired {
                dimension: "ergonomics".into(),
                reason: "subjective_dimension_requires_declared_evaluator".into(),
            }
        );

        // All declared: admission preserves the verdict and its authority.
        let declared = vec![SubjectiveDimensionV1 {
            name: "ergonomics".into(),
            declared_evaluator: Some(evaluator("human-a")),
        }];
        assert_eq!(
            admit_with_subjective_gate(verdict.clone(), &declared),
            GateAdmissionV1::Admitted {
                verdict: verdict_dominates()
            }
        );
        assert_eq!(
            admit_with_subjective_gate(verdict_equivalent(), &declared),
            GateAdmissionV1::Admitted {
                verdict: verdict_equivalent()
            }
        );

        // Duplicate names fail closed before the evaluator check.
        let duplicates = vec![
            SubjectiveDimensionV1 {
                name: "ergonomics".into(),
                declared_evaluator: Some(evaluator("human-a")),
            },
            SubjectiveDimensionV1 {
                name: "ergonomics".into(),
                declared_evaluator: Some(evaluator("human-b")),
            },
        ];
        assert_eq!(
            admit_with_subjective_gate(verdict.clone(), &duplicates),
            GateAdmissionV1::DecisionRequired {
                dimension: "ergonomics".into(),
                reason: "duplicate_subjective_dimension".into(),
            }
        );

        // Empty dimension list admits directly.
        assert_eq!(
            admit_with_subjective_gate(verdict, &[]),
            GateAdmissionV1::Admitted {
                verdict: verdict_dominates()
            }
        );

        // Invalid declared evaluator fails closed too.
        let bad_evaluator = EvaluatorIdentityV1 {
            evaluator_id: "human-a".into(),
            declaration_digest_hex: "XYZ".into(),
        };
        let invalid = vec![SubjectiveDimensionV1 {
            name: "ergonomics".into(),
            declared_evaluator: Some(bad_evaluator),
        }];
        assert_eq!(
            admit_with_subjective_gate(verdict_equivalent(), &invalid),
            GateAdmissionV1::DecisionRequired {
                dimension: "ergonomics".into(),
                reason: "invalid_declared_evaluator".into(),
            }
        );
    }

    #[test]
    fn reject_and_unknown_constructors_require_reasons() {
        assert_eq!(
            VerifierVerdictV1::reject(vec![]).unwrap_err().failure_code(),
            VerdictFailureCodeV1::EmptyReasons
        );
        assert_eq!(
            VerifierVerdictV1::unknown(vec![]).unwrap_err().failure_code(),
            VerdictFailureCodeV1::EmptyReasons
        );
        assert_eq!(
            VerifierVerdictV1::reject(vec!["   ".into()])
                .unwrap_err()
                .failure_code(),
            VerdictFailureCodeV1::EmptyReasons
        );
        assert_eq!(
            VerifierVerdictV1::unknown(vec!["".into()])
                .unwrap_err()
                .failure_code(),
            VerdictFailureCodeV1::EmptyReasons
        );

        // Reasons are sorted and deduplicated.
        assert_eq!(
            VerifierVerdictV1::reject(vec!["z".into(), "a".into(), "z".into()]).unwrap(),
            VerifierVerdictV1::Reject {
                reasons: vec!["a".into(), "z".into()]
            }
        );
        assert_eq!(
            VerifierVerdictV1::unknown(vec!["b".into(), "a".into(), "b".into()]).unwrap(),
            VerifierVerdictV1::Unknown {
                reasons: vec!["a".into(), "b".into()]
            }
        );
    }

    #[test]
    fn grants_candidate_authority_is_true_only_for_equivalent_and_dominates() {
        assert!(verdict_equivalent().grants_candidate_authority());
        assert!(verdict_dominates().grants_candidate_authority());
        assert!(!verdict_reject("r1").grants_candidate_authority());
        assert!(!verdict_unknown("u1").grants_candidate_authority());
    }

    #[test]
    fn evaluator_identity_validation_fails_closed() {
        assert!(evaluator("human-a").validate().is_ok());
        assert_eq!(
            EvaluatorIdentityV1 {
                evaluator_id: String::new(),
                declaration_digest_hex: digest_hex_ok(),
            }
            .validate()
            .unwrap_err()
            .failure_code(),
            VerdictFailureCodeV1::InvalidEvaluatorIdentity
        );
        assert_eq!(
            EvaluatorIdentityV1 {
                evaluator_id: "a".repeat(129),
                declaration_digest_hex: digest_hex_ok(),
            }
            .validate()
            .unwrap_err()
            .failure_code(),
            VerdictFailureCodeV1::InvalidEvaluatorIdentity
        );
        assert_eq!(
            EvaluatorIdentityV1 {
                evaluator_id: "human-a".into(),
                declaration_digest_hex: "AB".repeat(32),
            }
            .validate()
            .unwrap_err()
            .failure_code(),
            VerdictFailureCodeV1::InvalidEvaluatorIdentity
        );
        assert_eq!(
            EvaluatorIdentityV1 {
                evaluator_id: "human-a".into(),
                declaration_digest_hex: "ab".repeat(31),
            }
            .validate()
            .unwrap_err()
            .failure_code(),
            VerdictFailureCodeV1::InvalidEvaluatorIdentity
        );
        assert_eq!(
            SubjectiveDimensionV1 {
                name: String::new(),
                declared_evaluator: None,
            }
            .validate()
            .unwrap_err()
            .failure_code(),
            VerdictFailureCodeV1::InvalidSubjectiveDimension
        );
    }
