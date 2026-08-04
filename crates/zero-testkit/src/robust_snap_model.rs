//! Exhaustive checker for the frozen three-world, three-effect Robust Snap model.

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use zero_abi::{
        sha256_hex, validate_heuristic_world_order, DigestV1, EvidenceDecisionTree, EvidenceLeafV1,
        EvidenceObservationV1, ProtectedEffectClassV1, ProtectedEffectSet, ProtectedEffectV1,
        RobustSnapCertificate, RobustSnapFailureCodeV1, SnapLevel, WorldFiberDescriptor,
        ROBUST_SNAP_MAX_EFFECTS, ROBUST_SNAP_MAX_EVIDENCE_DEPTH, ROBUST_SNAP_MAX_LEAVES,
        ROBUST_SNAP_MAX_WORLDS, ROBUST_SNAP_MODEL_VERSION,
    };

    const MODEL: &str = include_str!("../../../conformance/models/robust-snap-v1.json");
    const CORRESPONDENCE: &str =
        include_str!("../../../conformance/models/robust-snap-rust-correspondence-v1.md");

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn effect(index: u8) -> ProtectedEffectV1 {
        ProtectedEffectV1 {
            effect_digest: d(20 + index),
            effect_class: ProtectedEffectClassV1::ReversibleMutation,
        }
    }

    fn effects(mask: u8) -> Vec<ProtectedEffectV1> {
        (0..3)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(effect)
            .collect()
    }

    fn fiber() -> WorldFiberDescriptor {
        WorldFiberDescriptor {
            model_version: ROBUST_SNAP_MODEL_VERSION.into(),
            assembly_manifest_digest: d(9),
            source_image_digest: d(8),
            task_fingerprint: d(7),
            assumptions: vec![
                "finite complete world enumeration".into(),
                "protected sets exact in frozen domain".into(),
            ],
            worlds: vec![d(1), d(2), d(3)],
        }
    }

    fn protected(masks: [u8; 3]) -> Vec<ProtectedEffectSet> {
        masks
            .into_iter()
            .enumerate()
            .map(|(index, mask)| ProtectedEffectSet {
                world_id: d(index as u8 + 1),
                effects: effects(mask),
            })
            .collect()
    }

    #[test]
    fn robust_snap_model_artifact_and_assumptions_are_frozen() {
        assert_eq!(
            sha256_hex(MODEL.as_bytes()),
            "c29ef4b4cd58371c314fc663a9837d50cddc66ea58827493f8f7358db5cf9622"
        );
        assert_eq!(
            sha256_hex(CORRESPONDENCE.as_bytes()),
            "50766924640fdedc06ba89124b76388b9a4bc46769227e46c76f35994a969b24"
        );
        let model: Value = serde_json::from_str(MODEL).unwrap();
        assert_eq!(model["model_version"], ROBUST_SNAP_MODEL_VERSION);
        assert_eq!(
            model["finite_domains"]["worlds_max"],
            ROBUST_SNAP_MAX_WORLDS
        );
        assert_eq!(
            model["finite_domains"]["effects_max"],
            ROBUST_SNAP_MAX_EFFECTS
        );
        assert_eq!(
            model["finite_domains"]["evidence_leaves_max"],
            ROBUST_SNAP_MAX_LEAVES
        );
        assert_eq!(
            model["finite_domains"]["evidence_depth_max"],
            ROBUST_SNAP_MAX_EVIDENCE_DEPTH
        );
        assert_eq!(model["unknown_is_verified"], false);
        assert_eq!(
            model["abstract_certificate_grants_operational_execution"],
            false
        );
    }

    #[test]
    fn robust_snap_model_exhaustively_proves_s0_common_effect_law() {
        let mut checked = 0_u64;
        let mut admitted = 0_u64;
        for world_a in 0_u8..8 {
            for world_b in 0_u8..8 {
                for world_c in 0_u8..8 {
                    for first_turn in 0_u8..8 {
                        for verifiable in 0_u8..8 {
                            checked += 1;
                            let common = world_a & world_b & world_c & first_turn & verifiable;
                            let selected = effect(common.trailing_zeros().min(2) as u8);
                            let result = RobustSnapCertificate::create_s0(
                                fiber(),
                                protected([world_a, world_b, world_c]),
                                effects(first_turn),
                                effects(verifiable),
                                selected.clone(),
                            );
                            if common == 0 {
                                assert_eq!(
                                    result.unwrap_err().code(),
                                    RobustSnapFailureCodeV1::EmptyCommonProtectedEffectSet
                                );
                            } else {
                                admitted += 1;
                                let certificate = result.unwrap();
                                certificate.validate().unwrap();
                                assert!(certificate
                                    .common_s0_effects()
                                    .unwrap()
                                    .contains(&selected));
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 32_768);
        assert!(admitted > 0 && admitted < checked);
    }

    #[test]
    fn robust_snap_model_exhaustively_proves_s1_nonempty_leaf_law() {
        let partitions: [&[&[u8]]; 5] = [
            &[&[1, 2, 3]],
            &[&[1], &[2, 3]],
            &[&[2], &[1, 3]],
            &[&[3], &[1, 2]],
            &[&[1], &[2], &[3]],
        ];
        let mut checked = 0_u64;
        let mut admitted = 0_u64;
        for masks_code in 0_u16..512 {
            let masks = [
                (masks_code & 7) as u8,
                ((masks_code >> 3) & 7) as u8,
                ((masks_code >> 6) & 7) as u8,
            ];
            for verifiable in 0_u8..8 {
                for partition in partitions {
                    checked += 1;
                    let mut valid = true;
                    let leaves = partition
                        .iter()
                        .enumerate()
                        .map(|(leaf_index, worlds)| {
                            let common = worlds.iter().fold(verifiable, |value, world| {
                                value & masks[usize::from(*world - 1)]
                            });
                            valid &= common != 0;
                            EvidenceLeafV1 {
                                path: vec![EvidenceObservationV1 {
                                    evidence_id: d(40),
                                    outcome_digest: d(50 + leaf_index as u8),
                                }],
                                admitted_worlds: worlds.iter().map(|world| d(*world)).collect(),
                                selected_effect: effect(common.trailing_zeros().min(2) as u8),
                            }
                        })
                        .collect();
                    let result = RobustSnapCertificate::create_s1(
                        fiber(),
                        protected(masks),
                        effects(verifiable),
                        EvidenceDecisionTree {
                            evidence_schema_digest: d(41),
                            leaves,
                        },
                    );
                    assert_eq!(result.is_ok(), valid);
                    if let Ok(certificate) = result {
                        admitted += 1;
                        certificate.validate().unwrap();
                    }
                }
            }
        }
        assert_eq!(checked, 20_480);
        assert!(admitted > 0 && admitted < checked);
    }

    #[test]
    fn robust_snap_model_rejects_all_preregistered_mutants() {
        let base_sets = protected([3, 5, 7]);
        let mut empty_leaf = EvidenceDecisionTree {
            evidence_schema_digest: d(41),
            leaves: vec![EvidenceLeafV1 {
                path: vec![EvidenceObservationV1 {
                    evidence_id: d(40),
                    outcome_digest: d(50),
                }],
                admitted_worlds: Vec::new(),
                selected_effect: effect(0),
            }],
        };
        assert_eq!(
            RobustSnapCertificate::create_s1(
                fiber(),
                base_sets.clone(),
                effects(7),
                empty_leaf.clone(),
            )
            .unwrap_err()
            .code(),
            RobustSnapFailureCodeV1::EmptyEvidenceLeaf
        );

        assert_eq!(
            validate_heuristic_world_order(&fiber(), &[d(1), d(2)])
                .unwrap_err()
                .code(),
            RobustSnapFailureCodeV1::HeuristicDroppedWorld
        );

        let irreversible = ProtectedEffectV1 {
            effect_digest: d(99),
            effect_class: ProtectedEffectClassV1::Irreversible,
        };
        assert_eq!(
            RobustSnapCertificate::create_s0(
                fiber(),
                base_sets,
                vec![effect(0), irreversible.clone()],
                vec![effect(0), irreversible.clone()],
                irreversible,
            )
            .unwrap_err()
            .code(),
            RobustSnapFailureCodeV1::SelectedEffectNotCommon
        );

        let mut certificate = RobustSnapCertificate::create_s0(
            fiber(),
            protected([1, 1, 1]),
            effects(1),
            effects(1),
            effect(0),
        )
        .unwrap();
        certificate.snap_level = SnapLevel::Unknown;
        assert_eq!(
            certificate.validate().unwrap_err().code(),
            RobustSnapFailureCodeV1::UnknownCannotPass
        );
        assert!(!SnapLevel::Unknown.is_verified());
        assert!(!SnapLevel::S1.permits_operational_execution());

        empty_leaf.leaves[0].admitted_worlds = vec![d(1)];
        assert_eq!(
            RobustSnapCertificate::create_s1(
                fiber(),
                protected([1, 1, 1]),
                effects(1),
                empty_leaf,
            )
            .unwrap_err()
            .code(),
            RobustSnapFailureCodeV1::EvidenceTreeDropsWorld
        );
    }
}
