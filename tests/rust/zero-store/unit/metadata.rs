    use super::*;
    use tempfile::tempdir;

    fn meta(n: usize) -> ObservationMetadata {
        ObservationMetadata {
            source_engine: format!("engine-{n}"),
            session: "s".into(),
            timestamp: format!("2026-07-28T00:00:0{n}Z"),
            declared_kind: "trace".into(),
        }
    }

    #[test]
    fn metadata_is_append_only_deterministic_and_digest_neutral() {
        let plain_root = tempdir().unwrap();
        let observed_root = tempdir().unwrap();
        let plain = SharedCas::open(plain_root.path());
        let observed = SharedCas::open(observed_root.path());
        let golden = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(plain.put(b"").unwrap(), golden);
        assert_eq!(observed.ingest_with_metadata(b"", meta(1)).unwrap(), golden);
        assert_eq!(observed.ingest_with_metadata(b"", meta(0)).unwrap(), golden);
        assert_eq!(observed.ingest_with_metadata(b"", meta(0)).unwrap(), golden);
        let events = observed.observation_metadata(golden).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events, observed.observation_metadata(golden).unwrap());
        assert!(events.contains(&meta(0)) && events.contains(&meta(1)));
        assert_eq!(observed.get_verified(golden).unwrap(), b"");
    }

    #[test]
    fn concurrent_identical_events_converge_and_distinct_events_persist() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let cas = cas.clone();
                std::thread::spawn(move || cas.ingest_with_metadata(b"same", meta(n % 2)).unwrap())
            })
            .collect();
        let ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(ids.iter().all(|id| id == &ids[0]));
        let events = cas.observation_metadata(&ids[0]).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&meta(0)) && events.contains(&meta(1)));
    }

    #[test]
    fn malformed_canonical_sidecar_is_typed_and_debris_is_ignored() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let id = cas.put(b"payload").unwrap();
        let dir = event_dir(root.path(), &id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("README"), b"debris").unwrap();
        assert!(cas.observation_metadata(&id).unwrap().is_empty());
        fs::write(dir.join(format!("{}.json", "0".repeat(64))), b"bad").unwrap();
        assert_eq!(
            cas.observation_metadata(&id).unwrap_err().class(),
            "malformed"
        );
        assert_eq!(
            cas.observation_metadata("../escape").unwrap_err().class(),
            "malformed"
        );
    }

    #[test]
    fn resident_objects_are_byte_exact_until_guarded_gc_removal() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let payloads = [
            Vec::new(),
            b"alpha".to_vec(),
            vec![0, 1, 2, 255],
            vec![b'x'; 4096],
        ];
        let resident: Vec<_> = payloads
            .iter()
            .enumerate()
            .map(|(n, bytes)| {
                let id = if n % 2 == 0 {
                    cas.ingest_with_metadata(bytes, meta(n)).unwrap()
                } else {
                    cas.put(bytes).unwrap()
                };
                assert_eq!(cas.get_verified(&id).unwrap(), *bytes);
                (id, bytes)
            })
            .collect();
        for (step, (id, _)) in resident.iter().enumerate() {
            for (candidate, expected) in resident.iter().skip(step) {
                assert_eq!(cas.get_verified(candidate).unwrap(), **expected);
            }
            let guard = cas.lock_for_sweep().unwrap();
            cas.remove_object(id, &guard).unwrap();
            drop(guard);
            assert_eq!(cas.get_verified(id).unwrap_err().class(), "missing");
        }
    }
