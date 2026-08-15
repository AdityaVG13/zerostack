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
            DigestV1::from_hex("e80d7959a30c34e8c494966403a6983c3d35d8abe93539617f209c781aba11bf")
                .unwrap()
        );
        assert_eq!(
            reasoning_contract_schema_digest_v1(),
            DigestV1::from_hex("42db4341e8dd2c8647c382ca32db8bf5b73e490b7663cfb4fb9ca33ba618bb31")
                .unwrap()
        );
    }

    // ---------------------------------------------------------------------
    // CONTRACT-002 invocation bindings: sampling params, stopping policy,
    // system prompt root, per-tool permissions.
    // ---------------------------------------------------------------------

    fn bound_contract() -> ReasoningContractV1 {
        contract()
            .with_invocation_bindings(
                SamplingParamsV1::new(500_000, 950_000, Some(42)).unwrap(),
                StoppingPolicyV1::new(
                    vec!["\\n\\n".to_owned(), "<|end|>".to_owned()],
                    Some(64),
                )
                .unwrap(),
                Some(digest(9)),
                BTreeMap::from([
                    (
                        "fs.multi_read".to_owned(),
                        ToolPermissionV1::new(true, false, Some(128)).unwrap(),
                    ),
                    (
                        "fs.transact".to_owned(),
                        ToolPermissionV1::new(false, true, None).unwrap(),
                    ),
                ]),
            )
            .unwrap()
    }

    #[test]
    fn invocation_bindings_participate_in_identity_and_round_trip() {
        let plain = contract();
        let bound = bound_contract();
        assert_ne!(
            plain.identity_digest().unwrap(),
            bound.identity_digest().unwrap(),
            "explicit invocation bindings must change the contract identity"
        );
        assert_eq!(
            bound.sampling_params(),
            Some(&SamplingParamsV1::new(500_000, 950_000, Some(42)).unwrap())
        );
        assert_eq!(bound.system_prompt_root(), Some(digest(9)));
        assert_eq!(bound.tool_permissions().len(), 2);
        let bytes = bound.canonical_bytes().unwrap();
        assert_eq!(
            ReasoningContractV1::from_canonical_bytes(&bytes).unwrap(),
            bound,
            "bound contract round-trips through canonical bytes"
        );
        // The canonical schema declares every new field.
        for key in [
            "sampling_params",
            "stopping_policy",
            "system_prompt_root",
            "tool_permissions",
        ] {
            assert!(
                reasoning_contract_schema_v1()["properties"].get(key).is_some(),
                "schema missing {key}"
            );
        }
        assert_eq!(
            reasoning_contract_schema_v1()["required"]
                .as_array()
                .unwrap()
                .len(),
            14,
            "the fourteen base fields stay required"
        );
    }

    #[test]
    fn invocation_binding_changes_reclassify_strict_pairs() {
        let baseline = bound_contract();
        let mut candidate = baseline.clone();
        candidate.sampling_params =
            Some(SamplingParamsV1::new(900_000, 950_000, Some(42)).unwrap());
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvocationBindingMismatch,
            "sampling change must reclassify"
        );
        let mut candidate = baseline.clone();
        candidate.stopping_policy = Some(
            StoppingPolicyV1::new(vec!["<|end|>".to_owned()], Some(32)).unwrap(),
        );
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvocationBindingMismatch,
            "stopping policy change must reclassify"
        );
        let mut candidate = baseline.clone();
        candidate.system_prompt_root = Some(digest(10));
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvocationBindingMismatch,
            "system prompt root change must reclassify"
        );
        let mut candidate = baseline.clone();
        candidate.tool_permissions.insert(
            "fs.write".to_owned(),
            ToolPermissionV1::new(false, true, Some(4)).unwrap(),
        );
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvocationBindingMismatch,
            "per-tool permission change must reclassify"
        );
        // Plain-vs-bound pair also reclassifies: absence is a declared state.
        assert_eq!(
            verify_strict_no_downshift_v1(&contract(), &bound_contract())
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvocationBindingMismatch
        );
    }

    #[test]
    fn invalid_invocation_bindings_fail_closed() {
        assert_eq!(
            SamplingParamsV1::new(2_000_001, 950_000, None)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidSamplingParams
        );
        assert_eq!(
            SamplingParamsV1::new(500_000, 0, None)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidSamplingParams
        );
        assert_eq!(
            StoppingPolicyV1::new(vec![], Some(0))
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidStoppingPolicy
        );
        assert_eq!(
            StoppingPolicyV1::new(vec!["bad\u{0}seq".to_owned()], None)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidStoppingPolicy
        );
        assert_eq!(
            contract()
                .with_invocation_bindings(
                    SamplingParamsV1::new(500_000, 950_000, None).unwrap(),
                    StoppingPolicyV1::new(vec![], None).unwrap(),
                    Some(DigestV1::ZERO),
                    BTreeMap::new(),
                )
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidSystemPromptRoot
        );
        assert_eq!(
            contract()
                .with_invocation_bindings(
                    SamplingParamsV1::new(500_000, 950_000, None).unwrap(),
                    StoppingPolicyV1::new(vec![], None).unwrap(),
                    None,
                    BTreeMap::from([(
                        "fs.read".to_owned(),
                        ToolPermissionV1 {
                            read_only: true,
                            approval_required: false,
                            max_calls: Some(0),
                        },
                    )]),
                )
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidToolPermissions
        );
        let oversized = (0..REASONING_CONTRACT_MAX_TOOL_PERMISSIONS_V1 + 1)
            .map(|index| {
                (
                    format!("tool.{index}"),
                    ToolPermissionV1::new(true, false, None).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            contract()
                .with_invocation_bindings(
                    SamplingParamsV1::new(500_000, 950_000, None).unwrap(),
                    StoppingPolicyV1::new(vec![], None).unwrap(),
                    None,
                    oversized,
                )
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InvalidToolPermissions
        );
    }
