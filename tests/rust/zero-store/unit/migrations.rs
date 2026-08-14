//! Migration tests (ZS-OPS-004 / V6-R9): version detection, loud refusal of
//! future versions, ordered idempotent steps v(n) -> v(n+1) that apply
//! exactly once, and golden round-trip fixtures.

use super::*;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

/// Deterministic fixture transform v1 -> v2: writes a fixed state file,
/// appends one line to a run log (so "applied exactly once" is observable),
/// and returns the state digest as the validation digest.
fn fixture_v2_transform(store_root: &Path) -> Result<MigrationStepOutcomeV1, MigrationErrorV1> {
    let state_bytes = b"fixture v2 store state".to_vec();
    let state_dir = store_root.join("gc").join("migrations-fixture");
    fs::create_dir_all(&state_dir).map_err(|e| MigrationErrorV1::Io(e.to_string()))?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("run-log.txt"))
        .and_then(|mut file| file.write_all(b"applied\n"))
        .map_err(|e| MigrationErrorV1::Io(e.to_string()))?;
    atomic_write_file(&state_dir.join("v2-state.json"), &state_bytes)
        .map_err(|e| MigrationErrorV1::Io(e.to_string()))?;
    Ok(MigrationStepOutcomeV1 {
        validation_digest: DigestV1::from_bytes(sha256(&state_bytes)),
    })
}

fn fixture_steps() -> Vec<MigrationStepV1> {
    vec![MigrationStepV1::new(
        1,
        2,
        "fixture-v2",
        fixture_v2_transform,
    )]
}

fn run_log_count(store_root: &Path) -> usize {
    let log = store_root.join("gc").join("migrations-fixture").join("run-log.txt");
    fs::read_to_string(&log)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

#[test]
fn migration_v1_to_v2_applies_exactly_once_and_rerun_is_idempotent() {
    let directory = tempdir().unwrap();
    assert_eq!(detect_store_format_version_v1(directory.path()).unwrap(), None);

    let steps = fixture_steps();
    let receipt = run_store_migrations_v1(directory.path(), &steps).unwrap();
    assert_eq!(receipt.old_version, 1);
    assert_eq!(receipt.new_version, 2);
    assert_eq!(receipt.steps_applied, 1);
    assert_eq!(receipt.applied_step_names, vec!["fixture-v2".to_string()]);
    assert_eq!(
        receipt.old_root,
        StoreFormatVersionV1::new(1).state_digest().unwrap()
    );
    assert_eq!(
        receipt.new_root,
        StoreFormatVersionV1::new(2).state_digest().unwrap()
    );
    assert_ne!(receipt.transform_digest, DigestV1::ZERO);
    assert_ne!(receipt.validation_digest, DigestV1::ZERO);
    receipt.validate().unwrap();

    // The on-disk version advanced; the transform ran exactly once.
    let detected = detect_store_format_version_v1(directory.path())
        .unwrap()
        .unwrap();
    assert_eq!(detected.format_version, 2);
    assert_eq!(run_log_count(directory.path()), 1);
    let marker_dir = directory.path().join("gc").join("migrations");
    let marker_files = fs::read_dir(&marker_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .count();
    assert_eq!(marker_files, 1);

    // Re-running is a no-op: receipt says already at v2, nothing re-applies,
    // and every run still emits its own auditable receipt.
    let again = run_store_migrations_v1(directory.path(), &steps).unwrap();
    assert_eq!(again.old_version, 2);
    assert_eq!(again.new_version, 2);
    assert_eq!(again.steps_applied, 0);
    assert_eq!(again.old_root, again.new_root);
    assert_eq!(run_log_count(directory.path()), 1);
    assert_eq!(
        fs::read_dir(&marker_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .count(),
        1
    );

    // Migration receipts were persisted (one per run) and read back
    // canonically; the no-op run's receipt carries steps_applied 0.
    let receipt_dir = directory.path().join("gc").join("migrations").join("receipts");
    let receipt_entries: Vec<_> = fs::read_dir(&receipt_dir).unwrap().collect();
    assert_eq!(receipt_entries.len(), 2);
    for entry in receipt_entries {
        let entry = entry.unwrap();
        let bytes = fs::read(entry.path()).unwrap();
        let round: MigrationReceiptV1 = serde_json::from_slice(&bytes).unwrap();
        round.validate().unwrap();
        assert!(round == receipt || (round.old_version == 2 && round.steps_applied == 0));
    }
}

#[test]
fn migration_crash_between_transform_and_version_advance_recovers_without_reapply() {
    let directory = tempdir().unwrap();
    let steps = fixture_steps();

    // Simulate a crash after the transform and marker landed but before the
    // version record advanced.
    let state_bytes = b"fixture v2 store state".to_vec();
    let marker = MigrationMarkerV1 {
        schema_version: STORE_FORMAT_SCHEMA_VERSION_V1,
        from_version: 1,
        to_version: 2,
        transform_name: "fixture-v2".to_string(),
        validation_digest: DigestV1::from_bytes(sha256(&state_bytes)),
    };
    let marker_path = steps[0].marker_path(directory.path());
    atomic_write_file(&marker_path, &marker.canonical_bytes().unwrap()).unwrap();
    assert_eq!(run_log_count(directory.path()), 0);
    assert_eq!(detect_store_format_version_v1(directory.path()).unwrap(), None);

    // The runner honors the marker: completes the version advance without
    // re-applying the transform.
    let receipt = run_store_migrations_v1(directory.path(), &steps).unwrap();
    assert_eq!(receipt.steps_applied, 1);
    assert_eq!(receipt.validation_digest, marker.validation_digest);
    assert_eq!(run_log_count(directory.path()), 0);
    assert_eq!(
        detect_store_format_version_v1(directory.path())
            .unwrap()
            .unwrap()
            .format_version,
        2
    );

    // A marker that does not describe this step (different identity triple)
    // is a loud immutable-marker conflict, never silently reused.
    let directory2 = tempdir().unwrap();
    let foreign = MigrationMarkerV1 {
        transform_name: "other-transform".to_string(),
        ..marker
    };
    atomic_write_file(
        &steps[0].marker_path(directory2.path()),
        &foreign.canonical_bytes().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        run_store_migrations_v1(directory2.path(), &steps),
        Err(MigrationErrorV1::ImmutableMarkerConflict(_))
    ));
}

#[test]
fn migration_refuses_future_versions_loudly() {
    // Future version against the production registry (today only
    // SchemaVersionMismatch rejection).
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join(STORE_FORMAT_VERSION_FILENAME),
        StoreFormatVersionV1::new(2).canonical_bytes().unwrap(),
    )
    .unwrap();
    let detected = detect_store_format_version_v1(directory.path()).unwrap().unwrap();
    assert_eq!(detected.format_version, 2);
    assert!(matches!(
        ensure_format_supported_v1(directory.path()),
        Err(MigrationErrorV1::SchemaVersionMismatch {
            detected: 2,
            max_supported: 1
        })
    ));
    assert!(matches!(
        run_store_migrations_v1(directory.path(), &production_migration_steps_v1()),
        Err(MigrationErrorV1::SchemaVersionMismatch {
            detected: 2,
            max_supported: 1
        })
    ));

    // A version beyond even a fixture step set is refused loudly too.
    let directory2 = tempdir().unwrap();
    fs::write(
        directory2.path().join(STORE_FORMAT_VERSION_FILENAME),
        StoreFormatVersionV1::new(3).canonical_bytes().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        run_store_migrations_v1(directory2.path(), &fixture_steps()),
        Err(MigrationErrorV1::SchemaVersionMismatch {
            detected: 3,
            max_supported: 2
        })
    ));

    // An unknown record schema inside the version record fails loudly.
    let directory3 = tempdir().unwrap();
    fs::write(
        directory3.path().join(STORE_FORMAT_VERSION_FILENAME),
        b"{\"format_version\":1,\"schema_version\":99}",
    )
    .unwrap();
    assert!(matches!(
        detect_store_format_version_v1(directory3.path()),
        Err(MigrationErrorV1::UnsupportedRecordSchema(99))
    ));
}

