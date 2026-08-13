    use std::borrow::Cow;
    use std::collections::BTreeSet;

    use super::*;
    use crate::q99::{
        CacheCoordinateV1, CacheValidityV1, CausalCacheComponentClaimV1, CausalCacheDecisionV1,
        VerifiedCausalCacheComponentV1, validate_causal_cache_v1,
    };
    use zero_abi::{
        DependencyEdgeKindV1, DependencyEdgeV1, EssentialDependencyCertificate,
        EvidenceDecisionTree, EvidenceLeafV1, EvidenceObservationV1, FreshnessHeadV1,
        ProducerDomainV1, ProtectedEffectClassV1, ProtectedEffectSet, ProtectedEffectV1,
        ROBUST_SNAP_MODEL_VERSION, WorldFiberDescriptor,
    };
    use zero_cert::{
        CompletenessWitness, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query,
        Resolver, SpanRef, TestId, verify,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn verifier_route() -> DigestV1 {
        domain_digest(
            b"zerostack.q99.verifier_identity.v1\0",
            canonical_json(&json!({
                "index_id": "invalidation-index",
                "index_version": "1",
                "operator_id": "invalidation-verifier",
                "operator_version": "1",
                "parser_id": "invalidation-parser",
                "parser_version": "1",
            }))
            .as_bytes(),
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
            (id == "invalidation-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "invalidation-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "invalidation-index").then_some("1")
        }
    }

    fn certificate(bytes: Vec<u8>) -> (EvidenceCertificate<'static>, TestResolver) {
        let digest = sha256(&bytes);
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(23) },
                spans: vec![SpanRef {
                    object_id: ObjectId(digest),
                    object_digest: digest,
                    byte_start: 0,
                    byte_len: bytes.len() as u64,
                    span_digest: digest,
                }],
                payload: Cow::Owned(bytes.clone()),
                provenance: Provenance {
                    parser_id: "invalidation-parser".into(),
                    parser_version: "1".into(),
                    index_id: "invalidation-index".into(),
                    index_version: "1".into(),
                    operator_id: "invalidation-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "invalidation-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(23),
                    exit_code: 0,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            TestResolver { bytes },
        )
    }

    fn effect(byte: u8) -> ProtectedEffectV1 {
        ProtectedEffectV1 {
            effect_digest: d(byte),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        }
    }

    fn snap_certificate() -> RobustSnapCertificate {
        RobustSnapCertificate::create_s0(
            WorldFiberDescriptor {
                model_version: ROBUST_SNAP_MODEL_VERSION.into(),
                assembly_manifest_digest: d(1),
                source_image_digest: d(2),
                task_fingerprint: d(3),
                assumptions: vec!["finite complete fiber".into()],
                worlds: vec![d(4), d(5)],
            },
            vec![
                ProtectedEffectSet {
                    world_id: d(4),
                    effects: vec![effect(6), effect(7)],
                },
                ProtectedEffectSet {
                    world_id: d(5),
                    effects: vec![effect(6), effect(8)],
                },
            ],
            vec![effect(6), effect(7), effect(8)],
            vec![effect(6), effect(7), effect(8)],
            effect(6),
        )
        .unwrap()
    }

    fn snap_claim(
        certificate: &RobustSnapCertificate,
        representation: SnapFiberRepresentationV1,
    ) -> RobustSnapIntakeClaimV1 {
        RobustSnapIntakeClaimV1::new(
            certificate.certificate_digest,
            representation,
            d(10),
            d(11),
            d(12),
            verifier_route(),
        )
        .unwrap()
    }

    #[test]
    fn robust_snap_requires_proof_of_exact_or_conservative_complete_fiber() {
        let snap = snap_certificate();
        let claim = snap_claim(&snap, SnapFiberRepresentationV1::FiniteExact);
        let claim_value = serde_json::to_value(&claim).unwrap();
        let claim_keys = claim_value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            claim_keys,
            [
                "schema_version",
                "snap_certificate_digest",
                "fiber_representation",
                "fiber_completeness_receipt_digest",
                "protected_use_scope_digest",
                "dominance_scope_digest",
                "verifier_identity_digest",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(claim_value["fiber_representation"], "finite_exact");
        let (evidence_certificate, resolver) =
            certificate(snap_envelope_bytes(&claim, &snap).unwrap());
        let evidence = verify(&evidence_certificate, &resolver).unwrap();
        let RobustSnapIntakeDecisionV1::Complete(authority) =
            verify_robust_snap_intake_v1(claim, snap, &evidence).unwrap()
        else {
            panic!("an exactly evidenced complete finite fiber must pass intake")
        };
        assert!(!authority.permits_operational_execution());
        authority.record().validate().unwrap();
        let bytes = authority.record().canonical_bytes().unwrap();
        assert_eq!(
            RobustSnapIntakeRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *authority.record()
        );

        let snap = snap_certificate();
        let unknown_claim = snap_claim(&snap, SnapFiberRepresentationV1::Unknown);
        let (evidence_certificate, resolver) =
            certificate(snap_envelope_bytes(&unknown_claim, &snap).unwrap());
        let evidence = verify(&evidence_certificate, &resolver).unwrap();
        let RobustSnapIntakeDecisionV1::Unknown(record) =
            verify_robust_snap_intake_v1(unknown_claim, snap, &evidence).unwrap()
        else {
            panic!("unknown fiber completeness must not mint Snap authority")
        };
        assert_eq!(record.disposition, RobustSnapIntakeDispositionV1::Unknown);
    }

    #[test]
    fn robust_snap_intake_rejects_zero_identity_and_unproven_evidence_tree() {
        let mut fiber = snap_certificate().fiber;
        fiber.source_image_digest = DigestV1::ZERO;
        let zero_identity = RobustSnapCertificate::create_s0(
            fiber,
            vec![
                ProtectedEffectSet {
                    world_id: d(4),
                    effects: vec![effect(6)],
                },
                ProtectedEffectSet {
                    world_id: d(5),
                    effects: vec![effect(6)],
                },
            ],
            vec![effect(6)],
            vec![effect(6)],
            effect(6),
        )
        .unwrap();
        let claim = snap_claim(&zero_identity, SnapFiberRepresentationV1::FiniteExact);
        let (evidence_certificate, resolver) =
            certificate(snap_envelope_bytes(&claim, &zero_identity).unwrap());
        let evidence = verify(&evidence_certificate, &resolver).unwrap();
        assert_eq!(
            verify_robust_snap_intake_v1(claim, zero_identity, &evidence)
                .unwrap_err()
                .failure_code(),
            InvalidationFailureCodeV1::ZeroDigest
        );

        let fiber = snap_certificate().fiber;
        let s1 = RobustSnapCertificate::create_s1(
            fiber,
            vec![
                ProtectedEffectSet {
                    world_id: d(4),
                    effects: vec![effect(6)],
                },
                ProtectedEffectSet {
                    world_id: d(5),
                    effects: vec![effect(6)],
                },
            ],
            vec![effect(6)],
            EvidenceDecisionTree {
                evidence_schema_digest: DigestV1::ZERO,
                leaves: vec![
                    EvidenceLeafV1 {
                        path: vec![EvidenceObservationV1 {
                            evidence_id: d(20),
                            outcome_digest: d(21),
                        }],
                        admitted_worlds: vec![d(4)],
                        selected_effect: effect(6),
                    },
                    EvidenceLeafV1 {
                        path: vec![EvidenceObservationV1 {
                            evidence_id: d(20),
                            outcome_digest: d(22),
                        }],
                        admitted_worlds: vec![d(5)],
                        selected_effect: effect(6),
                    },
                ],
            },
        )
        .unwrap();
        let claim = snap_claim(&s1, SnapFiberRepresentationV1::FiniteExact);
        let (evidence_certificate, resolver) =
            certificate(snap_envelope_bytes(&claim, &s1).unwrap());
        let evidence = verify(&evidence_certificate, &resolver).unwrap();
        assert_eq!(
            verify_robust_snap_intake_v1(claim, s1, &evidence)
                .unwrap_err()
                .failure_code(),
            InvalidationFailureCodeV1::ZeroDigest
        );
    }

    fn support_closure() -> CertifiedInfluenceClosure {
        let edge = DependencyEdgeV1::new(
            "graph.source",
            "graph.artifact",
            DependencyEdgeKindV1::Reads,
        )
        .unwrap();
        CertifiedInfluenceClosure::new(
            d(30),
            vec![FreshnessHeadV1::new("repo", "head").unwrap()],
            vec![ProducerDomainV1::GraphIndex],
            vec!["graph.artifact".into(), "graph.source".into()],
            vec![edge.clone()],
            vec![
                EssentialDependencyCertificate::new(
                    edge,
                    vec!["graph.source".into(), "graph.artifact".into()],
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn artifact_claim(
        closure: &CertifiedInfluenceClosure,
        support_class: SupportCompletenessClassV1,
        derivation_authority: DerivationAuthorityV1,
    ) -> CausalArtifactIntakeClaimV1 {
        CausalArtifactIntakeClaimV1::new(
            d(31),
            ArtifactOwnerV1::GraphZero,
            d(32),
            vec![d(33), d(34)],
            closure.certificate_digest,
            support_class,
            derivation_authority,
            d(35),
            d(36),
            d(37),
            d(38),
            d(39),
            verifier_route(),
        )
        .unwrap()
    }

    fn verify_artifact(
        support_class: SupportCompletenessClassV1,
        derivation_authority: DerivationAuthorityV1,
    ) -> InvalidationIntakeDecisionV1 {
        let closure = support_closure();
        let claim = artifact_claim(&closure, support_class, derivation_authority);
        let (evidence_certificate, resolver) =
            certificate(artifact_envelope_bytes(&claim, &closure).unwrap());
        let evidence = verify(&evidence_certificate, &resolver).unwrap();
        verify_causal_artifact_intake_v1(claim, closure, &evidence).unwrap()
    }

    fn verified_cache_components(
        binding: &CausalCacheBindingV1,
    ) -> Vec<VerifiedCausalCacheComponentV1> {
        [
            (
                CacheCoordinateV1::Source,
                ArtifactOwnerV1::FsZero,
                CacheValidityV1::Exact,
            ),
            (
                CacheCoordinateV1::Producer,
                ArtifactOwnerV1::GraphZero,
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
                CacheValidityV1::ProviderEligible,
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
        .into_iter()
        .enumerate()
        .map(|(index, (coordinate, owner, validity))| {
            let claim = CausalCacheComponentClaimV1::new(
                binding.clone(),
                coordinate,
                owner,
                validity,
                d(100 + index as u8),
                verifier_route(),
            )
            .unwrap();
            let (evidence_certificate, resolver) = certificate(claim.canonical_bytes().unwrap());
            let evidence = verify(&evidence_certificate, &resolver).unwrap();
            VerifiedCausalCacheComponentV1::verify(claim, &evidence).unwrap()
        })
        .collect()
    }

    #[test]
    fn exact_and_sound_support_mint_opaque_authority_and_bind_the_cache_line() {
        for support_class in [
            SupportCompletenessClassV1::Exact,
            SupportCompletenessClassV1::SoundOverapproximation,
        ] {
            let InvalidationIntakeDecisionV1::ProtectedSupport(authority) = verify_artifact(
                support_class,
                DerivationAuthorityV1::Witness {
                    witness_digest: d(40),
                },
            ) else {
                panic!("exact and sound-overapproximated support must authorize protected support")
            };
            authority.record().validate().unwrap();
            let claim = &authority.record().claim;
            let claim_value = serde_json::to_value(claim).unwrap();
            let claim_keys = claim_value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();
            assert_eq!(
                claim_keys,
                [
                    "schema_version",
                    "artifact_digest",
                    "artifact_owner",
                    "producer_identity_digest",
                    "declared_support_roots",
                    "support_closure_digest",
                    "support_class",
                    "derivation_authority",
                    "invalidation_predicate_digest",
                    "protected_use_scope_digest",
                    "verifier_scope_digest",
                    "validation_cost_profile_digest",
                    "recovery_route_digest",
                    "verifier_identity_digest",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
            );
            let binding = CausalCacheBindingV1 {
                artifact_digest: claim.artifact_digest,
                artifact_owner: claim.artifact_owner,
                source_root: d(33),
                dependency_root: claim.support_closure_digest,
                producer_contract_digest: claim.producer_identity_digest,
                protected_use_class_digest: claim.protected_use_scope_digest,
                reasoning_contract_digest: d(41),
                verifier_scope_digest: claim.verifier_scope_digest,
                invalidation_certificate_digest: authority.record().authority_digest,
                recovery_route_digest: claim.recovery_route_digest,
            };
            let bound = authority.bind_cache(&binding).unwrap();
            assert!(bound.authorizes(&binding).unwrap());
            assert!(matches!(
                validate_causal_cache_v1(verified_cache_components(&binding), &bound).unwrap(),
                CausalCacheDecisionV1::StrictReuse(_)
            ));

            let mut wrong = binding.clone();
            wrong.source_root = d(42);
            assert_eq!(
                authority.bind_cache(&wrong).unwrap_err().failure_code(),
                InvalidationFailureCodeV1::BindingMismatch
            );
        }
    }

    #[test]
    fn essential_edges_and_heuristics_never_launder_support_completeness() {
        let InvalidationIntakeDecisionV1::RetrievalOnly(record) = verify_artifact(
            SupportCompletenessClassV1::Heuristic,
            DerivationAuthorityV1::Witness {
                witness_digest: d(43),
            },
        ) else {
            panic!("heuristic support with EDCs is still retrieval-only")
        };
        assert!(!record.support_closure.essential_dependencies.is_empty());
        assert_eq!(
            record.disposition,
            InvalidationIntakeDispositionV1::RetrievalOnly
        );

        let InvalidationIntakeDecisionV1::Rejected(record) = verify_artifact(
            SupportCompletenessClassV1::Unknown,
            DerivationAuthorityV1::ReplayRecipe {
                recipe_digest: d(44),
            },
        ) else {
            panic!("unknown support must fail closed")
        };
        assert_eq!(
            record.disposition,
            InvalidationIntakeDispositionV1::Rejected
        );

        let mut impossible_exact = artifact_claim(
            &support_closure(),
            SupportCompletenessClassV1::Heuristic,
            DerivationAuthorityV1::Witness {
                witness_digest: d(45),
            },
        );
        impossible_exact.support_class = SupportCompletenessClassV1::Exact;
        impossible_exact.derivation_authority = DerivationAuthorityV1::OpaqueWholeUnit {
            unit_root_digest: d(45),
        };
        assert_eq!(
            impossible_exact.validate().unwrap_err().failure_code(),
            InvalidationFailureCodeV1::SupportClassMismatch
        );
    }

    #[test]
    fn record_tampering_and_noncanonical_replay_fail_closed() {
        let InvalidationIntakeDecisionV1::ProtectedSupport(authority) = verify_artifact(
            SupportCompletenessClassV1::Exact,
            DerivationAuthorityV1::ReplayRecipe {
                recipe_digest: d(46),
            },
        ) else {
            panic!("fixture must produce authority")
        };
        let mut record = authority.record().clone();
        record.claim.validation_cost_profile_digest = d(47);
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            InvalidationFailureCodeV1::RecordDigestMismatch
        );
        let canonical = authority.record().canonical_bytes().unwrap();
        let mut padded = canonical.clone();
        padded.push(b' ');
        assert_eq!(
            CausalArtifactIntakeRecordV1::from_canonical_bytes(&padded)
                .unwrap_err()
                .failure_code(),
            InvalidationFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn contract_and_external_schema_digests_are_stable() {
        // cb58308d4e55273c48a08d004b09d2183c34fa91267dbb86abb3a4845149b106
        assert_eq!(
            invalidation_intake_contract_digest_v1(),
            DigestV1::from_bytes([
                0xcb, 0x58, 0x30, 0x8d, 0x4e, 0x55, 0x27, 0x3c, 0x48, 0xa0, 0x8d, 0x00, 0x4b, 0x09,
                0xd2, 0x18, 0x3c, 0x34, 0xfa, 0x91, 0x26, 0x7d, 0xbb, 0x86, 0xab, 0xb3, 0xa4, 0x84,
                0x51, 0x49, 0xb1, 0x06,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/robust-snap-intake-v1.schema.json"
            )))
            .to_hex(),
            ROBUST_SNAP_INTAKE_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(sha256(include_bytes!(
                "../../../../conformance/schemas/causal-artifact-intake-v1.schema.json"
            )))
            .to_hex(),
            CAUSAL_ARTIFACT_SCHEMA_SHA256_V1
        );
    }
