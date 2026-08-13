    use super::*;
    use std::sync::{Mutex, mpsc};
    use std::thread;
    use zero_abi::zbf::{DurableProfileV1, ZbfArtifactKindV1, ZbfObjectV1};
    use zero_abi::{ArtifactOwnerV1, DigestV1};

    fn setup_rooted_store() -> (tempfile::TempDir, SharedCas, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let root = cas.put(b"live root").unwrap();
        let project = project_id(dir.path()).unwrap();
        publish_reachability_snapshot(
            dir.path(),
            "tokenzero",
            &project,
            1,
            std::slice::from_ref(&root),
        )
        .unwrap();
        (dir, cas, project, root)
    }

    fn verdict(report: &DryRunReport, hash: &str) -> GcVerdict {
        report
            .objects
            .iter()
            .find(|object| object.blob_hash == hash)
            .unwrap()
            .verdict
    }

    #[test]
    fn live_root_and_unreferenced_object_are_classified() {
        let (dir, cas, _, root) = setup_rooted_store();
        let orphan = cas.put(b"orphan").unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &root), GcVerdict::Retain);
        assert_eq!(verdict(&report, &orphan), GcVerdict::Collect);
    }

    #[test]
    fn pins_and_leases_preserve_unrooted_objects() {
        let (dir, cas, project, _) = setup_rooted_store();
        let pinned = cas.put(b"pinned").unwrap();
        publish_pin_record(
            dir.path(),
            &PinRecord {
                schema_version: GC_SCHEMA_VERSION.into(),
                record_type: "pin".into(),
                engine: "tokenzero".into(),
                project_id: project.clone(),
                store_contract_digest: Some(gc_contract_digest_hex()),
                pin_id: "pin-1".into(),
                created_at: format_system_time(SystemTime::now()),
                expires_at: None,
                blob_hash: pinned.clone(),
            },
        )
        .unwrap();
        let leased = cas.put(b"leased").unwrap();
        publish_lease_record(
            dir.path(),
            &LeaseRecord {
                schema_version: GC_SCHEMA_VERSION.into(),
                record_type: "lease".into(),
                engine: "tokenzero".into(),
                project_id: project,
                store_contract_digest: Some(gc_contract_digest_hex()),
                operation_id: "op-1".into(),
                epoch: 1,
                owner: LeaseOwner {
                    pid: 1,
                    host: "test".into(),
                },
                started_at: format_system_time(SystemTime::now()),
                expires_at: format_system_time(
                    SystemTime::now() + std::time::Duration::from_secs(300),
                ),
                grace_seconds: GC_MIN_GRACE_SECONDS,
                blob_hashes: vec![leased.clone()],
            },
        )
        .unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &pinned), GcVerdict::Retain);
        assert_eq!(verdict(&report, &leased), GcVerdict::Retain);
    }

    #[test]
    fn expired_pin_does_not_wedge_collection() {
        let (dir, cas, project, _) = setup_rooted_store();
        let expired = cas.put(b"expired pin").unwrap();
        let unrelated = cas.put(b"unrelated orphan").unwrap();
        publish_pin_record(
            dir.path(),
            &PinRecord {
                schema_version: GC_SCHEMA_VERSION.into(),
                record_type: GC_RECORD_TYPE_PIN.into(),
                engine: "tokenzero".into(),
                project_id: project,
                store_contract_digest: Some(gc_contract_digest_hex()),
                pin_id: "expired-pin".into(),
                created_at: format_system_time(UNIX_EPOCH + std::time::Duration::from_secs(100)),
                expires_at: Some(format_system_time(
                    UNIX_EPOCH + std::time::Duration::from_secs(200),
                )),
                blob_hash: expired.clone(),
            },
        )
        .unwrap();

        let report = run_gc(
            dir.path(),
            &GcConfig {
                now: UNIX_EPOCH + std::time::Duration::from_secs(300),
                ..GcConfig::default()
            },
        )
        .unwrap();
        assert_eq!(verdict(&report, &expired), GcVerdict::Collect);
        assert_eq!(verdict(&report, &unrelated), GcVerdict::Collect);
    }

    #[test]
    fn faulted_sweep_resumes_from_progress_record() {
        let (dir, cas, _, _) = setup_rooted_store();
        let first = cas.put(b"first orphan").unwrap();
        let second = cas.put(b"second orphan").unwrap();
        let failed = run_gc(
            dir.path(),
            &GcConfig {
                run_id: "resume-1".into(),
                apply: true,
                fault_after_deletes: Some(1),
                ..GcConfig::default()
            },
        );
        assert!(matches!(failed, Err(GcError::FaultInjected)));
        assert!(
            dir.path()
                .join("gc/reports/resume-1.progress.json")
                .is_file()
        );
        let resumed = run_gc(
            dir.path(),
            &GcConfig {
                run_id: "resume-1".into(),
                apply: true,
                ..GcConfig::default()
            },
        )
        .unwrap();
        assert!(!cas.contains(&first));
        assert!(!cas.contains(&second));
        assert!(
            resumed
                .objects
                .iter()
                .filter(|o| o.blob_hash == first || o.blob_hash == second)
                .all(|o| o
                    .evidence
                    .iter()
                    .any(|e| e.contains("deleted by this sweep")))
        );
        assert!(
            !dir.path()
                .join("gc/reports/resume-1.progress.json")
                .exists()
        );
    }

    #[test]
    fn stale_epoch_and_bad_version_fail_closed() {
        let (dir, _, project, _) = setup_rooted_store();
        let stale =
            publish_reachability_snapshot(dir.path(), "tokenzero", &project, 1, &[]).unwrap_err();
        assert!(matches!(stale, GcError::SchemaViolation(_)));
        let path = dir
            .path()
            .join("gc/roots/tokenzero")
            .join(&project)
            .join("current.json");
        fs::write(
            &path,
            br#"{"schema_version":"zerostack.cas-gc.v999","record_type":"reachability-snapshot"}"#,
        )
        .unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert!(
            report
                .objects
                .iter()
                .all(|object| object.verdict == GcVerdict::RetainUncertain)
        );
    }

    #[test]
    fn malformed_metadata_is_uncertain_not_collectable() {
        let (dir, cas, project, _) = setup_rooted_store();
        let orphan = cas.put(b"uncertain orphan").unwrap();
        let pin_dir = dir.path().join("gc/pins/tokenzero").join(&project);
        fs::create_dir_all(&pin_dir).unwrap();
        fs::write(pin_dir.join("bad.json"), b"not-json").unwrap();
        let report = run_gc(dir.path(), &GcConfig::default()).unwrap();
        assert_eq!(verdict(&report, &orphan), GcVerdict::RetainUncertain);
    }

    #[test]
    fn publish_is_blocked_during_sweep_unlink_window() {
        let (dir, cas, _, _) = setup_rooted_store();
        let payload = b"race payload";
        let hash = cas.put(payload).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let published_in_hook = Arc::clone(&published);
        let config = GcConfig {
            run_id: "race-1".into(),
            apply: true,
            now: SystemTime::now() + std::time::Duration::from_secs(86_400),
            before_unlink: Some(Arc::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                assert!(!published_in_hook.load(std::sync::atomic::Ordering::SeqCst));
            })),
            ..GcConfig::default()
        };
        let gc_root = dir.path().to_path_buf();
        let gc = thread::spawn(move || run_gc(&gc_root, &config));
        entered_rx.recv().unwrap();
        let publish_root = dir.path().to_path_buf();
        let published_flag = Arc::clone(&published);
        let publisher = thread::spawn(move || {
            let result = SharedCas::open(publish_root).put(payload);
            published_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            result
        });
        thread::sleep(std::time::Duration::from_millis(100));
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        gc.join().unwrap().unwrap();
        assert_eq!(publisher.join().unwrap().unwrap(), hash);
    }

    #[test]
    fn repair_replaces_corrupt_object_and_rejects_wrong_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let bytes = b"repair me";
        let hash = cas.put(bytes).unwrap();
        fs::write(cas.object_path(&hash), b"corrupt").unwrap();
        assert!(repair_object(dir.path(), &hash, bytes).unwrap());
        assert_eq!(cas.get_verified(&hash).unwrap(), bytes);
        assert!(repair_object(dir.path(), &hash, bytes).is_ok_and(|changed| !changed));
        assert!(repair_object(dir.path(), &hash, b"wrong").is_err());
    }

    #[test]
    fn writer_and_progress_bounds_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_id(dir.path()).unwrap();
        let too_many = vec!["a".repeat(64); GC_MAX_BLOB_HASHES + 1];
        assert!(matches!(
            publish_reachability_snapshot(dir.path(), "tokenzero", &project, 1, &too_many),
            Err(GcError::SchemaViolation(_))
        ));
        let lease = LeaseRecord {
            schema_version: GC_SCHEMA_VERSION.into(),
            record_type: "lease".into(),
            engine: "tokenzero".into(),
            project_id: project,
            store_contract_digest: Some(gc_contract_digest_hex()),
            operation_id: "op".into(),
            epoch: 1,
            owner: LeaseOwner {
                pid: 1,
                host: "test".into(),
            },
            started_at: format_system_time(SystemTime::now()),
            expires_at: format_system_time(SystemTime::now() + std::time::Duration::from_secs(300)),
            grace_seconds: GC_MIN_GRACE_SECONDS,
            blob_hashes: too_many,
        };
        assert!(matches!(
            publish_lease_record(dir.path(), &lease),
            Err(GcError::SchemaViolation(_))
        ));
        let progress = dir.path().join("gc/reports/bounds.progress.json");
        let hashes = vec!["a".repeat(64); GC_MAX_BLOB_HASHES + 1];
        fs::create_dir_all(progress.parent().unwrap()).unwrap();
        fs::write(
            &progress,
            serde_json::json!({
                "schema_version": GC_SCHEMA_VERSION,
                "record_type": GC_RECORD_TYPE_SWEEP_PROGRESS,
                "store_contract_digest": gc_contract_digest_hex(),
                "run_id": "bounds",
                "store_root": dir.path(),
                "evaluated_at": format_system_time(SystemTime::now()),
                "plan_digest": "b".repeat(64),
                "objects": hashes,
                "deleted": [],
                "state": "sweeping"
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            run_gc(
                dir.path(),
                &GcConfig {
                    run_id: "bounds".into(),
                    ..GcConfig::default()
                }
            ),
            Err(GcError::CorruptMetadata { .. })
        ));
    }

    #[test]
    fn json_entry_counter_enforces_global_bound() {
        let mut count = GC_MAX_BLOB_HASHES;
        assert!(matches!(
            count_gc_json_entry(&mut count),
            Err(GcError::Policy(_))
        ));
        count = GC_MAX_BLOB_HASHES - 1;
        count_gc_json_entry(&mut count).unwrap();
        assert_eq!(count, GC_MAX_BLOB_HASHES);
    }

    #[test]
    fn report_bounds_fail_closed() {
        let mut objects = Vec::with_capacity(GC_MAX_REPORT_OBJECTS + 1);
        for index in 0..=GC_MAX_REPORT_OBJECTS {
            objects.push(serde_json::json!({
                "blob_hash": format!("{index:064x}"), "verdict": "collect",
                "reason_codes": ["no-live-reference"], "evidence": ["none"]
            }));
        }
        let report = serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION,
            "record_type": GC_RECORD_TYPE_DRY_RUN,
            "store_contract_digest": gc_contract_digest_hex(),
            "run_id": "bounds",
            "store_root": "/tmp/store",
            "evaluated_at": "2026-01-01T00:00:00Z",
            "apply": false,
            "state": "evaluated",
            "objects": objects,
            "deleted": []
        });
        assert!(matches!(
            validate_dry_run_report(&report),
            Err(GcError::SchemaViolation(_))
        ));
        let evidence = vec![serde_json::Value::String("e".into()); GC_MAX_EVIDENCE_ITEMS + 1];
        let report = serde_json::json!({
            "schema_version": GC_SCHEMA_VERSION,
            "record_type": GC_RECORD_TYPE_DRY_RUN,
            "store_contract_digest": gc_contract_digest_hex(),
            "run_id": "bounds",
            "store_root": "/tmp/store",
            "evaluated_at": "2026-01-01T00:00:00Z",
            "apply": false,
            "state": "evaluated",
            "objects": [{
                "blob_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "verdict": "collect",
                "reason_codes": ["no-live-reference"],
                "evidence": evidence
            }],
            "deleted": []
        });
        assert!(matches!(
            validate_dry_run_report(&report),
            Err(GcError::SchemaViolation(_))
        ));
    }

    #[test]
    fn concurrent_atomic_writes_get_unique_temps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gc/reports/record.json");
        let mut workers = Vec::new();
        for index in 0..8 {
            let path = path.clone();
            workers.push(thread::spawn(move || {
                gc_atomic_write(&path, format!("{index}").as_bytes()).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(path.is_file());
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            !name.to_string_lossy().ends_with(".tmp")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gc_namespace_is_uncertain_and_not_followed() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let cas = SharedCas::open(dir.path().to_path_buf());
        let orphan = cas.put(b"orphan outside metadata").unwrap();
        let sentinel = external.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        fs::create_dir_all(dir.path().join("gc")).unwrap();
        symlink(external.path(), dir.path().join("gc/roots")).unwrap();
        let result = run_gc(dir.path(), &GcConfig::default());
        assert!(matches!(result, Err(GcError::CorruptMetadata { .. })));
        assert!(
            publish_reachability_snapshot(
                dir.path(),
                "tokenzero",
                &"a".repeat(64),
                1,
                std::slice::from_ref(&orphan)
            )
            .is_err()
        );
        assert!(cas.contains(&orphan));
        assert!(!external.path().join("gc").exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
    }

    fn zbf_profile() -> DurableProfileV1 {
        DurableProfileV1::portable_strict()
    }

    fn zbf_leaf(payload: &[u8]) -> ZbfObjectV1 {
        ZbfObjectV1::new_leaf(
            ZbfArtifactKindV1::Snapshot,
            ArtifactOwnerV1::ZeroStack,
            DigestV1::from_bytes([1; 32]),
            zbf_profile(),
            DigestV1::from_bytes([2; 32]),
            DigestV1::from_bytes([3; 32]),
            payload.to_vec(),
        )
        .unwrap()
    }

    fn zbf_container(children: Vec<ZbfObjectV1>) -> ZbfObjectV1 {
        ZbfObjectV1::new_container(
            ZbfArtifactKindV1::Snapshot,
            ArtifactOwnerV1::ZeroStack,
            DigestV1::from_bytes([1; 32]),
            zbf_profile(),
            DigestV1::from_bytes([2; 32]),
            DigestV1::from_bytes([3; 32]),
            children,
        )
        .unwrap()
    }

    /// Wrap `payload` in a structurally valid ZBF container header so tests can
    /// exceed the depth bound the object constructors already enforce.
    fn zbf_wrap_container(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(ZBF_HEADER_LEN_V1 + payload.len());
        out.extend_from_slice(&ZBF_MAGIC_V1);
        out.extend_from_slice(&ZBF_SCHEMA_MAJOR_V1.to_be_bytes());
        out.extend_from_slice(&ZBF_SCHEMA_MINOR_V1.to_be_bytes());
        out.extend_from_slice(&5u16.to_be_bytes()); // Plan kind
        out.push(0); // ZeroStack owner
        out.push(ZBF_CONTAINER_FLAG_V1);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        for _ in 0..5 {
            out.extend_from_slice(&[7u8; 32]);
        }
        out.extend_from_slice(&[0u8; 8]);
        out[152..184].copy_from_slice(&Sha256::digest(payload));
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn refs_extraction_is_total_for_leaf_bytes() {
        assert_eq!(
            refs_from_verified_bytes(b"plain bytes").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(refs_from_verified_bytes(&[]).unwrap(), Vec::<String>::new());
        let profile = zbf_profile();
        let leaf = zbf_leaf(b"leaf payload");
        assert_eq!(
            refs_from_verified_bytes(&leaf.to_bytes(profile).unwrap()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn refs_extraction_names_container_children_transitively() {
        let profile = zbf_profile();
        let leaf = zbf_leaf(b"child");
        let inner = zbf_container(vec![leaf.clone()]);
        let outer = zbf_container(vec![inner.clone(), leaf.clone()]);
        let inner_hash = content_sha256_hex(&inner.to_bytes(profile).unwrap());
        let leaf_hash = content_sha256_hex(&leaf.to_bytes(profile).unwrap());
        let refs = refs_from_verified_bytes(&outer.to_bytes(profile).unwrap()).unwrap();
        assert_eq!(refs, vec![inner_hash, leaf_hash]);
    }

    #[test]
    fn refs_extraction_rejects_corrupt_containers() {
        let profile = zbf_profile();
        let container = zbf_container(vec![zbf_leaf(b"child")]);
        let bytes = container.to_bytes(profile).unwrap();

        // Magic with a truncated header is corrupt refs evidence.
        assert!(refs_from_verified_bytes(&bytes[..16]).is_err());
        // Unsupported schema version fails closed.
        let mut wrong_schema = bytes.clone();
        wrong_schema[8] = 0;
        wrong_schema[9] = 0;
        assert!(refs_from_verified_bytes(&wrong_schema).is_err());
        // Unknown flags fail closed.
        let mut unknown_flags = bytes.clone();
        unknown_flags[15] = 0x02;
        assert!(refs_from_verified_bytes(&unknown_flags).is_err());
        // Nonzero reserved header bytes fail closed.
        let mut reserved = bytes.clone();
        reserved[191] = 1;
        assert!(refs_from_verified_bytes(&reserved).is_err());
        // A tampered payload breaks the declared payload digest.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(refs_from_verified_bytes(&tampered).is_err());
        // A leaf with the container flag but no children fails closed.
        let mut fake_container = zbf_leaf(b"x").to_bytes(profile).unwrap();
        fake_container[15] = ZBF_CONTAINER_FLAG_V1;
        assert!(refs_from_verified_bytes(&fake_container).is_err());
    }

    #[test]
    fn refs_extraction_enforces_zbf_depth_bound() {
        let profile = zbf_profile();
        let mut node = zbf_leaf(b"deepest");
        for _ in 0..ZBF_MAX_DEPTH_V1 as usize {
            node = zbf_container(vec![node]);
        }
        let chain = node.to_bytes(profile).unwrap();
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&(chain.len() as u64).to_be_bytes());
        payload.extend_from_slice(&chain);
        let err = refs_from_verified_bytes(&zbf_wrap_container(&payload)).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }
