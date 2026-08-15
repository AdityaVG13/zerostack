    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cas_temp_retry_preserves_stale_collision_and_publishes() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let bytes = b"retry payload";
        let hash = content_hash_hex(bytes);
        let dest = cas.object_path(&hash);
        let parent = dest.parent().unwrap();
        fs::create_dir_all(parent).unwrap();

        let stale_sequence = 41;
        let stale = parent.join(format!(
            "{TEMP_PREFIX}{}-{}-{stale_sequence}",
            &hash[..8],
            std::process::id()
        ));
        fs::write(&stale, b"stale candidate").unwrap();
        let mut sequences = [stale_sequence, stale_sequence + 1].into_iter();

        let outcome = cas
            .put_with_limit_and_sequence(bytes, CAS_MAX_OBJECT_BYTES, || sequences.next().unwrap())
            .unwrap();

        assert!(outcome.created);
        assert_eq!(outcome.hash, hash);
        assert_eq!(cas.get_verified(&hash).unwrap(), bytes);
        assert_eq!(fs::read(&stale).unwrap(), b"stale candidate");
    }

    #[test]
    fn put_get_roundtrip_and_dedup() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let h1 = cas.put(b"hello zerostack").unwrap();
        let h2 = cas.put(b"hello zerostack").unwrap();
        assert_eq!(h1, h2);
        assert!(cas.contains(&h1));
        assert_eq!(cas.get_verified(&h1).unwrap(), b"hello zerostack");
    }

    #[test]
    fn layout_constant_matches_object_path() {
        let cas = SharedCas::open("/store");
        let h = "4fdbc441ea7b546100e086ac1e4fc5ae6749b7314311c99db05be450eca12996";
        let p = cas.object_path(h);
        assert!(
            p.ends_with(format!("blobs/sha256/4f/{h}")),
            "{p:?} vs {CAS_LAYOUT}"
        );
    }

    #[test]
    fn layout_goldens_are_the_shared_contract() {
        assert_eq!(crate::BLOBS_DIR, "blobs");
        assert_eq!(CAS_LAYOUT, "blobs/sha256/<hh>/<hash>");
        assert_eq!(CAS_LAYOUT_VERSION, 1);
    }

    #[test]
    fn size_policy_is_enforced_on_put() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let err = cas
            .put_with_limit(b"tiny object over a tiny limit", 4)
            .expect_err("over-limit put");
        assert_eq!(err.class(), "policy_denied");
    }

    #[test]
    fn corrupted_object_is_loud_and_returns_no_bytes() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let h = cas.put(b"original bytes").unwrap();
        std::fs::write(cas.object_path(&h), b"tampered").unwrap();
        let err = cas.get_verified(&h).expect_err("tampered object");
        assert_eq!(err.class(), "digest_mismatch");
    }

    #[test]
    fn missing_and_malformed_identities() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let absent = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(cas.get_verified(absent).unwrap_err().class(), "missing");
        assert_eq!(cas.get_verified("SHORT").unwrap_err().class(), "malformed");
    }

    #[test]
    fn stale_temps_are_reaped_but_fresh_ones_survive() {
        let dir = tempdir().unwrap();
        let fresh = dir.path().join(".tmp-fresh");
        let plain = dir.path().join("not-a-temp");
        std::fs::write(&fresh, b"active writer").unwrap();
        std::fs::write(&plain, b"object-like").unwrap();

        reap_stale_temps(dir.path(), Duration::ZERO);
        assert!(!fresh.exists(), "stale temp must be reaped");
        assert!(plain.exists(), "non-temp names are never touched");

        let active = dir.path().join(".tmp-active");
        std::fs::write(&active, b"active").unwrap();
        reap_stale_temps(dir.path(), CAS_TEMP_REAP_AGE);
        assert!(active.exists(), "young temps are never raced");
    }

    /// Publish reports whether it created the object or deduped onto one.
    #[test]
    fn put_outcome_distinguishes_create_from_dedup() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let first = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(first.created);
        let second = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(!second.created);
        assert_eq!(first.hash, second.hash);
    }

    /// A wrong caller-supplied digest writes nothing at all.
    #[test]
    fn put_prehashed_rejects_a_wrong_digest_without_writing() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let real = content_hash_hex(b"payload");
        let wrong = content_hash_hex(b"other");
        let err = cas.put_prehashed(&wrong, b"payload").unwrap_err();
        assert_eq!(err.class(), "digest_mismatch");
        assert!(!cas.contains(&wrong));
        assert!(!cas.contains(&real), "nothing is published on mismatch");
        assert!(cas.put_prehashed(&real, b"payload").unwrap().created);
    }

    /// Deduping refreshes the mtime, because a dedup is a fresh reference and
    /// an age-based retention policy that never sees it collects a live object.
    #[test]
    fn dedup_refreshes_the_modification_time() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        let path = cas.object_path(&hash);
        let old = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
        fs::OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        cas.put(b"payload").unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(after > before, "dedup must refresh mtime");
    }

    #[test]
    fn touch_refreshes_only_existing_objects() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        assert!(cas.touch(&hash).is_ok());
        let absent = content_hash_hex(b"absent");
        assert_eq!(cas.touch(&absent).unwrap_err().class(), "missing");
        assert_eq!(cas.touch("nope").unwrap_err().class(), "malformed");
    }

    #[test]
    fn list_objects_reports_objects_and_ignores_debris() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let a = cas.put(b"a").unwrap();
        let b = cas.put(b"bb").unwrap();
        let shard = cas.object_path(&a).parent().unwrap().to_path_buf();
        fs::write(shard.join(".tmp-deadbeef-1-0"), b"debris").unwrap();
        fs::write(shard.join("not-a-hash"), b"debris").unwrap();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(cas.list_objects().unwrap(), expected);
    }

    #[test]
    fn bounded_list_objects_rejects_excess_objects() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        cas.put(b"a").unwrap();
        cas.put(b"b").unwrap();
        assert!(matches!(
            cas.list_objects_bounded(1),
            Err(CasError::Malformed(message)) if message.contains("exceeds 1 objects")
        ));
    }

    #[test]
    fn list_objects_is_empty_for_a_fresh_store() {
        let root = tempdir().unwrap();
        assert!(
            SharedCas::open(root.path())
                .list_objects()
                .unwrap()
                .is_empty()
        );
    }

    /// Removal is refused without the exclusive guard, so no caller can sweep
    /// while publishers are free to run.
    #[test]
    fn removal_requires_the_exclusive_guard() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        let publish_guard = cas.lock_for_publish().unwrap();
        let err = cas.remove_object(&hash, &publish_guard).unwrap_err();
        assert_eq!(err.class(), "policy_denied");
        assert!(
            cas.contains(&hash),
            "the object must survive a refused sweep"
        );
        let err = cas.quarantine_object(&hash, &publish_guard).unwrap_err();
        assert_eq!(err.class(), "policy_denied");
        assert!(cas.contains(&hash));
    }

    #[test]
    fn sweeping_under_the_exclusive_guard_removes_and_quarantines() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let doomed = cas.put(b"doomed").unwrap();
        let kept = cas.put(b"kept").unwrap();
        let guard = cas.lock_for_sweep().unwrap();
        cas.remove_object(&doomed, &guard).unwrap();
        assert!(!cas.contains(&doomed));
        cas.quarantine_object(&kept, &guard).unwrap();
        assert!(!cas.contains(&kept));
        let quarantined = root
            .path()
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR)
            .join(&kept);
        assert_eq!(
            content_hash_hex(&fs::read(&quarantined).unwrap()),
            kept,
            "a quarantined body stays verifiable, so a wrong verdict is recoverable"
        );
        assert_eq!(
            cas.remove_object(&doomed, &guard).unwrap_err().class(),
            "missing"
        );
    }

    /// Identity proof: a digest-mismatched body must not occupy
    /// `gc/quarantine/<hash>` as if it were recoverable under that digest.
    #[test]
    fn quarantine_corrupt_body_does_not_occupy_hash_slot() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"original bytes").unwrap();
        fs::write(cas.object_path(&hash), b"tampered").unwrap();
        let guard = cas.lock_for_sweep().unwrap();
        cas.quarantine_object(&hash, &guard).unwrap();
        assert!(!cas.contains(&hash));
        let dir = root
            .path()
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR);
        let verified_slot = dir.join(&hash);
        assert!(
            fs::symlink_metadata(&verified_slot).is_err(),
            "corrupt body must not occupy quarantine/{hash}"
        );
        let corrupt = dir.join(format!("{hash}.corrupt-0"));
        assert_eq!(fs::read(&corrupt).unwrap(), b"tampered");
    }

    /// Second verified quarantine versions the prior dest to `{hash}.1`.
    #[test]
    fn second_verified_quarantine_versions_prior_dest() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"same bytes").unwrap();
        let guard = cas.lock_for_sweep().unwrap();
        cas.quarantine_object(&hash, &guard).unwrap();
        drop(guard);
        let again = cas.put(b"same bytes").unwrap();
        assert_eq!(again, hash);
        let guard = cas.lock_for_sweep().unwrap();
        cas.quarantine_object(&hash, &guard).unwrap();
        let dir = root
            .path()
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR);
        assert_eq!(fs::read(dir.join(&hash)).unwrap(), b"same bytes");
        assert_eq!(fs::read(dir.join(format!("{hash}.1"))).unwrap(), b"same bytes");
    }

    /// A symlink dest must not be treated as a regular quarantined object.
    #[cfg(unix)]
    #[test]
    fn quarantine_refuses_symlink_dest() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        let dir = root
            .path()
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR);
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(&hash);
        std::os::unix::fs::symlink(root.path().join("elsewhere"), &dest).unwrap();
        let guard = cas.lock_for_sweep().unwrap();
        let err = cas.quarantine_object(&hash, &guard).unwrap_err();
        assert_eq!(err.class(), "malformed");
        assert!(
            err.to_string().contains("symlink"),
            "symlink dest must fail closed: {err}"
        );
        assert!(cas.contains(&hash), "source must stay in the object tree");
        assert!(
            dest.symlink_metadata().unwrap().file_type().is_symlink(),
            "symlink dest must not be overwritten"
        );
    }

    /// The publish/GC race, made deterministic with channel rendezvous rather
    /// than timing.
    ///
    /// A sweeper parks between its liveness decision and its unlink. Before the
    /// coordination lock existed, a publisher could complete inside that window
    /// and have its object deleted immediately afterwards, so publish returned
    /// Ok for an object that no longer existed. Now the publisher is excluded
    /// until the sweep releases, and the republished object survives.
    #[test]
    fn a_publisher_cannot_slip_between_a_sweep_decision_and_its_unlink() {
        use std::sync::mpsc;

        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"contested").unwrap();
        let expected = hash.clone();

        let (parked_tx, parked_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let sweep_root = root.path().to_path_buf();
        let sweep_hash = hash.clone();

        let sweeper = std::thread::spawn(move || {
            let cas = SharedCas::open(&sweep_root);
            let guard = cas.lock_for_sweep().unwrap();
            parked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            cas.remove_object(&sweep_hash, &guard).unwrap();
        });

        parked_rx.recv().unwrap();
        assert!(
            StoreLock::try_publish(root.path()).unwrap().is_none(),
            "a publish must not proceed while a sweep holds the guard"
        );
        release_tx.send(()).unwrap();
        sweeper.join().unwrap();

        assert!(!cas.contains(&hash), "the sweep completed its removal");
        let republished = cas.put(b"contested").unwrap();
        assert_eq!(republished, expected);
        assert_eq!(
            cas.get_verified(&republished).unwrap(),
            b"contested",
            "a publish that returns Ok must leave a readable object behind"
        );
    }

    /// Every historical engine temp shape is reaped, not just this crate's.
    #[test]
    fn all_engine_temp_shapes_are_reaped() {
        let dir = tempdir().unwrap();
        let hash = content_hash_hex(b"x");
        let shapes = [
            format!(".tmp-{}-{}-0", &hash[..8], std::process::id()),
            format!(".tmp-{hash}-1234567890-0.blob"),
            format!("{hash}.{}.0.tmp", std::process::id()),
        ];
        let stale = std::time::SystemTime::now() - CAS_TEMP_REAP_AGE - Duration::from_secs(60);
        for shape in &shapes {
            let path = dir.path().join(shape);
            fs::write(&path, b"debris").unwrap();
            fs::OpenOptions::new()
                .write(true)
                .truncate(false)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(stale))
                .unwrap();
        }
        reap_stale_temps(dir.path(), CAS_TEMP_REAP_AGE);
        for shape in &shapes {
            assert!(
                !dir.path().join(shape).exists(),
                "stale temp left behind: {shape}"
            );
        }
    }

    /// The other half of the equality gate in zerostack-oh7: the unified reaper
    /// must remove all three historical shapes when stale and none of them when
    /// young. Without this, a reaper that simply deleted everything temp-shaped
    /// would pass `all_engine_temp_shapes_are_reaped` while racing live writers
    /// of the other two engines.
    #[test]
    fn no_engine_temp_shape_is_reaped_while_young() {
        let dir = tempdir().unwrap();
        let hash = content_hash_hex(b"x");
        let shapes = [
            format!(".tmp-{}-{}-0", &hash[..8], std::process::id()),
            format!(".tmp-{hash}-1234567890-0.blob"),
            format!("{hash}.{}.0.tmp", std::process::id()),
        ];
        for shape in &shapes {
            fs::write(dir.path().join(shape), b"in flight").unwrap();
        }
        reap_stale_temps(dir.path(), CAS_TEMP_REAP_AGE);
        for shape in &shapes {
            assert!(
                dir.path().join(shape).exists(),
                "young temp of a concurrent publisher was reaped: {shape}"
            );
        }
    }

    /// zerostack-2x7 boundary vectors. The hub and GraphZero denied above
    /// 256 MiB while TokenZero and FSZero had no policy at all, so an
    /// oversized object published by one engine was permanently unreadable by
    /// another. Pin all three sides of the boundary so no engine can drift.
    #[test]
    fn size_policy_boundary_is_exact() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let limit = 4096u64;

        let under = vec![b'u'; limit as usize - 1];
        let at = vec![b'a'; limit as usize];
        let over = vec![b'o'; limit as usize + 1];

        cas.put_with_limit(&under, limit)
            .expect("limit minus one is accepted");
        cas.put_with_limit(&at, limit)
            .expect("exactly the limit is accepted");
        let err = cas
            .put_with_limit(&over, limit)
            .expect_err("limit plus one is refused");
        assert_eq!(err.class(), "policy_denied");
    }

    /// The shared constant itself is part of the cross-engine contract: an
    /// engine that hardcodes a different cap reintroduces the asymmetry.
    #[test]
    fn size_policy_constant_is_256_mib() {
        assert_eq!(CAS_MAX_OBJECT_BYTES, 256 * 1024 * 1024);
    }

    /// Short and non-hex identities must be refused, never sliced. The hub,
    /// TokenZero, and GraphZero all built object paths via `hash[..2]` without
    /// validating hex first, so a short input aborted the process.
    #[test]
    fn short_and_non_hex_identities_never_panic() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        for bad in ["", "a", "ab", "ZZ", &"g".repeat(64), &"A".repeat(64)] {
            assert_eq!(
                cas.get_verified(bad).unwrap_err().class(),
                "malformed",
                "identity {bad:?} must be refused as malformed"
            );
            // Path construction must also be total, not panicking.
            let _ = cas.object_path(bad);
        }
    }

    #[test]
    fn a_symlinked_object_is_not_present() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open_labeled(dir.path(), "test");
        let hash = cas.put(b"payload").unwrap();
        let real = cas.object_path(&hash);
        let moved = dir.path().join("elsewhere");
        fs::rename(&real, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &real).unwrap();

        assert!(!cas.contains(&hash), "a symlink is not a published object");
        assert!(matches!(cas.touch(&hash), Err(CasError::NotFound)));
        assert!(cas.get_verified(&hash).is_err());
    }

    #[test]
    fn converging_on_an_existing_object_is_not_a_creation() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open_labeled(dir.path(), "test");
        let first = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(first.created);
        let second = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(!second.created, "a dedup must not report creation");
        assert_eq!(first.hash, second.hash);
    }

    #[test]
    fn a_lock_from_another_store_is_refused() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let cas = SharedCas::open_labeled(a.path(), "a");
        let foreign = StoreLock::publish(b.path(), LOCK_DEADLINE).unwrap();
        let err = cas
            .put_in_lock(b"payload", CAS_MAX_OBJECT_BYTES, &foreign)
            .unwrap_err();
        assert!(matches!(err, CasError::PolicyDenied(_)), "got {err:?}");
        drop(foreign);

        let hash = cas.put(b"payload").unwrap();
        let foreign_sweep = StoreLock::sweep(b.path(), LOCK_DEADLINE).unwrap();
        assert!(matches!(
            cas.remove_object(&hash, &foreign_sweep),
            Err(CasError::PolicyDenied(_))
        ));
    }

    #[test]
    fn a_lock_on_the_same_store_spelled_differently_is_accepted() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open_labeled(dir.path(), "test");
        let alias = dir.path().join("..").join(dir.path().file_name().unwrap());
        let guard = StoreLock::publish(&alias, LOCK_DEADLINE).unwrap();
        let outcome = cas
            .put_in_lock(b"payload", CAS_MAX_OBJECT_BYTES, &guard)
            .unwrap();
        assert!(outcome.created);
    }

    #[test]
    fn a_partial_batch_keeps_earlier_objects_published() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open_labeled(dir.path(), "test");
        let guard = cas.lock_for_publish().unwrap();
        let first = cas
            .put_in_lock(b"one", CAS_MAX_OBJECT_BYTES, &guard)
            .unwrap();
        // Second member of the batch is refused by policy.
        assert!(matches!(
            cas.put_in_lock(b"two-is-too-large", 4, &guard),
            Err(CasError::PolicyDenied(_))
        ));
        // Documented contract: no cross-object rollback.
        assert!(cas.contains(&first.hash));
        assert_eq!(cas.list_objects().unwrap(), vec![first.hash]);
    }

    // -- ZS-SEC-005: gated verified reads --------------------------------

    struct RefuseAllGate;
    impl CasReadGate for RefuseAllGate {
        fn authorize_read(&self, sha256: &str) -> Result<(), CasError> {
            Err(CasError::PolicyDenied(format!(
                "gate refuses {sha256}"
            )))
        }
    }

    struct AllowAllGate;
    impl CasReadGate for AllowAllGate {
        fn authorize_read(&self, _sha256: &str) -> Result<(), CasError> {
            Ok(())
        }
    }

    /// A gate refusal short-circuits BEFORE any object lookup: even an
    /// existing object returns no bytes and no existence signal escapes.
    #[test]
    fn gated_read_refuses_before_lookup_and_returns_no_bytes() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"present payload").unwrap();

        let err = cas.get_verified_gated(&hash, &RefuseAllGate).unwrap_err();
        assert!(matches!(err, CasError::PolicyDenied(_)));
        assert_eq!(err.class(), "policy_denied");
    }

    /// An authorizing gate lets the verified read proceed; content still
    /// hashes to the requested identity before bytes are returned.
    #[test]
    fn gated_read_passes_through_an_authorizing_gate() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let bytes = b"authorized payload";
        let hash = cas.put(bytes).unwrap();

        let read = cas.get_verified_gated(&hash, &AllowAllGate).unwrap();
        assert_eq!(read, bytes);
    }
