    use std::borrow::Cow;

    use super::*;
    use zero_abi::robust_snap::ProtectedEffectClassV1;
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn effect(byte: u8) -> ProtectedEffectV1 {
        ProtectedEffectV1 {
            effect_digest: d(byte),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        }
    }

    fn route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "dcr-index",
                "index_version": "1",
                "operator_id": "dcr-verifier",
                "operator_version": "1",
                "parser_id": "dcr-parser",
                "parser_version": "1",
            }),
        )
    }

    fn fiber() -> WorldFiberDescriptor {
        WorldFiberDescriptor {
            model_version: ROBUST_SNAP_MODEL_VERSION.into(),
            assembly_manifest_digest: d(40),
            source_image_digest: d(41),
            task_fingerprint: d(42),
            assumptions: vec!["finite exact test fiber".into()],
            worlds: vec![d(1), d(2), d(3)],
        }
    }

    fn query(id: u8, cost: u64, cells: Vec<(u8, Vec<DigestV1>)>) -> RecoveryQueryV1 {
        RecoveryQueryV1 {
            query_digest: d(id),
            native_cost_units: cost,
            evidence_route_digest: d(id + 1),
            outcomes: cells
                .into_iter()
                .map(|(outcome, worlds)| RecoveryQueryOutcomeV1 {
                    outcome_digest: d(outcome),
                    worlds,
                })
                .collect(),
        }
    }

    fn conflict_problem(
        status: SourceFiberStatusV1,
        query_cost: u64,
        raw_cost: u64,
    ) -> DominanceRecoveryProblemV1 {
        DominanceRecoveryProblemV1::new(
            d(50),
            d(42),
            fiber(),
            status,
            d(51),
            d(52),
            d(53),
            vec![
                ProtectedEffectSet {
                    world_id: d(1),
                    effects: vec![effect(10), effect(11)],
                },
                ProtectedEffectSet {
                    world_id: d(2),
                    effects: vec![effect(11), effect(12)],
                },
                ProtectedEffectSet {
                    world_id: d(3),
                    effects: vec![effect(10), effect(12)],
                },
            ],
            vec![effect(10), effect(11), effect(12)],
            vec![
                WorldRecoveryBudgetV1 {
                    world_id: d(1),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
                WorldRecoveryBudgetV1 {
                    world_id: d(2),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
                WorldRecoveryBudgetV1 {
                    world_id: d(3),
                    probability_weight: 1,
                    raw_baseline_cost_units: raw_cost,
                },
            ],
            vec![
                query(
                    60,
                    query_cost,
                    vec![(61, vec![d(1)]), (62, vec![d(2), d(3)])],
                ),
                query(
                    70,
                    query_cost + 2,
                    vec![(71, vec![d(1), d(2)]), (72, vec![d(3)])],
                ),
            ],
            d(54),
            Vec::new(),
            route(),
            d(55),
        )
        .unwrap()
    }

    fn complete_problem(status: SourceFiberStatusV1) -> DominanceRecoveryProblemV1 {
        let mut problem = conflict_problem(status, 5, 100);
        for protected in &mut problem.protected_effects {
            protected.effects = vec![effect(10), effect(11), effect(12)];
        }
        problem.queries.clear();
        problem.validate().unwrap();
        problem
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }

    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "dcr-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "dcr-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "dcr-index").then_some("1")
        }
    }

    fn test_certificate(
        problem: &DominanceRecoveryProblemV1,
    ) -> (EvidenceCertificate<'static>, TestResolver) {
        let bytes = problem.canonical_bytes().unwrap();
        let digest = sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(7) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "dcr-parser".into(),
                    parser_version: "1".into(),
                    index_id: "dcr-index".into(),
                    index_version: "1".into(),
                    operator_id: "dcr-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "dcr-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(7),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn recover(problem: DominanceRecoveryProblemV1) -> RecoveryDecisionV1 {
        let (certificate, resolver) = test_certificate(&problem);
        let evidence = verify(&certificate, &resolver).unwrap();
        dominance_complete_recover_v1(problem, &evidence).unwrap()
    }

    fn hex(digest: DigestV1) -> String {
        digest.to_hex()
    }

    #[test]
    fn contract_digest_is_stable() {
        assert_eq!(
            hex(dcr_contract_digest_v1()),
            "a0a7a6951757472cdd1c5730ada5901a8c0f9a982c26b88338941dc34f5cf967"
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/dominance-complete-recovery-v1.schema.json"
            )))
            .to_hex(),
            DCR_SCHEMA_SHA256_V1
        );
        assert_eq!(
            serde_json::to_value(ExactRecoveryCostV1 {
                numerator: u128::MAX,
                denominator: u128::MAX - 1,
            })
            .unwrap(),
            json!({
                "denominator": (u128::MAX - 1).to_string(),
                "numerator": u128::MAX.to_string(),
            })
        );
    }

    #[test]
    fn exact_and_sound_overapproximation_mint_opaque_complete_authority() {
        for status in [
            SourceFiberStatusV1::Exact,
            SourceFiberStatusV1::SoundOverapproximation,
        ] {
            let RecoveryDecisionV1::Complete(certificate) = recover(complete_problem(status))
            else {
                panic!("complete finite intersection must mint DCR authority");
            };
            certificate.validate().unwrap();
            assert_eq!(
                certificate.common_effects(),
                &[effect(10), effect(11), effect(12)]
            );
            let record = certificate.record();
            let bytes = record.canonical_bytes().unwrap();
            assert_eq!(
                DominanceCompleteRecoveryCertificateRecordV1::from_canonical_bytes(&bytes).unwrap(),
                record
            );
        }
    }

    #[test]
    fn three_way_conflict_is_not_laundered_into_pairwise_completeness() {
        let RecoveryDecisionV1::Conflict(decision) =
            recover(conflict_problem(SourceFiberStatusV1::Exact, 5, 100))
        else {
            panic!("unresolved three-world ambiguity must trigger a query");
        };
        assert_eq!(decision.selected_query().query_digest, d(60));
        assert_eq!(decision.optimal_expected_cost().numerator(), 5);
        assert_eq!(decision.optimal_expected_cost().denominator(), 1);
        assert_eq!(decision.conflict_hyperedges().len(), 1);
        assert_eq!(
            decision.conflict_hyperedges()[0].worlds(),
            &[d(1), d(2), d(3)]
        );
        for pair in [[d(1), d(2)], [d(1), d(3)], [d(2), d(3)]] {
            assert!(
                !decision
                    .conflict_hyperedges()
                    .iter()
                    .any(|edge| edge.worlds() == pair)
            );
        }
    }

    #[test]
    fn oversized_exact_analysis_deoptimizes_instead_of_hanging_or_truncating() {
        let worlds = (1..=17).map(d).collect::<Vec<_>>();
        let protected_effects = worlds
            .iter()
            .enumerate()
            .map(|(index, world)| ProtectedEffectSet {
                world_id: *world,
                effects: vec![effect((index + 20) as u8)],
            })
            .collect::<Vec<_>>();
        let accessible = (20..=36).map(effect).collect::<Vec<_>>();
        let world_budgets = worlds
            .iter()
            .map(|world| WorldRecoveryBudgetV1 {
                world_id: *world,
                probability_weight: 1,
                raw_baseline_cost_units: 100,
            })
            .collect::<Vec<_>>();
        let problem = DominanceRecoveryProblemV1::new(
            d(50),
            d(42),
            WorldFiberDescriptor {
                model_version: ROBUST_SNAP_MODEL_VERSION.into(),
                assembly_manifest_digest: d(40),
                source_image_digest: d(41),
                task_fingerprint: d(42),
                assumptions: vec!["bounded large fiber".into()],
                worlds,
            },
            SourceFiberStatusV1::Exact,
            d(51),
            d(52),
            d(53),
            protected_effects,
            accessible,
            world_budgets,
            Vec::new(),
            d(54),
            Vec::new(),
            route(),
            d(55),
        )
        .unwrap();
        let RecoveryDecisionV1::Unknown(unknown) = recover(problem) else {
            panic!("bounded analysis exhaustion must deoptimize");
        };
        assert_eq!(
            unknown.reason(),
            RecoveryUnknownReasonV1::AnalysisBoundExceeded
        );
        assert!(unknown.raw_baseline_required());
    }

    #[test]
    fn unknown_fiber_and_nonbeneficial_query_require_raw_baseline() {
        let RecoveryDecisionV1::Unknown(unknown) =
            recover(conflict_problem(SourceFiberStatusV1::Unknown, 1, 100))
        else {
            panic!("unknown fiber must not complete");
        };
        assert_eq!(unknown.reason(), RecoveryUnknownReasonV1::FiberUnknown);
        assert!(unknown.raw_baseline_required());

        let RecoveryDecisionV1::Unknown(unknown) =
            recover(conflict_problem(SourceFiberStatusV1::Exact, 100, 5))
        else {
            panic!("raw baseline must win an equal-or-cheaper exact DP comparison");
        };
        assert_eq!(
            unknown.reason(),
            RecoveryUnknownReasonV1::RawBaselineCheaperOrEqual
        );
        assert!(unknown.raw_baseline_required());
    }

    #[test]
    fn exact_query_conditioning_reaches_complete_without_full_reconstruction() {
        let problem = conflict_problem(SourceFiberStatusV1::Exact, 5, 100);
        let RecoveryDecisionV1::Conflict(decision) = recover(problem.clone()) else {
            panic!("initial ambiguity must query");
        };
        let conditioned = problem
            .condition_on(
                decision.selected_query().query_digest,
                d(62),
                d(80),
                d(81),
                vec![effect(10), effect(11), effect(12)],
                d(82),
                route(),
                d(83),
            )
            .unwrap();
        assert_eq!(conditioned.fiber.worlds, vec![d(2), d(3)]);
        let RecoveryDecisionV1::Complete(certificate) = recover(conditioned) else {
            panic!("one deciding observation must reach a complete leaf");
        };
        assert_eq!(certificate.common_effects(), &[effect(12)]);
        assert_eq!(certificate.claim.fallback_safepoint, d(83));
        assert_eq!(certificate.claim.decision_view_digest, d(81));
        assert_eq!(certificate.claim.recovery_query_trace.len(), 1);
    }

    #[test]
    fn invalid_query_partition_and_tampered_record_fail_closed() {
        let mut invalid = conflict_problem(SourceFiberStatusV1::Exact, 5, 100);
        invalid.queries[0].outcomes[1].worlds.pop();
        assert_eq!(
            invalid.validate().unwrap_err().failure_code(),
            DcrFailureCodeV1::InvalidQuery
        );

        let RecoveryDecisionV1::Complete(certificate) =
            recover(complete_problem(SourceFiberStatusV1::Exact))
        else {
            panic!("complete fixture must mint authority");
        };
        let mut record = certificate.record();
        record.common_effects.pop();
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            DcrFailureCodeV1::CertificateDigestMismatch
        );
    }

    #[test]
    fn claim_shape_matches_external_schema_and_canonical_bytes_are_strict() {
        let RecoveryDecisionV1::Complete(certificate) =
            recover(complete_problem(SourceFiberStatusV1::Exact))
        else {
            panic!("complete fixture must mint authority");
        };
        let claim = certificate.claim();
        let object = serde_json::to_value(claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "accessible_effect_surface_digest",
            "baseline_identity",
            "common_baseline_dominant_effect_class",
            "conflict_hyperedges",
            "coverage_certificate",
            "decision_view_digest",
            "fallback_safepoint",
            "fiber_status",
            "project_root",
            "reasoning_contract_digest",
            "recovery_query_trace",
            "schema_version",
            "task_identity",
            "verifier_route",
            "world_fiber_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(object, expected);
        let value = serde_json::to_value(claim).unwrap();
        assert_eq!(value["fiber_status"], "exact");
        assert_eq!(
            value["common_baseline_dominant_effect_class"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        let mut bytes = claim.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            DominanceCompleteRecoveryClaimV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            DcrFailureCodeV1::NonCanonicalEncoding
        );
    }
