
use super::*;
use tempfile::tempdir;

fn write_snapshot(parent: &Path, kind: &str, stamp: u128, bytes: usize) -> PathBuf {
    let path = parent.join(format!("store.db.{kind}-{stamp}-1-0"));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("store.db"), vec![0_u8; bytes]).unwrap();
    path
}

#[test]
fn snapshot_retention_prunes_oldest_past_the_byte_cap() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let oldest = write_snapshot(dir.path(), "forensic", 1, 4096);
    let middle = write_snapshot(dir.path(), "salvage", 2, 4096);
    let newest = write_snapshot(dir.path(), "forensic", 3, 4096);

    let stats = snapshot_storage_stats(&db).unwrap();
    assert_eq!(
        stats
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>(),
        vec![newest.clone(), middle.clone(), oldest.clone()],
        "stats must report newest first"
    );

    let retained = prune_snapshot_destinations_to(&db, 8192).unwrap();

    assert!(
        retained <= 8192,
        "retained {retained} bytes must fit the cap"
    );
    assert!(!oldest.exists(), "oldest snapshot must be pruned first");
    assert!(
        middle.exists(),
        "recent snapshots under the cap must survive"
    );
    assert!(newest.exists(), "newest snapshot must survive");
}

#[test]
fn newest_snapshot_survives_a_budget_smaller_than_itself() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let oldest = write_snapshot(dir.path(), "forensic", 1, 4096);
    let newest = write_snapshot(dir.path(), "forensic", 2, 4096);

    let retained = prune_snapshot_destinations_to(&db, 0).unwrap();

    assert_eq!(retained, 4096);
    assert!(!oldest.exists());
    assert!(
        newest.exists(),
        "the most recent evidence must never be pruned"
    );
}

fn create_clean_db(path: &Path) {
    let conn = OracleConnection::open(path).unwrap();
    conn.execute_batch(CURRENT_TABLES).unwrap();
    conn.execute_batch(CURRENT_INDEXES).unwrap();
}

#[test]
fn vulnerable_writer_never_trusts_attestation_but_future_writer_can() {
    let fingerprint = BTreeMap::from([("store.db".to_string(), "1:2".to_string())]);
    let attestation = Attestation {
        gate_version: GATE_VERSION,
        fsqlite_version: "0.1.19".to_string(),
        fingerprint: fingerprint.clone(),
    };
    assert!(!attestation_matches(
        &attestation,
        &fingerprint,
        VULNERABLE_FSQLITE_VERSION,
        false
    ));
    assert!(attestation_matches(
        &attestation,
        &fingerprint,
        "0.1.19",
        false
    ));
    assert!(!attestation_matches(
        &attestation,
        &fingerprint,
        "0.1.19",
        true
    ));
}

#[test]
fn clean_existing_store_passes_and_writes_attestation() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    gate_existing_store(&db).unwrap();
    assert!(attestation_path(&db).is_file());
}

#[test]
fn wal_participates_in_check() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    let conn = OracleConnection::open(&db).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    conn.execute_batch(CURRENT_TABLES).unwrap();
    conn.execute("INSERT INTO meta(k, v) VALUES ('wal-only', 1)", [])
        .unwrap();
    assert!(dir.path().join("store.db-wal").is_file());
    gate_existing_store(&db).unwrap();
}

#[test]
fn successful_gate_holds_writer_lock_until_handoff() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let guard = gate_existing_store_with_timeout(&db, Duration::from_millis(10)).unwrap();
    let contender = OracleConnection::open(&db).unwrap();
    contender.busy_timeout(Duration::from_millis(10)).unwrap();
    assert!(contender.execute_batch("BEGIN IMMEDIATE").is_err());
    drop(guard);
    contender
        .execute_batch("BEGIN IMMEDIATE; ROLLBACK")
        .unwrap();
}

