    use std::borrow::Cow;

    use super::*;
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };
    use zero_ledger::{
        CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1, ParentCounterIdentityV1,
        ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn verifier_route() -> DigestV1 {
        digest_value(
            VERIFIER_DOMAIN_V1,
            &json!({
                "index_id": "q99-index",
                "index_version": "1",
                "operator_id": "q99-verifier",
                "operator_version": "1",
                "parser_id": "q99-parser",
                "parser_version": "1",
            }),
        )
    }

    struct TestResolver {
        bytes: Vec<u8>,
    }
    impl Resolver for TestResolver {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == sha256(&self.bytes)).then_some(self.bytes.as_slice())
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "q99-index").then_some("1")
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
                query: Query::TestTrace { test: TestId(99) },
                spans: vec![span],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "q99-parser".into(),
                    parser_version: "1".into(),
                    index_id: "q99-index".into(),
                    index_version: "1".into(),
                    operator_id: "q99-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "q99-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(99),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn binding() -> CausalCacheBindingV1 {
        CausalCacheBindingV1 {
            artifact_digest: d(1),
            artifact_owner: ArtifactOwnerV1::FsZero,
            source_root: d(2),
            dependency_root: d(3),
            producer_contract_digest: d(4),
            protected_use_class_digest: d(5),
            reasoning_contract_digest: d(6),
            verifier_scope_digest: d(7),
            invalidation_certificate_digest: d(8),
            recovery_route_digest: d(9),
        }
    }

    fn bound_invalidation() -> BoundCausalCacheInvalidationV1 {
        BoundCausalCacheInvalidationV1::test_only(&binding())
    }

    fn component_specs() -> Vec<(CacheCoordinateV1, ArtifactOwnerV1, CacheValidityV1)> {
        vec![
            (
                CacheCoordinateV1::Source,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Producer,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Graph,
                ArtifactOwnerV1::GraphZero,
                CacheValidityV1::SoundOverapproximation,
            ),
            (
                CacheCoordinateV1::Tokenization,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Rendering,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::ProviderCache,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::ProviderReportedHit { tokens: 123 },
            ),
            (
                CacheCoordinateV1::ReasoningContinuation,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::ExactReasoningContinuation,
            ),
            (
                CacheCoordinateV1::Verifier,
                ArtifactOwnerV1::ZeroStack,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Quality,
                ArtifactOwnerV1::ZeroStack,
                CacheValidityV1::Exact,
            ),
        ]
    }

    fn verified_components(
        override_status: Option<(CacheCoordinateV1, CacheValidityV1)>,
    ) -> Vec<VerifiedCausalCacheComponentV1> {
        component_specs()
            .into_iter()
            .enumerate()
            .map(|(index, (coordinate, owner, mut validity))| {
                if let Some((target, status)) = &override_status
                    && *target == coordinate
                {
                    validity = status.clone();
                }
                let claim = CausalCacheComponentClaimV1::new(
                    binding(),
                    coordinate,
                    owner,
                    validity,
                    d(20 + index as u8),
                    verifier_route(),
                )
                .unwrap();
                let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
                let evidence = verify(&certificate, &resolver).unwrap();
                VerifiedCausalCacheComponentV1::verify(claim, &evidence).unwrap()
            })
            .collect()
    }

    #[test]
    fn aggregate_cache_validation_keeps_semantics_telemetry_and_reasoning_distinct() {
        let CausalCacheDecisionV1::StrictReuse(admission) =
            validate_causal_cache_v1(verified_components(None), &bound_invalidation()).unwrap()
        else {
            panic!("complete exact coordinates must admit strict reuse")
        };
        assert_eq!(admission.record().contract_version, Q99_CONTRACT_VERSION_V2);
        assert_eq!(admission.record().provider_reported_hit_tokens, Some(123));
        assert!(admission.record().provider_eligible);
        assert!(admission.record().exact_reasoning_continuation);
        let component_value =
            serde_json::to_value(&admission.record().components[0].claim).unwrap();
        let component_keys = component_value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_component_keys = [
            "schema_version",
            "binding",
            "coordinate",
            "owner",
            "validity",
            "component_receipt_digest",
            "verifier_identity_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(component_keys, expected_component_keys);
        assert_eq!(component_value["binding"]["artifact_owner"], "fs_zero");
        admission.record().validate().unwrap();
        let bytes = admission.record().canonical_bytes().unwrap();
        assert_eq!(
            CausalCacheAssessmentRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *admission.record()
        );

        let CausalCacheDecisionV1::TelemetryOnly(prefix) = validate_causal_cache_v1(
            verified_components(Some((
                CacheCoordinateV1::Rendering,
                CacheValidityV1::ByteIdenticalPrefix,
            ))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("prefix reuse is telemetry, not exact semantic reuse")
        };
        assert_eq!(prefix.admission_class, CacheAdmissionClassV1::TelemetryOnly);

        let CausalCacheDecisionV1::ReuseProhibited(unknown) = validate_causal_cache_v1(
            verified_components(Some((CacheCoordinateV1::Quality, CacheValidityV1::Unknown))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("Unknown must fail closed")
        };
        assert_eq!(
            unknown.admission_class,
            CacheAdmissionClassV1::ReuseProhibited
        );
    }

    #[test]
    fn provider_eligibility_is_never_reported_as_a_hit_or_semantic_proof() {
        let CausalCacheDecisionV1::StrictReuse(admission) = validate_causal_cache_v1(
            verified_components(Some((
                CacheCoordinateV1::ProviderCache,
                CacheValidityV1::ProviderEligible,
            ))),
            &bound_invalidation(),
        )
        .unwrap() else {
            panic!("provider eligibility is orthogonal to semantic coordinates")
        };
        assert!(admission.record().provider_eligible);
        assert_eq!(admission.record().provider_reported_hit_tokens, None);
        assert!(
            CausalCacheComponentClaimV1::new(
                binding(),
                CacheCoordinateV1::Source,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::ProviderEligible,
                d(33),
                verifier_route(),
            )
            .is_err()
        );
        assert!(
            CausalCacheComponentClaimV1::new(
                binding(),
                CacheCoordinateV1::ReasoningContinuation,
                ArtifactOwnerV1::TokenZero,
                CacheValidityV1::Exact,
                d(34),
                verifier_route(),
            )
            .is_err()
        );
    }

    #[test]
    fn cache_authority_rejects_missing_components_and_unmatched_evidence() {
        let mut components = verified_components(None);
        components.pop();
        assert_eq!(
            validate_causal_cache_v1(components, &bound_invalidation())
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::IncompleteCoordinateSet
        );

        let mut unrelated_binding = binding();
        unrelated_binding.artifact_digest = d(99);
        let unrelated = BoundCausalCacheInvalidationV1::test_only(&unrelated_binding);
        assert_eq!(
            validate_causal_cache_v1(verified_components(None), &unrelated)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::InvalidationAuthorityMismatch
        );

        let claim = CausalCacheComponentClaimV1::new(
            binding(),
            CacheCoordinateV1::Source,
            ArtifactOwnerV1::FsZero,
            CacheValidityV1::Exact,
            d(35),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(b"not-the-claim".to_vec());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            VerifiedCausalCacheComponentV1::verify(claim, &evidence)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::EvidencePayloadMismatch
        );
    }

    fn metric_receipt(
        label: Q99LabelV1,
        numerator: u128,
        denominator: u128,
    ) -> VerifiedQ99MetricReceiptV1 {
        let claim = Q99MetricReceiptClaimV1::new(
            label,
            d(40),
            d(41),
            100,
            numerator,
            denominator,
            d(42),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99MetricReceiptV1::verify(claim, &evidence).unwrap()
    }

    #[test]
    fn state_and_input_claims_have_labeled_denominators_and_exact_integer_thresholds() {
        let Q99ClaimDecisionV1::Attained(state) =
            generate_q99_metric_claim_v1(metric_receipt(Q99LabelV1::Q99State, 99, 100)).unwrap()
        else {
            panic!("99 of 100 exact state reuses must attain Q99-State")
        };
        assert_eq!(state.record().label, Q99LabelV1::Q99State);
        assert_eq!(
            state.record().threshold_relation,
            Q99ThresholdRelationV1::AtLeast99Of100
        );
        assert_eq!(state.record().observed_numerator, "99");
        assert_eq!(state.record().denominator, "100");
        state.record().validate().unwrap();
        let mut impossible = state.record().clone();
        impossible.observed_numerator = "101".into();
        impossible.attained = false;
        impossible.claim_digest = impossible.expected_digest().unwrap();
        assert_eq!(
            impossible.validate().unwrap_err().failure_code(),
            Q99FailureCodeV1::InvalidClaim
        );

        let Q99ClaimDecisionV1::NotAttained(input) =
            generate_q99_metric_claim_v1(metric_receipt(Q99LabelV1::Q99Input, 98, 100)).unwrap()
        else {
            panic!("98 of 100 cannot attain Q99-Input")
        };
        assert!(!input.attained);
        assert_eq!(input.label, Q99LabelV1::Q99Input);
        assert!(
            Q99MetricReceiptClaimV1::new(
                Q99LabelV1::Q99Total,
                d(1),
                d(2),
                1,
                1,
                1,
                d(3),
                verifier_route(),
            )
            .is_err()
        );
    }

    fn work_receipt(
        total: u64,
        work_id: u8,
        boundary: u8,
        class: CausalWorkClassV1,
        unit: CausalCounterUnitV1,
    ) -> CausalWorkReceiptV1 {
        let identity = ParentCounterIdentityV1 {
            counter_id: "q99-complete-work".into(),
            unit,
            boundary_digest: d(boundary),
            adapter_digest: d(240),
            platform_profile_digest: d(241),
        };
        let CausalWorkOutcomeV1::Measured { receipt } = CausalWorkReceiptV1::build(
            d(242),
            ParentCounterObservationV1::Measured {
                window: ParentCounterWindowV1 {
                    identity,
                    start: 0,
                    end: total,
                },
            },
            vec![CausalWorkChargeV1 {
                work_unit_id: d(work_id),
                class,
                amount: total,
            }],
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap() else {
            panic!("measured fixture must produce a receipt")
        };
        receipt
    }

    fn verified_work(receipt: CausalWorkReceiptV1) -> VerifiedCausalWorkReceiptV1 {
        let bytes = canonical_causal_work_bytes(&receipt).unwrap();
        let (certificate, resolver) = certificate(bytes);
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedCausalWorkReceiptV1::verify(receipt, verifier_route(), &evidence).unwrap()
    }

    fn preparation(total: u64, work_id: u8) -> VerifiedQ99PreparationV1 {
        let work = verified_work(work_receipt(
            total,
            work_id,
            50,
            CausalWorkClassV1::Prewarm,
            CausalCounterUnitV1::Tokens,
        ));
        let claim =
            Q99PreparationClaimV1::new(d(40), d(41), work.receipt_digest(), verifier_route())
                .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99PreparationV1::verify(claim, work, &evidence).unwrap()
    }

    fn task_pair(
        task: u8,
        baseline_total: u64,
        complete_total: u64,
        baseline_work_id: u8,
        complete_work_id: u8,
    ) -> VerifiedQ99TaskPairV1 {
        let baseline = verified_work(work_receipt(
            baseline_total,
            baseline_work_id,
            60 + task,
            CausalWorkClassV1::Baseline,
            CausalCounterUnitV1::Tokens,
        ));
        let complete = verified_work(work_receipt(
            complete_total,
            complete_work_id,
            90 + task,
            CausalWorkClassV1::Candidate,
            CausalCounterUnitV1::Tokens,
        ));
        let claim = Q99TaskPairClaimV1::new(
            d(40),
            d(41),
            d(task),
            baseline.receipt_digest(),
            complete.receipt_digest(),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        VerifiedQ99TaskPairV1::verify(claim, baseline, complete, &evidence).unwrap()
    }

    #[test]
    fn total_claim_charges_preparation_and_complete_work_against_paired_raw_baseline() {
        let Q99ClaimDecisionV1::Attained(certificate) =
            generate_q99_total_claim_v1(preparation(1, 1), vec![task_pair(10, 1_000, 9, 2, 3)])
                .unwrap()
        else {
            panic!("one preparation plus nine residual must attain Q99-Total")
        };
        let record = certificate.record();
        assert_eq!(record.label, Q99LabelV1::Q99Total);
        assert_eq!(record.observed_numerator, "10");
        assert_eq!(record.denominator, "1000");
        assert_eq!(
            record.threshold_relation,
            Q99ThresholdRelationV1::AtMost1Of100
        );
        assert!(record.work_profile.is_some());
        let claim_keys = serde_json::to_value(record)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_claim_keys = [
            "schema_version",
            "label",
            "comparison_identity_digest",
            "workload_digest",
            "work_profile",
            "task_count",
            "observed_numerator",
            "denominator",
            "threshold_relation",
            "threshold_numerator",
            "threshold_denominator",
            "attained",
            "source_receipt_digests",
            "claim_digest",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(claim_keys, expected_claim_keys);
        assert_eq!(serde_json::to_value(record).unwrap()["label"], "q99-total");
        record.validate().unwrap();
        let mut forged = record.clone();
        forged.observed_numerator = "9".into();
        assert_eq!(
            forged.validate().unwrap_err().failure_code(),
            Q99FailureCodeV1::InvalidClaim
        );
        let bytes = record.canonical_bytes().unwrap();
        assert_eq!(
            Q99ClaimRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *record
        );

        let Q99ClaimDecisionV1::NotAttained(record) =
            generate_q99_total_claim_v1(preparation(1, 4), vec![task_pair(11, 1_000, 10, 5, 6)])
                .unwrap()
        else {
            panic!("eleven complete units cannot attain Q99-Total")
        };
        assert!(!record.attained);
    }

    #[test]
    fn total_claim_rejects_double_counting_and_mixed_native_coordinates() {
        assert_eq!(
            generate_q99_total_claim_v1(preparation(1, 7), vec![task_pair(12, 1_000, 9, 8, 7)],)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::DuplicateWorkUnit
        );

        let baseline = verified_work(work_receipt(
            1_000,
            10,
            70,
            CausalWorkClassV1::Baseline,
            CausalCounterUnitV1::Tokens,
        ));
        let complete = verified_work(work_receipt(
            9,
            11,
            71,
            CausalWorkClassV1::Candidate,
            CausalCounterUnitV1::Bytes,
        ));
        let claim = Q99TaskPairClaimV1::new(
            d(40),
            d(41),
            d(13),
            baseline.receipt_digest(),
            complete.receipt_digest(),
            verifier_route(),
        )
        .unwrap();
        let (certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
        let evidence = verify(&certificate, &resolver).unwrap();
        assert_eq!(
            VerifiedQ99TaskPairV1::verify(claim, baseline, complete, &evidence)
                .unwrap_err()
                .failure_code(),
            Q99FailureCodeV1::WorkProfileMismatch
        );
    }

    #[test]
    fn contract_and_external_schema_digests_are_stable() {
        // be957b1be9482626158db0b450e53dd0c764fe76286090bf2f87091a00f13630
        assert_eq!(
            q99_contract_digest_v1(),
            DigestV1::from_bytes([
                0xbe, 0x95, 0x7b, 0x1b, 0xe9, 0x48, 0x26, 0x26, 0x15, 0x8d, 0xb0, 0xb4, 0x50, 0xe5,
                0x3d, 0xd0, 0xc7, 0x64, 0xfe, 0x76, 0x28, 0x60, 0x90, 0xbf, 0x2f, 0x87, 0x09, 0x1a,
                0x00, 0xf1, 0x36, 0x30,
            ])
        );
        // ae0fa6885e08f28bd68613eb6430be3c904874946336a0fae5da5f8cf2df8236
        assert_eq!(
            q99_contract_digest_v2(),
            DigestV1::from_bytes([
                0xcc, 0x55, 0x7f, 0x1e, 0x4b, 0x6c, 0xa1, 0xbd, 0x17, 0x46, 0x7b, 0x34, 0x32, 0x4f,
                0xfd, 0x4d, 0xa2, 0xed, 0xb4, 0xd3, 0x26, 0xa0, 0xeb, 0x2b, 0x61, 0x23, 0x0b, 0x14,
                0x58, 0x34, 0xd1, 0x5b,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/q99-causal-cache-component-v1.schema.json"
            )))
            .to_hex(),
            Q99_CACHE_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/q99-claim-v1.schema.json"
            )))
            .to_hex(),
            Q99_CLAIM_SCHEMA_SHA256_V1
        );
    }
