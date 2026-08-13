    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn epistemic(soundness: CwirSoundnessV1) -> CwirEpistemicProductV1 {
        CwirEpistemicProductV1 {
            authority: ArtifactOwnerV1::FsZero,
            soundness,
            coverage: CwirCoverageV1::Complete,
            freshness: CwirFreshnessV1::Current,
            determinism: CwirDeterminismV1::Deterministic,
        }
    }

    fn sample(reverse: bool, soundness: CwirSoundnessV1, state_byte: u8) -> CausalWorkIrV1 {
        let snapshot = digest(state_byte);
        let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
        let state = CwirStateAnchorV1 {
            project_root: digest(3),
            fs_snapshot: snapshot,
            graph_indexed_through: digest(4),
            toolchain: digest(5),
            runtime_manifest: digest(6),
            capability_surface: digest(7),
        };
        let state_node = CwirNodeV1::new(
            CwirNodeKindV1::State,
            digest(8),
            Some(snapshot),
            true,
            epistemic(CwirSoundnessV1::Exact),
            vec![],
        )
        .unwrap();
        let evidence = CwirNodeV1::new(
            CwirNodeKindV1::Evidence,
            digest(9),
            Some(snapshot),
            true,
            epistemic(soundness),
            vec![state_node.id],
        )
        .unwrap();
        let witness = CwirNodeV1::new(
            CwirNodeKindV1::Witness,
            digest(10),
            Some(snapshot),
            true,
            epistemic(CwirSoundnessV1::Exact),
            vec![evidence.id],
        )
        .unwrap();
        let edge = CwirHyperEdgeV1::new(
            CwirEdgeKindV1::Supports,
            vec![state_node.id, evidence.id],
            witness.id,
            Some(witness.id),
        )
        .unwrap();
        let obligation =
            CwirObligationV1::new_open(CwirObligationKindV1::Verification, false, snapshot, vec![])
                .unwrap()
                .transition(CwirObligationStatusV1::Discharged, Some(witness.id))
                .unwrap();
        let expansion = CwirExpansionV1::new(
            ArtifactOwnerV1::GraphZero,
            "graph.expand",
            digest(11),
            snapshot,
            CwirExpansionCostV1 {
                max_input_bytes: 512,
                max_output_bytes: 1024,
                max_work_units: 100,
            },
        )
        .unwrap();
        let mut nodes = vec![state_node, evidence, witness];
        if reverse {
            nodes.reverse();
        }
        CausalWorkIrV1::new(
            task,
            state,
            nodes,
            vec![edge],
            vec![obligation],
            CwirEffectSpaceV1::new(vec![digest(12)], vec![digest(14), digest(13)]).unwrap(),
            CwirVerificationContractV1 {
                verifier_digest: digest(15),
                predicate_digest: digest(16),
                scope_digest: digest(17),
                class: CwirVerifierClassV1::ExactChecker,
            },
            vec![expansion],
        )
        .unwrap()
    }

    #[test]
    fn canonical_round_trip_and_contract_digest_are_stable() {
        let cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let bytes = cwir.canonical_bytes().unwrap();
        assert_eq!(CausalWorkIrV1::from_canonical_bytes(&bytes).unwrap(), cwir);
        assert_eq!(
            cwir_contract_digest_v1().to_hex(),
            "f64a0d73c075bb7330943379d52a1d2da6bb9272f02d6b15254baf829b32b30c"
        );
    }

    #[test]
    fn insertion_order_is_semantically_invariant() {
        let left = sample(false, CwirSoundnessV1::Exact, 1);
        let right = sample(true, CwirSoundnessV1::Exact, 1);
        assert_eq!(left.semantic_digest(), right.semantic_digest());
        assert_eq!(
            left.canonical_bytes().unwrap(),
            right.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn every_epistemic_and_state_field_changes_semantic_identity() {
        let base = sample(false, CwirSoundnessV1::Exact, 1);
        let base_digest = base.semantic_digest();
        let semantic_with_state = |state: CwirStateAnchorV1| {
            CausalWorkIrV1::new(
                base.task.clone(),
                state,
                base.nodes.clone(),
                base.edges.clone(),
                base.obligations.clone(),
                base.effect_space.clone(),
                base.verification,
                base.expansions.clone(),
            )
            .unwrap()
            .semantic_digest()
        };
        let mut state = base.state;
        state.project_root = digest(18);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.graph_indexed_through = digest(19);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.toolchain = digest(20);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.runtime_manifest = digest(21);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.capability_surface = digest(22);
        assert_ne!(base_digest, semantic_with_state(state));
        assert_ne!(
            base_digest,
            sample(false, CwirSoundnessV1::Exact, 23).semantic_digest()
        );

        let semantic_with_epistemic = |epistemic: CwirEpistemicProductV1| {
            let snapshot = digest(1);
            let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
            let state = CwirStateAnchorV1 {
                project_root: digest(3),
                fs_snapshot: snapshot,
                graph_indexed_through: digest(4),
                toolchain: digest(5),
                runtime_manifest: digest(6),
                capability_surface: digest(7),
            };
            let claim = CwirNodeV1::new(
                CwirNodeKindV1::Claim,
                digest(8),
                Some(snapshot),
                true,
                epistemic,
                vec![],
            )
            .unwrap();
            CausalWorkIrV1::new(
                task,
                state,
                vec![claim],
                vec![],
                vec![],
                CwirEffectSpaceV1::new(vec![], vec![]).unwrap(),
                CwirVerificationContractV1 {
                    verifier_digest: digest(15),
                    predicate_digest: digest(16),
                    scope_digest: digest(17),
                    class: CwirVerifierClassV1::ExactChecker,
                },
                vec![],
            )
            .unwrap()
            .semantic_digest()
        };
        let base_epistemic = epistemic(CwirSoundnessV1::Exact);
        let base_epistemic_digest = semantic_with_epistemic(base_epistemic);
        let mut changed = base_epistemic;
        changed.authority = ArtifactOwnerV1::GraphZero;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.soundness = CwirSoundnessV1::SoundRestricted;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.coverage = CwirCoverageV1::ScopedComplete;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.freshness = CwirFreshnessV1::Unknown;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.determinism = CwirDeterminismV1::Conditional;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
    }

    #[test]
    fn stale_or_unbound_active_evidence_is_rejected() {
        let invalid_evidence = |required_snapshot: Option<DigestV1>, freshness: CwirFreshnessV1| {
            let snapshot = digest(1);
            let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
            let state = CwirStateAnchorV1 {
                project_root: digest(3),
                fs_snapshot: snapshot,
                graph_indexed_through: digest(4),
                toolchain: digest(5),
                runtime_manifest: digest(6),
                capability_surface: digest(7),
            };
            let state_node = CwirNodeV1::new(
                CwirNodeKindV1::State,
                digest(8),
                Some(snapshot),
                true,
                epistemic(CwirSoundnessV1::Exact),
                vec![],
            )
            .unwrap();
            let mut evidence_epistemic = epistemic(CwirSoundnessV1::Exact);
            evidence_epistemic.freshness = freshness;
            let evidence = CwirNodeV1::new(
                CwirNodeKindV1::Evidence,
                digest(9),
                required_snapshot,
                true,
                evidence_epistemic,
                vec![state_node.id],
            )
            .unwrap();
            CausalWorkIrV1::new(
                task,
                state,
                vec![state_node, evidence],
                vec![],
                vec![],
                CwirEffectSpaceV1::new(vec![], vec![]).unwrap(),
                CwirVerificationContractV1 {
                    verifier_digest: digest(15),
                    predicate_digest: digest(16),
                    scope_digest: digest(17),
                    class: CwirVerifierClassV1::ExactChecker,
                },
                vec![],
            )
            .unwrap_err()
            .failure_code()
        };

        assert_eq!(
            invalid_evidence(Some(digest(99)), CwirFreshnessV1::Current),
            CwirFailureCodeV1::SnapshotMismatch
        );
        assert_eq!(
            invalid_evidence(None, CwirFreshnessV1::Current),
            CwirFailureCodeV1::SnapshotMismatch
        );
        assert_eq!(
            invalid_evidence(Some(digest(1)), CwirFreshnessV1::Stale),
            CwirFailureCodeV1::StaleFact
        );
    }

    #[test]
    fn dangling_and_duplicate_references_are_rejected() {
        let error = CwirNodeV1::new(
            CwirNodeKindV1::Claim,
            digest(20),
            None,
            false,
            epistemic(CwirSoundnessV1::SoundRestricted),
            vec![digest(21), digest(21)],
        )
        .unwrap_err();
        assert_eq!(error.failure_code(), CwirFailureCodeV1::DuplicateIdentity);

        let mut cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let node = &mut cwir.nodes[0];
        node.provenance.push(digest(250));
        node.provenance.sort();
        node.id = node.expected_id().unwrap();
        cwir.nodes.sort_by_key(|item| item.id);
        assert_eq!(
            cwir.validate_body().unwrap_err().failure_code(),
            CwirFailureCodeV1::DanglingReference
        );
    }

    #[test]
    fn obligation_lifecycle_is_monotone_and_non_advisory_cannot_be_waived() {
        let obligation =
            CwirObligationV1::new_open(CwirObligationKindV1::Decision, false, digest(1), vec![])
                .unwrap();
        let error = obligation
            .transition(CwirObligationStatusV1::Waived, Some(digest(2)))
            .unwrap_err();
        assert_eq!(error.failure_code(), CwirFailureCodeV1::IllegalWaiver);
        let discharged = obligation
            .transition(CwirObligationStatusV1::Discharged, Some(digest(2)))
            .unwrap();
        assert_eq!(
            discharged
                .transition(CwirObligationStatusV1::InProgress, None)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::InvalidObligationTransition
        );
    }

    #[test]
    fn noncanonical_and_tampered_wire_bytes_are_rejected() {
        let cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let mut bytes = cwir.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::NonCanonicalEncoding
        );

        let mut value = serde_json::to_value(&cwir).unwrap();
        value["semantic_digest"] = Value::String(digest(99).to_hex());
        let bytes = canonical_json(&value).into_bytes();
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::SemanticDigestMismatch
        );

        let mut value = serde_json::to_value(&cwir).unwrap();
        value["nodes"].as_array_mut().unwrap().swap(0, 1);
        let bytes = canonical_json(&value).into_bytes();
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::NonCanonicalOrder
        );
    }

    #[test]
    fn exact_epistemic_status_fails_closed() {
        let mut invalid = epistemic(CwirSoundnessV1::Exact);
        invalid.coverage = CwirCoverageV1::Partial;
        assert_eq!(
            CwirNodeV1::new(
                CwirNodeKindV1::Evidence,
                digest(1),
                Some(digest(2)),
                true,
                invalid,
                vec![digest(3)],
            )
            .unwrap_err()
            .failure_code(),
            CwirFailureCodeV1::InvalidEpistemicProduct
        );
    }

    #[test]
    fn expansion_bounds_fail_loud() {
        let error = CwirExpansionV1::new(
            ArtifactOwnerV1::FsZero,
            "fs.expand",
            digest(1),
            digest(2),
            CwirExpansionCostV1 {
                max_input_bytes: CWIR_MAX_EXPANSION_INPUT_BYTES_V1 + 1,
                max_output_bytes: 1,
                max_work_units: 1,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.failure_code(),
            CwirFailureCodeV1::ExpansionLimitExceeded
        );
    }
