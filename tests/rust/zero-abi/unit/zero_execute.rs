    use super::*;

    use crate::verdict::PremiseV1;
    use serde_json::{Value, json};

    const SCHEMA_PROPERTIES: &[&str] = &[
        "abi_version",
        "kind",
        "continuation_handle",
        "project_root",
        "successor_root",
        "decision_view_root",
        "result_root",
        "exact_delta_root",
        "verification_receipt_root",
        "successor_certificate_root",
        "resource_ledger_root",
        "cache_report_root",
        "question",
        "choices",
        "unknown_reasons",
        "no_mutation_receipt_root",
        "audit_event_range",
    ];

    const SCHEMA_REQUIRED: &[&str] = &["abi_version", "kind", "audit_event_range", "resource_ledger_root"];

    fn range() -> AuditEventRangeV1 {
        AuditEventRangeV1::new(0, 10).expect("valid range")
    }

    fn base_fields() -> ZeroExecuteFieldsV6 {
        ZeroExecuteFieldsV6 {
            continuation_handle: Some("cont:abc".into()),
            project_root: Some("fz://root/project".into()),
            ..ZeroExecuteFieldsV6::default()
        }
    }

    fn safe_verdict() -> SafetyVerdictV1 {
        SafetyVerdictV1::from_premises(&[
            PremiseV1::new("p1", Some(true)).unwrap(),
            PremiseV1::new("p2", Some(true)).unwrap(),
        ])
    }

    fn root(value: &str) -> String {
        format!("fz://blob/{value}")
    }

    /// ZS-KERNEL-004 acceptance: `Completed` requires a `Safe` verdict.
    /// Unknown or Unsafe verdicts (e.g. after removing one premise) yield
    /// `VerdictNotSafe`, never a `Completed` envelope.
    #[test]
    fn completed_requires_safe_verdict_and_roots() {
        let fields = ZeroExecuteFieldsV6 {
            successor_root: Some(root("succ")),
            result_root: Some(root("result")),
            verification_receipt_root: Some(root("verif")),
            ..base_fields()
        };

        // Safe verdict: Completed is constructible.
        let completed = ZeroExecuteResultV6::completed(
            safe_verdict(),
            fields.clone(),
            root("ledger"),
            range(),
        )
        .expect("Safe verdict with roots completes");
        assert_eq!(completed.kind(), ZeroExecuteKindV6::Completed);

        // Missing premise -> Unknown verdict -> VerdictNotSafe.
        let unknown = SafetyVerdictV1::from_premises(&[
            PremiseV1::new("p1", Some(true)).unwrap(),
            PremiseV1::new("p2", None).unwrap(),
        ]);
        assert!(matches!(unknown, SafetyVerdictV1::Unknown { .. }));
        assert_eq!(
            ZeroExecuteResultV6::completed(unknown, fields.clone(), root("ledger"), range()),
            Err(ZeroExecuteErrorV6::VerdictNotSafe)
        );

        // Falsified premise -> Unsafe verdict -> VerdictNotSafe.
        let unsafe_verdict = SafetyVerdictV1::from_premises(&[
            PremiseV1::new("p1", Some(true)).unwrap(),
            PremiseV1::new("p2", Some(false)).unwrap(),
        ]);
        assert_eq!(
            ZeroExecuteResultV6::completed(
                unsafe_verdict.clone(),
                fields.clone(),
                root("ledger"),
                range()
            ),
            Err(ZeroExecuteErrorV6::VerdictNotSafe)
        );

        // Missing required roots fail closed even with a Safe verdict.
        for missing in [
            ZeroExecuteFieldsV6 {
                successor_root: None,
                ..fields.clone()
            },
            ZeroExecuteFieldsV6 {
                result_root: None,
                ..fields.clone()
            },
            ZeroExecuteFieldsV6 {
                verification_receipt_root: None,
                ..fields.clone()
            },
        ] {
            assert!(matches!(
                ZeroExecuteResultV6::completed(safe_verdict(), missing, root("ledger"), range()),
                Err(ZeroExecuteErrorV6::MissingRequiredField(_))
            ));
        }

        // from_verdict_never_completed rejects Safe by design and maps
        // Unsafe -> RejectedNoMutation, Unknown -> VerificationUnknown.
        assert_eq!(
            ZeroExecuteResultV6::from_verdict_never_completed(
                &safe_verdict(),
                fields.clone(),
                root("ledger"),
                range()
            ),
            Err(ZeroExecuteErrorV6::VerdictMustNotBeSafe)
        );
        let rejected = ZeroExecuteResultV6::from_verdict_never_completed(
            &unsafe_verdict,
            ZeroExecuteFieldsV6 {
                no_mutation_receipt_root: Some(root("no_mutation")),
                ..fields.clone()
            },
            root("ledger"),
            range(),
        )
        .expect("Unsafe maps to RejectedNoMutation");
        assert_eq!(rejected.kind(), ZeroExecuteKindV6::RejectedNoMutation);
        let unknown_env = ZeroExecuteResultV6::from_verdict_never_completed(
            &SafetyVerdictV1::Unknown {
                reasons: vec!["r".into()],
            },
            ZeroExecuteFieldsV6 {
                unknown_reasons: vec!["verifier_timeout:verifier-a".into()],
                ..fields
            },
            root("ledger"),
            range(),
        )
        .expect("Unknown maps to VerificationUnknown");
        assert_eq!(unknown_env.kind(), ZeroExecuteKindV6::VerificationUnknown);
    }

    /// Every kind serializes to the schema's property set with the schema's
    /// required fields present; the six base kinds are schema kinds and the
    /// two D5 adapter extensions are not.
    #[test]
    fn every_kind_serializes_to_schema_field_set() {
        let completed_fields = ZeroExecuteFieldsV6 {
            successor_root: Some(root("succ")),
            result_root: Some(root("result")),
            verification_receipt_root: Some(root("verif")),
            ..base_fields()
        };
        let decision_fields = ZeroExecuteFieldsV6 {
            question: Some("which direction?".into()),
            choices: vec![json!("north"), json!("south")],
            ..base_fields()
        };
        let expansion_fields = base_fields();
        let unknown_fields = ZeroExecuteFieldsV6 {
            unknown_reasons: vec!["verifier_timeout".into()],
            ..base_fields()
        };
        let fallback_fields = ZeroExecuteFieldsV6 {
            unknown_reasons: vec!["missing_premise:p2".into()],
            ..base_fields()
        };
        let no_mutation_fields = ZeroExecuteFieldsV6 {
            no_mutation_receipt_root: Some(root("no_mutation")),
            ..base_fields()
        };
        let cancelled_fields = ZeroExecuteFieldsV6 {
            successor_root: None,
            ..base_fields()
        };

        let envelopes = [
            ZeroExecuteResultV6::completed(
                safe_verdict(),
                completed_fields,
                root("ledger"),
                range(),
            )
            .unwrap(),
            ZeroExecuteResultV6::decision_required(decision_fields, root("ledger"), range())
                .unwrap(),
            ZeroExecuteResultV6::evidence_expansion_required(
                expansion_fields,
                root("ledger"),
                range(),
            )
            .unwrap(),
            ZeroExecuteResultV6::verification_unknown(unknown_fields, root("ledger"), range())
                .unwrap(),
            ZeroExecuteResultV6::baseline_fallback_required(
                fallback_fields,
                root("ledger"),
                range(),
            )
            .unwrap(),
            ZeroExecuteResultV6::rejected_no_mutation(
                no_mutation_fields,
                root("ledger"),
                range(),
            )
            .unwrap(),
            ZeroExecuteResultV6::cancelled(cancelled_fields.clone(), root("ledger"), range())
                .unwrap(),
            ZeroExecuteResultV6::failed_no_authority(
                cancelled_fields,
                root("ledger"),
                range(),
            )
            .unwrap(),
        ];

        for envelope in &envelopes {
            let value = serde_json::to_value(envelope).expect("serializes");
            let object = value.as_object().expect("object");
            for key in object.keys() {
                assert!(
                    SCHEMA_PROPERTIES.contains(&key.as_str()),
                    "serialized field {key} not in schema property set"
                );
            }
            for required in SCHEMA_REQUIRED {
                assert!(
                    object.contains_key(*required),
                    "missing schema-required field {required}"
                );
            }
            assert_eq!(
                object.get("abi_version").and_then(Value::as_str),
                Some(ZERO_EXECUTE_ABI_VERSION_V6)
            );
            let kind = object.get("kind").and_then(Value::as_str).unwrap();
            assert_eq!(kind, envelope.kind().kind_name());
        }

        // Base-kind classification.
        let base_kinds = [
            ZeroExecuteKindV6::Completed,
            ZeroExecuteKindV6::DecisionRequired,
            ZeroExecuteKindV6::EvidenceExpansionRequired,
            ZeroExecuteKindV6::VerificationUnknown,
            ZeroExecuteKindV6::BaselineFallbackRequired,
            ZeroExecuteKindV6::RejectedNoMutation,
        ];
        for kind in base_kinds {
            assert!(kind.is_v6_base_schema_kind(), "{kind:?} must be base");
        }
        assert!(!ZeroExecuteKindV6::Cancelled.is_v6_base_schema_kind());
        assert!(!ZeroExecuteKindV6::FailedNoAuthority.is_v6_base_schema_kind());

        // Cancelled/FailedNoAuthority reject successor_root.
        let forbidden = ZeroExecuteFieldsV6 {
            successor_root: Some(root("succ")),
            ..base_fields()
        };
        assert_eq!(
            ZeroExecuteResultV6::cancelled(forbidden.clone(), root("ledger"), range()),
            Err(ZeroExecuteErrorV6::ForbiddenRoot("successor_root"))
        );
        assert_eq!(
            ZeroExecuteResultV6::failed_no_authority(forbidden, root("ledger"), range()),
            Err(ZeroExecuteErrorV6::ForbiddenRoot("successor_root"))
        );
    }

    #[test]
    fn kind_specific_constructor_requirements_fail_closed() {
        let range = range();
        // DecisionRequired needs question, choices, continuation handle.
        assert_eq!(
            ZeroExecuteResultV6::decision_required(
                ZeroExecuteFieldsV6 {
                    question: None,
                    ..base_fields()
                },
                root("ledger"),
                range,
            ),
            Err(ZeroExecuteErrorV6::MissingRequiredField("question"))
        );
        assert_eq!(
            ZeroExecuteResultV6::decision_required(
                ZeroExecuteFieldsV6 {
                    question: Some("q".into()),
                    choices: vec![],
                    ..base_fields()
                },
                root("ledger"),
                range,
            ),
            Err(ZeroExecuteErrorV6::EmptyChoices)
        );
        assert_eq!(
            ZeroExecuteResultV6::decision_required(
                ZeroExecuteFieldsV6 {
                    question: Some("q".into()),
                    choices: vec![json!("a")],
                    continuation_handle: None,
                    ..base_fields()
                },
                root("ledger"),
                range,
            ),
            Err(ZeroExecuteErrorV6::MissingRequiredField("continuation_handle"))
        );
        // EvidenceExpansionRequired needs a continuation handle.
        assert_eq!(
            ZeroExecuteResultV6::evidence_expansion_required(
                ZeroExecuteFieldsV6 {
                    continuation_handle: None,
                    ..base_fields()
                },
                root("ledger"),
                range,
            ),
            Err(ZeroExecuteErrorV6::MissingRequiredField("continuation_handle"))
        );
        // Unknown/fallback kinds need nonempty reasons.
        assert_eq!(
            ZeroExecuteResultV6::verification_unknown(base_fields(), root("ledger"), range),
            Err(ZeroExecuteErrorV6::EmptyUnknownReasons)
        );
        assert_eq!(
            ZeroExecuteResultV6::baseline_fallback_required(base_fields(), root("ledger"), range),
            Err(ZeroExecuteErrorV6::EmptyUnknownReasons)
        );
        // RejectedNoMutation needs the no-mutation receipt.
        assert_eq!(
            ZeroExecuteResultV6::rejected_no_mutation(base_fields(), root("ledger"), range),
            Err(ZeroExecuteErrorV6::MissingRequiredField("no_mutation_receipt_root"))
        );
        // Audit range must be valid.
        assert_eq!(
            AuditEventRangeV1::new(7, 3),
            Err(ZeroExecuteErrorV6::InvalidAuditRange { start: 7, end: 3 })
        );
    }

    #[test]
    fn deserialization_round_trip_and_validation() {
        let envelope = ZeroExecuteResultV6::completed(
            safe_verdict(),
            ZeroExecuteFieldsV6 {
                successor_root: Some(root("succ")),
                result_root: Some(root("result")),
                verification_receipt_root: Some(root("verif")),
                ..base_fields()
            },
            root("ledger"),
            range(),
        )
        .unwrap();
        let value = serde_json::to_value(&envelope).unwrap();
        let round: ZeroExecuteResultV6 = serde_json::from_value(value).unwrap();
        assert_eq!(round, envelope);
        assert!(round.validate().is_ok());

        // Wrong abi_version fails validation and deserialization stays valid
        // structurally but validate() rejects.
        let mut tampered = serde_json::to_value(&envelope).unwrap();
        tampered["abi_version"] = json!("zerostack.racc.v5");
        let round: ZeroExecuteResultV6 = serde_json::from_value(tampered).unwrap();
        assert_eq!(
            round.validate(),
            Err(ZeroExecuteErrorV6::WrongAbiVersion {
                actual: "zerostack.racc.v5".into()
            })
        );

        // Unknown fields are rejected by serde (deny_unknown_fields).
        let mut extra = serde_json::to_value(&envelope).unwrap();
        extra["future_field"] = json!(1);
        assert!(serde_json::from_value::<ZeroExecuteResultV6>(extra).is_err());

        // A Cancelled envelope carrying successor_root fails validation.
        let cancelled = ZeroExecuteResultV6::cancelled(base_fields(), root("ledger"), range())
            .unwrap();
        let mut with_successor = serde_json::to_value(&cancelled).unwrap();
        with_successor["successor_root"] = json!(root("succ"));
        let round: ZeroExecuteResultV6 = serde_json::from_value(with_successor).unwrap();
        assert_eq!(
            round.validate(),
            Err(ZeroExecuteErrorV6::ForbiddenRoot("successor_root"))
        );
    }

    /// D5 forbidden transitions are rejected for BOTH policy_supplied values;
    /// the legal edge set is exactly the documented one; terminals have no
    /// outgoing edges.
    #[test]
    fn forbidden_transitions_are_rejected() {
        use ContinuationStateV1::*;
        let all = [
            Bound,
            Snapshotted,
            Resolved,
            WaitingDecision,
            Planned,
            Executing,
            DeltaSealed,
            Verifying,
            Authorized,
            Committed,
            Restored,
            Rejected,
            Unknown,
            Cancelled,
        ];
        // D5 hard-forbidden transitions regardless of policy.
        for policy in [false, true] {
            assert!(!ContinuationStateV1::allowed_transition(Unknown, Authorized, policy));
            assert!(!ContinuationStateV1::allowed_transition(Executing, Committed, policy));
            assert!(!ContinuationStateV1::allowed_transition(WaitingDecision, Executing, policy));
        }
        // WaitingDecision -> Planned requires a supplied policy.
        assert!(!ContinuationStateV1::allowed_transition(WaitingDecision, Planned, false));
        assert!(ContinuationStateV1::allowed_transition(WaitingDecision, Planned, true));

        // Documented legal edges (policy=false): forward chain + branch +
        // restoration + cancellation escapes.
        let legal = [
            (Bound, Snapshotted),
            (Snapshotted, Resolved),
            (Resolved, Planned),
            (Resolved, WaitingDecision),
            (Planned, Executing),
            (Executing, DeltaSealed),
            (DeltaSealed, Verifying),
            (Verifying, Authorized),
            (Verifying, Rejected),
            (Verifying, Unknown),
            (Authorized, Committed),
            (Unknown, Restored),
            (Unknown, Cancelled),
            (WaitingDecision, Cancelled),
            (Bound, Cancelled),
            (Snapshotted, Cancelled),
            (Resolved, Cancelled),
            (Planned, Cancelled),
            (Executing, Cancelled),
            (DeltaSealed, Cancelled),
            (Verifying, Cancelled),
            (Authorized, Cancelled),
        ];
        for from in all {
            for to in all {
                let expected = legal.contains(&(from, to));
                assert_eq!(
                    ContinuationStateV1::allowed_transition(from, to, false),
                    expected,
                    "edge {from:?} -> {to:?} (no policy) mismatch"
                );
            }
        }
        // With policy, WaitingDecision -> Planned joins the legal set.
        let legal_with_policy = {
            let mut edges = legal.to_vec();
            edges.push((WaitingDecision, Planned));
            edges
        };
        for from in all {
            for to in all {
                let expected = legal_with_policy.contains(&(from, to));
                assert_eq!(
                    ContinuationStateV1::allowed_transition(from, to, true),
                    expected,
                    "edge {from:?} -> {to:?} (policy) mismatch"
                );
            }
        }
        // Terminals have no outgoing edges at all.
        for terminal in [Committed, Restored, Rejected, Cancelled] {
            assert!(ContinuationStateV1::is_terminal(terminal));
            for to in all {
                assert!(
                    !ContinuationStateV1::allowed_transition(terminal, to, true),
                    "terminal {terminal:?} must have no outgoing edge to {to:?}"
                );
            }
        }
        for non_terminal in [Bound, Snapshotted, Resolved, WaitingDecision, Planned, Executing, DeltaSealed, Verifying, Authorized, Unknown] {
            assert!(!ContinuationStateV1::is_terminal(non_terminal));
        }
    }
