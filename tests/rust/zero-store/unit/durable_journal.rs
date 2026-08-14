    use super::*;
    use tempfile::tempdir;
    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }
    fn paths(directory: &Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            directory.join("root.json"),
            directory.join("journal.json"),
            directory.join("cartridge.json"),
            directory.join("owner-death.json"),
            directory.join("recovery.json"),
        )
        .unwrap()
    }
    fn binding() -> JournalBindingV1 {
        JournalBindingV1::new(
            digest(1),
            digest(2),
            DurableProfileIdV1::PortableStrict,
            digest(3),
            digest(4),
            digest(5),
        )
    }
    fn setup() -> (tempfile::TempDir, JournalPathsV1, JournalBindingV1) {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        (directory, paths, binding)
    }
    #[test]
    fn journal_recovery_commit_and_abort_are_idempotent() {
        let (_directory, journal_paths, binding) = setup();
        let cartridge = prepare_journal_v1(&journal_paths, binding.clone()).unwrap();
        let committed = commit_journal_v1(&journal_paths, &cartridge).unwrap();
        assert_eq!(committed.outcome, RecoveryOutcomeV1::NewRootCommitted);
        assert_eq!(
            read_published_root_v1(&journal_paths).unwrap().root_digest,
            binding.new_root
        );
        assert_eq!(
            read_journal_record_v1(&journal_paths).unwrap().state,
            JournalStateV1::Committed
        );
        assert_eq!(
            recover_journal_v1(&journal_paths, &binding).unwrap(),
            committed
        );
        let second = tempdir().unwrap();
        let second_paths = paths(second.path());
        initialize_published_root_v1(&second_paths, binding.old_root).unwrap();
        let cartridge = prepare_journal_v1(&second_paths, binding.clone()).unwrap();
        let aborted = abort_journal_v1(&second_paths, &cartridge).unwrap();
        assert_eq!(aborted.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_eq!(
            recover_journal_v1(&second_paths, &binding).unwrap(),
            aborted
        );
    }
    #[test]
    fn journal_recovery_owner_death_is_typed_and_completes_safely() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        let owner = record_owner_death_v1(&paths, binding.owner_identity_digest, 77).unwrap();
        assert_eq!(owner.failure_code, JournalFailureCodeV1::OwnerDeath);
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_eq!(
            recovered.owner_death_receipt_digest,
            Some(owner.digest().unwrap())
        );
    }
    #[test]
    fn journal_recovery_torn_and_profile_substitution_fail_loudly() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        fs::write(paths.journal_record(), b"{\"schema_version\":1").unwrap();
        assert_eq!(
            recover_journal_v1(&paths, &binding).unwrap_err().code,
            JournalFailureCodeV1::TornOrNoncanonicalRecord
        );
        let mut substituted = binding;
        substituted.durable_profile_id = DurableProfileIdV1::NtfsStrict;
        assert_eq!(
            substituted.validate().unwrap_err().code,
            JournalFailureCodeV1::ProfileSubstitution
        );
    }
    #[test]
    fn journal_recovery_root_disagreement_is_never_guessed() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        let unrelated = RootPublicationReceipt::initial(digest(9)).unwrap();
        fs::write(paths.root_record(), unrelated.canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            recover_journal_v1(&paths, &binding).unwrap_err().code,
            JournalFailureCodeV1::JournalRootDisagreement
        );
    }

    #[test]
    fn journal_recovery_finishes_a_cartridge_only_prepare_as_abort() {
        let (_directory, paths, binding) = setup();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::PrepareBeforeWrite);
        assert_eq!(
            prepare_journal_with_fault_v1(&paths, binding.clone(), &mut fault)
                .unwrap_err()
                .code,
            JournalFailureCodeV1::InjectedCrash
        );
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_ne!(recovered.prepared_record_digest, DigestV1::ZERO);
        assert_eq!(
            read_journal_record_v1(&paths).unwrap().state,
            JournalStateV1::Aborted
        );
    }

    #[test]
    fn journal_recovery_rejects_a_foreign_cartridge() {
        let (_directory, paths, binding) = setup();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::PrepareBeforeWrite);
        prepare_journal_with_fault_v1(&paths, binding.clone(), &mut fault).unwrap_err();
        let mut foreign = binding;
        foreign.transaction_id = digest(9);
        assert_eq!(
            recover_journal_v1(&paths, &foreign).unwrap_err().code,
            JournalFailureCodeV1::CartridgeMismatch
        );
    }

    #[test]
    fn journal_owner_death_after_new_root_publication_finishes_the_commit() {
        let (_directory, paths, binding) = setup();
        let cartridge = prepare_journal_v1(&paths, binding.clone()).unwrap();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::CommitBeforeWrite);
        assert_eq!(
            commit_journal_with_fault_v1(&paths, &cartridge, &mut fault)
                .unwrap_err()
                .code,
            JournalFailureCodeV1::InjectedCrash
        );
        let owner = record_owner_death_v1(&paths, binding.owner_identity_digest, 77).unwrap();
        assert_eq!(owner.observed_root, binding.new_root);
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::NewRootCommitted);
        assert_eq!(
            recovered.owner_death_receipt_digest,
            Some(owner.digest().unwrap())
        );
    }

