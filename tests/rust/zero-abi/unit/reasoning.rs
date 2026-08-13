    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn contract() -> ReasoningContractV1 {
        ReasoningContractV1::new(
            digest(1),
            digest(2),
            digest(3),
            digest(4),
            digest(5),
            "native-reasoning",
            "high",
            4_096,
            8_192,
            2_048,
            1_024,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::from([("sampler".into(), json!({"temperature_ppm": 0}))]),
        )
        .unwrap()
    }

    #[test]
    fn canonical_schema_and_contract_round_trip() {
        let contract = contract();
        let bytes = contract.canonical_bytes().unwrap();
        assert_eq!(
            ReasoningContractV1::from_canonical_bytes(&bytes).unwrap(),
            contract
        );
        let mut spaced = bytes.clone();
        spaced.push(b' ');
        assert_eq!(
            ReasoningContractV1::from_canonical_bytes(&spaced)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::NonCanonicalEncoding
        );
        assert_eq!(
            reasoning_contract_schema_v1()["required"]
                .as_array()
                .unwrap()
                .len(),
            14
        );
        let published: Value = serde_json::from_str(include_str!(
            "../../../../conformance/schemas/reasoning-contract-v1.schema.json"
        ))
        .unwrap();
        assert_eq!(published, reasoning_contract_schema_v1());
    }

    #[test]
    fn strict_equal_contract_mints_opaque_admission() {
        let baseline = contract();
        let admission = verify_strict_no_downshift_v1(&baseline, &baseline).unwrap();
        assert!(admission.same_comparison_class());
        admission.validate().unwrap();
        let record = admission.record();
        record.validate().unwrap();
        let bytes = record.canonical_bytes().unwrap();
        assert_eq!(
            StrictReasoningAdmissionRecordV1::from_canonical_bytes(&bytes).unwrap(),
            record
        );
        assert_eq!(
            admission.baseline_contract_digest(),
            baseline.identity_digest().unwrap()
        );
    }

    #[test]
    fn strict_identity_mode_effort_state_and_provider_changes_reclassify() {
        let baseline = contract();
        let mut candidate = baseline.clone();
        candidate.tool_schema_digest = digest(99);
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ComparisonIdentityMismatch
        );
        let mut candidate = baseline.clone();
        candidate.reasoning_mode = "other".into();
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ReasoningModeMismatch
        );
        let mut candidate = baseline.clone();
        candidate.reasoning_effort = "low".into();
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ReasoningEffortMismatch
        );
        let mut candidate = baseline.clone();
        candidate.native_state_policy = NativeStatePolicyV1::CleanRestart;
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::NativeStatePolicyMismatch
        );
        let mut candidate = baseline.clone();
        candidate
            .provider_extension
            .insert("phase".into(), json!(2));
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ProviderExtensionMismatch
        );
    }

    #[test]
    fn strict_rejects_every_numeric_downshift_and_effort_flag() {
        let baseline = contract();
        for (code, candidate) in [
            (
                ReasoningContractFailureCodeV1::OutputCeilingDownshift,
                ReasoningContractV1 {
                    max_output_tokens: baseline.max_output_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::ReasoningReserveDownshift,
                ReasoningContractV1 {
                    reserved_reasoning_tokens: baseline.reserved_reasoning_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::VisibleOutputReserveDownshift,
                ReasoningContractV1 {
                    reserved_visible_output_tokens: baseline.reserved_visible_output_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::RecoveryReserveDownshift,
                ReasoningContractV1 {
                    reserved_recovery_tokens: baseline.reserved_recovery_tokens - 1,
                    ..baseline.clone()
                },
            ),
        ] {
            assert_eq!(
                verify_strict_no_downshift_v1(&baseline, &candidate)
                    .unwrap_err()
                    .failure_code(),
                code
            );
        }
        let candidate = ReasoningContractV1 {
            allow_effort_downshift: true,
            ..baseline.clone()
        };
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::EffortDownshiftAllowed
        );
    }

    #[test]
    fn strict_permits_only_visible_numeric_reinvestment() {
        let baseline = contract();
        let candidate = ReasoningContractV1 {
            max_output_tokens: baseline.max_output_tokens + 100,
            reserved_reasoning_tokens: baseline.reserved_reasoning_tokens + 200,
            reserved_visible_output_tokens: baseline.reserved_visible_output_tokens + 50,
            reserved_recovery_tokens: baseline.reserved_recovery_tokens + 25,
            ..baseline.clone()
        };
        let admission = verify_strict_no_downshift_v1(&baseline, &candidate).unwrap();
        assert!(!admission.same_comparison_class());
        let record = admission.record();
        assert_eq!(record.max_output_tokens_added, 100);
        assert_eq!(record.reasoning_tokens_added, 200);
        assert_eq!(record.visible_output_tokens_added, 50);
        assert_eq!(record.recovery_tokens_added, 25);
    }

    #[test]
    fn headroom_is_reserved_before_input() {
        let contract = contract();
        assert_eq!(
            contract.admitted_input_ceiling(32_768, 1_024).unwrap(),
            20_480
        );
        assert_eq!(contract.admit_input(32_768, 1_024, 20_000).unwrap(), 480);
        assert_eq!(
            contract
                .admit_input(32_768, 1_024, 20_481)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InputExceedsHeadroom
        );
        assert_eq!(
            contract
                .admitted_input_ceiling(1_000, 1_024)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ContextCapacityExceeded
        );
    }

    #[test]
    fn admission_and_extension_tampering_fail_closed() {
        let baseline = contract();
        let admission = verify_strict_no_downshift_v1(&baseline, &baseline).unwrap();
        let mut record = admission.record();
        record.admission_digest = digest(99);
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::AdmissionDigestMismatch
        );
        let mut record = admission.record();
        record.reasoning_tokens_added = 1;
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::InvalidAdmission
        );
        let mut candidate = baseline.clone();
        candidate.provider_extension = BTreeMap::from([(
            "oversized".into(),
            Value::String("x".repeat(REASONING_CONTRACT_MAX_EXTENSION_BYTES_V1)),
        )]);
        assert_eq!(
            candidate.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge
        );
    }

    #[test]
    fn reasoning_contract_digest_is_stable() {
        assert_eq!(
            reasoning_contract_digest_v1(),
            DigestV1::from_hex("4906ff9514b220cbb8193f845d9f86eb5ea2423914a1974ec3eb309007230339")
                .unwrap()
        );
        assert_eq!(
            reasoning_contract_schema_digest_v1(),
            DigestV1::from_hex("80258e0d9c5b24ccdabd94bd5806a3e1407c99343def267b8ad99ca39f230db9")
                .unwrap()
        );
    }
