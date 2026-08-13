    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn target(byte: u8, snapshot: DigestV1) -> EffectTargetV1 {
        EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: digest(byte),
            required_snapshot: snapshot,
        }
    }

    fn verification(snapshot: DigestV1) -> EffectVerificationPlanV1 {
        EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
            verifier_digest: digest(40),
            predicate_digest: digest(41),
            environment_digest: digest(42),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        }])
        .unwrap()
    }

    fn sample(target_order_reversed: bool) -> EffectProgramV1 {
        let snapshot = digest(1);
        let mut targets = vec![target(10, snapshot), target(11, snapshot)];
        if target_order_reversed {
            targets.reverse();
        }
        EffectProgramV1::new(
            snapshot,
            "replace_file",
            targets,
            vec![EffectPredicateV1 {
                predicate_digest: digest(20),
                scope_digest: digest(21),
                required_snapshot: snapshot,
            }],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: digest(11),
                expected_before: digest(12),
                replacement: digest(13),
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::SingleAtomic,
        )
        .unwrap()
    }

    #[test]
    fn canonical_round_trip_and_contract_digest_are_stable() {
        let program = sample(false);
        let bytes = program.canonical_bytes().unwrap();
        assert_eq!(
            EffectProgramV1::from_canonical_bytes(&bytes).unwrap(),
            program
        );
        assert_eq!(
            effect_ir_contract_digest_v1().to_hex(),
            "1cd2b4189f6a58172cca389a2acafb0ed420773f5f2bb33c941a01822d17204c"
        );
    }

    #[test]
    fn set_order_is_invariant_but_operation_order_is_semantic() {
        assert_eq!(sample(false).action_digest(), sample(true).action_digest());
        let snapshot = digest(1);
        let first = TypedEffectOperationV1::ReturnLiteral {
            bytes: b"first".to_vec(),
            payload_digest: DigestV1::from_bytes(sha256(b"first")),
        };
        let second = TypedEffectOperationV1::ReturnLiteral {
            bytes: b"second".to_vec(),
            payload_digest: DigestV1::from_bytes(sha256(b"second")),
        };
        let make = |operations| {
            EffectProgramV1::new(
                snapshot,
                "return_literal",
                vec![],
                vec![],
                operations,
                vec![],
                verification(snapshot),
                EffectRollbackV1::ReadOnly,
            )
            .unwrap()
        };
        assert_ne!(
            make(vec![first.clone(), second.clone()]).action_digest(),
            make(vec![second, first]).action_digest()
        );
    }

    #[test]
    fn stale_state_and_unlisted_capability_fail_closed() {
        let snapshot = digest(1);
        let program = EffectProgramV1::new(
            snapshot,
            "recover",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::RecoverExact {
                owner: ArtifactOwnerV1::FsZero,
                capability: "fs.recover".into(),
                generation: 7,
                capability_contract_digest: digest(30),
                arguments_digest: digest(31),
                expected_output_digest: digest(32),
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::ReadOnly,
        )
        .unwrap();
        let stale = EffectAdmissionV1::new(digest(2), vec!["recover".into()], vec![]).unwrap();
        assert_eq!(
            program.validate_against(&stale).unwrap_err().failure_code(),
            EffectIrFailureCodeV1::StaleBaseState
        );
        let wrong_intent = EffectAdmissionV1::new(snapshot, vec!["other".into()], vec![]).unwrap();
        assert_eq!(
            program
                .validate_against(&wrong_intent)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::UnlistedIntent
        );
        let unlisted = EffectAdmissionV1::new(snapshot, vec!["recover".into()], vec![]).unwrap();
        assert_eq!(
            program
                .validate_against(&unlisted)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::UnlistedCapability
        );
        let wrong_generation = EffectAdmissionV1::new(
            snapshot,
            vec!["recover".into()],
            vec![EffectCapabilityBindingV1 {
                owner: ArtifactOwnerV1::FsZero,
                capability: "fs.recover".into(),
                generation: 8,
                contract_digest: digest(30),
                max_effect_class: EffectClass::ReadOnly,
            }],
        )
        .unwrap();
        assert_eq!(
            program
                .validate_against(&wrong_generation)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::CapabilityGenerationMismatch
        );
        let admitted = EffectAdmissionV1::new(
            snapshot,
            vec!["recover".into()],
            vec![EffectCapabilityBindingV1 {
                owner: ArtifactOwnerV1::FsZero,
                capability: "fs.recover".into(),
                generation: 7,
                contract_digest: digest(30),
                max_effect_class: EffectClass::ReadOnly,
            }],
        )
        .unwrap();
        program.validate_against(&admitted).unwrap();

        let mutating = EffectProgramV1::new(
            snapshot,
            "mutate",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::InvokeCapability {
                owner: ArtifactOwnerV1::FsZero,
                capability: "fs.mutate".into(),
                generation: 1,
                capability_contract_digest: digest(50),
                arguments_digest: digest(51),
                effect_class: EffectClass::ReversibleMutation,
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::SingleAtomic,
        )
        .unwrap();
        let read_only_admission = EffectAdmissionV1::new(
            snapshot,
            vec!["mutate".into()],
            vec![EffectCapabilityBindingV1 {
                owner: ArtifactOwnerV1::FsZero,
                capability: "fs.mutate".into(),
                generation: 1,
                contract_digest: digest(50),
                max_effect_class: EffectClass::ReadOnly,
            }],
        )
        .unwrap();
        assert_eq!(
            mutating
                .validate_against(&read_only_admission)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::CapabilityEffectClassExceeded
        );
    }

    #[test]
    fn operation_targets_and_exceptions_must_resolve() {
        let snapshot = digest(1);
        let error = EffectProgramV1::new(
            snapshot,
            "replace_file",
            vec![target(10, snapshot)],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: digest(11),
                expected_before: digest(12),
                replacement: digest(13),
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::SingleAtomic,
        )
        .unwrap_err();
        assert_eq!(error.failure_code(), EffectIrFailureCodeV1::MissingTarget);

        let error = EffectProgramV1::new(
            snapshot,
            "transform",
            vec![target(10, snapshot)],
            vec![],
            vec![TypedEffectOperationV1::DeterministicTransform {
                owner: ArtifactOwnerV1::ZeroStack,
                capability: "zero.transform".into(),
                generation: 1,
                capability_contract_digest: digest(30),
                targets: vec![digest(10)],
                arguments_digest: digest(31),
                exceptions: vec![digest(32)],
                effect_class: EffectClass::ReadOnly,
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::ReadOnly,
        )
        .unwrap_err();
        assert_eq!(
            error.failure_code(),
            EffectIrFailureCodeV1::MissingException
        );
    }

    #[test]
    fn raw_fallback_is_first_class_and_never_mixed() {
        let raw = EffectProgramV1::new(
            digest(1),
            "raw_fallback",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::RawFallback],
            vec![],
            EffectVerificationPlanV1::new(vec![]).unwrap(),
            EffectRollbackV1::RawFallback,
        )
        .unwrap();
        assert_eq!(raw.operations(), &[TypedEffectOperationV1::RawFallback]);
        let error = EffectProgramV1::new(
            digest(1),
            "raw_fallback",
            vec![],
            vec![],
            vec![
                TypedEffectOperationV1::RawFallback,
                TypedEffectOperationV1::ReturnLiteral {
                    bytes: vec![],
                    payload_digest: DigestV1::from_bytes(sha256(&[])),
                },
            ],
            vec![],
            EffectVerificationPlanV1::new(vec![]).unwrap(),
            EffectRollbackV1::RawFallback,
        )
        .unwrap_err();
        assert_eq!(
            error.failure_code(),
            EffectIrFailureCodeV1::RawFallbackMixed
        );
    }

    #[test]
    fn weak_rollback_and_literal_tamper_fail_loud() {
        let snapshot = digest(1);
        let error = EffectProgramV1::new(
            snapshot,
            "replace_file",
            vec![target(10, snapshot)],
            vec![],
            vec![TypedEffectOperationV1::ReplaceExactFile {
                target: digest(10),
                expected_before: digest(11),
                replacement: digest(12),
            }],
            vec![],
            verification(snapshot),
            EffectRollbackV1::ReadOnly,
        )
        .unwrap_err();
        assert_eq!(error.failure_code(), EffectIrFailureCodeV1::RollbackTooWeak);
        let error = TypedEffectOperationV1::ReturnLiteral {
            bytes: b"bytes".to_vec(),
            payload_digest: digest(99),
        }
        .validate()
        .unwrap_err();
        assert_eq!(
            error.failure_code(),
            EffectIrFailureCodeV1::LiteralDigestMismatch
        );
        let oversized = vec![0_u8; EFFECT_IR_MAX_LITERAL_BYTES_V1 + 1];
        let error = TypedEffectOperationV1::ReturnLiteral {
            payload_digest: DigestV1::from_bytes(sha256(&oversized)),
            bytes: oversized,
        }
        .validate()
        .unwrap_err();
        assert_eq!(error.failure_code(), EffectIrFailureCodeV1::LiteralTooLarge);
    }

    #[test]
    fn canonical_tamper_and_whitespace_are_rejected() {
        let program = sample(false);
        let mut bytes = program.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            EffectProgramV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::NonCanonicalEncoding
        );
        let mut value = serde_json::to_value(program).unwrap();
        value["action_digest"] = Value::String(digest(99).to_hex());
        let bytes = canonical_json(&value).into_bytes();
        assert_eq!(
            EffectProgramV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            EffectIrFailureCodeV1::ActionDigestMismatch
        );
    }
