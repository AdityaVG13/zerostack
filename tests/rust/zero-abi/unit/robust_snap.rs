    use super::*;

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn effect(byte: u8) -> ProtectedEffectV1 {
        ProtectedEffectV1 {
            effect_digest: d(byte),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        }
    }

    fn fiber() -> WorldFiberDescriptor {
        WorldFiberDescriptor {
            model_version: ROBUST_SNAP_MODEL_VERSION.into(),
            assembly_manifest_digest: d(9),
            source_image_digest: d(8),
            task_fingerprint: d(7),
            assumptions: vec!["finite complete world enumeration".into()],
            worlds: vec![d(1), d(2)],
        }
    }

    fn sets() -> Vec<ProtectedEffectSet> {
        vec![
            ProtectedEffectSet {
                world_id: d(1),
                effects: vec![effect(1), effect(2)],
            },
            ProtectedEffectSet {
                world_id: d(2),
                effects: vec![effect(1), effect(3)],
            },
        ]
    }

    #[test]
    fn robust_snap_s0_common_effect_and_digest_are_stable() {
        let certificate = RobustSnapCertificate::create_s0(
            fiber(),
            sets(),
            vec![effect(1), effect(2), effect(3)],
            vec![effect(1), effect(2), effect(3)],
            effect(1),
        )
        .unwrap();
        certificate.validate().unwrap();
        assert_eq!(certificate.common_s0_effects().unwrap(), vec![effect(1)]);
        assert_eq!(
            certificate.certificate_digest.to_hex(),
            "fb8091547a3842546b58403f696b1818ca32d8ac1c4fdf5c67280ad2f2651637"
        );
        assert_eq!(
            robust_snap_contract_digest_v1().to_hex(),
            "ac04866b2a0737d5845376d165b516ee5eeb4656c535cffd881ebbfbcf6075fa"
        );
        assert!(!certificate.snap_level.permits_operational_execution());
    }

    #[test]
    fn robust_snap_s1_nonempty_exhaustive_leaves_preserve_effects() {
        let tree = EvidenceDecisionTree {
            evidence_schema_digest: d(10),
            leaves: vec![
                EvidenceLeafV1 {
                    path: vec![EvidenceObservationV1 {
                        evidence_id: d(11),
                        outcome_digest: d(1),
                    }],
                    admitted_worlds: vec![d(1)],
                    selected_effect: effect(2),
                },
                EvidenceLeafV1 {
                    path: vec![EvidenceObservationV1 {
                        evidence_id: d(11),
                        outcome_digest: d(2),
                    }],
                    admitted_worlds: vec![d(2)],
                    selected_effect: effect(3),
                },
            ],
        };
        let certificate = RobustSnapCertificate::create_s1(
            fiber(),
            sets(),
            vec![effect(1), effect(2), effect(3)],
            tree,
        )
        .unwrap();
        certificate.validate().unwrap();
        assert_eq!(certificate.snap_level, SnapLevel::S1);
    }

    #[test]
    fn robust_snap_mutants_are_typed_and_unknown_never_passes() {
        let mut certificate = RobustSnapCertificate::create_s0(
            fiber(),
            sets(),
            vec![effect(1)],
            vec![effect(1)],
            effect(1),
        )
        .unwrap();
        certificate.snap_level = SnapLevel::Unknown;
        assert_eq!(
            certificate.validate().unwrap_err().code(),
            RobustSnapFailureCodeV1::UnknownCannotPass
        );
        assert!(!SnapLevel::Unknown.is_verified());

        let mut effects = sets();
        effects[1].effects = vec![effect(3)];
        assert_eq!(
            RobustSnapCertificate::create_s0(
                fiber(),
                effects,
                vec![effect(1)],
                vec![effect(1)],
                effect(1)
            )
            .unwrap_err()
            .code(),
            RobustSnapFailureCodeV1::EmptyCommonProtectedEffectSet
        );
    }

    #[test]
    fn robust_snap_heuristics_can_reorder_but_not_drop_worlds() {
        let fiber = fiber();
        validate_heuristic_world_order(&fiber, &[d(2), d(1)]).unwrap();
        assert_eq!(
            validate_heuristic_world_order(&fiber, &[d(1)])
                .unwrap_err()
                .code(),
            RobustSnapFailureCodeV1::HeuristicDroppedWorld
        );
    }