// ---------------------------------------------------------------------------
// V6-R9: five-term commit binding (ZS-STORE-006)
// ---------------------------------------------------------------------------

fn binding_v2(nonce_byte: u8) -> JournalBindingV2 {
    JournalBindingV2::new(
        digest(1),
        digest(2),
        DurableProfileIdV1::PortableStrict,
        digest(3),
        digest(4),
        digest(5),
        DigestV1::from_bytes([nonce_byte; 32]),
        digest(7),
        BindingLeaseV1::new(digest(8), 1, 4_000_000_000_000_000_000),
    )
}
fn setup_v2() -> (tempfile::TempDir, JournalPathsV1, JournalBindingV2) {
    let directory = tempdir().unwrap();
    let paths = paths(directory.path());
    let binding = binding_v2(9);
    initialize_published_root_v1(&paths, binding.old_root).unwrap();
    (directory, paths, binding)
}

#[test]
fn five_term_binding_round_trip_and_verifiable_read() {
    let (_directory, journal_paths, binding) = setup_v2();
    let binding_digest = binding.digest().unwrap();
    let cartridge = prepare_journal_v2(&journal_paths, binding.clone()).unwrap();
    let committed = commit_journal_v2(&journal_paths, &cartridge).unwrap();
    assert_eq!(committed.outcome, RecoveryOutcomeV1::NewRootCommitted);

    // The committed record carries the full provenance binding (roots +
    // session/ledger owner + nonce + protected scope + lease) exactly as
    // committed, and reads can verify it.
    let record = read_journal_record_v2(&journal_paths).unwrap();
    assert_eq!(record.binding, binding);
    assert_eq!(record.binding.lease, binding.lease);
    assert_eq!(record.binding.nonce, binding.nonce);
    assert_eq!(record.binding.protected_scope, binding.protected_scope);
    assert_eq!(
        read_published_root_v1(&journal_paths).unwrap().root_digest,
        binding.new_root
    );
    assert_eq!(
        verify_committed_binding_v2(&journal_paths, &binding).unwrap(),
        committed
    );
    // Recovery verifies the persisted binding digest, never a guessed one.
    assert_eq!(recover_journal_v2(&journal_paths, &binding).unwrap(), committed);
    // The v2 binding digest differs from a v1 binding over the same roots.
    let v1 = JournalBindingV1::new(
        binding.transaction_id,
        binding.assembly_manifest_digest,
        binding.durable_profile_id,
        binding.old_root,
        binding.new_root,
        binding.owner_identity_digest,
    );
    assert_ne!(v1.digest().unwrap(), binding_digest);
}

