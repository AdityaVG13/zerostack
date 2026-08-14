    use super::*;
    use crate::quality::{
        DistributionalCertificateV1, DistributionalClaimV1, ExactNeutralCertificateV1,
        FrozenBaselineV1, MetricOrderV1, PointwiseDominanceCertificateV1, ProtectedMetricV1,
        QualityEvidenceV1, QualityPairV1,
    };
    use crate::semantic_cut::{
        ReasoningSafepointV1, ReasoningStateStatusV1, SemanticCutClaimV1, SemanticCutFailureCodeV1,
    };
    use crate::transaction::RestorationScopeV1;
    use std::{borrow::Cow, collections::BTreeMap};
    use zero_abi::{
        sha256, CwirVerifierClassV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
        EffectVerificationPlanV1, EffectVerificationStepV1, NativeStatePolicyV1,
        ProtectedEffectClassV1, ProtectedEffectSet, ProtectedEffectV1, TypedEffectOperationV1,
        WorldFiberDescriptor, ROBUST_SNAP_MODEL_VERSION,
    };
    use zero_cert::{
        accept_effect_verification_v1, verify, CompletenessWitness, EffectVerificationOutcomeV1,
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver, SpanRef, TestId,
    };

    fn digest(byte: u8) -> DigestV1 {
        [byte; 32]
    }

    fn abi(byte: u8) -> AbiDigestV1 {
        AbiDigestV1::from_bytes(digest(byte))
    }

    struct Resident<'a> {
        bytes: &'a [u8],
    }

    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(self.bytes) == object_id.0).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            matches!(id, "read-span" | "semantic-cut-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "tree-sitter").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zero-index").then_some("2")
        }
    }

    fn certificate(bytes: &[u8]) -> EvidenceCertificate<'_> {
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        EvidenceCertificate {
            query: Query::ReadSpan(span.clone()),
            spans: vec![span],
            payload: Cow::Borrowed(bytes),
            provenance: Provenance {
                parser_id: "tree-sitter".into(),
                parser_version: "1".into(),
                index_id: "zero-index".into(),
                index_version: "2".into(),
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::ReadSpan {
                operator: OperatorLock {
                    operator_id: "read-span".into(),
                    operator_version: "1".into(),
                },
            },
            input_token_cost: 1,
            backend_work_units: 1,
        }
    }

    fn semantic_certificate(bytes: &[u8]) -> EvidenceCertificate<'_> {
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        EvidenceCertificate {
            query: Query::TestTrace { test: TestId(74) },
            spans: vec![span],
            payload: Cow::Borrowed(bytes),
            provenance: Provenance {
                parser_id: "tree-sitter".into(),
                parser_version: "1".into(),
                index_id: "zero-index".into(),
                index_version: "2".into(),
                operator_id: "semantic-cut-verifier".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::TestTrace {
                operator: OperatorLock {
                    operator_id: "semantic-cut-verifier".into(),
                    operator_version: "1".into(),
                },
                test: TestId(74),
                exit_code: 0,
                trace_digest: object,
            },
            input_token_cost: 1,
            backend_work_units: 1,
        }
    }

    fn effect_program(snapshot: DigestV1) -> EffectProgramV1 {
        let snapshot = AbiDigestV1::from_bytes(snapshot);
        let target = EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: abi(10),
            required_snapshot: snapshot,
        };
        EffectProgramV1::new(
            snapshot,
            "kernel_test",
            vec![target],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: abi(10),
                expected_before: abi(11),
                replacement: abi(12),
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
                verifier_digest: abi(20),
                predicate_digest: abi(21),
                environment_digest: abi(22),
                required_snapshot: snapshot,
                verifier_class: CwirVerifierClassV1::ExactChecker,
            }])
            .unwrap(),
            EffectRollbackV1::Journaled,
        )
        .unwrap()
    }

    fn accepted_effect() -> EffectAcceptedV1 {
        let bytes = b"exact kernel evidence";
        let certificate = certificate(bytes);
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        let program = effect_program(digest(13));
        let outcome = accept_effect_verification_v1(
            abi(70),
            &program,
            abi(71),
            abi(21),
            abi(13),
            abi(20),
            &verified,
        )
        .unwrap();
        let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
            panic!("expected accepted effect")
        };
        accepted
    }

    fn read_only_shield() -> SafetyShieldEvidenceV1 {
        let bytes = b"verified read-only kernel evidence";
        let certificate = certificate(bytes);
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        SafetyShieldEvidenceV1::from_read_only_verified(digest(13), digest(72), &verified).unwrap()
    }

    fn reasoning_contract() -> ReasoningContractV1 {
        ReasoningContractV1::new(
            abi(15),
            abi(74),
            abi(75),
            abi(76),
            abi(77),
            "enabled",
            "high",
            8_192,
            4_096,
            2_048,
            1_024,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn semantic_claim(
        plan_digest: DigestV1,
        reasoning_contract_digest: DigestV1,
    ) -> SemanticCutClaimV1 {
        let terminal = |receipt| {
            ReasoningSafepointV1::new(
                digest(30),
                digest(31),
                digest(32),
                reasoning_contract_digest,
                digest(15),
                digest(33),
                ReasoningStateStatusV1::ExactPreserved,
                digest(34),
                digest(35),
                digest(36),
                digest(37),
                digest(receipt),
            )
            .unwrap()
        };
        SemanticCutClaimV1::new_exact(
            digest(13),
            digest(38),
            plan_digest,
            terminal(39),
            terminal(40),
            digest(41),
            digest(41),
            digest(42),
            digest(42),
            digest(43),
            digest(44),
            digest(4),
            digest(14),
            digest(45),
        )
        .unwrap()
    }

    fn semantic_cut(
        plan_digest: DigestV1,
        reasoning_contract_digest: DigestV1,
    ) -> SemanticCutEvidenceV1 {
        let claim = semantic_claim(plan_digest, reasoning_contract_digest);
        let bytes = claim.canonical_bytes().unwrap();
        let certificate = semantic_certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified = verify(&certificate, &resident).unwrap();
        SemanticCutEvidenceV1::verify_owner_scoped(claim, &verified).unwrap()
    }

    fn artifacts() -> CanonicalArtifactSetV1 {
        let assembly = abi(1);
        let source = abi(2);
        let profile = DurableProfileV1::portable_strict();
        let specifications = [
            (ArtifactOwnerV1::FsZero, ZbfArtifactKindV1::FsPack, 31),
            (ArtifactOwnerV1::GraphZero, ZbfArtifactKindV1::GraphPack, 32),
            (ArtifactOwnerV1::TokenZero, ZbfArtifactKindV1::TokenPack, 33),
        ];
        let inputs = specifications
            .into_iter()
            .map(|(owner, kind, producer)| PeerArtifactInputV1 {
                bytes: ZbfObjectV1::new_leaf(
                    kind,
                    owner,
                    assembly,
                    profile,
                    source,
                    abi(producer),
                    vec![producer],
                )
                .unwrap()
                .to_bytes(profile)
                .unwrap(),
                expected_owner: owner,
                expected_kind: kind,
                expected_producer_contract_digest: digest(producer),
            })
            .collect();
        CanonicalArtifactSetV1::verify(digest(1), digest(2), inputs).unwrap()
    }

    fn plan(effect_class: EffectClass) -> ControllerPlan {
        let mut instructions = vec![
            ControllerInstruction::Dispatch {
                owner: PeerOwner::FsZero,
            },
            ControllerInstruction::DeterministicTransform,
            ControllerInstruction::Verify,
        ];
        if effect_class != EffectClass::ReadOnly {
            instructions.push(ControllerInstruction::StageEffect);
        }
        instructions.push(ControllerInstruction::BufferVisible);
        instructions.push(ControllerInstruction::CloseTransaction);
        ControllerPlan { instructions }
    }

    fn quality_admission(candidate_identity: AbiDigestV1) -> PerformanceAdmission {
        let certificate = ExactNeutralCertificateV1::verify(
            abi(14),
            abi(4),
            abi(16),
            candidate_identity,
            abi(17),
            abi(17),
            abi(18),
            abi(18),
            abi(19),
            abi(19),
        )
        .unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn pointwise_quality_admission(candidate_identity: AbiDigestV1) -> PerformanceAdmission {
        let pair = QualityPairV1::new(
            abi(14),
            abi(4),
            abi(16),
            candidate_identity,
            abi(19),
            abi(21),
            abi(22),
            abi(26),
            vec![ProtectedMetricV1 {
                metric_id: "protected_outcome".into(),
                order: MetricOrderV1::AtLeast,
                baseline_value: 1,
                candidate_value: 2,
            }],
        )
        .unwrap();
        let bytes = pair.canonical_bytes().unwrap();
        let evidence_certificate = certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resident).unwrap();
        let dominance = PointwiseDominanceCertificateV1::verify(&pair, abi(23), &verified).unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::PointwiseDominance(dominance),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn distributional_quality_admission() -> PerformanceAdmission {
        let claim = DistributionalClaimV1::new(
            abi(24),
            abi(4),
            abi(25),
            abi(16),
            abi(19),
            abi(22),
            abi(26),
            abi(27),
            100,
            10,
            2,
            88,
            80_000,
            50_000,
            950_000,
        )
        .unwrap();
        let bytes = claim.canonical_bytes().unwrap();
        let evidence_certificate = certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resident).unwrap();
        let distributional = DistributionalCertificateV1::verify(&claim, &verified).unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::Distributional(distributional),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap()
    }

    fn request(surface: ExecutionSurface, effect_class: EffectClass) -> PrepareRequest {
        let plan = plan(effect_class);
        let plan_digest = plan.digest();
        let artifacts = artifacts();
        let image_digest = artifacts.image_digest;
        let safety_shield = if effect_class == EffectClass::ReadOnly {
            read_only_shield()
        } else {
            SafetyShieldEvidenceV1::from_effect_accepted(accepted_effect()).unwrap()
        };
        let baseline_reasoning = reasoning_contract();
        let candidate_reasoning = baseline_reasoning.clone();
        let reasoning_admission =
            zero_abi::verify_strict_no_downshift_v1(&baseline_reasoning, &candidate_reasoning)
                .unwrap();
        let baseline_reasoning_contract_digest =
            *baseline_reasoning.identity_digest().unwrap().as_bytes();
        let reasoning_contract_digest = *candidate_reasoning.identity_digest().unwrap().as_bytes();
        let semantic_cut = semantic_cut(plan_digest, reasoning_contract_digest);
        let binding = ExecutionBinding {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            assembly_manifest_digest: digest(1),
            source_tree_digest: digest(2),
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest,
            state_snapshot_digest: digest(13),
            task_fingerprint_digest: digest(14),
            plan_digest,
            fixed_model_digest: digest(15),
            baseline_reasoning_contract: baseline_reasoning,
            reasoning_contract: candidate_reasoning,
            baseline_reasoning_contract_digest,
            reasoning_contract_digest,
            comparison_identity_digest: digest(4),
            semantic_cut_verifier_identity_digest: semantic_cut.verifier_identity_digest(),
            predecessor_receipt_head: digest(5),
        };
        let candidate_identity = AbiDigestV1::from_bytes(candidate_protocol_identity_v1(&binding));
        PrepareRequest {
            binding,
            surface,
            effect_class,
            plan,
            envelope: WorkerEnvelope {
                fuel: 100,
                deadline_ms: 100,
                io_bytes: 100,
                output_bytes: 32,
                memory_bytes: 1_024,
                processes: 1,
                risk_units: 10,
                worker_steps: 8,
            },
            evidence: GuardEvidence {
                artifacts,
                reasoning_admission,
                semantic_cut,
                snap: SnapEvidence::NotClaimed,
                safety_shield,
                approval_grant_digest: (effect_class == EffectClass::ApprovalRequiredMutation)
                    .then(|| digest(12)),
                irreversible_pre_action_evidence_digest: if effect_class
                    == EffectClass::Irreversible
                {
                    Some(digest(8))
                } else {
                    None
                },
                performance: quality_admission(candidate_identity),
            },
            expiry_deadline_ms: 4_102_444_800_000, // year 2100: live for all fixtures
            epoch: 1,
            caller_session_id: "fixture-session".into(),
        }
    }

    fn snap_certificate(effect_digest: DigestV1, image_digest: DigestV1) -> RobustSnapCertificate {
        let selected = ProtectedEffectV1 {
            effect_digest: AbiDigestV1::from_bytes(effect_digest),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        };
        let worlds = vec![abi(40), abi(41)];
        RobustSnapCertificate::create_s0(
            WorldFiberDescriptor {
                model_version: ROBUST_SNAP_MODEL_VERSION.into(),
                assembly_manifest_digest: abi(1),
                source_image_digest: AbiDigestV1::from_bytes(image_digest),
                task_fingerprint: abi(14),
                assumptions: vec!["fixed inputs".into()],
                worlds: worlds.clone(),
            },
            worlds
                .into_iter()
                .map(|world_id| ProtectedEffectSet {
                    world_id,
                    effects: vec![selected.clone()],
                })
                .collect(),
            vec![selected.clone()],
            vec![selected.clone()],
            selected,
        )
        .unwrap()
    }

    fn action_and_acceptance() -> (DigestV1, DigestV1) {
        let accepted = accepted_effect();
        (
            *accepted.action_digest().as_bytes(),
            *accepted.acceptance_digest().as_bytes(),
        )
    }

    fn commit_closure() -> TransactionClosure {
        let (action_digest, acceptance_digest) = action_and_acceptance();
        TransactionClosure {
            kind: ClosureKind::Commit,
            root: digest(11),
            transaction_receipt_digest: digest(17),
            deoptimization_execution_receipt_digest: None,
            deoptimization_kernel_binding_digest: None,
            deoptimization_kernel_admission_digest: None,
            action_digest,
            acceptance_digest: Some(acceptance_digest),
            baseline_state: digest(13),
            candidate_state: digest(11),
            restoration_scope: RestorationScopeV1::NotApplicableCandidateCommit,
            external_restoration_debt_count: 0,
            restoration: RestorationAccounting::default(),
        }
    }

    fn fallback_closure(request: &PrepareRequest) -> TransactionClosure {
        TransactionClosure {
            kind: ClosureKind::Fallback,
            root: digest(13),
            transaction_receipt_digest: digest(18),
            deoptimization_execution_receipt_digest: Some(digest(20)),
            deoptimization_kernel_binding_digest: Some(request.binding.digest()),
            deoptimization_kernel_admission_digest: Some(request.admission_digest()),
            action_digest: digest(19),
            acceptance_digest: None,
            baseline_state: digest(13),
            candidate_state: digest(11),
            restoration_scope: RestorationScopeV1::DeclaredEffectClosure,
            external_restoration_debt_count: 0,
            restoration: RestorationAccounting {
                attempted: 1,
                completed: 1,
                debt: 0,
            },
        }
    }

    fn staged(effect_class: EffectClass) -> StagedEffect {
        let (effect_digest, acceptance_digest) = action_and_acceptance();
        StagedEffect {
            effect_digest,
            effect_class,
            acceptance_digest: Some(acceptance_digest),
            approval_grant_digest: (effect_class == EffectClass::ApprovalRequiredMutation)
                .then(|| digest(12)),
            pre_action_evidence_digest: (effect_class == EffectClass::Irreversible)
                .then(|| digest(8)),
        }
    }

    fn execute_request(
        request: PrepareRequest,
        closure: TransactionClosure,
    ) -> Result<ReadyToFinalize, KernelError> {
        let permit = prepare(request).map_err(|failure| failure.into_parts().0)?;
        validate_permit_record(&permit.record())?;
        let mut execution = permit.start().unwrap();
        execution.dispatch(
            PeerOwner::FsZero,
            ResourceUsage {
                fuel: 10,
                elapsed_ms: 4,
                io_bytes: 8,
                memory_bytes: 64,
                processes: 1,
                risk_units: 1,
                worker_steps: 1,
            },
        )?;
        execution.deterministic_transform()?;
        execution.record_verification(digest(9))?;
        execution.stage_effect(staged(EffectClass::ReversibleMutation))?;
        assert_eq!(
            execution.reject_early_publish().code,
            FailureCode::EarlyVisibleByte
        );
        execution.buffer_visible(b"accepted")?;
        execution.close_transaction(closure)
    }

    fn run_to_ready(surface: ExecutionSurface) -> ReadyToFinalize {
        execute_request(
            request(surface, EffectClass::ReversibleMutation),
            commit_closure(),
        )
        .unwrap()
    }

    #[test]
    fn state_machine_contract_digest_is_stable() {
        assert_eq!(
            two_phase_contract_digest_v2(),
            [
                0xe8, 0x4b, 0x6e, 0xe8, 0x08, 0x63, 0x79, 0xc6, 0x37, 0xcf, 0x50, 0x65, 0x1e, 0x16,
                0x6a, 0x2e, 0xca, 0x62, 0x58, 0xfc, 0x49, 0xaa, 0xdb, 0x40, 0x52, 0x69, 0x68, 0xf7,
                0xa5, 0x30, 0xa6, 0x87,
            ]
        );
        assert_eq!(
            two_phase_contract_digest_v3(),
            [
                0x12, 0x18, 0x25, 0xd4, 0x3e, 0xee, 0x2a, 0xbc, 0xe2, 0x6a, 0x88, 0x6b, 0x67, 0xd5,
                0xde, 0xf6, 0x44, 0x03, 0x74, 0xc3, 0x98, 0xe8, 0x4b, 0x77, 0x4d, 0x77, 0x28, 0xd0,
                0x32, 0x13, 0x99, 0x34
            ]
        );
        // 10061c07954c7cad6347e822e18e2253bd193d497b2e67c9273bcfb977cd1189
        assert_eq!(
            two_phase_contract_digest_v4(),
            [
                0x10, 0x06, 0x1c, 0x07, 0x95, 0x4c, 0x7c, 0xad, 0x63, 0x47, 0xe8, 0x22, 0xe1, 0x8e,
                0x22, 0x53, 0xbd, 0x19, 0x3d, 0x49, 0x7b, 0x2e, 0x67, 0xc9, 0x27, 0x3b, 0xcf, 0xb9,
                0x77, 0xcd, 0x11, 0x89,
            ]
        );
        // v6 schema: permit lease (expiry/epoch/caller_session_id) entered the
        // admission and permit digests; contract digest re-pinned.
        // 11abf18e9a440007b80b34e068bdeaccea85ddcfa4f9241a883f8600206c4139
        assert_eq!(
            two_phase_contract_digest_v5(),
            [
                0xdb, 0x22, 0x96, 0xe6, 0xde, 0x0d, 0xde, 0xde, 0xbd, 0x63, 0x5c, 0xdb, 0x02, 0x5d,
                0x4b, 0x1b, 0xfb, 0xec, 0xb1, 0x90, 0x0d, 0x3a, 0xd7, 0x98, 0x7f, 0x87, 0xfa, 0x18,
                0xee, 0x26, 0xe3, 0x7e,
            ]
        );
    }

    #[test]
    fn state_machine_quality_envelope_guards_candidate_and_distributional_fallback() {
        let mut pointwise = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        let candidate_identity =
            AbiDigestV1::from_bytes(candidate_protocol_identity_v1(&pointwise.binding));
        pointwise.evidence.performance = pointwise_quality_admission(candidate_identity);
        let FinalReceipt::Commit(receipt) = execute_request(pointwise, commit_closure())
            .unwrap()
            .finalize()
            .unwrap()
        else {
            panic!("pointwise candidate must commit")
        };
        let record = receipt.record();
        assert_eq!(
            record.quality_admission.evidence_class,
            QualityEvidenceClassV1::PointwiseDominance
        );
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.quality_admission.guarantee,
            QualityGuaranteeV1::PointwiseNoWorse
        );
        assert!(record.quality_admission.strict_improvement);
        validate_receipt_record(&record).unwrap();

        let mut distributional = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        distributional.evidence.performance = distributional_quality_admission();
        let fallback = fallback_closure(&distributional);
        let FinalReceipt::Fallback(receipt) = execute_request(distributional, fallback)
            .unwrap()
            .finalize()
            .unwrap()
        else {
            panic!("distributional evidence must select the frozen baseline")
        };
        let record = receipt.record();
        assert_eq!(
            record.quality_admission.evidence_class,
            QualityEvidenceClassV1::Distributional
        );
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::FrozenBaseline
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::FrozenBaseline
        );
        assert_eq!(
            record.quality_admission.guarantee,
            QualityGuaranteeV1::DistributionalOnly
        );
        assert!(!record.quality_admission.strict_improvement);
        validate_receipt_record(&record).unwrap();

        let mut candidate_mismatch =
            request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        candidate_mismatch.evidence.performance = quality_admission(abi(99));
        assert_eq!(
            prepare(candidate_mismatch).unwrap_err().error().code,
            FailureCode::PerformanceUnknown
        );

        let mismatched = ExactNeutralCertificateV1::verify(
            abi(14),
            abi(99),
            abi(16),
            abi(28),
            abi(17),
            abi(17),
            abi(18),
            abi(18),
            abi(19),
            abi(19),
        )
        .unwrap();
        let mut request = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        request.evidence.performance = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(mismatched),
            FrozenBaselineV1::new(abi(16), abi(19), abi(20)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            prepare(request).unwrap_err().error().code,
            FailureCode::PerformanceUnknown
        );
    }

    #[test]
    fn state_machine_prepare_execute_finalize_is_complete_and_linear() {
        let ready = run_to_ready(ExecutionSurface::Mcp);
        let FinalReceipt::Commit(receipt) = ready.finalize().unwrap() else {
            panic!("expected commit")
        };
        receipt.trace().verify_complete().unwrap();
        assert_eq!(
            receipt
                .trace()
                .events()
                .iter()
                .map(|event| event.guard)
                .collect::<Vec<_>>(),
            Guard::ALL
        );
        let record = receipt.record();
        assert_eq!(record.assembly_manifest_digest, digest(1));
        assert_eq!(record.state_snapshot_digest, digest(13));
        assert_eq!(record.predecessor_receipt_head, digest(5));
        assert_eq!(record.successor_root, digest(11));
        assert_eq!(record.transaction_receipt_digest, digest(17));
        validate_receipt_record(&record).unwrap();
        let published = receipt.publish();
        assert_eq!(
            published.durability,
            PublicationDurabilityV1::JournalRootCommitted {
                transaction_receipt_digest: digest(17)
            }
        );
        assert_eq!(published.visible_bytes, b"accepted");
        assert_eq!(published.approved_effects.len(), 1);
    }

    #[test]
    fn state_machine_all_surfaces_have_identical_guard_semantics() {
        for surface in [
            ExecutionSurface::Mcp,
            ExecutionSurface::Cli,
            ExecutionSurface::ClaudeCode,
            ExecutionSurface::Pi,
        ] {
            let FinalReceipt::Commit(receipt) = run_to_ready(surface).finalize().unwrap() else {
                panic!("expected commit")
            };
            assert_eq!(receipt.record().surface, surface);
            receipt.trace().verify_complete().unwrap();
        }
    }

    #[test]
    fn state_machine_strict_artifacts_omitted_guards_and_forged_predecessors_fail() {
        let malformed = vec![PeerArtifactInputV1 {
            bytes: b"not-zbf".to_vec(),
            expected_owner: ArtifactOwnerV1::FsZero,
            expected_kind: ZbfArtifactKindV1::FsPack,
            expected_producer_contract_digest: digest(31),
        }];
        assert_eq!(
            CanonicalArtifactSetV1::verify(digest(1), digest(2), malformed)
                .unwrap_err()
                .code,
            FailureCode::InvalidSourceIdentity
        );

        let FinalReceipt::Commit(receipt) = run_to_ready(ExecutionSurface::Cli).finalize().unwrap()
        else {
            panic!("expected commit")
        };
        let complete = receipt.trace().clone();
        for index in 0..GUARD_COUNT {
            let mut mutant = complete.clone();
            mutant.events.remove(index);
            assert!(matches!(
                mutant.verify_complete().unwrap_err().code,
                FailureCode::IncompleteTrace | FailureCode::ForgedPredecessor
            ));
        }
        let mut mutant = complete;
        mutant.events[8].predecessor = Some(Guard::G6SafetyShield);
        assert_eq!(
            mutant.verify_complete().unwrap_err().code,
            FailureCode::ForgedPredecessor
        );
    }

    #[test]
    fn state_machine_forged_permit_unbounded_worker_semantic_cut_and_image_fail() {
        let contract = reasoning_contract();
        let reasoning_digest = *contract.identity_digest().unwrap().as_bytes();
        let claim = semantic_claim(plan(EffectClass::ReadOnly).digest(), reasoning_digest);
        let bytes = claim.canonical_bytes().unwrap();
        let read_certificate = certificate(&bytes);
        let resident = Resident { bytes: &bytes };
        let verified_read = verify(&read_certificate, &resident).unwrap();
        assert_eq!(
            SemanticCutEvidenceV1::verify_owner_scoped(claim, &verified_read)
                .unwrap_err()
                .failure_code(),
            SemanticCutFailureCodeV1::UnsupportedEvidenceClass
        );

        let permit = prepare(request(ExecutionSurface::Pi, EffectClass::ReadOnly)).unwrap();
        let mut record = permit.record();
        record.permit_id[0] ^= 1;
        assert_eq!(
            validate_permit_record(&record).unwrap_err().code,
            FailureCode::ForgedPermit
        );
        let mut unbounded = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        unbounded.envelope.fuel = 0;
        assert_eq!(
            prepare(unbounded).unwrap_err().error().code,
            FailureCode::UnboundedWorker
        );
        let mut reasoning = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        reasoning.binding.baseline_reasoning_contract_digest[0] ^= 1;
        assert_eq!(
            prepare(reasoning).unwrap_err().error().code,
            FailureCode::ReasoningContractMismatch
        );
        let mut cut = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        cut.binding.semantic_cut_verifier_identity_digest = digest(99);
        assert_eq!(
            prepare(cut).unwrap_err().error().code,
            FailureCode::SemanticCutCrossing
        );
        let mut image = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        image.binding.image_digest = digest(99);
        assert_eq!(
            prepare(image).unwrap_err().error().code,
            FailureCode::CoherenceFailure
        );
        let mut snap = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        snap.evidence.snap = SnapEvidence::Verified {
            certificate: snap_certificate(digest(99), snap.binding.image_digest),
        };
        assert_eq!(
            prepare(snap).unwrap_err().error().code,
            FailureCode::MissingSnapCertificate
        );
        let mut snap = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        let action = snap.evidence.safety_shield.action_digest.unwrap();
        snap.evidence.snap = SnapEvidence::Verified {
            certificate: snap_certificate(action, snap.binding.image_digest),
        };
        prepare(snap).unwrap();
        let mut order = request(ExecutionSurface::Pi, EffectClass::ReversibleMutation);
        order.plan.instructions.swap(2, 3);
        order.binding.plan_digest = order.plan.digest();
        assert_eq!(
            prepare(order).unwrap_err().error().code,
            FailureCode::InvalidPlan
        );
    }

    #[test]
    fn state_machine_rejects_cross_execution_deoptimization_receipt_replay() {
        let bound = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        let closure = fallback_closure(&bound);
        let other = request(ExecutionSurface::Pi, EffectClass::ReadOnly);
        let execution = prepare(other).unwrap().start().unwrap();
        assert_eq!(
            execution
                .abort(FailureCode::PerformanceUnknown, closure)
                .unwrap_err()
                .code,
            FailureCode::UnaccountedFallback
        );
    }

    #[test]
    fn state_machine_buffer_overflow_requires_verified_baseline_execution() {
        let bound_request = request(ExecutionSurface::ClaudeCode, EffectClass::ReadOnly);
        let mut bad = fallback_closure(&bound_request);
        let permit = prepare(bound_request).unwrap();
        let mut execution = permit.start().unwrap();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let error = execution.buffer_visible(&[0; 33]).unwrap_err();
        assert_eq!(error.code, FailureCode::BufferOverflow);
        bad.deoptimization_execution_receipt_digest = Some([0; 32]);
        assert_eq!(
            execution.abort(error.code, bad).unwrap_err().code,
            FailureCode::IncompleteTransactionClosure
        );

        let request = request(ExecutionSurface::ClaudeCode, EffectClass::ReadOnly);
        let fallback = fallback_closure(&request);
        let permit = prepare(request).unwrap();
        let execution = permit.start().unwrap();
        let ready = execution
            .abort(FailureCode::BufferOverflow, fallback)
            .unwrap();
        let FinalReceipt::Fallback(receipt) = ready.finalize().unwrap() else {
            panic!("expected fallback")
        };
        receipt.trace().verify_complete().unwrap();
        let record = receipt.record();
        assert_eq!(record.failure_code, Some(FailureCode::BufferOverflow));
        assert_eq!(record.successor_root, digest(13));
        assert_eq!(
            record.quality_admission.selection,
            QualitySelectionV1::Candidate
        );
        assert_eq!(
            record.final_quality_selection,
            QualitySelectionV1::FrozenBaseline
        );
        validate_receipt_record(&record).unwrap();
    }

    #[test]
    fn state_machine_effects_require_matching_acceptance_and_pre_action_evidence() {
        let permit = prepare(request(ExecutionSurface::Mcp, EffectClass::Irreversible)).unwrap();
        let mut execution = permit.start().unwrap();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let mut effect = staged(EffectClass::Irreversible);
        effect.pre_action_evidence_digest = Some(digest(99));
        assert_eq!(
            execution.stage_effect(effect).unwrap_err().code,
            FailureCode::IrreversiblePreEvidenceEffect
        );

        let permit = prepare(request(
            ExecutionSurface::Mcp,
            EffectClass::ApprovalRequiredMutation,
        ))
        .unwrap();
        let mut execution = permit.start().unwrap();
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
        execution.deterministic_transform().unwrap();
        execution.record_verification(digest(9)).unwrap();
        let mut effect = staged(EffectClass::ApprovalRequiredMutation);
        effect.approval_grant_digest = Some(digest(99));
        assert_eq!(
            execution.stage_effect(effect).unwrap_err().code,
            FailureCode::MissingApprovalGrant
        );
    }

    #[test]
    fn state_machine_admission_and_receipt_commitments_reject_tampering() {
        let mut missing = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        missing.binding.predecessor_receipt_head = [0; 32];
        assert_eq!(
            prepare(missing).unwrap_err().error().code,
            FailureCode::MissingBinding
        );

        let base = request(ExecutionSurface::Mcp, EffectClass::ReversibleMutation);
        let base_digest = base.admission_digest();
        let mut changed_envelope = base.clone();
        changed_envelope.envelope.fuel += 1;
        assert_ne!(base_digest, changed_envelope.admission_digest());
        let mut changed_evidence = base.clone();
        changed_evidence.evidence.safety_shield.shield_digest = digest(99);
        assert_ne!(base_digest, changed_evidence.admission_digest());
        assert_eq!(
            prepare(changed_evidence).unwrap_err().error().code,
            FailureCode::MissingSafetyShield
        );

        let permit = prepare(base).unwrap();
        let mut permit_record = permit.record();
        validate_permit_record(&permit_record).unwrap();
        permit_record.admission_digest[0] ^= 1;
        assert_eq!(
            validate_permit_record(&permit_record).unwrap_err().code,
            FailureCode::ForgedPermit
        );

        let FinalReceipt::Commit(receipt) = run_to_ready(ExecutionSurface::Mcp).finalize().unwrap()
        else {
            panic!("expected commit")
        };
        let mut receipt_record = receipt.record();
        validate_receipt_record(&receipt_record).unwrap();
        receipt_record.transaction_receipt_digest[0] ^= 1;
        assert_eq!(
            validate_receipt_record(&receipt_record).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
        let mut reasoning_tamper = receipt.record();
        reasoning_tamper.reasoning_admission.reasoning_tokens_added = 1;
        assert_eq!(
            validate_receipt_record(&reasoning_tamper).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
        let mut semantic_tamper = receipt.record();
        semantic_tamper.semantic_cut.claim_digest[0] ^= 1;
        assert_eq!(
            validate_receipt_record(&semantic_tamper).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
        let mut quality_tamper = receipt.record();
        quality_tamper.quality_admission.selection = QualitySelectionV1::FrozenBaseline;
        assert_eq!(
            validate_receipt_record(&quality_tamper).unwrap_err().code,
            FailureCode::ForgedReceipt
        );
    }

    #[test]
    fn permit_lease_expired_permit_is_refused_and_receipted_at_start() {
        let mut leased = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        leased.expiry_deadline_ms = 1_000;
        leased.epoch = 7;
        leased.caller_session_id = "session:alpha".into();
        let permit = prepare(leased).unwrap();
        let record = permit.record();
        // The lease rides the record and is bound into the permit identity.
        assert_eq!(record.expiry_deadline_ms, 1_000);
        assert_eq!(record.epoch, 7);
        assert_eq!(record.caller_session_id, "session:alpha");
        validate_permit_record(&record).unwrap();

        // Use-time refusal: starting after the deadline fails loudly with a
        // typed receipt (ExpiredPermit) and produces no execution.
        let error = permit.start_at(1_001).unwrap_err();
        assert_eq!(error.code, FailureCode::ExpiredPermit);
        assert!(error.detail.contains("session:alpha"));
        assert!(error.detail.contains("deadline_ms=1000"));
    }

    #[test]
    fn permit_lease_live_permit_passes_start_bound_lease() {
        let mut leased = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        leased.expiry_deadline_ms = 5_000;
        leased.epoch = 3;
        leased.caller_session_id = "session:beta".into();
        let permit = prepare(leased).unwrap();
        let mut execution = permit.start_at(4_000).unwrap();
        assert_eq!(execution.epoch(), 3);
        assert_eq!(execution.caller_session_id(), "session:beta");
        assert_eq!(execution.expiry_deadline_ms(), 5_000);
        execution
            .dispatch(
                PeerOwner::FsZero,
                ResourceUsage {
                    worker_steps: 1,
                    ..ResourceUsage::default()
                },
            )
            .unwrap();
    }

    #[test]
    fn permit_lease_replay_after_expiry_fails_and_lease_tamper_is_forged() {
        // The same prepared permit replays: valid before the deadline, refused
        // after it -- replay-after-expiry can never start an execution.
        let mut leased = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        leased.expiry_deadline_ms = 2_000;
        leased.epoch = 5;
        leased.caller_session_id = "session:gamma".into();
        let first = prepare(leased.clone()).unwrap();
        first.start_at(1_500).unwrap();
        let replay = prepare(leased).unwrap();
        // Lease tampering breaks the digest-bound permit identity.
        let mut tampered = replay.record();
        tampered.expiry_deadline_ms += 1;
        assert_eq!(
            validate_permit_record(&tampered).unwrap_err().code,
            FailureCode::ForgedPermit
        );
        let mut tampered = replay.record();
        tampered.epoch += 1;
        assert_eq!(
            validate_permit_record(&tampered).unwrap_err().code,
            FailureCode::ForgedPermit
        );
        let mut tampered = replay.record();
        let _ = tampered.caller_session_id.push('x');
        assert_eq!(
            validate_permit_record(&tampered).unwrap_err().code,
            FailureCode::ForgedPermit
        );
        // Replay after expiry can never start an execution.
        assert_eq!(
            replay.start_at(2_001).unwrap_err().code,
            FailureCode::ExpiredPermit
        );
    }

    #[test]
    fn permit_lease_without_caller_session_or_epoch_is_refused_at_prepare() {
        let mut no_session = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        no_session.caller_session_id.clear();
        assert_eq!(
            prepare(no_session).unwrap_err().error().code,
            FailureCode::MissingBinding
        );
        let mut no_epoch = request(ExecutionSurface::Mcp, EffectClass::ReadOnly);
        no_epoch.epoch = 0;
        assert_eq!(
            prepare(no_epoch).unwrap_err().error().code,
            FailureCode::MissingBinding
        );
    }