#[test]
fn writer_contention_fails_closed_without_unlocked_copy() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let writer = OracleConnection::open(&db).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO meta(k, v) VALUES ('pending', 1);")
        .unwrap();
    let error = gate_existing_store_with_timeout(&db, Duration::from_millis(10)).unwrap_err();
    assert!(matches!(error, GateError::Busy(_)), "got {error:?}");
    assert!(error.to_string().contains("writer-excluding lock failed"));
    assert!(!dir.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("forensic-")
    }));
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn forensic_copy_is_complete_and_non_overwriting() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    fs::write(pack_gen_path(&db, 0), b"pack-zero").unwrap();
    fs::write(pack_gen_path(&db, 2), b"pack-two").unwrap();
    let (first, _, _) = create_forensic_and_salvage(&db).unwrap();
    assert!(first.join("SHA256SUMS").is_file());
    assert!(!first.join("INCOMPLETE").exists());
    assert_eq!(
        fs::read(first.join("store.db.pack.g2")).unwrap(),
        b"pack-two"
    );
    // A distinct store state gets its own destination; an identical one is
    // deduplicated (see `repeated_refusal_of_one_store_state_...`).
    fs::write(pack_gen_path(&db, 3), b"pack-three").unwrap();
    let (second, _, _) = create_forensic_and_salvage(&db).unwrap();
    assert_ne!(first, second);
    assert!(second.join("SHA256SUMS").is_file());
    assert!(!second.join("INCOMPLETE").exists());
    assert_eq!(
        fs::read(second.join("store.db.pack.g3")).unwrap(),
        b"pack-three"
    );
    assert!(
        existing_snapshot_destinations(&db).unwrap().len() <= MAX_SNAPSHOT_DESTINATIONS,
        "creating a second pair must stay inside the snapshot cap"
    );
}

#[test]
fn salvage_rebuilds_indexes_and_validates_payload_order_hash_and_locator() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let inline = b"inline payload";
    let packed = b"packed payload";
    let inline_hash = fszero_core::hexutil::sha256_hex_of(Sha256::digest(inline).into());
    let packed_hash = fszero_core::hexutil::sha256_hex_of(Sha256::digest(packed).into());
    let conn = OracleConnection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO payloads VALUES (?1, ?2)",
        rusqlite::params![
            format!("fz://blob/{inline_hash}"),
            [vec![PAYLOAD_TAG_INLINE], inline.to_vec()].concat()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO payloads VALUES (?1, ?2)",
        rusqlite::params![
            format!("fz://blob/{packed_hash}"),
            super::super::encode_packed_locator(0, packed.len() as u32)
        ],
    )
    .unwrap();
    drop(conn);
    fs::write(pack_gen_path(&db, 0), packed).unwrap();
    let (_, salvage, report_path) = create_forensic_and_salvage(&db).unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert_eq!(report["verified"], true);
    assert_eq!(report["payload"]["order_matches"], true);
    assert_eq!(report["payload"]["readable_locators"], 1);
    let salvaged = OracleConnection::open(salvage.join("store.db")).unwrap();
    let indexes: i64 = salvaged
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_payload_lru_tick'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexes, 1);
}

/// Append `count` unreferenced pages and tell the header they exist. This
/// is exactly what fsqlite 0.1.18 leaves behind: pages reachable from
/// neither the b-tree nor the freelist, so `freelist_count` stays 0 and
/// stock SQLite reports `Page N: never used`.
fn leak_pages(db: &Path, count: u32) {
    let mut bytes = fs::read(db).unwrap();
    let page_size = u16::from_be_bytes([bytes[16], bytes[17]]) as usize;
    let page_size = if page_size == 1 { 65536 } else { page_size };
    let pages = u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
    bytes.extend(std::iter::repeat_n(0_u8, page_size * count as usize));
    bytes[28..32].copy_from_slice(&(pages + count).to_be_bytes());
    // Bump the change counter and keep the version-valid-for marker in
    // step, so SQLite trusts the header page-count instead of the file size.
    let change_counter = u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) + 1;
    bytes[24..28].copy_from_slice(&change_counter.to_be_bytes());
    bytes[92..96].copy_from_slice(&change_counter.to_be_bytes());
    fs::write(db, &bytes).unwrap();
}

#[test]
fn observed_store_findings_are_classified_as_repairable() {
    // Verbatim shapes from the corrupt 127 MiB shared store.
    for row in [
        "Page 6288: never used",
        "rowid 41 out of order",
        "row 12 missing from index sqlite_autoindex_payloads_1",
        "wrong # of entries in index idx_call_edges_callee",
        "wrong # of entries in index sqlite_autoindex_payloads_1",
    ] {
        let repairable = is_repairable_finding(row);
        let expected = !row.starts_with("row 12 missing");
        assert_eq!(repairable, expected, "misclassified {row:?}");
    }
}

#[test]
fn data_loss_findings_stay_destructive() {
    for row in [
        "Page 4 is never used",
        "NULL value in payloads.key",
        "database disk image is malformed",
        "corruption at page 3",
    ] {
        assert!(
            !is_repairable_finding(row),
            "{row:?} must not be repairable"
        );
    }
}

