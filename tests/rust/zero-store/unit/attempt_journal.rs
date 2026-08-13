    use super::*;
    use tempfile::tempdir;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }
    fn make_paths(directory: &Path) -> AttemptJournalPathsV1 {
        AttemptJournalPathsV1::new(directory.join("attempts")).unwrap()
    }
    fn binding() -> AttemptBindingV1 {
        AttemptBindingV1::new(
            digest(1),
            digest(2),
            EffectClass::ReversibleMutation,
            digest(3),
            DurableProfileIdV1::PortableStrict,
            digest(5),
        )
    }
    fn setup() -> (tempfile::TempDir, AttemptJournalPathsV1, AttemptBindingV1) {
        let directory = tempdir().unwrap();
        let paths = make_paths(directory.path());
        (directory, paths, binding())
    }
    fn completion(receipt: u8) -> AttemptEvidenceV1 {
        AttemptEvidenceV1::Completion {
            receipt_digest: digest(receipt),
            observed_at_unix_ns: 100,
        }
    }
    fn failure(receipt: u8) -> AttemptEvidenceV1 {
        AttemptEvidenceV1::Failure {
            failure_receipt_digest: digest(receipt),
            observed_at_unix_ns: 200,
        }
    }
    fn crash_error(
        result: Result<AttemptEntryV1, AttemptJournalErrorV1>,
        boundary: AttemptBoundaryV1,
        published: bool,
    ) {
        let error = result.unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InjectedCrash);
        assert_eq!(error.boundary, Some(boundary));
        assert_eq!(error.entry_published, published);
    }

    #[test]
    fn attempt_prepare_cross_succeed_is_immutable_and_idempotent() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        assert_eq!(prepared.state, AttemptStateV1::Prepared);
        assert_eq!(prepared.sequence, 1);
        // Prepared is persisted before admission: visible on disk immediately.
        assert_eq!(read_attempt_entry_v1(&paths, 1).unwrap(), prepared);
        let prepared_digest = prepared.digest().unwrap();
        // Re-prepare with the same binding is idempotent.
        assert_eq!(
            prepare_attempt_v1(&paths, binding.clone()).unwrap(),
            prepared
        );

        let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        assert_eq!(crossed.state, AttemptStateV1::DispatchCrossed);
        assert_eq!(crossed.sequence, 2);
        assert_eq!(crossed.predecessor_entry_digest, Some(prepared_digest));
        assert_eq!(crossed.crossed_at_unix_ns, Some(50));
        let crossed_digest = crossed.digest().unwrap();
        // Re-crossing is idempotent even with a different timestamp.
        assert_eq!(
            mark_dispatch_crossed_v1(&paths, prepared_digest, 999).unwrap(),
            crossed
        );

        let succeeded = mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap();
        assert_eq!(succeeded.state, AttemptStateV1::Succeeded);
        assert_eq!(succeeded.sequence, 3);
        assert_eq!(succeeded.evidence, Some(completion(7)));
        assert_eq!(succeeded.predecessor_entry_digest, Some(crossed_digest));
        // Terminal entries are immutable: different evidence conflicts.
        let error = mark_succeeded_v1(&paths, crossed_digest, digest(8), 100).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::ImmutableEntryConflict);
        // Identical evidence is idempotent.
        assert_eq!(
            mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap(),
            succeeded
        );
        assert_eq!(read_current_attempt_v1(&paths).unwrap(), Some(succeeded));
    }

    #[test]
    fn attempt_failed_and_aborted_paths_are_terminal_and_idempotent() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let failed = mark_failed_v1(&paths, crossed_digest, digest(9), 200).unwrap();
        assert_eq!(failed.state, AttemptStateV1::Failed);
        assert_eq!(failed.evidence, Some(failure(9)));
        assert_eq!(
            mark_failed_v1(&paths, crossed_digest, digest(9), 200).unwrap(),
            failed
        );

        let second = tempdir().unwrap();
        let second_paths = make_paths(second.path());
        let prepared = prepare_attempt_v1(&second_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let aborted = abort_attempt_v1(&second_paths, prepared_digest).unwrap();
        assert_eq!(aborted.state, AttemptStateV1::Aborted);
        assert_eq!(
            aborted.abort_reason,
            Some(AttemptAbortReasonV1::ExplicitAbort)
        );
        assert_eq!(
            abort_attempt_v1(&second_paths, prepared_digest).unwrap(),
            aborted
        );

        let third = tempdir().unwrap();
        let third_paths = make_paths(third.path());
        let prepared = prepare_attempt_v1(&third_paths, binding).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&third_paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let indeterminate = mark_indeterminate_v1(&third_paths, crossed_digest).unwrap();
        assert_eq!(indeterminate.state, AttemptStateV1::Indeterminate);
        assert_eq!(indeterminate.evidence, None);
        assert_eq!(
            mark_indeterminate_v1(&third_paths, crossed_digest).unwrap(),
            indeterminate
        );
    }

    #[test]
    fn attempt_prepare_conflicts_and_terminal_guards() {
        let (_directory, paths, binding) = setup();
        prepare_attempt_v1(&paths, binding.clone()).unwrap();
        // Same binding: idempotent. Different binding: immutable conflict.
        let mut foreign = binding.clone();
        foreign.attempt_id = digest(9);
        let error = prepare_attempt_v1(&paths, foreign.clone()).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::ImmutableEntryConflict);

        // After dispatch, prepare is refused.
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        let error = prepare_attempt_v1(&paths, binding.clone()).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);

        // After a terminal, prepare is refused too.
        let crossed = read_current_attempt_v1(&paths).unwrap().unwrap();
        let crossed_digest = crossed.digest().unwrap();
        mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap();
        let error = prepare_attempt_v1(&paths, binding).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);
    }

    #[test]
    fn attempt_typed_invalid_transitions_and_evidence() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();

        // Succeed before dispatch is an invalid transition.
        let error = mark_succeeded_v1(&paths, prepared_digest, digest(7), 100).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        // Indeterminate before dispatch is invalid too.
        let error = mark_indeterminate_v1(&paths, prepared_digest).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        // Dispatch with the wrong prepared digest is a receipt mismatch.
        let error = mark_dispatch_crossed_v1(&paths, digest(99), 50).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::ReceiptMismatch);
        // Zero completion receipt digest is invalid evidence.
        let error = mark_succeeded_v1(&paths, prepared_digest, DigestV1::ZERO, 100).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidEvidence);

        let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        // Abort after dispatch crossed is an invalid transition.
        let error = abort_attempt_v1(&paths, prepared_digest).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        // Succeed with the wrong dispatch digest is a receipt mismatch.
        let error = mark_succeeded_v1(&paths, digest(99), digest(7), 100).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::ReceiptMismatch);

        let succeeded = mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap();
        // Dispatch on a terminal attempt is refused (no redispatch).
        let error = mark_dispatch_crossed_v1(&paths, prepared_digest, 60).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);
        // Failed after Succeeded is an invalid transition.
        let error = mark_failed_v1(&paths, crossed_digest, digest(9), 200).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        // Indeterminate after Succeeded is an invalid transition.
        let error = mark_indeterminate_v1(&paths, crossed_digest).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        assert_eq!(read_current_attempt_v1(&paths).unwrap().unwrap(), succeeded);

        // A crafted Prepared entry carrying evidence is rejected on read.
        let second = tempdir().unwrap();
        let second_paths = make_paths(second.path());
        let prepared = prepare_attempt_v1(&second_paths, binding).unwrap();
        let mut forged = prepared.clone();
        forged.evidence = Some(completion(7));
        fs::write(
            second_paths.entry_path(1),
            canonical_bytes(&forged).unwrap(),
        )
        .unwrap();
        let error = read_current_attempt_v1(&second_paths).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidEvidence);
    }

    #[test]
    fn attempt_recovery_outcomes_for_every_state() {
        // Prepared proves the effect never ran: recovery classifies
        // SafeToRetry and terminates the journal; evidence cannot change it.
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let receipt = recover_attempt_v1(&paths, &binding, Some(completion(7))).unwrap();
        assert_eq!(
            receipt.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
        );
        assert_eq!(receipt.terminal_state, AttemptStateV1::SafeToRetry);
        assert_eq!(
            read_current_attempt_v1(&paths).unwrap().unwrap().state,
            AttemptStateV1::SafeToRetry
        );
        // No API can redispatch a recovered journal.
        let error = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);

        // DispatchCrossed with no evidence classifies Indeterminate.
        let second = tempdir().unwrap();
        let second_paths = make_paths(second.path());
        let prepared = prepare_attempt_v1(&second_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&second_paths, prepared_digest, 50).unwrap();
        let receipt = recover_attempt_v1(&second_paths, &binding, None).unwrap();
        assert_eq!(
            receipt.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
        );
        assert_eq!(receipt.terminal_state, AttemptStateV1::Indeterminate);
        assert_eq!(
            read_current_attempt_v1(&second_paths)
                .unwrap()
                .unwrap()
                .state,
            AttemptStateV1::Indeterminate
        );

        // DispatchCrossed with completion evidence classifies Succeeded.
        let third = tempdir().unwrap();
        let third_paths = make_paths(third.path());
        let prepared = prepare_attempt_v1(&third_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&third_paths, prepared_digest, 50).unwrap();
        let receipt = recover_attempt_v1(&third_paths, &binding, Some(completion(7))).unwrap();
        assert_eq!(
            receipt.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSucceeded
        );
        assert_eq!(
            read_current_attempt_v1(&third_paths)
                .unwrap()
                .unwrap()
                .evidence,
            Some(completion(7))
        );

        // DispatchCrossed with failure evidence classifies Failed.
        let fourth = tempdir().unwrap();
        let fourth_paths = make_paths(fourth.path());
        let prepared = prepare_attempt_v1(&fourth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&fourth_paths, prepared_digest, 50).unwrap();
        let receipt = recover_attempt_v1(&fourth_paths, &binding, Some(failure(9))).unwrap();
        assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::ClassifiedFailed);
        assert_eq!(receipt.terminal_state, AttemptStateV1::Failed);

        // Terminals are returned unchanged under every outcome.
        let fifth = tempdir().unwrap();
        let fifth_paths = make_paths(fifth.path());
        let prepared = prepare_attempt_v1(&fifth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&fifth_paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let succeeded = mark_succeeded_v1(&fifth_paths, crossed_digest, digest(7), 100).unwrap();
        let receipt = recover_attempt_v1(&fifth_paths, &binding, Some(completion(7))).unwrap();
        assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::AlreadySucceeded);
        assert_eq!(receipt.terminal_entry_digest, succeeded.digest().unwrap());
        assert_eq!(
            recover_attempt_v1(&fifth_paths, &binding, None).unwrap(),
            receipt
        );

        let sixth = tempdir().unwrap();
        let sixth_paths = make_paths(sixth.path());
        let prepared = prepare_attempt_v1(&sixth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&sixth_paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let failed = mark_failed_v1(&sixth_paths, crossed_digest, digest(9), 200).unwrap();
        let receipt = recover_attempt_v1(&sixth_paths, &binding, Some(failure(9))).unwrap();
        assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::AlreadyFailed);
        assert_eq!(receipt.terminal_entry_digest, failed.digest().unwrap());

        let seventh = tempdir().unwrap();
        let seventh_paths = make_paths(seventh.path());
        let prepared = prepare_attempt_v1(&seventh_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&seventh_paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let indeterminate = mark_indeterminate_v1(&seventh_paths, crossed_digest).unwrap();
        let receipt = recover_attempt_v1(&seventh_paths, &binding, None).unwrap();
        assert_eq!(
            receipt.outcome,
            AttemptRecoveryOutcomeV1::AlreadyIndeterminate
        );
        assert_eq!(
            receipt.terminal_entry_digest,
            indeterminate.digest().unwrap()
        );

        let eighth = tempdir().unwrap();
        let eighth_paths = make_paths(eighth.path());
        let prepared = prepare_attempt_v1(&eighth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let aborted = abort_attempt_v1(&eighth_paths, prepared_digest).unwrap();
        let receipt = recover_attempt_v1(&eighth_paths, &binding, None).unwrap();
        assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::AlreadyAborted);
        assert_eq!(receipt.terminal_entry_digest, aborted.digest().unwrap());
        // Explicit Aborted is preserved for the user abort.
        assert_eq!(
            aborted.abort_reason,
            Some(AttemptAbortReasonV1::ExplicitAbort)
        );

        // A SafeToRetry terminal is returned unchanged, and the explicit
        // abort API still refuses to touch it.
        let ninth = tempdir().unwrap();
        let ninth_paths = make_paths(ninth.path());
        let prepared = prepare_attempt_v1(&ninth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let classified = recover_attempt_v1(&ninth_paths, &binding, None).unwrap();
        assert_eq!(
            classified.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
        );
        let receipt = recover_attempt_v1(&ninth_paths, &binding, None).unwrap();
        assert_eq!(
            receipt.outcome,
            AttemptRecoveryOutcomeV1::AlreadySafeToRetry
        );
        assert_eq!(
            receipt.terminal_entry_digest,
            classified.terminal_entry_digest
        );
        let error = abort_attempt_v1(&ninth_paths, prepared_digest).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);

        // Recovery on an empty journal is loud.
        let empty = tempdir().unwrap();
        let empty_paths = make_paths(empty.path());
        let error = recover_attempt_v1(&empty_paths, &binding, None).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::JournalMissing);
    }

    #[test]
    fn attempt_recovery_never_redispatches() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        recover_attempt_v1(&paths, &binding, None).unwrap();
        // The recovered chain is Prepared -> SafeToRetry: no dispatch entry
        // ever appears, and dispatch is refused on the terminal journal.
        let chain = read_chain(&paths).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].value.state, AttemptStateV1::Prepared);
        assert_eq!(chain[1].value.state, AttemptStateV1::SafeToRetry);
        let error = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);

        let second = tempdir().unwrap();
        let second_paths = make_paths(second.path());
        let prepared = prepare_attempt_v1(&second_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&second_paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        recover_attempt_v1(&second_paths, &binding, None).unwrap();
        // Indeterminate is terminal: the attempt may have run, so no API may
        // dispatch it again.
        let chain = read_chain(&second_paths).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[2].value.state, AttemptStateV1::Indeterminate);
        let error = mark_dispatch_crossed_v1(&second_paths, prepared_digest, 60).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);
        let error = mark_succeeded_v1(&second_paths, crossed_digest, digest(7), 100).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
        // Recovery is idempotent and returns the same receipt.
        assert_eq!(
            recover_attempt_v1(&second_paths, &binding, None).unwrap(),
            recover_attempt_v1(&second_paths, &binding, Some(completion(7))).unwrap()
        );
    }

    #[test]
    fn attempt_crash_boundaries_prepare_classify_safe_to_retry() {
        // A crash before or during the file write leaves no entry at all:
        // the attempt was never durably admitted and recovery is loud.
        for boundary in [
            AttemptBoundaryV1::PrepareBeforeWrite,
            AttemptBoundaryV1::PrepareAfterFileSync,
        ] {
            let (_directory, paths, binding) = setup();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result = prepare_attempt_with_fault_v1(&paths, binding.clone(), &mut fault);
            crash_error(result, boundary, false);
            assert!(read_chain(&paths).unwrap().is_empty());
            let error = recover_attempt_v1(&paths, &binding, None).unwrap_err();
            assert_eq!(error.code, AttemptFailureCodeV1::JournalMissing);
            let error = mark_dispatch_crossed_v1(&paths, digest(1), 50).unwrap_err();
            assert_eq!(error.code, AttemptFailureCodeV1::JournalMissing);
        }
        // A crash after the rename or directory sync leaves the Prepared
        // entry: recovery classifies SafeToRetry (never dispatched, never
        // executed) and never dispatches.
        for boundary in [
            AttemptBoundaryV1::PrepareAfterRename,
            AttemptBoundaryV1::PrepareAfterDirectorySync,
        ] {
            let (_directory, paths, binding) = setup();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result = prepare_attempt_with_fault_v1(&paths, binding.clone(), &mut fault);
            crash_error(result, boundary, true);
            let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
            assert_eq!(
                receipt.outcome,
                AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
            );
            let chain = read_chain(&paths).unwrap();
            assert_eq!(chain.len(), 2);
            assert_eq!(chain[0].value.state, AttemptStateV1::Prepared);
            assert_eq!(chain[1].value.state, AttemptStateV1::SafeToRetry);
            assert_eq!(chain[1].value.abort_reason, None);
            assert!(
                chain
                    .iter()
                    .all(|entry| entry.value.state != AttemptStateV1::DispatchCrossed)
            );
            let error = mark_dispatch_crossed_v1(&paths, digest(1), 50).unwrap_err();
            assert_eq!(error.code, AttemptFailureCodeV1::AlreadyTerminal);
        }
    }

    #[test]
    fn attempt_crash_boundaries_dispatch_classify() {
        // Crash before the dispatch entry is written (before write or during
        // file sync): still Prepared, so recovery classifies SafeToRetry.
        for boundary in [
            AttemptBoundaryV1::DispatchCrossBeforeWrite,
            AttemptBoundaryV1::DispatchCrossAfterFileSync,
        ] {
            let (_directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result =
                mark_dispatch_crossed_with_fault_v1(&paths, prepared_digest, 50, &mut fault);
            crash_error(result, boundary, false);
            assert_eq!(
                read_current_attempt_v1(&paths).unwrap().unwrap().state,
                AttemptStateV1::Prepared
            );
            assert_eq!(
                recover_attempt_v1(&paths, &binding, None).unwrap().outcome,
                AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
            );
        }

        // Crash after the dispatch entry landed: recovery classifies by
        // authoritative evidence, else Indeterminate.
        for boundary in [
            AttemptBoundaryV1::DispatchCrossAfterRename,
            AttemptBoundaryV1::DispatchCrossAfterDirectorySync,
        ] {
            let (directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result =
                mark_dispatch_crossed_with_fault_v1(&paths, prepared_digest, 50, &mut fault);
            crash_error(result, boundary, true);
            assert_eq!(
                read_current_attempt_v1(&paths).unwrap().unwrap().state,
                AttemptStateV1::DispatchCrossed
            );
            let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
            assert_eq!(
                receipt.outcome,
                AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
            );
            assert_eq!(
                read_current_attempt_v1(&paths).unwrap().unwrap().state,
                AttemptStateV1::Indeterminate
            );
            drop(directory);
        }

        // Completion evidence at recovery proves completion; failure
        // evidence proves safe rollback.
        let binding = binding();
        let third = tempdir().unwrap();
        let third_paths = make_paths(third.path());
        let prepared = prepare_attempt_v1(&third_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let mut fault = AttemptFaultPlanV1::crash_at(AttemptBoundaryV1::DispatchCrossAfterRename);
        mark_dispatch_crossed_with_fault_v1(&third_paths, prepared_digest, 50, &mut fault)
            .unwrap_err();
        assert_eq!(
            recover_attempt_v1(&third_paths, &binding, Some(completion(7)))
                .unwrap()
                .outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSucceeded
        );

        let fourth = tempdir().unwrap();
        let fourth_paths = make_paths(fourth.path());
        let prepared = prepare_attempt_v1(&fourth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let mut fault = AttemptFaultPlanV1::crash_at(AttemptBoundaryV1::DispatchCrossAfterRename);
        mark_dispatch_crossed_with_fault_v1(&fourth_paths, prepared_digest, 50, &mut fault)
            .unwrap_err();
        assert_eq!(
            recover_attempt_v1(&fourth_paths, &binding, Some(failure(9)))
                .unwrap()
                .outcome,
            AttemptRecoveryOutcomeV1::ClassifiedFailed
        );
    }

    #[test]
    fn attempt_crash_boundaries_terminal_entries_are_immutable() {
        // Succeed: before the terminal lands recovery re-classifies; after
        // it lands the persisted terminal is authoritative and immutable.
        for (boundary, landed) in [
            (AttemptBoundaryV1::SucceedBeforeWrite, false),
            (AttemptBoundaryV1::SucceedAfterFileSync, false),
            (AttemptBoundaryV1::SucceedAfterRename, true),
            (AttemptBoundaryV1::SucceedAfterDirectorySync, true),
        ] {
            let (_directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
            let crossed_digest = crossed.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result =
                mark_succeeded_with_fault_v1(&paths, crossed_digest, digest(7), 100, &mut fault);
            crash_error(result, boundary, landed);
            if !landed {
                // Terminal never landed: recovery classifies, and the first
                // recovery decides the outcome — later evidence cannot
                // change an already-terminal journal.
                let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
                assert_eq!(
                    receipt.outcome,
                    AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
                );
                assert_eq!(
                    recover_attempt_v1(&paths, &binding, Some(completion(7)))
                        .unwrap()
                        .outcome,
                    AttemptRecoveryOutcomeV1::AlreadyIndeterminate
                );
            } else {
                // Terminal landed: recovery returns it unchanged and the
                // persisted evidence is authoritative.
                let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
                assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::AlreadySucceeded);
                assert_eq!(
                    read_current_attempt_v1(&paths).unwrap().unwrap().evidence,
                    Some(completion(7))
                );
            }
        }

        for (boundary, landed) in [
            (AttemptBoundaryV1::AbortBeforeWrite, false),
            (AttemptBoundaryV1::AbortAfterFileSync, false),
            (AttemptBoundaryV1::AbortAfterRename, true),
            (AttemptBoundaryV1::AbortAfterDirectorySync, true),
        ] {
            let (_directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result = abort_attempt_with_fault_v1(&paths, prepared_digest, &mut fault);
            crash_error(result, boundary, landed);
            if !landed {
                // Explicit abort never landed: recovery classifies
                // SafeToRetry instead of aborting.
                let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
                assert_eq!(
                    receipt.outcome,
                    AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
                );
                let current = read_current_attempt_v1(&paths).unwrap().unwrap();
                assert_eq!(current.state, AttemptStateV1::SafeToRetry);
                assert_eq!(current.abort_reason, None);
            } else {
                let receipt = recover_attempt_v1(&paths, &binding, None).unwrap();
                assert_eq!(receipt.outcome, AttemptRecoveryOutcomeV1::AlreadyAborted);
                assert_eq!(
                    read_current_attempt_v1(&paths)
                        .unwrap()
                        .unwrap()
                        .abort_reason,
                    Some(AttemptAbortReasonV1::ExplicitAbort)
                );
            }
        }

        for (boundary, landed) in [
            (AttemptBoundaryV1::FailBeforeWrite, false),
            (AttemptBoundaryV1::FailAfterFileSync, false),
            (AttemptBoundaryV1::FailAfterRename, true),
            (AttemptBoundaryV1::FailAfterDirectorySync, true),
        ] {
            let (_directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
            let crossed_digest = crossed.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result =
                mark_failed_with_fault_v1(&paths, crossed_digest, digest(9), 200, &mut fault);
            crash_error(result, boundary, landed);
            if !landed {
                assert_eq!(
                    recover_attempt_v1(&paths, &binding, Some(failure(9)))
                        .unwrap()
                        .outcome,
                    AttemptRecoveryOutcomeV1::ClassifiedFailed
                );
            } else {
                assert_eq!(
                    recover_attempt_v1(&paths, &binding, None).unwrap().outcome,
                    AttemptRecoveryOutcomeV1::AlreadyFailed
                );
            }
        }
    }

    #[test]
    fn attempt_crash_boundaries_indeterminate_and_recovery_are_idempotent() {
        for (boundary, landed) in [
            (AttemptBoundaryV1::IndeterminateBeforeWrite, false),
            (AttemptBoundaryV1::IndeterminateAfterFileSync, false),
            (AttemptBoundaryV1::IndeterminateAfterRename, true),
            (AttemptBoundaryV1::IndeterminateAfterDirectorySync, true),
        ] {
            let (_directory, paths, binding) = setup();
            let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let prepared_digest = prepared.digest().unwrap();
            let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
            let crossed_digest = crossed.digest().unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let result = mark_indeterminate_with_fault_v1(&paths, crossed_digest, &mut fault);
            crash_error(result, boundary, landed);
            let expected = if landed {
                AttemptRecoveryOutcomeV1::AlreadyIndeterminate
            } else {
                AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
            };
            assert_eq!(
                recover_attempt_v1(&paths, &binding, None).unwrap().outcome,
                expected
            );
        }

        // A crash inside recovery leaves either the old or the new entry;
        // re-running recovery returns the same terminal either way.
        for boundary in [
            AttemptBoundaryV1::RecoverBeforeWrite,
            AttemptBoundaryV1::RecoverAfterFileSync,
            AttemptBoundaryV1::RecoverAfterRename,
            AttemptBoundaryV1::RecoverAfterDirectorySync,
        ] {
            let (_directory, paths, binding) = setup();
            prepare_attempt_v1(&paths, binding.clone()).unwrap();
            let mut fault = AttemptFaultPlanV1::crash_at(boundary);
            let error =
                recover_attempt_with_fault_v1(&paths, &binding, None, &mut fault).unwrap_err();
            assert_eq!(error.code, AttemptFailureCodeV1::InjectedCrash);
            assert_eq!(error.boundary, Some(boundary));
            let landed = matches!(
                boundary,
                AttemptBoundaryV1::RecoverAfterRename
                    | AttemptBoundaryV1::RecoverAfterDirectorySync
            );
            let first = recover_attempt_v1(&paths, &binding, None).unwrap();
            let second = recover_attempt_v1(&paths, &binding, None).unwrap();
            assert_eq!(
                first.outcome,
                if landed {
                    AttemptRecoveryOutcomeV1::AlreadySafeToRetry
                } else {
                    AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
                }
            );
            assert_eq!(second.outcome, AttemptRecoveryOutcomeV1::AlreadySafeToRetry);
            // Both receipts bind the same immutable terminal entry.
            assert_eq!(first.terminal_entry_digest, second.terminal_entry_digest);
        }
    }

    #[test]
    fn attempt_recovery_and_reads_fail_loudly_on_corruption() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap();

        // Torn JSON fails loudly.
        fs::write(paths.entry_path(1), br#"{"schema_version":1"#).unwrap();
        let error = recover_attempt_v1(&paths, &binding, None).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::TornOrNoncanonicalRecord);

        // Non-canonical bytes (duplicate key) fail loudly.
        let second = tempdir().unwrap();
        let second_paths = make_paths(second.path());
        let prepared = prepare_attempt_v1(&second_paths, binding.clone()).unwrap();
        let canonical = String::from_utf8(prepared.canonical_bytes().unwrap()).unwrap();
        let noncanonical = format!(r#"{{"schema_version":1,{}"#, &canonical[1..]);
        fs::write(second_paths.entry_path(1), noncanonical.as_bytes()).unwrap();
        let error = read_current_attempt_v1(&second_paths).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::TornOrNoncanonicalRecord);

        // Oversized entries fail loudly.
        let third = tempdir().unwrap();
        let third_paths = make_paths(third.path());
        let prepared = prepare_attempt_v1(&third_paths, binding.clone()).unwrap();
        fs::write(
            third_paths.entry_path(1),
            vec![b'x'; ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1 as usize + 1],
        )
        .unwrap();
        let error = read_current_attempt_v1(&third_paths).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::RecordTooLarge);
        assert_eq!(prepared.state, AttemptStateV1::Prepared);

        // A missing middle entry breaks the chain loudly.
        let fourth = tempdir().unwrap();
        let fourth_paths = make_paths(fourth.path());
        let prepared = prepare_attempt_v1(&fourth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&fourth_paths, prepared_digest, 50).unwrap();
        let crossed = read_current_attempt_v1(&fourth_paths).unwrap().unwrap();
        let crossed_digest = crossed.digest().unwrap();
        mark_succeeded_v1(&fourth_paths, crossed_digest, digest(7), 100).unwrap();
        fs::remove_file(fourth_paths.entry_path(2)).unwrap();
        let error = read_current_attempt_v1(&fourth_paths).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::SequenceMismatch);

        // Recovery with a foreign binding fails loudly.
        let fifth = tempdir().unwrap();
        let fifth_paths = make_paths(fifth.path());
        let prepared = prepare_attempt_v1(&fifth_paths, binding.clone()).unwrap();
        let prepared_digest = prepared.digest().unwrap();
        mark_dispatch_crossed_v1(&fifth_paths, prepared_digest, 50).unwrap();
        let mut foreign = binding.clone();
        foreign.attempt_id = digest(9);
        let error = recover_attempt_v1(&fifth_paths, &foreign, None).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidBinding);

        // A recovery receipt with a contradictory pairing fails validation.
        let mut receipt = recover_attempt_v1(&fifth_paths, &binding, None).unwrap();
        receipt.outcome = AttemptRecoveryOutcomeV1::ClassifiedSucceeded;
        let error = receipt.validate().unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidBinding);
        // A forged chain (Succeeded directly after Prepared) is rejected.
        let sixth = tempdir().unwrap();
        let sixth_paths = make_paths(sixth.path());
        let prepared = prepare_attempt_v1(&sixth_paths, binding.clone()).unwrap();
        let mut forged = prepared.clone();
        forged.state = AttemptStateV1::Succeeded;
        forged.sequence = 2;
        forged.predecessor_entry_digest = Some(prepared.digest().unwrap());
        forged.evidence = Some(completion(7));
        fs::write(sixth_paths.entry_path(2), canonical_bytes(&forged).unwrap()).unwrap();
        let error = read_current_attempt_v1(&sixth_paths).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidTransition);
    }

    #[test]
    fn attempt_digests_and_contract_are_canonical_and_stable() {
        let (_directory, paths, binding) = setup();
        let prepared = prepare_attempt_v1(&paths, binding.clone()).unwrap();
        // Entry digests are stable and binding-sensitive.
        assert_eq!(prepared.digest().unwrap(), prepared.digest().unwrap());
        let mut other = binding.clone();
        other.attempt_id = digest(9);
        let other_prepared = AttemptEntryV1::prepared(other, 1);
        assert_ne!(prepared.digest().unwrap(), other_prepared.digest().unwrap());
        // Canonical round trip: persisted bytes are exactly canonical bytes.
        assert_eq!(
            fs::read(paths.entry_path(1)).unwrap(),
            prepared.canonical_bytes().unwrap()
        );

        let prepared_digest = prepared.digest().unwrap();
        let crossed = mark_dispatch_crossed_v1(&paths, prepared_digest, 50).unwrap();
        let crossed_digest = crossed.digest().unwrap();
        let succeeded = mark_succeeded_v1(&paths, crossed_digest, digest(7), 100).unwrap();
        assert_eq!(
            fs::read(paths.entry_path(3)).unwrap(),
            succeeded.canonical_bytes().unwrap()
        );

        let contract = attempt_journal_contract_v1();
        assert_eq!(
            contract["states"],
            json!([
                "prepared",
                "dispatch_crossed",
                "succeeded",
                "failed",
                "indeterminate",
                "safe_to_retry",
                "aborted"
            ])
        );
        assert_eq!(
            contract["transitions"]["prepared"],
            json!(["dispatch_crossed", "aborted", "safe_to_retry"])
        );
        assert_eq!(
            contract["transitions"]["dispatch_crossed"],
            json!(["succeeded", "failed", "indeterminate"])
        );
        assert_eq!(contract["transitions"]["succeeded"], json!([]));
        assert_eq!(contract["transitions"]["safe_to_retry"], json!([]));
        assert_eq!(
            contract["max_entries"],
            json!(ATTEMPT_JOURNAL_MAX_ENTRIES_V1)
        );

        // AttemptJournalPathsV1 rejects degenerate directories.
        let error = AttemptJournalPathsV1::new(Path::new("")).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidBinding);
        let error = AttemptJournalPathsV1::new(Path::new("/")).unwrap_err();
        assert_eq!(error.code, AttemptFailureCodeV1::InvalidBinding);
    }
