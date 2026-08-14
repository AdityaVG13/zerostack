    use super::*;

    use crate::zero_execute::ContinuationStateV1::{
        Authorized, Bound, Committed, Executing, Planned, WaitingDecision,
    };

    fn root(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    use crate::zero_execute::ContinuationStateV1 as S;

    /// Walk the legal forward chain to the given state (one hop per call).
    fn chain(handle: ContinuationHandleV1, to: S) -> ContinuationHandleV1 {
        let order = [
            S::Bound,
            S::Snapshotted,
            S::Resolved,
            S::WaitingDecision,
            S::Planned,
            S::Executing,
            S::DeltaSealed,
            S::Verifying,
            S::Authorized,
            S::Committed,
        ];
        let target = order.iter().position(|state| *state == to).expect("chainable target");
        let start = order
            .iter()
            .position(|state| *state == handle.state())
            .expect("current state chainable");
        let mut current = handle;
        for state in &order[start + 1..=target] {
            let policy = matches!((current.state(), *state), (S::WaitingDecision, S::Planned));
            current = current.advance(*state, policy).unwrap_or_else(|error| {
                panic!("chain to {to:?} failed at {:?} -> {state:?}: {error}", current.state())
            });
        }
        current
    }

    fn roots() -> ContinuationRootsV1 {
        ContinuationRootsV1::new(
            root(1),
            root(2),
            root(3),
            root(4),
            root(5),
            root(6),
            root(7),
            42,
        )
    }

    /// ZS-SESSION-002 acceptance: the D5 forbidden transitions are
    /// unreachable through the handle API for both policy values.
    #[test]
    fn forbidden_transitions_are_unreachable_through_the_handle() {
        // Unknown -> Authorized: an Unknown handle can only restore or cancel.
        let unknown = chain(
            ContinuationHandleV1::bind(roots()).unwrap(),
            S::Verifying,
        )
        .advance(S::Unknown, false)
        .unwrap();
        assert_eq!(
            unknown.advance(Authorized, false),
            Err(ContinuationErrorV1::ForbiddenTransition {
                from: crate::zero_execute::ContinuationStateV1::Unknown,
                to: Authorized,
            })
        );
        assert_eq!(
            unknown.advance(Authorized, true),
            Err(ContinuationErrorV1::ForbiddenTransition {
                from: crate::zero_execute::ContinuationStateV1::Unknown,
                to: Authorized,
            })
        );

        // Executing -> Committed: must pass through DeltaSealed/Verifying.
        let executing = chain(ContinuationHandleV1::bind(roots()).unwrap(), S::Executing);
        assert_eq!(
            executing.advance(Committed, true),
            Err(ContinuationErrorV1::ForbiddenTransition {
                from: Executing,
                to: Committed,
            })
        );

        // WaitingDecision -> Executing: must resolve via a supplied policy
        // into Planned first.
        let waiting = chain(
            ContinuationHandleV1::bind(roots()).unwrap(),
            S::WaitingDecision,
        );
        assert_eq!(
            waiting.advance(Executing, true),
            Err(ContinuationErrorV1::ForbiddenTransition {
                from: WaitingDecision,
                to: Executing,
            })
        );
    }

    /// ZS-SESSION-002 acceptance: WaitingDecision -> Planned requires a
    /// supplied contingent policy.
    #[test]
    fn waiting_decision_requires_policy_to_advance() {
        let waiting = chain(
            ContinuationHandleV1::bind(roots()).unwrap(),
            S::WaitingDecision,
        );
        assert_eq!(
            waiting.advance(Planned, false),
            Err(ContinuationErrorV1::ForbiddenTransition {
                from: WaitingDecision,
                to: Planned,
            })
        );
        assert!(waiting.advance(Planned, true).is_ok());
    }

    /// ZS-ADAPTER-004 acceptance: forged, cross-project, stale, and
    /// revoked-epoch handles fail closed without mutation.
    #[test]
    fn forged_cross_project_stale_and_revoked_handles_fail_closed() {
        let handle = ContinuationHandleV1::bind(roots()).unwrap();
        let project = root(2);
        let epoch = 42;
        handle.validate_against(ROOTED_ABI_VERSION_V6, project, epoch).unwrap();

        // Wrong ABI version rejected (the error reports the handle's own
        // version, which is the rooted V6 constant).
        assert_eq!(
            handle.validate_against("zerostack.racc.v5", project, epoch),
            Err(ContinuationErrorV1::WrongAbiVersion {
                actual: ROOTED_ABI_VERSION_V6.into()
            })
        );

        // Forged handle id: tamper the wire form and rebuild; the id no
        // longer matches the recomputed root.
        let mut value = serde_json::to_value(&handle).unwrap();
        value["state"] = serde_json::json!("committed");
        let forged: ContinuationHandleV1 = serde_json::from_value(value).unwrap();
        assert_eq!(forged.verify_id(), Err(ContinuationErrorV1::ForgedHandle));
        assert_eq!(
            forged.validate_against(ROOTED_ABI_VERSION_V6, project, epoch),
            Err(ContinuationErrorV1::ForgedHandle)
        );

        // Cross-project scope rejected.
        let other_project = ContinuationRootsV1::new(
            root(1),
            root(0xAA),
            root(3),
            root(4),
            root(5),
            root(6),
            root(7),
            42,
        );
        let cross = ContinuationHandleV1::bind(other_project).unwrap();
        assert_eq!(
            cross.validate_against(ROOTED_ABI_VERSION_V6, project, epoch),
            Err(ContinuationErrorV1::CrossProjectScope)
        );

        // Revoked epoch rejected.
        let stale_epoch = ContinuationHandleV1::bind(ContinuationRootsV1::new(
            root(1), root(2), root(3), root(4), root(5), root(6), root(7), 7,
        ))
        .unwrap();
        assert_eq!(
            stale_epoch.validate_against(ROOTED_ABI_VERSION_V6, project, epoch),
            Err(ContinuationErrorV1::RevokedEpoch {
                expected: epoch,
                actual: 7
            })
        );

        // The forged handle can never advance: verify_id gates every
        // mutation path.
        assert_eq!(
            forged.advance(Planned, false),
            Err(ContinuationErrorV1::ForgedHandle)
        );
        assert_eq!(
            forged.spawn_child(Planned),
            Err(ContinuationErrorV1::ForgedHandle)
        );
    }

    /// ZS-SESSION-003 acceptance: branching spawns children with recorded
    /// parents; the committed child is the verified one and the parent never
    /// mutates.
    #[test]
    fn branching_children_never_mutate_the_parent_and_one_commits() {
        let parent = ContinuationHandleV1::bind(roots()).unwrap();
        assert_eq!(parent.state(), Bound);

        let resolved = chain(parent.clone(), S::Resolved);

        let child_a = resolved.spawn_child(Planned).unwrap();
        let child_b = resolved.spawn_child(Planned).unwrap();
        assert_eq!(child_a.parent(), Some(resolved.handle_id()));
        assert_eq!(child_b.parent(), Some(resolved.handle_id()));
        // Branches are the same continuation until they diverge; divergence
        // must change the handle id.
        assert_eq!(child_a.handle_id(), child_b.handle_id());
        let divergent = child_b.advance(S::Executing, false).unwrap();
        assert_ne!(child_a.handle_id(), divergent.handle_id());

        // A losing child is rejected; only the verified committed child wins.
        let rejected = chain(child_b, S::Verifying)
            .advance(S::Rejected, false)
            .unwrap();
        assert!(rejected.is_terminal());

        let committed = chain(child_a, S::Committed);
        assert!(committed.is_verified_child_of(resolved.handle_id()));
        assert!(!rejected.is_verified_child_of(resolved.handle_id()));

        // The parent's roots never changed; the parent handle id is stable.
        assert_eq!(resolved.roots(), &roots());
        assert_eq!(resolved.handle_id(), chain(parent, S::Resolved).handle_id());

        // Branching from a non-Resolved state is rejected.
        let bound = ContinuationHandleV1::bind(roots()).unwrap();
        assert_eq!(
            bound.spawn_child(Planned),
            Err(ContinuationErrorV1::IllegalBranch(Bound))
        );
    }

    /// ZS-SESSION-004 acceptance: compaction requires a sealed snapshot root
    /// and a committed terminal state; replay of the compacted record yields
    /// the identical authoritative handle and sealed root.
    #[test]
    fn compaction_requires_sealed_root_and_replay_is_identical() {
        let committed = chain(ContinuationHandleV1::bind(roots()).unwrap(), S::Committed);

        let sealed = root(0xEE);
        // Not permitted before sealing: zero sealed root.
        assert!(!committed.compaction_permitted(DigestV1::ZERO));
        // Permitted after sealing.
        assert!(committed.compaction_permitted(sealed));

        let record = ContinuationCompactRecordV1::seal(&committed, sealed).unwrap();
        record.validate().unwrap();

        // Replay against the resumed handle is identical.
        record.replay_against(&committed).unwrap();
        assert_eq!(record.handle_id, committed.handle_id());
        assert_eq!(record.sealed_snapshot_root, sealed);

        // A different handle cannot replay the record (forged identity).
        let other = ContinuationHandleV1::bind(roots()).unwrap();
        assert_eq!(
            record.replay_against(&other),
            Err(ContinuationErrorV1::ForgedHandle)
        );

        // Sealing a non-committed handle is rejected.
        let executing = chain(ContinuationHandleV1::bind(roots()).unwrap(), S::Executing);
        assert_eq!(
            ContinuationCompactRecordV1::seal(&executing, sealed),
            Err(ContinuationErrorV1::CompactionNotPermitted {
                state: Executing
            })
        );

        // Durable round trip: wire form reproduces the identical handle id.
        let bytes = committed.canonical_bytes().unwrap();
        let wire: ContinuationHandleV1 =
            serde_json::from_slice(&bytes).expect("durable wire form decodes");
        assert_eq!(wire.handle_id(), committed.handle_id());
        assert_eq!(wire, committed);
    }

    /// ZS-SESSION-001 acceptance: a resumed handle reproduces identical
    /// authoritative state after restart without replaying model-visible
    /// history.
    #[test]
    fn durable_handle_round_trip_preserves_authoritative_state() {
        let advanced = chain(ContinuationHandleV1::bind(roots()).unwrap(), S::Planned);
        let bytes = serde_json::to_vec(&advanced).unwrap();
        let resumed: ContinuationHandleV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(resumed.handle_id(), advanced.handle_id());
        assert_eq!(resumed.state(), Planned);
        assert_eq!(resumed.roots(), &roots());
        resumed.validate_against(ROOTED_ABI_VERSION_V6, root(2), 42).unwrap();
    }

    #[test]
    fn handle_validation_fails_closed_on_abi_and_structure() {
        let handle = ContinuationHandleV1::bind(roots()).unwrap();
        // Tampered abi_version fails validation.
        let mut value = serde_json::to_value(&handle).unwrap();
        value["abi_version"] = serde_json::json!("zerostack.racc.v5");
        let tampered: ContinuationHandleV1 = serde_json::from_value(value).unwrap();
        assert_eq!(
            tampered.validate_against(ROOTED_ABI_VERSION_V6, root(2), 42),
            Err(ContinuationErrorV1::WrongAbiVersion {
                actual: "zerostack.racc.v5".into()
            })
        );
        // Extra unknown fields are rejected by serde (deny_unknown_fields).
        let mut extra = serde_json::to_value(&handle).unwrap();
        extra["future"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ContinuationHandleV1>(extra).is_err());
    }
