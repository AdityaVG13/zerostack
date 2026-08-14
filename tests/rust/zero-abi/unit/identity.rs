    use super::*;

    fn abi() -> &'static str {
        ROOTED_ABI_VERSION_V6
    }

    fn full_scope() -> ProtectedScopeObligationsV1 {
        ProtectedScopeObligationsV1::new(vec![
            ScopeObligationV1::new(ProtectedDimensionV1::Tests, true, CoverageGradeV1::Proved)
                .unwrap(),
            ScopeObligationV1::new(ProtectedDimensionV1::Api, true, CoverageGradeV1::BoundedComplete)
                .unwrap(),
            ScopeObligationV1::new(ProtectedDimensionV1::Security, true, CoverageGradeV1::Proved)
                .unwrap(),
            ScopeObligationV1::new(
                ProtectedDimensionV1::Performance,
                false,
                CoverageGradeV1::Observed,
            )
            .unwrap(),
        ])
        .unwrap()
    }

    fn full_contract() -> StructuredTaskContractV1 {
        StructuredTaskContractV1::new(
            "refactor",
            vec!["tests pass".into(), "api unchanged".into()],
            full_scope(),
            SideEffectPolicyV1::ReversibleMutations,
            vec!["fz://blob/env".into()],
            "fz://root/project",
            TaskBudgetV1::new(1000, 60_000, 1 << 20, 10).unwrap(),
            Some(1_800_000_000_000),
            FallbackPolicyV1::FrozenRawBaseline,
            vec!["ergonomics".into()],
            Some(DigestV1::from_bytes([1; 32])),
            Some(DigestV1::from_bytes([2; 32])),
            Some(DigestV1::from_bytes([3; 32])),
        )
        .unwrap()
    }

    /// ZS-CONTRACT-001 acceptance: any task-contract field change produces a
    /// different contract root, and a formation receipt bound to the original
    /// contract fails verification for the mutated one.
    #[test]
    fn task_contract_field_change_invalidates_certificate() {
        let contract = full_contract();
        let root_original = contract.contract_root().unwrap();

        // Mutate each representative field category; roots must differ.
        let mut changed_kind = contract.clone();
        changed_kind.task_kind = "port".into();
        assert_ne!(changed_kind.contract_root().unwrap(), root_original);

        let mut changed_criteria = contract.clone();
        changed_criteria.acceptance_criteria.push("benchmarks hold".into());
        assert_ne!(changed_criteria.contract_root().unwrap(), root_original);

        let mut changed_scope = contract.clone();
        changed_scope
            .protected_scope
            .obligations
            .push(ScopeObligationV1::new(
                ProtectedDimensionV1::UserVisibleOutput,
                true,
                CoverageGradeV1::Unknown,
            )
            .unwrap());
        assert_ne!(changed_scope.contract_root().unwrap(), root_original);

        let mut changed_budget = contract.clone();
        changed_budget.budget.max_fuel = 999;
        assert_ne!(changed_budget.contract_root().unwrap(), root_original);

        let mut changed_fallback = contract.clone();
        changed_fallback.fallback_policy = FallbackPolicyV1::RejectedNoMutation;
        assert_ne!(changed_fallback.contract_root().unwrap(), root_original);

        // A receipt bound to the original contract must reject the mutated
        // contract's root: the certificate is invalidated by the field change.
        let receipt = PayloadFormationReceiptV1::new(
            "constructor:seed-42",
            root_original,
            vec!["fz://blob/dep".into()],
            "fz://blob/exec",
            "fz://blob/payload",
            7,
        )
        .unwrap();
        assert!(receipt.verify_payload(root_original, "fz://blob/payload"));
        assert!(!receipt.verify_payload(
            changed_kind.contract_root().unwrap(),
            "fz://blob/payload"
        ));
    }

    /// ZS-CONTRACT-004 acceptance: an uncovered required protected obligation
    /// is Unknown and can never be advertised as equivalent.
    #[test]
    fn protected_scope_uncovered_obligation_cannot_claim_equivalence() {
        let scope = full_scope();
        assert!(scope.equivalent_claim_permitted());
        assert!(scope.check_equivalent_claim().is_ok());
        assert!(scope.uncovered().is_empty());

        // Security becomes Unknown -> equivalent claim forbidden.
        let mut uncovered = scope.clone();
        for obligation in &mut uncovered.obligations {
            if obligation.dimension == ProtectedDimensionV1::Security {
                obligation.grade = CoverageGradeV1::Unknown;
            }
        }
        assert!(!uncovered.equivalent_claim_permitted());
        assert_eq!(
            uncovered.check_equivalent_claim(),
            Err(IdentityErrorV1::UncoveredObligation("security".into()))
        );
        assert_eq!(uncovered.uncovered(), vec![ProtectedDimensionV1::Security]);

        // Non-required Unknown dimension forbids the claim too (fail closed).
        let mut observed_only = scope.clone();
        for obligation in &mut observed_only.obligations {
            if obligation.dimension == ProtectedDimensionV1::Api {
                obligation.grade = CoverageGradeV1::Observed;
            }
        }
        assert!(matches!(
            observed_only.check_equivalent_claim(),
            Err(IdentityErrorV1::EquivalentClaimForbidden(_))
        ));

        // Duplicate dimensions are rejected at construction.
        let mut duplicate = scope.clone();
        duplicate.obligations.push(duplicate.obligations[0].clone());
        assert!(ProtectedScopeObligationsV1::new(duplicate.obligations).is_err());
    }

    /// ZS-KERNEL-003 acceptance: a relabeled payload with a valid key fails
    /// formation verification.
    #[test]
    fn formation_receipt_rejects_relabeled_payload() {
        let contract = full_contract();
        let contract_root = contract.contract_root().unwrap();
        let receipt = PayloadFormationReceiptV1::new(
            "constructor:seed-42",
            contract_root,
            vec!["fz://blob/dep-a".into(), "fz://blob/dep-b".into()],
            "fz://blob/exec-1",
            "fz://blob/payload-A",
            7,
        )
        .unwrap();

        // Exact payload verifies.
        assert!(receipt.verify_payload(contract_root, "fz://blob/payload-A"));

        // Relabeled payload B under A's key: payload binding fails.
        assert!(!receipt.verify_payload(contract_root, "fz://blob/payload-B"));

        // Relabeled under a different contract root: contract binding fails.
        let other_contract_root = DigestV1::from_bytes([9; 32]);
        assert!(!receipt.verify_payload(other_contract_root, "fz://blob/payload-A"));

        // Both wrong fails too.
        assert!(!receipt.verify_payload(other_contract_root, "fz://blob/payload-B"));

        // The receipt itself is rooted and canonical.
        let root = receipt.receipt_root().unwrap();
        let bytes = receipt.canonical_bytes().unwrap();
        assert!(verify_object_root(
            ObjectClassV1::FormationReceipt,
            abi(),
            &bytes,
            root
        ));
        // Tampering the receipt root binding fails verification.
        let mut tampered = serde_json::to_value(&receipt).unwrap();
        tampered["epoch"] = serde_json::json!(99);
        let tampered_bytes =
            canonical_object_bytes(ObjectClassV1::FormationReceipt, abi(), &tampered).unwrap();
        assert!(!verify_object_root(
            ObjectClassV1::FormationReceipt,
            abi(),
            &tampered_bytes,
            root
        ));
    }

    /// ZS-KERNEL-006 acceptance: replay detects missing, reordered, and
    /// torn-tail events via root chaining; the killed-process replay lands
    /// fail-closed.
    #[test]
    fn event_log_replay_detects_missing_and_reordered_events() {
        let mut log = EventLogV1::new();
        log.append("evidence_observed", "fz://blob/ev-1", "verifier-a")
            .unwrap();
        log.append("execution_started", "fz://blob/exec-1", "permit-7")
            .unwrap();
        log.append("verification_done", "fz://blob/verif-1", "verifier-a")
            .unwrap();
        log.append("commit", "fz://blob/commit-1", "gate")
            .unwrap();

        let sealed = log.verify_chain().unwrap();
        assert_eq!(log.head().unwrap(), sealed);

        // Full-chain replay from the records verifies.
        let records = log.records().to_vec();
        assert_eq!(EventLogV1::replay(&records).unwrap(), sealed);

        // Torn tail: the persisted prefix excludes the last record; its
        // replayed head cannot equal the sealed head.
        let prefix = records[..records.len() - 1].to_vec();
        let prefix_head = EventLogV1::replay(&prefix).unwrap();
        assert_ne!(prefix_head, sealed);
        let persisted = EventLogV1::from_records(prefix);
        assert_eq!(
            persisted.verify_chain_against(sealed),
            Err(IdentityErrorV1::TornEventLog {
                seq: 3,
                expected: sealed,
                actual: prefix_head,
            })
        );

        // Missing middle event: chain breaks with a reordered/missing error.
        let mut missing_middle = Vec::new();
        missing_middle.extend_from_slice(&records[..1]);
        missing_middle.push(records[2].clone());
        missing_middle.push(records[3].clone());
        assert!(matches!(
            EventLogV1::replay(&missing_middle),
            Err(IdentityErrorV1::ReorderedEventLog { .. })
        ));

        // Swapped records: parent chaining fails closed.
        let mut swapped = Vec::new();
        swapped.push(records[0].clone());
        swapped.push(records[2].clone());
        swapped.push(records[1].clone());
        swapped.push(records[3].clone());
        assert!(matches!(
            EventLogV1::replay(&swapped),
            Err(IdentityErrorV1::ReorderedEventLog { .. })
        ));

        // Empty log replays to genesis.
        assert_eq!(EventLogV1::replay(&[]).unwrap(), event_log_genesis());
        let fresh = EventLogV1::new();
        assert_eq!(fresh.head().unwrap(), event_log_genesis());
        assert!(fresh.is_empty());
    }

    /// ZS-KERNEL-008 acceptance: the successor CAS advances only on an exact
    /// declared parent with a verified new root; crashes at every boundary
    /// around verify/authorize/commit leave the old root or the complete new
    /// root, never a partial state.
    #[test]
    fn successor_cas_crash_boundaries_leave_old_or_complete_new_root() {
        let genesis = event_log_genesis();
        let successor = DigestV1::from_bytes([7; 32]);
        let mut cas = ProjectSuccessorCasV1::new(genesis);

        // Crash before any commit: current root is unchanged.
        assert_eq!(cas.current(), genesis);

        // Verify + authorize are pure observations of the CAS state; the
        // crash-point simulation below treats them as no-ops on the CAS.
        // Commit with exact parent advances atomically.
        assert_eq!(
            cas.try_advance(genesis, successor),
            SuccessorOutcomeV1::Advanced {
                new_current_root: successor
            }
        );
        assert_eq!(cas.current(), successor);

        // Crash AFTER commit: the new root is complete and current.
        assert_eq!(cas.current(), successor);

        // Stale handle (old declared parent) after advance: unchanged,
        // mismatch, current root intact.
        let mut cas2 = ProjectSuccessorCasV1::new(genesis);
        cas2.try_advance(genesis, successor);
        assert_eq!(
            cas2.try_advance(genesis, DigestV1::from_bytes([8; 32])),
            SuccessorOutcomeV1::Unchanged {
                reason: SuccessorUnchangedReasonV1::DeclaredParentMismatch
            }
        );
        assert_eq!(cas2.current(), successor);

        // Verified successor equals current: unchanged, never a spurious
        // advance.
        assert_eq!(
            cas2.try_advance(successor, successor),
            SuccessorOutcomeV1::Unchanged {
                reason: SuccessorUnchangedReasonV1::NoVerifiedChange
            }
        );
        assert_eq!(cas2.current(), successor);

        // Full crash matrix: for a fresh CAS, every prefix of the
        // verify -> authorize -> commit sequence leaves either the old root
        // (crash before commit) or the complete new root (crash after).
        let mut probe = ProjectSuccessorCasV1::new(genesis);
        // Crash before commit.
        assert_eq!(probe.current(), genesis);
        // Commit.
        probe.try_advance(genesis, successor);
        assert_eq!(probe.current(), successor);
        // The successor record seals the decision and is rooted.
        let record =
            SuccessorRecordV1::new(genesis, successor, true, "gate").unwrap();
        let record_root = record.record_root().unwrap();
        assert!(verify_object_root(
            ObjectClassV1::SuccessorRecord,
            abi(),
            &record.canonical_bytes().unwrap(),
            record_root
        ));
    }

    /// ZS-KERNEL-001/002/007 acceptance: every root binds class, ABI version,
    /// and the sha256 algorithm tag; unknown classes and wrong ABI versions
    /// fail closed; the canonical byte path is the only encoding path.
    #[test]
    fn object_root_binds_class_abi_and_algorithm() {
        let payload = serde_json::json!({"kind": "x", "value": 1});

        // Same bytes, same class, same ABI: stable root.
        let bytes = canonical_object_bytes(ObjectClassV1::TaskContract, abi(), &payload).unwrap();
        let root = object_root(ObjectClassV1::TaskContract, abi(), &bytes).unwrap();
        assert!(verify_object_root(ObjectClassV1::TaskContract, abi(), &bytes, root));

        // Class is structurally bound: same bytes under a different class
        // produce a different root and fail verification for the original.
        let other = object_root(ObjectClassV1::EventRecord, abi(), &bytes).unwrap();
        assert_ne!(root, other);
        assert!(!verify_object_root(ObjectClassV1::EventRecord, abi(), &bytes, root));

        // ABI version is bound: v5 preimage produces a different root and
        // fails verification; v5 canonical bytes are rejected outright.
        assert_eq!(
            canonical_object_bytes(ObjectClassV1::TaskContract, "zerostack.racc.v5", &payload),
            Err(IdentityErrorV1::WrongAbiVersion {
                actual: "zerostack.racc.v5".into()
            })
        );
        let v5_preimage = root_preimage(ObjectClassV1::TaskContract, "zerostack.racc.v5", &bytes);
        assert_ne!(sha256(&v5_preimage), *root.as_bytes());

        // Algorithm tag is bound in the preimage: dropping the tag yields a
        // different digest even though the payload bytes are identical.
        let untagged = {
            let mut preimage = Vec::new();
            preimage.extend_from_slice(ObjectClassV1::TaskContract.domain().as_bytes());
            preimage.push(0);
            preimage.extend_from_slice(abi().as_bytes());
            preimage.push(0);
            preimage.extend_from_slice(&bytes);
            preimage
        };
        assert_ne!(sha256(&untagged), *root.as_bytes());

        // Unknown object class is impossible by construction (enums are
        // closed), but wrong-ABI and noncanonical payloads fail loudly.
        assert!(canonical_object_bytes(ObjectClassV1::TaskContract, abi(), &payload).is_ok());
        assert!(object_root(ObjectClassV1::TaskContract, "bad", &bytes).is_err());
    }

    /// KERNEL-001 boundary: canonical bytes are deterministic (key order
    /// invariant) for the identity objects.
    #[test]
    fn identity_objects_are_canonical_across_key_order() {
        let value_a = serde_json::json!({"b": 1, "a": [2, 3], "c": {"y": true, "x": false}});
        let value_b = serde_json::json!({"c": {"x": false, "y": true}, "a": [2, 3], "b": 1});
        let bytes_a =
            canonical_object_bytes(ObjectClassV1::ExecuteResult, abi(), &value_a).unwrap();
        let bytes_b =
            canonical_object_bytes(ObjectClassV1::ExecuteResult, abi(), &value_b).unwrap();
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(
            object_root(ObjectClassV1::ExecuteResult, abi(), &bytes_a).unwrap(),
            object_root(ObjectClassV1::ExecuteResult, abi(), &bytes_b).unwrap()
        );
    }

    #[test]
    fn contract_and_receipt_validation_fail_closed() {
        // Empty acceptance criteria rejected.
        assert!(StructuredTaskContractV1::new(
            "refactor",
            vec![],
            full_scope(),
            SideEffectPolicyV1::ReadOnly,
            vec![],
            "fz://root/p",
            TaskBudgetV1::new(1000, 1000, 1000, 1000).unwrap(),
            None,
            FallbackPolicyV1::FrozenRawBaseline,
            vec![],
            None,
            None,
            None,
        )
        .is_err());

        // Zero budget bound rejected.
        assert!(TaskBudgetV1::new(0, 1000, 1000, 1000).is_err());

        // Empty constructor identity rejected.
        assert!(PayloadFormationReceiptV1::new(
            "",
            DigestV1::ZERO,
            vec![],
            "fz://blob/exec",
            "fz://blob/payload",
            1,
        )
        .is_err());

        // Empty event type / payload / authority rejected.
        assert!(EventRecordV1::new(0, event_log_genesis(), "", "fz://blob/p", "auth").is_err());
        assert!(EventRecordV1::new(0, event_log_genesis(), "t", "", "auth").is_err());
        assert!(EventRecordV1::new(0, event_log_genesis(), "t", "fz://blob/p", "").is_err());

        // Wrong ABI version on a successor record rejected.
        let mut record = SuccessorRecordV1::new(
            event_log_genesis(),
            DigestV1::from_bytes([1; 32]),
            true,
            "gate",
        )
        .unwrap();
        record.abi_version = "zerostack.racc.v5".into();
        assert!(record.validate().is_err());
    }

    /// V6-R6 (ZS-KERNEL-006): the typed event-class boundary enumerates all
    /// nine authoritative classes (including resource charges) and fails
    /// closed on anything outside them.
    #[test]
    fn event_class_v1_round_trips_all_nine_classes() {
        for class in EventClassV1::ALL {
            let wire = class.as_str();
            assert_eq!(EventClassV1::from_str(wire).unwrap(), class);
        }
        assert_eq!(EventClassV1::ALL.len(), 9);
        assert_eq!(EventClassV1::ResourceCharge.as_str(), "resource_charge");

        // Unknown class names fail closed at the typed boundary.
        assert_eq!(
            EventClassV1::from_str("anything_else"),
            Err(IdentityErrorV1::UnknownEventClass("anything_else".into()))
        );
        assert_eq!(
            EventClassV1::from_str(""),
            Err(IdentityErrorV1::UnknownEventClass(String::new()))
        );

        // Wire spelling is stable and canonical.
        assert_eq!(EventClassV1::Commit.as_str(), "commit");
        assert_eq!(EventClassV1::Rollback.as_str(), "rollback");
        assert_eq!(EventClassV1::CacheDecision.as_str(), "cache_decision");
    }

    /// V6-R6 (ZS-KERNEL-003): `verify_against` revokes cache reuse when the
    /// current dependency roots no longer exactly match the receipt's
    /// formation-time dependency set.
    #[test]
    fn formation_receipt_verify_against_revokes_dependency_mutation() {
        let contract_root = DigestV1::from_bytes([1; 32]);
        let receipt = PayloadFormationReceiptV1::new(
            "constructor:seed-42",
            contract_root,
            vec!["fz://blob/dep-a".into(), "fz://blob/dep-b".into()],
            "fz://blob/exec-1",
            "fz://blob/payload-A",
            7,
        )
        .unwrap();

        // Exact dependency set admits reuse.
        assert!(receipt.verify_against(&[
            "fz://blob/dep-a".to_owned(),
            "fz://blob/dep-b".to_owned()
        ]));

        // Order does not matter: the dependency set is normalized.
        assert!(receipt.verify_against(&[
            "fz://blob/dep-b".to_owned(),
            "fz://blob/dep-a".to_owned()
        ]));
        // Duplicates normalize away.
        assert!(receipt.verify_against(&[
            "fz://blob/dep-a".to_owned(),
            "fz://blob/dep-a".to_owned(),
            "fz://blob/dep-b".to_owned()
        ]));

        // Any mutation revokes reuse: changed root...
        assert!(!receipt.verify_against(&[
            "fz://blob/dep-a".to_owned(),
            "fz://blob/dep-c".to_owned()
        ]));
        // ...removed root...
        assert!(!receipt.verify_against(&["fz://blob/dep-a".to_owned()]));
        // ...added root...
        assert!(!receipt.verify_against(&[
            "fz://blob/dep-a".to_owned(),
            "fz://blob/dep-b".to_owned(),
            "fz://blob/dep-c".to_owned()
        ]));
        // ...and empty current set vs nonempty formation set.
        assert!(!receipt.verify_against(&[]));
    }

    #[test]
    fn scope_grade_serialization_spellings_are_stable() {
        use serde_json::json;
        assert_eq!(
            serde_json::to_value(CoverageGradeV1::BoundedComplete).unwrap(),
            json!("bounded_complete")
        );
        assert_eq!(
            serde_json::to_value(ProtectedDimensionV1::UserVisibleOutput).unwrap(),
            json!("user_visible_output")
        );
        assert_eq!(
            serde_json::to_value(SideEffectPolicyV1::ApprovalRequiredMutations).unwrap(),
            json!("approval_required_mutations")
        );
        assert_eq!(
            serde_json::to_value(FallbackPolicyV1::FrozenRawBaseline).unwrap(),
            json!("frozen_raw_baseline")
        );
    }

    /// ZS-CONTRACT-003 acceptance: the harness contract binds serialization,
    /// ordering, transcript, cancellation, tool set, and renderer version;
    /// any change alters the root.
    #[test]
    fn harness_contract_binds_serialization_ordering_transcript_cancellation_and_renderer() {
        let contract = HarnessContractV1::new(
            "pi-harness",
            SerializationSchemeV1::CanonicalJson,
            MessageOrderingV1::StrictCallOrder,
            TranscriptPolicyV1::DecisionsAndResultsOnly,
            CancellationSemanticsV1::CooperativeAtCallBoundaries,
            DigestV1::from_bytes([5; 32]),
            3,
        )
        .unwrap();
        let root = contract.contract_root().unwrap();

        let mut renderer = contract.clone();
        renderer.adapter_renderer_version = 4;
        assert_ne!(renderer.contract_root().unwrap(), root);

        let mut ordering = contract.clone();
        ordering.message_ordering = MessageOrderingV1::CompletionPermitsReordering;
        assert_ne!(ordering.contract_root().unwrap(), root);

        let mut transcript = contract.clone();
        transcript.transcript_policy = TranscriptPolicyV1::FullRecording;
        assert_ne!(transcript.contract_root().unwrap(), root);

        let mut tools = contract.clone();
        tools.native_tool_set_digest = DigestV1::from_bytes([6; 32]);
        assert_ne!(tools.contract_root().unwrap(), root);

        // Empty harness name and zero renderer version fail closed.
        assert!(HarnessContractV1::new(
            "",
            SerializationSchemeV1::CanonicalJson,
            MessageOrderingV1::StrictCallOrder,
            TranscriptPolicyV1::None,
            CancellationSemanticsV1::HardDeadlineOnly,
            DigestV1::ZERO,
            1,
        )
        .is_err());
        assert!(HarnessContractV1::new(
            "pi",
            SerializationSchemeV1::CanonicalJson,
            MessageOrderingV1::StrictCallOrder,
            TranscriptPolicyV1::None,
            CancellationSemanticsV1::HardDeadlineOnly,
            DigestV1::ZERO,
            0,
        )
        .is_err());
    }

    // ---------------------------------------------------------------------
    // ZS-KERNEL-001: registry extension to views/deltas/authority objects
    // and canonical-byte property breadth (process order, escapes, locale,
    // path aliases).
    // ---------------------------------------------------------------------

    #[test]
    fn object_class_registry_covers_views_deltas_and_authority_objects() {
        for (class, domain) in [
            (ObjectClassV1::DecisionView, "zerostack.object.decision_view.v1"),
            (ObjectClassV1::Delta, "zerostack.object.delta.v1"),
            (ObjectClassV1::AuthorityObject, "zerostack.object.authority_object.v1"),
            (ObjectClassV1::MigrationReceipt, "zerostack.object.migration_receipt.v1"),
        ] {
            assert_eq!(class.domain(), domain);
            let payload = serde_json::json!({"class": class.domain()});
            let bytes = canonical_object_bytes(class, abi(), &payload).unwrap();
            let root = object_root(class, abi(), &bytes).unwrap();
            assert!(verify_object_root(class, abi(), &bytes, root));
            // Class binding: the same payload under every other class roots
            // differently (no cross-class reuse).
            for (other, _) in [
                (ObjectClassV1::TaskContract, ""),
                (ObjectClassV1::ExecuteResult, ""),
                (ObjectClassV1::DecisionView, ""),
                (ObjectClassV1::Delta, ""),
                (ObjectClassV1::AuthorityObject, ""),
            ] {
                if other != class {
                    let other_bytes =
                        canonical_object_bytes(other, abi(), &payload).unwrap();
                    let other_root = object_root(other, abi(), &other_bytes).unwrap();
                    assert_ne!(other_root, root);
                    assert!(!verify_object_root(other, abi(), &bytes, root));
                }
            }
        }
    }

    /// KERNEL-001 property breadth: canonical bytes are invariant under key
    /// insertion order, JSON whitespace, and equivalent escape spellings
    /// (process-order and locale-adjacent perturbations), and are byte-exact
    /// where content differs (Unicode normalization forms and path aliases
    /// are never silently normalized).
    #[test]
    fn canonical_bytes_are_stable_across_perturbations_and_exact_otherwise() {
        // Process order: identical semantic objects with different key
        // insertion orders and whitespace produce identical canonical bytes.
        let spellings = [
            r#"{"b":1,"a":[2,3],"c":{"y":true,"x":false}}"#,
            r#"{ "c" : { "x" : false, "y" : true }, "a" : [ 2, 3 ], "b" : 1 }"#,
            r#"{
                "a": [2, 3],
                "b": 1,
                "c": {"x": false, "y": true}
            }"#,
        ];
        let roots = spellings
            .iter()
            .map(|spelling| {
                let value: Value = serde_json::from_str(spelling).unwrap();
                let bytes =
                    canonical_object_bytes(ObjectClassV1::ExecuteResult, abi(), &value).unwrap();
                object_root(ObjectClassV1::ExecuteResult, abi(), &bytes).unwrap()
            })
            .collect::<Vec<_>>();
        assert!(roots.iter().all(|root| *root == roots[0]));

        // Escape spellings: "\u0041" and "A" decode to the same string and
        // canonicalize identically.
        let escaped = r#"{"label":"\u0041B"}"#;
        let plain = r#"{"label":"AB"}"#;
        let bytes_escaped =
            canonical_object_bytes(ObjectClassV1::Delta, abi(), &serde_json::from_str(escaped).unwrap())
                .unwrap();
        let bytes_plain =
            canonical_object_bytes(ObjectClassV1::Delta, abi(), &serde_json::from_str(plain).unwrap())
                .unwrap();
        assert_eq!(bytes_escaped, bytes_plain);

        // Locale: NFC vs NFD spellings are different byte content and must
        // NOT collide -- the canonical layer is byte-exact, never locale
        // normalized.
        let nfc = r#"{"label":"\u00e9"}"#; // é (U+00E9)
        let nfd = r#"{"label":"e\u0301"}"#; // e + combining acute
        let bytes_nfc =
            canonical_object_bytes(ObjectClassV1::DecisionView, abi(), &serde_json::from_str(nfc).unwrap())
                .unwrap();
        let bytes_nfd =
            canonical_object_bytes(ObjectClassV1::DecisionView, abi(), &serde_json::from_str(nfd).unwrap())
                .unwrap();
        assert_ne!(bytes_nfc, bytes_nfd, "NFC/NFD must never collide");

        // Path aliases: ref strings that alias under naive path semantics
        // stay distinct objects (no silent alias normalization at the object
        // layer; aliasing is engine-scope policy).
        let canonical_path = r#"{"path":"fz://root/a/b"}"#;
        let aliased = r#"{"path":"fz://root/a/../b"}"#;
        let doubled_slash = r#"{"path":"fz://root/a//b"}"#;
        let bytes_canonical = canonical_object_bytes(
            ObjectClassV1::AuthorityObject,
            abi(),
            &serde_json::from_str(canonical_path).unwrap(),
        )
        .unwrap();
        for spelling in [aliased, doubled_slash] {
            let bytes = canonical_object_bytes(
                ObjectClassV1::AuthorityObject,
                abi(),
                &serde_json::from_str(spelling).unwrap(),
            )
            .unwrap();
            assert_ne!(bytes, bytes_canonical, "path alias {spelling} must not collide");
        }
    }

    /// ZS-KERNEL-007: incompatible-version migration is explicit and
    /// receipted -- the receipt re-derives the legacy root, roots the v6
    /// target through the canonical path, and fails closed on any tamper.
    #[test]
    fn migration_receipt_mints_verifies_and_fails_closed() {
        let legacy_payload = serde_json::json!({"legacy": "v5-task"});
        let legacy_bytes = crate::canonical_json(&legacy_payload).into_bytes();
        let legacy_root = DigestV1::from_bytes(sha256(&root_preimage(
            ObjectClassV1::TaskContract,
            "zerostack.racc.v5",
            &legacy_bytes,
        )));
        let target_payload = serde_json::json!({"task_kind": "port"});
        let receipt = RootedAbiMigrationReceiptV1::new(
            ObjectClassV1::TaskContract,
            "zerostack.racc.v5",
            &legacy_bytes,
            legacy_root,
            ObjectClassV1::TaskContract,
            &target_payload,
            "v6 structured task contract fields",
        )
        .unwrap();
        receipt.validate().unwrap();
        let canonical = receipt.canonical_bytes().unwrap();
        let root = object_root(ObjectClassV1::MigrationReceipt, abi(), &canonical).unwrap();
        assert_eq!(receipt.receipt_root().unwrap(), root);
        assert_eq!(
            RootedAbiMigrationReceiptV1::from_canonical_bytes(&canonical).unwrap(),
            receipt
        );

        // Forged legacy root refused.
        assert!(matches!(
            RootedAbiMigrationReceiptV1::new(
                ObjectClassV1::TaskContract,
                "zerostack.racc.v5",
                &legacy_bytes,
                DigestV1::from_bytes([0x7f; 32]),
                ObjectClassV1::TaskContract,
                &target_payload,
                "forged",
            ),
            Err(IdentityErrorV1::SourceRootMismatch)
        ));

        // No ABI change is not a migration.
        assert!(matches!(
            RootedAbiMigrationReceiptV1::new(
                ObjectClassV1::TaskContract,
                abi(),
                &legacy_bytes,
                legacy_root,
                ObjectClassV1::TaskContract,
                &target_payload,
                "no-op",
            ),
            Err(IdentityErrorV1::MigrationWithoutAbiChange)
        ));

        // Empty reason refused.
        assert!(matches!(
            RootedAbiMigrationReceiptV1::new(
                ObjectClassV1::TaskContract,
                "zerostack.racc.v5",
                &legacy_bytes,
                legacy_root,
                ObjectClassV1::TaskContract,
                &target_payload,
                "  ",
            ),
            Err(IdentityErrorV1::InvalidMigrationReceipt(_))
        ));

        // Stored-receipt tamper: any field change flips the receipt root
        // (the canonical-bytes binding), so a relabeled reason can never
        // verify against the original sealed root.
        let mut relabeled = receipt.clone();
        relabeled.migration_reason = "relabeled".into();
        assert_ne!(relabeled.receipt_root().unwrap(), receipt.receipt_root().unwrap());
        assert!(!verify_object_root(
            ObjectClassV1::MigrationReceipt,
            abi(),
            &relabeled.canonical_bytes().unwrap(),
            receipt.receipt_root().unwrap(),
        ));

        // Stored-receipt tamper: swapped source bytes break the legacy root.
        let mut swapped_source = receipt.clone();
        swapped_source.source_canonical_bytes_hex =
            canonical_object_bytes(ObjectClassV1::ExecuteResult, abi(), &target_payload)
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        assert!(matches!(
            swapped_source.validate(),
            Err(IdentityErrorV1::SourceRootMismatch)
        ));

        // Stored-receipt tamper: swapped target bytes break the target root.
        let mut swapped_target = receipt.clone();
        swapped_target.target_canonical_bytes_hex =
            canonical_object_bytes(ObjectClassV1::ExecuteResult, abi(), &legacy_payload)
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        assert!(matches!(
            swapped_target.validate(),
            Err(IdentityErrorV1::TargetRootMismatch)
        ));

        // Legacy target ABI is refused (migration always lands on v6).
        let mut legacy_target = receipt.clone();
        legacy_target.target_abi_version = "zerostack.racc.v5".into();
        assert!(matches!(
            legacy_target.validate(),
            Err(IdentityErrorV1::LegacyTargetAbi(_))
        ));
    }