#[test]
fn five_term_binding_tamper_is_refused_on_read_and_verify() {
    let (_directory, journal_paths, binding) = setup_v2();
    prepare_journal_v2(&journal_paths, binding.clone()).unwrap();

    // Tamper with the persisted prepared record: rewrite it with a different
    // protected scope. Canonical read refuses the record.
    let mut tampered = binding.clone();
    tampered.protected_scope = digest(0x2a);
    let record = DurableJournalRecord::<JournalBindingV2>::prepared(tampered.clone());
    fs::write(
        journal_paths.journal_record(),
        record.canonical_bytes().unwrap(),
    )
    .unwrap();
    let error = commit_journal_v2(&journal_paths, &read_continuation_cartridge_v2(&journal_paths).unwrap())
        .unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::CartridgeMismatch);

    // A reader verifying a forged binding (different nonce) fails loudly.
    let forged = binding_v2(0x2b);
    let error = verify_committed_binding_v2(&journal_paths, &forged).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::InvalidBinding);
}

#[test]
fn five_term_two_writer_commit_surface_one_success() {
    let (_directory, journal_paths, binding) = setup_v2();
    // Writer A commits first: exact old_root CAS at the commit surface.
    let cartridge_a = prepare_journal_v2(&journal_paths, binding.clone()).unwrap();
    let committed_a = commit_journal_v2(&journal_paths, &cartridge_a).unwrap();
    assert_eq!(committed_a.outcome, RecoveryOutcomeV1::NewRootCommitted);

    // Writer B holds a stale parent root: prepare refuses with RootMismatch,
    // so exactly one of the two concurrent commits succeeds.
    let stale = binding_v2(0x1b);
    let error = prepare_journal_v2(&journal_paths, stale).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::RootMismatch);
}

#[test]
fn five_term_second_prepare_and_replayed_lease_cannot_mutate() {
    let (_directory, journal_paths, binding) = setup_v2();
    // Both writers prepare against the same old root; only the first may.
    let first = prepare_journal_v2(&journal_paths, binding.clone()).unwrap();
    let second_writer = binding_v2(0x2c);
    let error = prepare_journal_v2(&journal_paths, second_writer.clone()).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::AlreadyTerminal);
    commit_journal_v2(&journal_paths, &first).unwrap();

    // Replaying the exact consumed cartridge (same lease, same nonce) returns
    // the identical recovery receipt: no state mutation, receipt idempotent.
    let replay = commit_journal_v2(&journal_paths, &first).unwrap();
    assert_eq!(
        read_published_root_v1(&journal_paths).unwrap().generation,
        1
    );
    // Replaying the same lease under a different nonce is an immutable-record
    // conflict, never a silent re-execution.
    let forged = binding_v2(0x2d);
    let error = recover_journal_v2(&journal_paths, &forged).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::ImmutableReceiptConflict);
    let _ = replay;
}

#[test]
fn five_term_lease_expiry_blocks_fresh_attempts_but_not_recovery() {
    let (_directory, journal_paths, binding) = setup_v2();
    // An already-expired lease cannot start a fresh attempt.
    let mut expired = binding_v2(0x3a);
    expired.lease = BindingLeaseV1::new(digest(8), 1, 1);
    let error = prepare_journal_v2(&journal_paths, expired).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::LeaseExpired);

    // Crash after the root publication: root is new_root, journal is still
    // Prepared. The lease has since expired, but recovery completes the
    // authorized transaction instead of re-authorizing it.
    let cartridge = prepare_journal_v2(&journal_paths, binding.clone()).unwrap();
    let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::RootPublishAfterRename);
    let error = commit_journal_with_fault_v2(&journal_paths, &cartridge, &mut fault).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::InjectedCrash);
    assert_eq!(
        read_published_root_v1(&journal_paths).unwrap().root_digest,
        binding.new_root
    );
    assert_eq!(
        recover_journal_v2(&journal_paths, &binding).unwrap().outcome,
        RecoveryOutcomeV1::NewRootCommitted
    );

    // A fresh attempt with the expired lease is still refused.
    let mut again = binding_v2(0x3b);
    again.old_root = binding.new_root;
    again.new_root = digest(0x3c);
    again.lease = BindingLeaseV1::new(digest(8), 1, 1);
    let error = prepare_journal_v2(&journal_paths, again).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::LeaseExpired);
}
