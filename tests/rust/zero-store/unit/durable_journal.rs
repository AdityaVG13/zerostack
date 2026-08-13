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
