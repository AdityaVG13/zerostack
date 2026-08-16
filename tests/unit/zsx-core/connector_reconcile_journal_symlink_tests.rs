    use super::{
        AttemptBindingV1, AttemptJournalPathsV1, AttemptStateV1, DurableProfileIdV1,
        prepare_attempt_v1, read_current_attempt_v1, reconcile_all_attempts,
        reconcile_request_attempts, refuse_planted_journal_symlinks,
    };
    use std::path::Path;
    use zero_abi::{DigestV1, EffectClass, sha256};

    fn prepared_journal(dir: &Path) {
        std::fs::create_dir_all(dir).expect("journal dir");
        let paths = AttemptJournalPathsV1::new(dir).expect("paths");
        let binding = AttemptBindingV1::new(
            DigestV1::from_bytes(sha256(b"4js1-attempt")),
            DigestV1::from_bytes(sha256(b"4js1-effect")),
            EffectClass::ReversibleMutation,
            DigestV1::from_bytes(sha256(b"4js1-anchor")),
            DurableProfileIdV1::PortableStrict,
            DigestV1::from_bytes(sha256(b"4js1-owner")),
        );
        prepare_attempt_v1(&paths, binding).expect("prepare");
    }

    fn journal_entry_names(dir: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(dir)
            .expect("read journal")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn prepared_real_journal_is_safe_to_retry() {
        let root = std::env::temp_dir().join(format!("zerostack-4js1-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let journal = root.join("g1").join("r1").join("d1");
        prepared_journal(&journal);
        let statuses = reconcile_request_attempts(&root, 1, 1).expect("reconcile");
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(statuses[0].state, AttemptStateV1::SafeToRetry);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn planted_journal_dir_symlink_is_not_followed() {
        let root =
            std::env::temp_dir().join(format!("zerostack-4js1-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let request = root.join("g1").join("r1");
        std::fs::create_dir_all(&request).expect("request dir");
        let victim = root.join("victim");
        prepared_journal(&victim);
        let before = journal_entry_names(&victim);
        std::os::unix::fs::symlink(&victim, request.join("planted")).expect("symlink");

        let statuses =
            reconcile_request_attempts(&root, 1, 1).expect("symlink journal dir must be ignored");
        assert!(
            statuses.is_empty(),
            "planted journal symlink must not become a resume status: {statuses:?}"
        );
        assert_eq!(
            journal_entry_names(&victim),
            before,
            "recovery must not write SafeToRetry through a journal-dir symlink"
        );
        let paths = AttemptJournalPathsV1::new(&victim).expect("victim paths");
        let current = read_current_attempt_v1(&paths)
            .expect("read victim")
            .expect("victim still has a journal");
        assert_eq!(
            current.state,
            AttemptStateV1::Prepared,
            "victim must stay Prepared; write-through would classify SafeToRetry"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn planted_request_dir_symlink_is_not_followed() {
        let root =
            std::env::temp_dir().join(format!("zerostack-orwr-request-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let generation = root.join("g1");
        std::fs::create_dir_all(&generation).expect("generation dir");
        let victim = root.join("victim");
        prepared_journal(&victim.join("d1"));
        let before = journal_entry_names(&victim.join("d1"));
        std::os::unix::fs::symlink(&victim, generation.join("r1")).expect("request-dir symlink");

        let statuses = reconcile_request_attempts(&root, 1, 1)
            .expect("planted request-dir symlink must be ignored");
        assert!(
            statuses.is_empty(),
            "planted g1/r1 symlink must not become a resume status: {statuses:?}"
        );
        assert_eq!(
            journal_entry_names(&victim.join("d1")),
            before,
            "recovery must not write SafeToRetry through a request-dir symlink"
        );
        let paths = AttemptJournalPathsV1::new(&victim.join("d1")).expect("victim paths");
        let current = read_current_attempt_v1(&paths)
            .expect("read victim")
            .expect("victim still has a journal");
        assert_eq!(
            current.state,
            AttemptStateV1::Prepared,
            "victim must stay Prepared; write-through would classify SafeToRetry"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn planted_generation_dir_symlink_is_not_followed() {
        let root =
            std::env::temp_dir().join(format!("zerostack-orwr-generation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("attempts root");
        let victim = root.join("victim");
        prepared_journal(&victim.join("r1").join("d1"));
        let before = journal_entry_names(&victim.join("r1").join("d1"));
        std::os::unix::fs::symlink(&victim, root.join("g1")).expect("generation-dir symlink");

        let statuses = reconcile_request_attempts(&root, 1, 1)
            .expect("planted generation-dir symlink must be ignored");
        assert!(
            statuses.is_empty(),
            "planted g1 symlink must not become a resume status: {statuses:?}"
        );
        assert_eq!(
            journal_entry_names(&victim.join("r1").join("d1")),
            before,
            "recovery must not write SafeToRetry through a generation-dir symlink"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn planted_attempts_root_symlink_is_not_followed() {
        let root =
            std::env::temp_dir().join(format!("zerostack-orwr-attempts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let victim = root.join("victim");
        prepared_journal(&victim.join("g1").join("r1").join("d1"));
        let planted = root.join("planted");
        std::os::unix::fs::symlink(&victim, &planted).expect("attempts-root symlink");

        let statuses = reconcile_all_attempts(&planted, 1)
            .expect("planted attempts-root symlink must be ignored");
        assert!(
            statuses.is_empty(),
            "planted attempts root must not become a resume status: {statuses:?}"
        );
        let paths = AttemptJournalPathsV1::new(&victim.join("g1").join("r1").join("d1"))
            .expect("victim paths");
        let current = read_current_attempt_v1(&paths)
            .expect("read victim")
            .expect("victim still has a journal");
        assert_eq!(current.state, AttemptStateV1::Prepared);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn prepare_refuses_planted_journal_dir_and_ancestor_symlinks() {
        let root =
            std::env::temp_dir().join(format!("zerostack-orwr-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let attempts = root.join("attempts");
        let request = attempts.join("g1").join("r1");
        std::fs::create_dir_all(&request).expect("request dir");
        let victim = root.join("victim");
        std::fs::create_dir_all(&victim).expect("victim");
        std::os::unix::fs::symlink(&victim, request.join("9")).expect("journal-dir symlink");

        let err = refuse_planted_journal_symlinks(&attempts, &request.join("9"))
            .expect_err("journal-dir symlink");
        assert!(
            err.to_string().contains("planted journal-dir symlink"),
            "{err}"
        );

        std::os::unix::fs::symlink(&victim, attempts.join("g2")).expect("generation symlink");
        let err =
            refuse_planted_journal_symlinks(&attempts, &attempts.join("g2").join("r1").join("1"))
                .expect_err("ancestor symlink");
        assert!(
            err.to_string().contains("planted journal ancestor symlink"),
            "{err}"
        );

        let real = request.join("10");
        refuse_planted_journal_symlinks(&attempts, &real)
            .expect("missing real slot is not a planted symlink");
        let _ = std::fs::remove_dir_all(&root);
    }

