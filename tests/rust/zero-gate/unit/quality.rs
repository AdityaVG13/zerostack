    use super::*;
    use std::borrow::Cow;
    use zero_cert::{
        CompletenessWitness, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query,
        Resolver, SpanRef, verify,
    };

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    struct Resident<'a> {
        bytes: &'a [u8],
    }

    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(self.bytes) == object_id.0).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "quality-checker").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "canonical-json").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "quality-evidence").then_some("1")
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
                parser_id: "canonical-json".into(),
                parser_version: "1".into(),
                index_id: "quality-evidence".into(),
                index_version: "1".into(),
                operator_id: "quality-checker".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::ReadSpan {
                operator: OperatorLock {
                    operator_id: "quality-checker".into(),
                    operator_version: "1".into(),
                },
            },
            input_token_cost: 1,
            backend_work_units: 1,
        }
    }

    fn baseline() -> FrozenBaselineV1 {
        FrozenBaselineV1::new(digest(3), digest(4), digest(5)).unwrap()
    }

    fn pair(candidate_value: i64) -> QualityPairV1 {
        QualityPairV1::new(
            digest(1),
            digest(2),
            digest(3),
            digest(10),
            digest(4),
            digest(6),
            digest(7),
            digest(11),
            vec![
                ProtectedMetricV1 {
                    metric_id: "correctness".into(),
                    order: MetricOrderV1::AtLeast,
                    baseline_value: 1,
                    candidate_value,
                },
                ProtectedMetricV1 {
                    metric_id: "latency".into(),
                    order: MetricOrderV1::AtMost,
                    baseline_value: 20,
                    candidate_value: 10,
                },
                ProtectedMetricV1 {
                    metric_id: "safety".into(),
                    order: MetricOrderV1::Exact,
                    baseline_value: 1,
                    candidate_value: 1,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn exact_neutral_requires_every_protected_identity_to_match() {
        let certificate = ExactNeutralCertificateV1::verify(
            digest(1),
            digest(2),
            digest(3),
            digest(10),
            digest(8),
            digest(8),
            digest(9),
            digest(9),
            digest(4),
            digest(4),
        )
        .unwrap();
        let admission = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            baseline(),
        )
        .unwrap();
        assert_eq!(admission.selection(), QualitySelectionV1::Candidate);
        assert_eq!(admission.guarantee(), QualityGuaranteeV1::ExactSubstitution);
        let record = admission.record();
        assert_eq!(record.candidate_identity_digest, Some(digest(10)));
        assert_eq!(record.candidate_outcome_digest, Some(digest(4)));
        assert_ne!(record.pairing_method_digest, DigestV1::ZERO);
        assert_ne!(record.protected_predicate_digest, DigestV1::ZERO);
        assert_ne!(record.verifier_identity_digest, DigestV1::ZERO);
        assert_eq!(
            ExactNeutralCertificateV1::verify(
                digest(1),
                digest(2),
                digest(3),
                digest(10),
                digest(8),
                digest(99),
                digest(9),
                digest(9),
                digest(4),
                digest(4),
            )
            .unwrap_err()
            .failure_code(),
            QualityEnvelopeFailureCodeV1::ExactNeutralMismatch
        );
    }

    #[test]
    fn pointwise_vector_admits_only_no_regression_and_exact_payload() {
        let pair = pair(1);
        let bytes = pair.canonical_bytes().unwrap();
        let evidence_certificate = certificate(&bytes);
        let resolver = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resolver).unwrap();
        let pointwise =
            PointwiseDominanceCertificateV1::verify(&pair, digest(8), &verified).unwrap();
        let admission = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::PointwiseDominance(pointwise),
            baseline(),
        )
        .unwrap();
        assert_eq!(admission.selection(), QualitySelectionV1::Candidate);
        assert_eq!(admission.guarantee(), QualityGuaranteeV1::PointwiseNoWorse);
        assert!(admission.strict_improvement());
        let record = admission.record();
        assert_eq!(record.candidate_identity_digest, Some(digest(10)));
        assert_eq!(record.candidate_outcome_digest, Some(digest(6)));
        assert_eq!(record.pairing_method_digest, digest(11));
        assert_eq!(record.protected_predicate_digest, digest(8));
        assert_eq!(
            QualityPairV1::new(
                digest(1),
                digest(2),
                digest(3),
                digest(10),
                digest(4),
                digest(6),
                digest(7),
                digest(11),
                vec![ProtectedMetricV1 {
                    metric_id: "correctness".into(),
                    order: MetricOrderV1::AtLeast,
                    baseline_value: 1,
                    candidate_value: 0,
                }],
            )
            .unwrap_err()
            .failure_code(),
            QualityEnvelopeFailureCodeV1::CandidateRegression
        );
        let wrong = b"{}";
        let wrong_certificate = certificate(wrong);
        let wrong_resolver = Resident { bytes: wrong };
        let wrong_verified = verify(&wrong_certificate, &wrong_resolver).unwrap();
        assert_eq!(
            PointwiseDominanceCertificateV1::verify(&pair, digest(8), &wrong_verified)
                .unwrap_err()
                .failure_code(),
            QualityEnvelopeFailureCodeV1::EvidencePayloadMismatch
        );
    }

    #[test]
    fn scoped_class_requires_both_rule_and_membership_evidence() {
        let rule = ClassDominanceRuleV1::new(
            digest(10),
            digest(2),
            digest(7),
            digest(11),
            digest(3),
            digest(12),
            DominanceClaimV1::NoWorse,
        )
        .unwrap();
        let membership =
            TaskClassMembershipV1::new(digest(10), digest(1), digest(11), digest(13)).unwrap();
        let rule_bytes = rule.canonical_bytes().unwrap();
        let membership_bytes = membership.canonical_bytes().unwrap();
        let rule_certificate = certificate(&rule_bytes);
        let rule_resolver = Resident { bytes: &rule_bytes };
        let rule_verified = verify(&rule_certificate, &rule_resolver).unwrap();
        let membership_certificate = certificate(&membership_bytes);
        let membership_resolver = Resident {
            bytes: &membership_bytes,
        };
        let membership_verified = verify(&membership_certificate, &membership_resolver).unwrap();
        let scoped = ScopedClassDominanceCertificateV1::verify(
            &rule,
            &membership,
            &rule_verified,
            &membership_verified,
        )
        .unwrap();
        let admission = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ScopedClassDominance(scoped),
            baseline(),
        )
        .unwrap();
        assert_eq!(admission.selection(), QualitySelectionV1::Candidate);
        assert_eq!(
            admission.guarantee(),
            QualityGuaranteeV1::ScopedClassNoWorse
        );
        let record = admission.record();
        assert_eq!(
            record.class_certificate_digest,
            Some(record.evidence_digest)
        );
        assert!(record.candidate_outcome_digest.is_none());
        let wrong_membership =
            TaskClassMembershipV1::new(digest(99), digest(1), digest(11), digest(13)).unwrap();
        assert_eq!(
            ScopedClassDominanceCertificateV1::verify(
                &rule,
                &wrong_membership,
                &rule_verified,
                &membership_verified,
            )
            .unwrap_err()
            .failure_code(),
            QualityEnvelopeFailureCodeV1::ClassMembershipMismatch
        );
    }

    #[test]
    fn distributional_evidence_is_valid_but_selects_baseline_in_strict_mode() {
        let claim = DistributionalClaimV1::new(
            digest(20),
            digest(2),
            digest(21),
            digest(3),
            digest(4),
            digest(7),
            digest(8),
            digest(9),
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
        let resolver = Resident { bytes: &bytes };
        let verified = verify(&evidence_certificate, &resolver).unwrap();
        let distributional = DistributionalCertificateV1::verify(&claim, &verified).unwrap();
        let admission = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::Distributional(distributional),
            baseline(),
        )
        .unwrap();
        assert_eq!(admission.selection(), QualitySelectionV1::FrozenBaseline);
        assert_eq!(
            admission.guarantee(),
            QualityGuaranteeV1::DistributionalOnly
        );
        let record = admission.record();
        assert_eq!(record.confidence_scope_digest, Some(digest(20)));
        assert!(record.candidate_outcome_digest.is_none());
        assert_eq!(
            DistributionalClaimV1::new(
                digest(20),
                digest(2),
                digest(21),
                digest(3),
                digest(4),
                digest(7),
                digest(8),
                digest(9),
                100,
                10,
                2,
                88,
                80_000,
                0,
                950_000,
            )
            .unwrap_err()
            .failure_code(),
            QualityEnvelopeFailureCodeV1::NonPositiveDistributionalBound
        );
    }

    #[test]
    fn unidentified_and_binding_mismatch_fall_back_loudly() {
        let evidence = QualityEvidenceV1::unidentified(
            digest(1),
            digest(2),
            digest(30),
            UnidentifiedReasonV1::MissingEvidence,
        )
        .unwrap();
        let admission = QualityAdmissionV1::admit_strict(evidence, baseline()).unwrap();
        assert_eq!(admission.selection(), QualitySelectionV1::FrozenBaseline);

        let exact = ExactNeutralCertificateV1::verify(
            digest(1),
            digest(2),
            digest(99),
            digest(10),
            digest(8),
            digest(8),
            digest(9),
            digest(9),
            digest(4),
            digest(4),
        )
        .unwrap();
        assert_eq!(
            QualityAdmissionV1::admit_strict(QualityEvidenceV1::ExactNeutral(exact), baseline())
                .unwrap_err()
                .failure_code(),
            QualityEnvelopeFailureCodeV1::BaselineBindingMismatch
        );
    }

    #[test]
    fn canonical_pair_rejects_whitespace_unknown_fields_and_order_drift() {
        let pair = pair(1);
        let bytes = pair.canonical_bytes().unwrap();
        assert_eq!(QualityPairV1::from_canonical_bytes(&bytes).unwrap(), pair);
        let mut whitespace = bytes.clone();
        whitespace.push(b'\n');
        assert_eq!(
            QualityPairV1::from_canonical_bytes(&whitespace)
                .unwrap_err()
                .failure_code(),
            QualityEnvelopeFailureCodeV1::NonCanonicalEncoding
        );
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(true));
        assert_eq!(
            QualityPairV1::from_canonical_bytes(canonical_json(&value).as_bytes())
                .unwrap_err()
                .failure_code(),
            QualityEnvelopeFailureCodeV1::SerializationFailure
        );
    }

    #[test]
    fn quality_admission_digest_rejects_tampering() {
        let certificate = ExactNeutralCertificateV1::verify(
            digest(1),
            digest(2),
            digest(3),
            digest(10),
            digest(8),
            digest(8),
            digest(9),
            digest(9),
            digest(4),
            digest(4),
        )
        .unwrap();
        let mut admission = QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            baseline(),
        )
        .unwrap();
        let record = admission.record();
        let bytes = record.canonical_bytes().unwrap();
        assert_eq!(
            QualityAdmissionRecordV1::from_canonical_bytes(&bytes).unwrap(),
            record
        );
        admission.evidence_digest = digest(99);
        assert_eq!(
            admission.validate().unwrap_err().failure_code(),
            QualityEnvelopeFailureCodeV1::CertificateDigestMismatch
        );
    }

    #[test]
    fn quality_contract_digest_is_stable() {
        assert_eq!(
            quality_envelope_contract_digest_v1(),
            DigestV1::from_hex("6859414e694bc2d0eb941f8c3594f0bb192bb8504efc547fea1f36481388bceb")
                .unwrap()
        );
    }
