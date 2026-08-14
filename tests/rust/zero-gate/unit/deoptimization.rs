    use std::{
        borrow::Cow,
        collections::{BTreeMap, BTreeSet},
    };

    use super::*;
    use crate::{
        transaction::{
            EffectClosureManifestV1, EffectClosureRequestV1, EffectResourceClosureV1,
            ResourceIsolationModeV1, ResourceRestorationModeV1, TransactionAccessV1,
            TransactionResourceKindV1, TransactionResourceRequirementV1,
            begin_effect_transaction_v1, effect_journal_binding_v1, validate_effect_closure_v1,
        },
        two_phase::{ClosureKind, TransactionClosure},
    };
    use tempfile::tempdir;
    use zero_abi::{
        ArtifactOwnerV1, CwirVerifierClassV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
        EffectVerificationPlanV1, EffectVerificationStepV1, TypedEffectOperationV1,
    };
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };
    use zero_store::{DurableProfileIdV1, JournalPathsV1, initialize_published_root_v1};

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn verifier_route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "deopt-index",
                "index_version": "1",
                "operator_id": "deopt-verifier",
                "operator_version": "1",
                "parser_id": "deopt-parser",
                "parser_version": "1",
            }),
        )
    }

    fn reasoning_contract(policy: NativeStatePolicyV1) -> ReasoningContractV1 {
        ReasoningContractV1::new(
            d(5),
            d(6),
            d(7),
            d(8),
            d(9),
            "enabled",
            "high",
            4_096,
            2_048,
            512,
            512,
            policy,
            false,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn work_limit() -> RaccWorkV1 {
        RaccWorkV1 {
            logical_input_tokens: 1_000,
            uncached_input_tokens: 900,
            cached_input_tokens: 100,
            reasoning_tokens: 1_000,
            visible_output_tokens: 1_000,
            tool_calls: 100,
            verifier_work: 1_000,
            fallback_work: 10_000,
            latency_micros: 10_000_000,
            peak_memory_bytes: 64 * 1024 * 1024,
        }
    }

    fn envelope() -> WorkerEnvelope {
        WorkerEnvelope {
            fuel: 10_000,
            deadline_ms: 10_000,
            io_bytes: 1_000_000,
            output_bytes: 1_000_000,
            memory_bytes: 64 * 1024 * 1024,
            processes: 4,
            risk_units: 10,
            worker_steps: 1_000,
        }
    }

    fn reserve() -> FallbackReserveV1 {
        FallbackReserveV1 {
            deoptimization_envelope: envelope(),
            raw_baseline_envelope: envelope(),
            deoptimization_work_limit: work_limit(),
            raw_baseline_work_limit: work_limit(),
        }
    }

    fn usage() -> RouteUsageV1 {
        RouteUsageV1 {
            fuel: 10,
            elapsed_ms: 2,
            io_bytes: 100,
            output_bytes: 10,
            memory_bytes: 1_024,
            processes: 1,
            risk_units: 1,
            worker_steps: 2,
            work: RaccWorkV1 {
                logical_input_tokens: 10,
                uncached_input_tokens: 8,
                cached_input_tokens: 2,
                reasoning_tokens: 0,
                visible_output_tokens: 0,
                tool_calls: 1,
                verifier_work: 1,
                fallback_work: 100,
                latency_micros: 2_000,
                peak_memory_bytes: 1_024,
            },
        }
    }

    fn reasoning_safepoint(
        contract: &ReasoningContractV1,
        status: ReasoningStateStatusV1,
        state: DigestV1,
    ) -> ReasoningSafepointV1 {
        ReasoningSafepointV1::new(
            *d(1).as_bytes(),
            *d(2).as_bytes(),
            *d(3).as_bytes(),
            *contract.identity_digest().unwrap().as_bytes(),
            *contract.model_identity().as_bytes(),
            *state.as_bytes(),
            status,
            *d(10).as_bytes(),
            *d(11).as_bytes(),
            *d(12).as_bytes(),
            *d(13).as_bytes(),
            *d(14).as_bytes(),
        )
        .unwrap()
    }

    fn safepoint_claim(external_inventory: DigestV1) -> BaselineSafepointClaimV1 {
        let contract = reasoning_contract(NativeStatePolicyV1::ExactRequired);
        BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            external_inventory,
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactPreserved, d(28)),
            BaselineReasoningEntryV1::ExactNativeContinuation {
                opaque_state_digest: d(28),
                parent_response_digest: d(29),
                session_identity_digest: d(30),
            },
            d(31),
            verifier_route(),
            reserve(),
            d(32),
            d(33),
            d(14),
        )
        .unwrap()
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }

    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "deopt-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "deopt-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "deopt-index").then_some("1")
        }
    }

    fn certificate(bytes: Vec<u8>) -> (EvidenceCertificate<'static>, TestResolver) {
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
                query: Query::TestTrace { test: TestId(9) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "deopt-parser".into(),
                    parser_version: "1".into(),
                    index_id: "deopt-index".into(),
                    index_version: "1".into(),
                    operator_id: "deopt-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "deopt-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(9),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn capture(external_inventory: DigestV1) -> BaselineSafepointEvidenceV1 {
        let claim = safepoint_claim(external_inventory);
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        BaselineSafepointEvidenceV1::verify_owner_scoped(claim, &evidence).unwrap()
    }

    fn effect_program(snapshot: DigestV1) -> EffectProgramV1 {
        EffectProgramV1::new(
            snapshot,
            "deoptimization_test",
            vec![EffectTargetV1 {
                owner: ArtifactOwnerV1::FsZero,
                target_digest: d(40),
                required_snapshot: snapshot,
            }],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: d(40),
                expected_before: d(41),
                replacement: d(42),
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
                verifier_digest: d(43),
                predicate_digest: d(44),
                environment_digest: d(45),
                required_snapshot: snapshot,
                verifier_class: CwirVerifierClassV1::ExactChecker,
            }])
            .unwrap(),
            EffectRollbackV1::Journaled,
        )
        .unwrap()
    }

    fn resource(
        kind: TransactionResourceKindV1,
        scope: u8,
        baseline: DigestV1,
        access: TransactionAccessV1,
    ) -> TransactionResourceRequirementV1 {
        TransactionResourceRequirementV1 {
            owner: if kind == TransactionResourceKindV1::ProjectFilesystem {
                ArtifactOwnerV1::FsZero
            } else {
                ArtifactOwnerV1::ZeroStack
            },
            kind,
            scope_digest: d(scope),
            baseline_state_digest: baseline,
            access,
            authority_digest: d(scope.wrapping_add(1)),
        }
    }

    struct AbortedFixture {
        receipt: TransactionReceiptV1,
        action_digest: DigestV1,
        candidate_state: DigestV1,
        closure_manifest_digest: DigestV1,
        external_inventory_digest: DigestV1,
    }

    fn aborted_fixture(external_debt: bool) -> AbortedFixture {
        let snapshot = d(1);
        let candidate = d(60);
        let program = effect_program(snapshot);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            50,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let external = resource(
            if external_debt {
                TransactionResourceKindV1::ExternalDatabase
            } else {
                TransactionResourceKindV1::Time
            },
            52,
            d(53),
            if external_debt {
                TransactionAccessV1::ReadWrite
            } else {
                TransactionAccessV1::Read
            },
        );
        let request = EffectClosureRequestV1::new(&program, vec![project, external]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
                EffectResourceClosureV1 {
                    requirement: external,
                    isolation: if external_debt {
                        ResourceIsolationModeV1::Transactional
                    } else {
                        ResourceIsolationModeV1::RecordedReplay
                    },
                    restoration: if external_debt {
                        ResourceRestorationModeV1::TransactionRollback
                    } else {
                        ResourceRestorationModeV1::RecordedReplay
                    },
                },
            ],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        let temp = tempdir().unwrap();
        let paths = JournalPathsV1::new(
            temp.path().join("root.json"),
            temp.path().join("journal.json"),
            temp.path().join("cartridge.json"),
            temp.path().join("owner-death.json"),
            temp.path().join("recovery.json"),
        )
        .unwrap();
        initialize_published_root_v1(&paths, snapshot).unwrap();
        let binding = effect_journal_binding_v1(
            &boundary,
            d(61),
            DurableProfileIdV1::PortableStrict,
            candidate,
            d(62),
        )
        .unwrap();
        let receipt = begin_effect_transaction_v1(paths, binding, &boundary)
            .unwrap()
            .abort()
            .unwrap();
        AbortedFixture {
            receipt,
            action_digest: boundary.action_digest(),
            candidate_state: candidate,
            closure_manifest_digest: boundary.manifest_digest(),
            external_inventory_digest: boundary.external_inventory_digest(),
        }
    }

    fn plan(fixture: &AbortedFixture) -> DeoptimizationPlanV1 {
        let plan = DeoptimizationPlanV1::for_fail_closed(
            capture(fixture.external_inventory_digest),
            FailureCode::PerformanceUnknown,
            d(70),
            fixture.action_digest,
            fixture.candidate_state,
            fixture.closure_manifest_digest,
            d(71),
            d(76),
            d(77),
        )
        .unwrap();
        let claim = plan.claim();
        let keys = serde_json::to_value(&claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "schema_version",
            "safepoint_certificate_digest",
            "trigger",
            "candidate_action_digest",
            "candidate_state_digest",
            "candidate_closure_manifest_digest",
            "prior_work_receipt_digest",
            "kernel_binding_digest",
            "kernel_admission_digest",
            "plan_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(keys, expected);
        let bytes = claim.canonical_bytes().unwrap();
        assert_eq!(
            DeoptimizationPlanClaimV1::from_canonical_bytes(&bytes).unwrap(),
            claim
        );
        plan
    }

    #[test]
    fn deoptimization_contract_digest_is_stable() {
        // 9273a6fa5e0a48658e6aea9b2cb893d156d1d16d2b8fe5f9f4163b18b90a0518
        assert_eq!(
            deoptimization_contract_digest_v1(),
            DigestV1::from_bytes([
                0x92, 0x73, 0xa6, 0xfa, 0x5e, 0x0a, 0x48, 0x65, 0x8e, 0x6a, 0xea, 0x9b, 0x2c, 0xb8,
                0x93, 0xd1, 0x56, 0xd1, 0xd1, 0x6d, 0x2b, 0x8f, 0xe5, 0xf9, 0xf4, 0x16, 0x3b, 0x18,
                0xb9, 0x0a, 0x05, 0x18,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/exact-deoptimization-resume-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_RESUME_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/exact-deoptimization-execution-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_EXECUTION_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/exact-deoptimization-plan-v1.schema.json"
            )))
            .to_hex(),
            DEOPTIMIZATION_PLAN_SCHEMA_SHA256_V1
        );
    }

    #[test]
    fn exact_restoration_mints_linear_baseline_invocation_and_g8_closure() {
        let fixture = aborted_fixture(false);
        let plan = plan(&fixture);
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            usage(),
        )
        .unwrap();
        let restoration_keys = serde_json::to_value(&claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_restoration_keys = [
            "schema_version",
            "plan_digest",
            "safepoint_certificate_digest",
            "transaction_receipt_digest",
            "restored_project_root",
            "restored_external_inventory_digest",
            "restored_reasoning_contract_digest",
            "restored_fixed_model_digest",
            "restored_reasoning_entry_digest",
            "raw_baseline_identity_digest",
            "raw_baseline_input_digest",
            "raw_decision_view_digest",
            "candidate_overlay_disposition_digest",
            "visible_buffer_disposition_digest",
            "prior_receipt_head_digest",
            "successor_receipt_head_digest",
            "restoration_verifier_identity_digest",
            "deoptimization_usage",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(restoration_keys, expected_restoration_keys);
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        let permit =
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence)
                .unwrap();
        permit.validate().unwrap();
        let resume_record = permit.record();
        let bytes = resume_record.canonical_bytes().unwrap();
        assert_eq!(
            BaselineResumeReceiptRecordV1::from_canonical_bytes(&bytes).unwrap(),
            resume_record
        );
        let invocation = permit.into_invocation().unwrap();
        invocation.validate().unwrap();
        assert_eq!(invocation.raw_baseline_identity_digest(), d(22));
        assert_eq!(invocation.project_snapshot_root(), d(1));
        let mut overrun = usage();
        overrun.fuel = envelope().fuel + 1;
        assert_eq!(
            BaselineExecutionClaimV1::new(
                &invocation,
                d(80),
                d(81),
                d(83),
                d(84),
                d(85),
                d(86),
                overrun,
                d(82),
                verifier_route(),
            )
            .unwrap_err()
            .failure_code(),
            DeoptimizationFailureCodeV1::ResourceReserveExceeded
        );
        let execution_claim = BaselineExecutionClaimV1::new(
            &invocation,
            d(80),
            d(81),
            d(83),
            d(84),
            d(85),
            d(86),
            usage(),
            d(82),
            verifier_route(),
        )
        .unwrap();
        let keys = serde_json::to_value(&execution_claim)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = [
            "schema_version",
            "invocation_digest",
            "resume_permit_digest",
            "transaction_receipt_digest",
            "project_snapshot_root",
            "raw_baseline_identity_digest",
            "raw_baseline_input_digest",
            "raw_decision_view_digest",
            "baseline_reasoning_contract_digest",
            "reasoning_entry_digest",
            "predecessor_receipt_head_digest",
            "output_digest",
            "effects_digest",
            "baseline_action_digest",
            "baseline_acceptance_digest",
            "baseline_successor_root",
            "baseline_transaction_receipt_digest",
            "raw_baseline_usage",
            "successor_receipt_head_digest",
            "execution_verifier_identity_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(keys, expected);
        let claim_bytes = execution_claim.canonical_bytes().unwrap();
        assert_eq!(
            BaselineExecutionClaimV1::from_canonical_bytes(&claim_bytes).unwrap(),
            execution_claim
        );
        let (certificate, resolver) = self::certificate(claim_bytes);
        let evidence = verify(&certificate, &resolver).unwrap();
        let execution_receipt =
            BaselineExecutionReceiptV1::verify_execution(invocation, execution_claim, &evidence)
                .unwrap();
        let execution_record = execution_receipt.record();
        let bytes = execution_record.canonical_bytes().unwrap();
        assert_eq!(
            BaselineExecutionReceiptRecordV1::from_canonical_bytes(&bytes).unwrap(),
            execution_record
        );
        let mut tampered = execution_record.clone();
        tampered.receipt_digest = d(99);
        assert_eq!(
            tampered.validate().unwrap_err().failure_code(),
            DeoptimizationFailureCodeV1::CertificateDigestMismatch
        );
        let closure = TransactionClosure::from_baseline_execution(execution_receipt).unwrap();
        assert_eq!(closure.kind(), ClosureKind::Fallback);
        assert_eq!(closure.root(), *d(85).as_bytes());
        assert_eq!(closure.transaction_receipt_digest(), *d(86).as_bytes());
    }

    #[test]
    fn bare_fallback_transaction_cannot_enter_g8_without_resume_authority() {
        let fixture = aborted_fixture(false);
        let error = TransactionClosure::from_receipt(fixture.receipt).unwrap_err();
        assert_eq!(error.code, FailureCode::UnaccountedFallback);
    }

    #[test]
    fn journal_root_only_recovery_never_mints_exact_deoptimization() {
        let fixture = aborted_fixture(true);
        assert_eq!(
            fixture.receipt.restoration_scope(),
            RestorationScopeV1::ProjectJournalRootOnly
        );
        let plan = plan(&fixture);
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            usage(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence,)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::TransactionMismatch
        );
    }

    #[test]
    fn stale_reasoning_entry_and_resource_overrun_fail_closed() {
        let contract = reasoning_contract(NativeStatePolicyV1::ExactRequired);
        let bad = BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            d(28),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactCleanRestart, d(29)),
            BaselineReasoningEntryV1::ExactNativeContinuation {
                opaque_state_digest: d(29),
                parent_response_digest: d(30),
                session_identity_digest: d(31),
            },
            d(32),
            verifier_route(),
            reserve(),
            d(33),
            d(34),
            d(14),
        );
        assert_eq!(
            bad.unwrap_err().failure_code(),
            DeoptimizationFailureCodeV1::ReasoningEntryMismatch
        );

        let fixture = aborted_fixture(false);
        let plan = plan(&fixture);
        let mut overrun = usage();
        overrun.fuel = envelope().fuel + 1;
        let claim = BaselineRestorationClaimV1::new(
            &plan,
            fixture.receipt.receipt_digest(),
            d(72),
            d(73),
            d(74),
            verifier_route(),
            overrun,
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            BaselineResumePermitV1::verify_restoration(plan, fixture.receipt, claim, &evidence,)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::ResourceReserveExceeded
        );
    }

    #[test]
    fn clean_start_is_exact_only_when_frozen_as_the_baseline_entry() {
        let contract = reasoning_contract(NativeStatePolicyV1::CleanRestart);
        let claim = BaselineSafepointClaimV1::new(
            d(1),
            d(20),
            d(21),
            d(22),
            d(23),
            d(24),
            d(25),
            d(26),
            d(27),
            d(28),
            contract.clone(),
            reasoning_safepoint(&contract, ReasoningStateStatusV1::ExactCleanRestart, d(29)),
            BaselineReasoningEntryV1::CanonicalCleanStart {
                clean_start_identity_digest: d(29),
            },
            d(30),
            verifier_route(),
            reserve(),
            d(31),
            d(32),
            d(14),
        )
        .unwrap();
        claim.validate().unwrap();
        let mut bytes = claim.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            BaselineSafepointClaimV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            DeoptimizationFailureCodeV1::NonCanonicalEncoding
        );
    }
