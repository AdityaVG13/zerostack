use graphzero_reserve::{ReservationLedger, ledger_state_hash};
use graphzero_store::ContentHash;
use graphzero_store::store::delta_log::{DeltaEntry, DeltaLog, entry_type};

#[test]
fn empty_wal_directory_replays_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".graphzero");
    std::fs::create_dir_all(store.join("wal")).unwrap();

    let ledger = ReservationLedger::open(&store).unwrap();
    assert!(ledger.records().is_empty());
    assert_eq!(
        ledger_state_hash(ledger.records()).unwrap(),
        ledger_state_hash(&Vec::<graphzero_reserve::IntentReservation>::new()).unwrap()
    );
}

#[test]
fn malformed_reservation_entry_returns_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".graphzero");
    let mut log = DeltaLog::open(&store).unwrap();
    let payload = b"not-json-reservation".to_vec();
    log.append(DeltaEntry {
        entry_type: entry_type::RESERVATION,
        blob_hash: ContentHash::of(&payload).0,
        payload,
    })
    .unwrap();
    log.commit().unwrap();

    let err = match ReservationLedger::open(&store) {
        Ok(_) => panic!("malformed reservation entry unexpectedly replayed"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("decode reservation ledger entry in segment 0"),
        "unexpected error: {err}"
    );
}

#[test]
fn crash_boundary_corpus_lists_reservation_recovery_cases() {
    let corpus = include_str!("../../../benchmarks/graphzero/crash-boundary/cases.jsonl");
    assert!(corpus.contains("\"component\":\"reservation\""));
    assert!(corpus.contains("empty_wal_directory_replays_as_empty"));
}