#[test]
fn repair_that_drops_rows_is_reported_as_loss() {
    let before = BTreeMap::from([
        ("payloads".to_string(), 12_u64),
        ("call_edges".to_string(), 3_u64),
    ]);
    assert_eq!(lost_rows(&before, &before), None);

    let dropped = BTreeMap::from([
        ("payloads".to_string(), 11_u64),
        ("call_edges".to_string(), 3_u64),
    ]);
    assert_eq!(
        lost_rows(&before, &dropped).as_deref(),
        Some("payloads: 12 -> 11")
    );

    let missing = BTreeMap::from([("call_edges".to_string(), 3_u64)]);
    assert_eq!(
        lost_rows(&before, &missing).as_deref(),
        Some("payloads: 12 -> 0")
    );
}

#[test]
fn self_heal_preserves_every_row_and_index_entry() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    {
        let conn = OracleConnection::open(&db).unwrap();
        for index in 0..64_i64 {
            conn.execute(
                "INSERT INTO payloads(key, value) VALUES (?1, ?2)",
                rusqlite::params![format!("k{index}"), vec![index as u8; 8]],
            )
            .unwrap();
        }
    }
    let before = table_row_counts(&OracleConnection::open(&db).unwrap()).unwrap();
    leak_pages(&db, 3);

    drop(gate_existing_store(&db).expect("repairable findings must not be rejected"));

    let conn = OracleConnection::open(&db).unwrap();
    assert_eq!(integrity_rows(&conn).unwrap(), ["ok"]);
    assert_eq!(table_row_counts(&conn).unwrap(), before);
    assert_eq!(lost_rows(&before, &table_row_counts(&conn).unwrap()), None);
}

#[test]
fn leaked_pages_self_heal_instead_of_quarantining() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    leak_pages(&db, 2);
    {
        let conn = OracleConnection::open(&db).unwrap();
        let rows = integrity_rows(&conn).unwrap();
        let findings = integrity_findings(&rows);
        assert!(
            !findings.is_empty() && findings.iter().all(|row| is_leaked_page_finding(row)),
            "fixture must produce only leaked-page findings, got {rows:?}"
        );
    }

    let guard = gate_existing_store(&db).expect("benign leaked pages must not be rejected");
    drop(guard);

    assert!(attestation_path(&db).is_file());
    let conn = OracleConnection::open(&db).unwrap();
    assert_eq!(integrity_rows(&conn).unwrap(), ["ok"]);
    assert!(
        !dir.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("forensic-")
        }),
        "a repairable finding must not dump a forensic copy"
    );
}

#[test]
fn leaked_page_classifier_rejects_destructive_findings() {
    assert!(is_leaked_page_finding("Page 844: never used"));
    assert!(!is_leaked_page_finding("Page : never used"));
    assert!(!is_leaked_page_finding("Page 844: never used extra"));
    assert!(!is_leaked_page_finding(
        "row 3 missing from index idx_payload_lru_tick"
    ));
    assert!(!is_leaked_page_finding("wrong # of entries in index"));
    // The report is one row with embedded newlines and a banner.
    assert_eq!(
        integrity_findings(&["*** in database main ***\nPage 47: never used".to_string()]),
        ["Page 47: never used"]
    );
    assert!(integrity_findings(&["ok".to_string()]).is_empty());
}

/// Mutation-verified in spirit: making `self_heal_leaked_pages` accept any
/// finding lets a torn b-tree through and fails this.
#[test]
fn destructive_finding_still_quarantines() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    {
        let conn = OracleConnection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE damage(v BLOB); INSERT INTO damage VALUES (zeroblob(12000));",
        )
        .unwrap();
    }
    let mut bytes = fs::read(&db).unwrap();
    let end = bytes.len().min(9000);
    for byte in &mut bytes[5000..end] {
        *byte = 0xA5;
    }
    fs::write(&db, &bytes).unwrap();

    let error = gate_existing_store(&db).unwrap_err();
    assert!(matches!(error, GateError::Destructive(_)), "got {error:?}");
    assert!(
        dir.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("forensic-")
        }),
        "a destructive finding must still be quarantined"
    );
}

#[test]
fn destructive_reset_quarantines_fsqlite_namespace_sidecars() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let gate = PathBuf::from(format!("{}-fsqlite-ns-gate", db.display()));
    let usage = PathBuf::from(format!("{}-fsqlite-ns-use", db.display()));
    fs::write(&gate, b"stale gate").unwrap();
    fs::write(&usage, b"stale usage").unwrap();

    let quarantine = reset_live_store_after_destructive(&db, "test reset").unwrap();
    assert!(!gate.exists());
    assert!(!usage.exists());
    assert!(quarantine.join("store.db-fsqlite-ns-gate").is_file());
    assert!(quarantine.join("store.db-fsqlite-ns-use").is_file());
}

