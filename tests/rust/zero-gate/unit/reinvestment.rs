    use super::*;
    use std::{
        borrow::Cow,
        collections::{BTreeMap, BTreeSet},
    };
    use tempfile::tempdir;
    use zero_abi::{
        ArtifactOwnerV1, CwirVerifierClassV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
        EffectVerificationPlanV1, EffectVerificationStepV1, NativeStatePolicyV1,
        ReasoningContractV1, TypedEffectOperationV1, sha256, verify_strict_no_downshift_v1,
    };
    use zero_cert::{
        CompletenessWitness, EffectVerificationOutcomeV1, EvidenceCertificate, ObjectId,
        OperatorLock, Provenance, Query, Resolver, SpanRef, TestId, accept_effect_verification_v1,
        verify,
    };
    use zero_ledger::{
        CausalCounterUnitV1, CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1,
        ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1,
    };
    use zero_store::{
        DurableProfileIdV1, JournalBindingV1, JournalPathsV1, initialize_published_root_v1,
    };

    use crate::{
        EffectClosureManifestV1, EffectClosureRequestV1, EffectResourceClosureV1,
        ResourceIsolationModeV1, ResourceRestorationModeV1, TransactionAccessV1,
        TransactionResourceKindV1, TransactionResourceRequirementV1, begin_effect_transaction_v1,
        effect_journal_binding_v1, validate_effect_closure_v1,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    struct Resident {
        bytes: Vec<u8>,
    }

    impl Resolver for Resident {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(&self.bytes) == object_id.0).then_some(&self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "read-span").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "tree-sitter").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zero-index").then_some("2")
        }
    }

    fn evidence(bytes: &[u8]) -> (EvidenceCertificate<'static>, Resident) {
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(99) },
                spans: vec![span],
                payload: Cow::Owned(bytes.to_vec()),
                provenance: Provenance {
                    parser_id: "tree-sitter".into(),
                    parser_version: "1".into(),
                    index_id: "zero-index".into(),
                    index_version: "2".into(),
                    operator_id: "read-span".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "read-span".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(99),
                    exit_code: 0,
                    trace_digest: object,
                },
                input_token_cost: 1,
                backend_work_units: 1,
            },
            Resident {
                bytes: bytes.to_vec(),
            },
        )
    }

    fn verifier_identity() -> DigestV1 {
        let (certificate, resident) = evidence(b"verifier identity");
        let verified = verify(&certificate, &resident).unwrap();
        q99_verifier_identity_v1(&verified)
    }

    fn reasoning_admission() -> StrictReasoningAdmissionV1 {
        let contract = ReasoningContractV1::new(
            d(1),
            d(2),
            d(3),
            d(4),
            d(5),
            "strict",
            "high",
            4096,
            1024,
            512,
            256,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::new(),
        )
        .unwrap();
        verify_strict_no_downshift_v1(&contract, &contract).unwrap()
    }

    fn counter(id: &str, unit: CausalCounterUnitV1, byte: u8) -> ParentCounterIdentityV1 {
        ParentCounterIdentityV1 {
            counter_id: id.into(),
            unit,
            boundary_digest: d(byte),
            adapter_digest: d(byte.wrapping_add(1)),
            platform_profile_digest: d(byte.wrapping_add(2)),
        }
    }

    fn vector(cpu: u64, tokens: u64) -> NativeResourceVectorV1 {
        NativeResourceVectorV1::new(vec![
            NativeResourceAmountV1 {
                identity: counter("parent.cpu_ns", CausalCounterUnitV1::CpuNanoseconds, 80),
                amount: cpu,
            },
            NativeResourceAmountV1 {
                identity: counter("parent.tokens", CausalCounterUnitV1::Tokens, 90),
                amount: tokens,
            },
        ])
        .unwrap()
    }

    fn program(snapshot: DigestV1) -> EffectProgramV1 {
        let target = EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: d(120),
            required_snapshot: snapshot,
        };
        let step = EffectVerificationStepV1 {
            verifier_digest: d(121),
            predicate_digest: d(122),
            environment_digest: d(123),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        };
        EffectProgramV1::new(
            snapshot,
            "reinvestment_test",
            vec![target],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: d(120),
                expected_before: d(124),
                replacement: d(125),
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![step]).unwrap(),
            EffectRollbackV1::Journaled,
        )
        .unwrap()
    }

    fn closed(snapshot: DigestV1) -> (EffectProgramV1, crate::ClosedEffectBoundaryV1) {
        let program = program(snapshot);
        let resource = TransactionResourceRequirementV1 {
            owner: ArtifactOwnerV1::FsZero,
            kind: TransactionResourceKindV1::ProjectFilesystem,
            scope_digest: d(126),
            baseline_state_digest: snapshot,
            access: TransactionAccessV1::ReadWrite,
            authority_digest: d(127),
        };
        let request = EffectClosureRequestV1::new(&program, vec![resource]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: resource,
                isolation: ResourceIsolationModeV1::Journaled,
                restoration: ResourceRestorationModeV1::JournalRollback,
            }],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        (program, boundary)
    }

    fn accepted(program: &EffectProgramV1) -> zero_cert::EffectAcceptedV1 {
        let (certificate, resident) = evidence(b"effect evidence");
        let verified = verify(&certificate, &resident).unwrap();
        let outcome = accept_effect_verification_v1(
            d(130),
            program,
            d(131),
            d(122),
            program.base_state(),
            d(121),
            &verified,
        )
        .unwrap();
        let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
            panic!("effect must be accepted")
        };
        accepted
    }

    fn journal_paths(dir: &std::path::Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            dir.join("root.json"),
            dir.join("journal.json"),
            dir.join("cartridge.json"),
            dir.join("owner-death.json"),
            dir.join("recovery.json"),
        )
        .unwrap()
    }

    fn binding(boundary: &crate::ClosedEffectBoundaryV1, new_root: DigestV1) -> JournalBindingV1 {
        effect_journal_binding_v1(
            boundary,
            d(132),
            DurableProfileIdV1::PortableStrict,
            new_root,
            d(133),
        )
        .unwrap()
    }

    fn verified_action(
        boundary: &crate::ClosedEffectBoundaryV1,
        kind: ReinvestmentActionKindV1,
        reserved: NativeResourceVectorV1,
    ) -> Result<VerifiedReinvestmentActionV1, ReinvestmentErrorV1> {
        let reasoning = reasoning_admission();
        let claim = ReinvestmentActionClaimV1::new(
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            kind,
            d(26),
            boundary.action_digest(),
            d(27),
            &reasoning,
            reserved,
            verifier_identity(),
        )?;
        let (certificate, resident) = evidence(&claim.canonical_bytes()?);
        let verified = verify(&certificate, &resident).unwrap();
        verify_reinvestment_action_v1(claim, &reasoning, &verified)
    }

    fn plan(
        action: VerifiedReinvestmentActionV1,
        baseline: NativeResourceVectorV1,
        extra: NativeResourceVectorV1,
        candidate: NativeResourceVectorV1,
        fallback: NativeResourceVectorV1,
    ) -> Result<ReinvestmentPlanAuthorityV1, ReinvestmentErrorV1> {
        admit_reinvestment_plan_v1(
            d(20),
            d(21),
            d(22),
            d(28),
            d(23),
            d(24),
            reasoning_admission().baseline_contract_digest(),
            baseline,
            extra,
            candidate,
            fallback,
            vec![action],
        )
    }

    fn work_receipt(
        identity: ParentCounterIdentityV1,
        amount: u64,
        work_id: u8,
    ) -> CausalWorkReceiptV1 {
        let CausalWorkOutcomeV1::Measured { receipt } = CausalWorkReceiptV1::build(
            d(23),
            ParentCounterObservationV1::Measured {
                window: ParentCounterWindowV1 {
                    identity,
                    start: 10,
                    end: 10 + amount,
                },
            },
            vec![CausalWorkChargeV1 {
                work_unit_id: d(work_id),
                class: CausalWorkClassV1::Candidate,
                amount,
            }],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap() else {
            panic!("measured receipt expected")
        };
        receipt
    }

    fn quality_admission() -> QualityAdmissionV1 {
        let pair = crate::QualityPairV1::new(
            d(20),
            d(21),
            d(22),
            d(26),
            d(140),
            d(141),
            d(142),
            d(143),
            vec![crate::ProtectedMetricV1 {
                metric_id: "functional".into(),
                order: crate::MetricOrderV1::AtLeast,
                baseline_value: 0,
                candidate_value: 1,
            }],
        )
        .unwrap();
        let (certificate, resident) = evidence(&pair.canonical_bytes().unwrap());
        let verified = verify(&certificate, &resident).unwrap();
        let dominance =
            crate::PointwiseDominanceCertificateV1::verify(&pair, d(144), &verified).unwrap();
        crate::QualityAdmissionV1::admit_strict(
            crate::QualityEvidenceV1::PointwiseDominance(dominance),
            crate::FrozenBaselineV1::new(d(22), d(140), d(28)).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn portfolio_reserves_fallback_and_computes_coordinatewise_slack() {
        let (_, boundary) = closed(d(24));
        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::SameModelSecondCandidate,
            vector(40, 20),
        )
        .unwrap();
        let plan = plan(
            action,
            vector(100, 50),
            vector(0, 0),
            vector(20, 10),
            vector(30, 10),
        )
        .unwrap();
        assert_eq!(plan.record().causal_slack, vector(50, 30));
        assert_eq!(
            plan.record().cost_position,
            ReinvestmentCostPositionV1::WithinRawBaseline
        );
        assert!(!plan.permits_publication());
        let bytes = plan.record().canonical_bytes().unwrap();
        let value = serde_json::to_value(plan.record()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "contract_version",
                "scope_digest",
                "comparison_identity_digest",
                "raw_baseline_identity_digest",
                "raw_baseline_receipt_digest",
                "assembly_manifest_digest",
                "baseline_state_digest",
                "baseline_reasoning_contract_digest",
                "baseline_budget",
                "declared_additional_budget",
                "strict_candidate_guarded_bound",
                "fallback_reserve",
                "causal_slack",
                "actions",
                "cost_position",
                "plan_digest",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            ReinvestmentPlanRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *plan.record()
        );
        let mut whitespace = bytes;
        whitespace.push(b'\n');
        assert_eq!(
            ReinvestmentPlanRecordV1::from_canonical_bytes(&whitespace)
                .err()
                .unwrap()
                .failure_code(),
            ReinvestmentFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn extra_budget_is_labeled_and_budget_mutants_fail_closed() {
        let (_, boundary) = closed(d(24));
        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::StrongerVerifier,
            vector(60, 35),
        )
        .unwrap();
        let admitted = plan(
            action,
            vector(100, 50),
            vector(20, 10),
            vector(20, 10),
            vector(30, 10),
        )
        .unwrap();
        assert_eq!(
            admitted.record().cost_position,
            ReinvestmentCostPositionV1::DeclaredAdditionalBudget
        );

        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::StrongerVerifier,
            vector(80, 40),
        )
        .unwrap();
        assert_eq!(
            plan(
                action,
                vector(100, 50),
                vector(10, 10),
                vector(20, 10),
                vector(30, 10),
            )
            .err()
            .unwrap()
            .failure_code(),
            ReinvestmentFailureCodeV1::BudgetExceeded
        );

        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::AdditionalTests,
            vector(10, 5),
        )
        .unwrap();
        assert_eq!(
            plan(
                action,
                vector(100, 50),
                vector(0, 0),
                vector(20, 10),
                vector(0, 0),
            )
            .err()
            .unwrap()
            .failure_code(),
            ReinvestmentFailureCodeV1::MissingFallbackReserve
        );
    }

    #[test]
    fn higher_effort_without_ordered_theorem_is_not_laundered() {
        let (_, boundary) = closed(d(24));
        assert_eq!(
            verified_action(
                &boundary,
                ReinvestmentActionKindV1::HigherReasoningEffort,
                vector(10, 5),
            )
            .err()
            .unwrap()
            .failure_code(),
            ReinvestmentFailureCodeV1::UnsupportedReasoningChange
        );
    }

    #[test]
    fn replay_claim_cannot_forge_fixed_model_reasoning_admission() {
        let (_, boundary) = closed(d(24));
        let reasoning = reasoning_admission();
        let mut claim = ReinvestmentActionClaimV1::new(
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            ReinvestmentActionKindV1::SameModelCritique,
            d(26),
            boundary.action_digest(),
            d(27),
            &reasoning,
            vector(10, 5),
            verifier_identity(),
        )
        .unwrap();
        claim.reasoning_admission_digest = d(200);
        let (certificate, resident) = evidence(&claim.canonical_bytes().unwrap());
        let verified = verify(&certificate, &resident).unwrap();
        assert_eq!(
            verify_reinvestment_action_v1(claim, &reasoning, &verified)
                .err()
                .unwrap()
                .failure_code(),
            ReinvestmentFailureCodeV1::ActionBindingMismatch
        );
    }

    #[test]
    fn measured_isolated_branch_reenters_quality_and_exact_dominance_gate() {
        let (program, boundary) = closed(d(24));
        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::SameModelSecondCandidate,
            vector(40, 20),
        )
        .unwrap();
        let plan = plan(
            action,
            vector(100, 50),
            vector(0, 0),
            vector(20, 10),
            vector(30, 10),
        )
        .unwrap();
        let temp = tempdir().unwrap();
        let paths = journal_paths(temp.path());
        initialize_published_root_v1(&paths, d(24)).unwrap();
        let transaction = begin_effect_transaction_v1(paths, binding(&boundary, d(150)), &boundary)
            .unwrap()
            .commit(&accepted(&program))
            .unwrap();
        let receipts = vec![
            work_receipt(
                counter("parent.cpu_ns", CausalCounterUnitV1::CpuNanoseconds, 80),
                20,
                151,
            ),
            work_receipt(
                counter("parent.tokens", CausalCounterUnitV1::Tokens, 90),
                10,
                152,
            ),
        ];
        let quality = quality_admission();
        let branch =
            complete_reinvestment_branch_v1(&plan, d(25), &transaction, &receipts, &quality)
                .unwrap();
        assert!(branch.is_strictly_improved_candidate());
        let claim = reinvestment_selection_claim_v1(
            &plan,
            &[&branch],
            d(25),
            PortfolioSelectionBasisV1::PairwiseDominant,
            d(153),
            verifier_identity(),
        )
        .unwrap();
        let expected_branch_digest = claim.selected_branch_digest;
        let expected_quality_digest = claim.selected_quality_admission_digest;
        let (wrong_certificate, wrong_resident) = evidence(b"unrelated dominance");
        let wrong_verified = verify(&wrong_certificate, &wrong_resident).unwrap();
        assert_eq!(
            select_reinvestment_winner_v1(&plan, vec![branch], claim.clone(), &wrong_verified)
                .err()
                .unwrap()
                .failure_code(),
            ReinvestmentFailureCodeV1::EvidencePayloadMismatch
        );
        let branch =
            complete_reinvestment_branch_v1(&plan, d(25), &transaction, &receipts, &quality)
                .unwrap();
        let (certificate, resident) = evidence(&claim.canonical_bytes().unwrap());
        let verified = verify(&certificate, &resident).unwrap();
        let selection =
            select_reinvestment_winner_v1(&plan, vec![branch], claim, &verified).unwrap();
        assert_eq!(selection.selected_action_digest(), d(25));
        assert_eq!(selection.selected_branch_digest(), expected_branch_digest);
        assert_eq!(
            selection.selected_quality_admission_digest(),
            expected_quality_digest
        );
        assert!(!selection.permits_publication());
        let bytes = selection.record().canonical_bytes().unwrap();
        let value = serde_json::to_value(selection.record()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "contract_version",
                "claim",
                "claim_digest",
                "evidence_digest",
                "authority_digest"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            ReinvestmentSelectionRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *selection.record()
        );

        let branch =
            complete_reinvestment_branch_v1(&plan, d(25), &transaction, &receipts, &quality)
                .unwrap();
        let baseline = fall_back_reinvestment_portfolio_v1(
            &plan,
            vec![branch],
            ReinvestmentBaselineReasonV1::OperatorSelectedBaseline,
        )
        .unwrap();
        baseline.validate().unwrap();
    }

    #[test]
    fn incomplete_or_overbound_measured_work_fails_closed() {
        let (program, boundary) = closed(d(24));
        let action = verified_action(
            &boundary,
            ReinvestmentActionKindV1::AdditionalTests,
            vector(40, 20),
        )
        .unwrap();
        let plan = plan(
            action,
            vector(100, 50),
            vector(0, 0),
            vector(20, 10),
            vector(30, 10),
        )
        .unwrap();
        let temp = tempdir().unwrap();
        let paths = journal_paths(temp.path());
        initialize_published_root_v1(&paths, d(24)).unwrap();
        let transaction = begin_effect_transaction_v1(paths, binding(&boundary, d(150)), &boundary)
            .unwrap()
            .commit(&accepted(&program))
            .unwrap();
        let quality = quality_admission();
        let only_cpu = vec![work_receipt(
            counter("parent.cpu_ns", CausalCounterUnitV1::CpuNanoseconds, 80),
            20,
            151,
        )];
        assert_eq!(
            complete_reinvestment_branch_v1(&plan, d(25), &transaction, &only_cpu, &quality,)
                .err()
                .unwrap()
                .failure_code(),
            ReinvestmentFailureCodeV1::IncompleteMeasuredWork
        );
        let over = vec![
            work_receipt(
                counter("parent.cpu_ns", CausalCounterUnitV1::CpuNanoseconds, 80),
                41,
                151,
            ),
            work_receipt(
                counter("parent.tokens", CausalCounterUnitV1::Tokens, 90),
                10,
                152,
            ),
        ];
        assert_eq!(
            complete_reinvestment_branch_v1(&plan, d(25), &transaction, &over, &quality,)
                .err()
                .unwrap()
                .failure_code(),
            ReinvestmentFailureCodeV1::WorkBoundExceeded
        );
    }

    #[test]
    fn contract_and_external_schema_digests_are_stable() {
        // d0c24fd011e6b2cc47eedbefb82901533277c5df1fb22a172c366a59a835b81c
        assert_eq!(
            reinvestment_contract_digest_v1(),
            DigestV1::from_bytes([
                0xd0, 0xc2, 0x4f, 0xd0, 0x11, 0xe6, 0xb2, 0xcc, 0x47, 0xee, 0xdb, 0xef, 0xb8, 0x29,
                0x01, 0x53, 0x32, 0x77, 0xc5, 0xdf, 0x1f, 0xb2, 0x2a, 0x17, 0x2c, 0x36, 0x6a, 0x59,
                0xa8, 0x35, 0xb8, 0x1c,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/reinvestment-plan-v1.schema.json"
            )))
            .to_hex(),
            REINVESTMENT_PLAN_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/reinvestment-selection-v1.schema.json"
            )))
            .to_hex(),
            REINVESTMENT_SELECTION_SCHEMA_SHA256_V1
        );
    }