#[test]
fn migration_rejects_non_contiguous_and_non_incrementing_chains() {
    let step = MigrationStepV1::new(1, 3, "skip-v2", fixture_v2_transform);
    assert!(matches!(
        step.validate(),
        Err(MigrationErrorV1::InvalidStepChain(_))
    ));
    let directory = tempdir().unwrap();
    let chain = vec![
        MigrationStepV1::new(1, 2, "to-v2", fixture_v2_transform),
        MigrationStepV1::new(3, 4, "to-v4", fixture_v2_transform),
    ];
    assert!(matches!(
        run_store_migrations_v1(directory.path(), &chain),
        Err(MigrationErrorV1::InvalidStepChain(_))
    ));
}

#[test]
fn migration_golden_round_trip_fixtures() {
    // Golden canonical bytes and format-state root digest for v1.
    let version = StoreFormatVersionV1::new(1);
    let bytes = version.canonical_bytes().unwrap();
    assert_eq!(bytes, br#"{"format_version":1,"schema_version":1}"#);
    assert_eq!(
        version.state_digest().unwrap().to_hex(),
        "3d69716d52e52833438c48fddbdb795c079ddaca8801219576d18e1ed83ff001"
    );
    let round: StoreFormatVersionV1 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(round, version);

    // Golden marker fixture round trip.
    let validation_digest = DigestV1::from_bytes(sha256(b"fixture v2 store state"));
    let marker = MigrationMarkerV1 {
        schema_version: STORE_FORMAT_SCHEMA_VERSION_V1,
        from_version: 1,
        to_version: 2,
        transform_name: "fixture-v2".to_string(),
        validation_digest,
    };
    let marker_bytes = marker.canonical_bytes().unwrap();
    assert_eq!(
        marker_bytes,
        br#"{"from_version":1,"schema_version":1,"to_version":2,"transform_name":"fixture-v2","validation_digest":"36977e66f16e5848eab9e37d0e389011579f0915534106380b37206f8178eb91"}"#
    );
    assert_eq!(
        marker.digest().unwrap().to_hex(),
        "0c9cefe1985fa3b4474f065b36cdee97e74a94b0fca7166d6b0db7a62e9ac831"
    );
    let round_marker: MigrationMarkerV1 = serde_json::from_slice(&marker_bytes).unwrap();
    assert_eq!(round_marker, marker);

    // Golden receipt digest for the deterministic v1 -> v2 fixture run.
    let directory = tempdir().unwrap();
    let receipt = run_store_migrations_v1(directory.path(), &fixture_steps()).unwrap();
    assert_eq!(
        receipt.digest().unwrap().to_hex(),
        "5764d54bb3c4b142142a37acdb40653c0d148edd58df40927418def999ffc7f9"
    );
}