#[test]

fn repeated_refusal_of_one_store_state_makes_one_snapshot_then_stops() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let (forensic, _, _) = create_forensic_and_salvage(&db).unwrap();

    // Same store state: recognise the earlier snapshot, do not copy again.
    let duplicate = create_forensic_and_salvage(&db).unwrap_err();
    assert!(
        duplicate.contains(&forensic.display().to_string()),
        "duplicate refusal must name the existing snapshot: {duplicate}"
    );

    // Distinct states still get snapshots. Oldest siblings are pruned so
    // the live store can never get stuck behind a full cap.
    let first = forensic.clone();
    for generation in 0..8 {
        fs::write(pack_gen_path(&db, generation), format!("pack-{generation}")).unwrap();
        create_forensic_and_salvage(&db)
            .unwrap_or_else(|error| panic!("cap must prune, not refuse: {error}"));
        let count = existing_snapshot_destinations(&db).unwrap().len();
        assert!(
            count <= MAX_SNAPSHOT_DESTINATIONS,
            "snapshot count {count} exceeded cap {MAX_SNAPSHOT_DESTINATIONS}"
        );
    }
    assert!(
        !first.exists(),
        "oldest forensic must be pruned once the cap would otherwise refuse"
    );
}

#[test]
fn create_or_open_resets_malformed_store_even_when_snapshot_cap_is_full() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    {
        let conn = OracleConnection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE damage(v BLOB); INSERT INTO damage VALUES (zeroblob(12000));",
        )
        .unwrap();
    }
    let mut bytes = fs::read(&db).unwrap();
    let end = bytes.len().min(9000);
    for byte in &mut bytes[5000..end] {
        *byte = 0xA5;
    }
    fs::write(&db, &bytes).unwrap();
    for stamp in 1..=3_u128 {
        write_snapshot(dir.path(), "forensic", stamp, 4096);
    }
    assert_eq!(existing_snapshot_destinations(&db).unwrap().len(), 3);

    super::super::RecoveryStore::try_with_durable(&db).expect("CreateOrOpen must recreate");
    assert!(db.is_file(), "live store must exist after reset");
    let conn = OracleConnection::open(&db).unwrap();
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
    let quarantined = dir
        .path()
        .join("quarantine")
        .read_dir()
        .unwrap()
        .any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("reset-")
        });
    assert!(quarantined, "corrupt bytes must land in quarantine/reset-*");
    assert!(
        existing_snapshot_destinations(&db).unwrap().len() <= MAX_SNAPSHOT_DESTINATIONS,
        "reset must not exceed the snapshot cap"
    );
}

#[test]
fn forensic_packs_are_hardlinked_not_duplicated() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    fs::write(pack_gen_path(&db, 0), b"pack-zero").unwrap();
    let (forensic, _, _) = create_forensic_and_salvage(&db).unwrap();
    let copied = forensic.join("store.db.pack");
    assert_eq!(fs::read(&copied).unwrap(), b"pack-zero");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&copied).unwrap().ino(),
            fs::metadata(pack_gen_path(&db, 0)).unwrap().ino(),
            "packs are content-addressed and must not be byte-duplicated"
        );
    }
}

#[test]
fn corrupt_existing_light_is_rejected_without_source_mutation() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    {
        let conn = OracleConnection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE damage(v BLOB); INSERT INTO damage VALUES (zeroblob(12000));",
        )
        .unwrap();
    }
    let mut bytes = fs::read(&db).unwrap();
    let end = bytes.len().min(9000);
    for byte in &mut bytes[5000..end] {
        *byte = 0xA5;
    }
    fs::write(&db, &bytes).unwrap();
    let before = fs::read(&db).unwrap();
    let result = super::super::RecoveryStore::try_open_existing_durable_with_options(&db, false);
    assert!(result.is_err());
    assert_eq!(fs::read(&db).unwrap(), before);
}

