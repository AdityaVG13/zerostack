    use super::*;
    use tempfile::tempdir;

    fn wal_at(dir: &Path) -> SessionWal {
        SessionWal::new(dir.join("recovery.json"), SessionWalConfig::default()).unwrap()
    }

    #[test]
    fn append_replay_roundtrip_preserves_opaque_bytes() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        assert_eq!(wal.append(b"alpha").unwrap(), AppendOutcome::Appended);
        assert_eq!(wal.append(b"beta").unwrap(), AppendOutcome::Appended);
        let replay = wal.replay().unwrap();
        assert_eq!(replay.records, [b"alpha".to_vec(), b"beta".to_vec()]);
        assert!(!replay.truncated);
    }

    #[test]
    fn megabyte_record_is_accepted() {
        let dir = tempdir().unwrap();
        let mut config = SessionWalConfig::default();
        config.segment_limit = 4 * 1024 * 1024;
        let wal = SessionWal::new(dir.path().join("recovery.json"), config).unwrap();
        let payload = vec![0x7a; 1024 * 1024];
        assert_eq!(wal.append(&payload).unwrap(), AppendOutcome::Appended);
        let replay = wal.replay().unwrap();
        assert_eq!(replay.records, [payload]);
    }

    #[test]
    fn torn_tail_keeps_the_prefix() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        wal.append(b"keep").unwrap();
        wal.append(b"drop-me").unwrap();
        let path = wal.wal_path();
        let mut bytes = fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 3);
        fs::write(&path, bytes).unwrap();
        let replay = wal.replay().unwrap();
        assert_eq!(replay.records, [b"keep".to_vec()]);
        assert!(replay.truncated);
    }

    #[test]
    fn trailer_mismatch_fails_open() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        wal.append(b"one").unwrap();
        let path = wal.wal_path();
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, bytes).unwrap();
        let replay = wal.replay().unwrap();
        assert!(replay.records.is_empty());
        assert!(replay.truncated);
    }

    #[test]
    fn record_larger_than_segment_asks_for_compaction() {
        let dir = tempdir().unwrap();
        let mut config = SessionWalConfig::default();
        config.segment_limit = 32;
        let wal = SessionWal::new(dir.path().join("recovery.json"), config).unwrap();
        assert_eq!(
            wal.append(&[0; 64]).unwrap(),
            AppendOutcome::NeedsCompaction
        );
    }

    #[test]
    fn sealed_segments_replay_in_order() {
        let dir = tempdir().unwrap();
        let mut config = SessionWalConfig::default();
        config.segment_limit = 24;
        config.max_sealed_segments = 2;
        let wal = SessionWal::new(dir.path().join("recovery.json"), config).unwrap();
        assert_eq!(wal.append(b"aaaa").unwrap(), AppendOutcome::Appended);
        assert_eq!(wal.append(b"bbbb").unwrap(), AppendOutcome::Appended);
        assert_eq!(wal.append(b"cccc").unwrap(), AppendOutcome::Appended);
        assert!(wal.sealed_path(1).exists());
        let replay = wal.replay().unwrap();
        assert_eq!(
            replay.records,
            [b"aaaa".to_vec(), b"bbbb".to_vec(), b"cccc".to_vec()]
        );
    }

    #[test]
    fn full_segments_ask_for_compaction() {
        let dir = tempdir().unwrap();
        let mut config = SessionWalConfig::default();
        config.segment_limit = 16;
        config.max_sealed_segments = 1;
        let wal = SessionWal::new(dir.path().join("recovery.json"), config).unwrap();
        assert_eq!(wal.append(b"aa").unwrap(), AppendOutcome::Appended);
        assert_eq!(wal.append(b"bb").unwrap(), AppendOutcome::Appended);
        assert_eq!(wal.append(b"cc").unwrap(), AppendOutcome::NeedsCompaction);
    }

    #[test]
    fn publish_snapshot_clears_the_wal() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        wal.append(b"pending").unwrap();
        wal.publish_snapshot(b"{\"ok\":true}\n").unwrap();
        assert_eq!(fs::read(wal.snapshot_path()).unwrap(), b"{\"ok\":true}\n");
        assert!(!wal.wal_path().exists());
        assert!(wal.replay().unwrap().records.is_empty());
    }

    #[test]
    fn foreign_write_detects_replaced_snapshot() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        wal.publish_snapshot(b"first").unwrap();
        let snap = wal.snapshot_identity();
        let journal = wal.wal_identity();
        assert!(!wal.foreign_write_since(snap, journal));
        fs::write(wal.snapshot_path(), b"foreign").unwrap();
        assert!(wal.foreign_write_since(snap, journal));
    }

    #[test]
    fn missing_wal_replays_empty() {
        let dir = tempdir().unwrap();
        let wal = wal_at(dir.path());
        let replay = wal.replay().unwrap();
        assert!(replay.records.is_empty());
        assert!(!replay.truncated);
    }

    #[test]
    fn contract_names_the_gaps_durable_journal_cannot_cover() {
        let contract = session_wal_contract_v1();
        assert_eq!(contract["torn_tail"], "fail_open");
        assert_eq!(contract["merge"], "caller_owned");
        assert_eq!(contract["not"], "durable_journal_v2");
        assert!(
            contract["max_record_bytes"].as_u64().unwrap() > 64 * 1024,
            "must accept megabyte snapshots"
        );
    }