#[test]
fn store_gc_plan_is_read_only_and_reports_exact_targets() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let oldest = write_snapshot(dir.path(), "forensic", 1, 4096);
    let middle = write_snapshot(dir.path(), "salvage", 2, 4096);
    let newest = write_snapshot(dir.path(), "forensic", 3, 4096);

    let plan = store_gc_plan(&db, 8192).unwrap();

    assert_eq!(plan.store, "store.db");
    assert_eq!(plan.scanned, 3);
    assert_eq!(plan.total_bytes, 12288);
    assert_eq!(plan.delete.len(), 1);
    assert_eq!(
        plan.delete[0].name,
        oldest.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(plan.delete[0].kind, "forensic");
    assert_eq!(plan.delete_bytes, 4096);
    assert_eq!(plan.retained.len(), 2);
    assert_eq!(
        plan.retained[0].name,
        newest.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(
        plan.retained[1].name,
        middle.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(plan.retained_bytes, 8192);
    // Planning must not mutate anything.
    assert!(oldest.exists() && middle.exists() && newest.exists());
}

#[test]
fn store_gc_apply_prunes_oldest_until_count_cap_when_budget_is_loose() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let mut snapshots = Vec::new();
    for stamp in 1..=6_u128 {
        snapshots.push(write_snapshot(dir.path(), "forensic", stamp, 4096));
    }

    // Budget is loose; the count cap alone forces pruning to the 3 newest.
    let plan = store_gc_apply(&db, u64::MAX).unwrap();

    assert_eq!(plan.count_cap, 3);
    assert_eq!(plan.retained.len(), 3);
    assert_eq!(plan.delete.len(), 3);
    for (index, snapshot) in snapshots[..3].iter().enumerate() {
        assert!(!snapshot.exists(), "oldest {index} must be pruned");
    }
    for snapshot in &snapshots[3..] {
        assert!(snapshot.exists(), "newest snapshot must survive");
    }
}

#[test]
fn store_gc_apply_keeps_newest_when_budget_is_zero() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let oldest = write_snapshot(dir.path(), "forensic", 1, 4096);
    let newest = write_snapshot(dir.path(), "forensic", 2, 4096);

    let plan = store_gc_apply(&db, 0).unwrap();

    assert_eq!(plan.retained_bytes, 4096);
    assert_eq!(plan.delete.len(), 1);
    assert!(!oldest.exists());
    assert!(newest.exists());
}

#[test]
fn store_gc_apply_touches_only_recognized_sibling_dirs() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    let oldest = write_snapshot(dir.path(), "forensic", 1, 4096);
    let newest = write_snapshot(dir.path(), "salvage", 2, 4096);
    // Unrelated neighbors must survive untouched.
    let ast_dir = dir.path().join("store.db.ast");
    fs::create_dir(&ast_dir).unwrap();
    fs::write(ast_dir.join("index"), b"ast").unwrap();
    let pack_file = dir.path().join("store.db.pack");
    fs::write(&pack_file, b"pack").unwrap();
    let other_dir = dir.path().join("other.db.forensic-9-1-0");
    fs::create_dir(&other_dir).unwrap();
    fs::write(other_dir.join("store.db"), vec![0_u8; 8]).unwrap();

    let plan = store_gc_apply(&db, 0).unwrap();

    assert_eq!(plan.delete.len(), 1);
    assert!(!oldest.exists());
    assert!(newest.exists());
    assert!(ast_dir.exists());
    assert!(pack_file.exists());
    assert!(other_dir.exists());
}

#[test]
fn read_only_mtime_bump_does_not_change_attestation_fingerprint() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    gate_existing_store(&db).unwrap();
    let before = fingerprint_store(&db).unwrap();
    let later = fs::metadata(&db).unwrap().modified().unwrap() + Duration::from_secs(3600);
    File::open(&db).unwrap().set_modified(later).unwrap();
    assert_eq!(fingerprint_store(&db).unwrap(), before);
    assert_eq!(before.get("mutation_epoch").map(String::as_str), Some("0"));
}

#[test]
fn second_gate_without_mutation_keeps_attestation_bytes() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    gate_existing_store(&db).unwrap();
    let first = fs::read(attestation_path(&db)).unwrap();
    let later = fs::metadata(&db).unwrap().modified().unwrap() + Duration::from_secs(3600);
    File::open(&db).unwrap().set_modified(later).unwrap();
    gate_existing_store(&db).unwrap();
    assert_eq!(
        fs::read(attestation_path(&db)).unwrap(),
        first,
        "mtime-only change must hit the attestation and skip rewrite"
    );
}

#[test]
fn mutation_epoch_bump_invalidates_attestation_and_next_gate_re_attests() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("store.db");
    create_clean_db(&db);
    gate_existing_store(&db).unwrap();
    let first = fs::read(attestation_path(&db)).unwrap();
    bump_mutation_epoch(&db).unwrap();
    assert_eq!(read_mutation_epoch(&db), 1);
    gate_existing_store(&db).unwrap();
    let second = fs::read(attestation_path(&db)).unwrap();
    assert_ne!(first, second);
    let attested: Attestation = serde_json::from_slice(&second).unwrap();
    assert_eq!(
        attested
            .fingerprint
            .get("mutation_epoch")
            .map(String::as_str),
        Some("1")
    );
}
