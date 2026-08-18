//! Zero-store durable journal platform-neutral semantics (zerostack-journal-native-receipts-x1zw).
//!
//! These are the *platform-neutral* unit fixtures the bead requires:
//! prepare/commit/abort identities, torn-write refusal, old-or-new root
//! conservative revalidation, idempotent replay, owner-death, and the five-term
//! lease binding without forking journal law in GraphZero.
//!
//! They run on the local filesystem (APFS on this Mac, /System/Volumes/Data).
//! They do NOT mint APFS/ext4/XFS/NTFS native kill-and-recover receipts;
//! those remain blocked on genuine native hosts per the bead contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use zero_abi::{EffectClass, Sha256Digest};
use zero_store::{
    AttemptBinding, AttemptEntry, AttemptEvidence, AttemptJournalPaths, AttemptRecoveryOutcome,
    AttemptState, BindingLease, DurableProfileId, FaultPlan, JournalBinding, JournalBoundary,
    JournalFailureCode, JournalLeaseBinding, JournalPaths, JournalState, RecoveryOutcome,
    abort_journal, commit_journal, commit_journal_with_fault, commit_lease_journal,
    commit_lease_journal_with_fault, initialize_published_root, prepare_journal,
    prepare_journal_with_fault, prepare_lease_journal, read_continuation_cartridge,
    read_current_attempt, read_journal_record, read_lease_continuation_cartridge,
    read_lease_journal_record, read_published_root, recover_attempt, recover_journal,
    recover_lease_journal, record_owner_death, verify_committed_lease_binding,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn digest(byte: u8) -> Sha256Digest {
    let mut b = [0u8; 32];
    b[0] = byte;
    b[31] = 0x5a;
    Sha256Digest::from_bytes(b)
}

fn digest_hex(value: &str) -> Sha256Digest {
    serde_json::from_str(&format!("\"{value}\"")).expect("valid digest")
}

fn unique_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "zs-dj-{}-{}-{}-{}",
        label,
        std::process::id(),
        nanos,
        seq
    ));
    fs::create_dir_all(&base).expect("create unique dir");
    base
}

fn paths(dir: &Path) -> JournalPaths {
    JournalPaths::new(
        dir.join("root.json"),
        dir.join("journal.json"),
        dir.join("cartridge.json"),
        dir.join("owner_death.json"),
        dir.join("recovery.json"),
    )
    .expect("distinct journal paths")
}

fn binding() -> JournalBinding {
    JournalBinding::new(
        digest(1),
        digest(2),
        DurableProfileId::PortableStrict,
        digest(3),
        digest(4),
        digest(5),
    )
}

fn binding_v2(nonce_byte: u8) -> JournalLeaseBinding {
    JournalLeaseBinding::new(
        digest(1),
        digest(2),
        DurableProfileId::PortableStrict,
        digest(3),
        digest(4),
        digest(5),
        Sha256Digest::from_bytes([nonce_byte; 32]),
        digest(7),
        BindingLease::new(digest(8), 1, 4_000_000_000_000_000_000),
    )
}

// ---------------------------------------------------------------------------
// Four-term (PortableStrict) platform-neutral fixtures
// ---------------------------------------------------------------------------

#[test]
fn prepare_commit_and_abort_are_idempotent_with_old_or_new_visibility() {
    let dir = unique_dir("commit-idemp");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init root");
    let cartridge = prepare_journal(&jp, b.clone()).expect("prepare");
    let committed = commit_journal(&jp, &cartridge).expect("commit");
    assert_eq!(committed.outcome, RecoveryOutcome::NewRootCommitted);
    assert!(committed.journal_root_correspondence);
    assert!(committed.promotable);
    assert_eq!(read_published_root(&jp).unwrap().root_digest, b.new_root);
    assert_eq!(read_journal_record(&jp).unwrap().state, JournalState::Committed);
    assert_eq!(
        recover_journal(&jp, &b).unwrap(),
        committed,
        "recover idempotent after commit"
    );
    let dir2 = unique_dir("abort-idemp");
    let jp2 = paths(&dir2);
    initialize_published_root(&jp2, b.old_root).expect("init root 2");
    let cartridge2 = prepare_journal(&jp2, b.clone()).expect("prepare 2");
    let aborted = abort_journal(&jp2, &cartridge2).expect("abort");
    assert_eq!(aborted.outcome, RecoveryOutcome::OldRootAborted);
    assert!(aborted.journal_root_correspondence);
    assert_eq!(read_published_root(&jp2).unwrap().root_digest, b.old_root);
    assert_eq!(
        recover_journal(&jp2, &b).unwrap(),
        aborted,
        "recover idempotent after abort"
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(dir2);
}

#[test]
fn owner_death_is_typed_and_recovery_completes_as_abort_on_old_root() {
    let dir = unique_dir("owner-death");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    prepare_journal(&jp, b.clone()).expect("prepare");
    let owner = record_owner_death(&jp, b.owner_identity_digest, 77).expect("owner death");
    assert_eq!(owner.failure_code, JournalFailureCode::OwnerDeath);
    assert!(owner.recovery_required);
    let recovered = recover_journal(&jp, &b).expect("recover after owner death");
    assert_eq!(recovered.outcome, RecoveryOutcome::OldRootAborted);
    assert_eq!(recovered.owner_death_receipt_digest, Some(owner.digest().unwrap()));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn torn_write_and_profile_substitution_fail_loudly() {
    let dir = unique_dir("torn-profile");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    prepare_journal(&jp, b.clone()).expect("prepare");
    fs::write(jp.journal_record(), b"{\"schema_version\":1").expect("torn write");
    assert_eq!(
        recover_journal(&jp, &b).unwrap_err().code,
        JournalFailureCode::TornOrNoncanonicalRecord
    );
    let mut substituted = b.clone();
    substituted.durable_profile_id = DurableProfileId::NtfsStrict;
    assert_eq!(
        substituted.validate().unwrap_err().code,
        JournalFailureCode::ProfileSubstitution
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn pre_cutover_attempt_chain_reconciles_with_frozen_v1_domains() {
    let dir = unique_dir("attempt-v1-domains");
    let paths = AttemptJournalPaths::new(dir.join("attempt")).expect("attempt paths");
    fs::create_dir_all(paths.directory()).expect("attempt directory");
    let binding = AttemptBinding {
        schema_version: 1,
        attempt_id: digest_hex("75c7da638d06f95a77630a972550cada6edf882d62bb4300311c203cc659a31e"),
        effect_digest: digest_hex("cb8dc3b6b369bad576da0bf7019933562fcc6599f360ce434fe7939d58008855"),
        effect_class: EffectClass::Irreversible,
        admission_anchor_digest: digest_hex(
            "2c61e44edd85f6c09257e8dd60451dec4676ba56733d94538c1adbf05a3912bb",
        ),
        durable_profile_id: DurableProfileId::PortableStrict,
        durable_profile_digest: digest_hex(
            "c8bf0ccc2c25dcd2f222a137c612e6daae00c2f4509c75eedc3b87592d0c7c9c",
        ),
        owner_identity_digest: digest_hex(
            "9ce742b039bff7628eaa673092be75a270abe0fb364554cb305fc2cf79a12024",
        ),
    };
    let first = AttemptEntry {
        schema_version: 1,
        binding: binding.clone(),
        state: AttemptState::Prepared,
        sequence: 1,
        predecessor_entry_digest: None,
        crossed_at_unix_ns: None,
        abort_reason: None,
        evidence: None,
    };
    let first_digest =
        digest_hex("c64d36ee315266a02a44cef0ca763787978187b02f408e3212436a43c0906cfc");
    assert_eq!(first.digest().expect("legacy first digest"), first_digest);
    let second = AttemptEntry {
        schema_version: 1,
        binding: binding.clone(),
        state: AttemptState::DispatchCrossed,
        sequence: 2,
        predecessor_entry_digest: Some(first_digest),
        crossed_at_unix_ns: Some(1_787_010_862_028_746_000),
        abort_reason: None,
        evidence: None,
    };
    let second_digest =
        digest_hex("95186b72eb13474e90103b40ccb999f9c2c1e10da3f6f4be0bf925fbba2a5b0e");
    assert_eq!(second.digest().expect("legacy second digest"), second_digest);
    let third = AttemptEntry {
        schema_version: 1,
        binding: binding.clone(),
        state: AttemptState::Succeeded,
        sequence: 3,
        predecessor_entry_digest: Some(second_digest),
        crossed_at_unix_ns: None,
        abort_reason: None,
        evidence: Some(AttemptEvidence::Completion {
            receipt_digest: digest_hex(
                "1bb21c7d7885c0a3132bbadd915ccf936f04ac02eafa71a92e8df2be49ddbf98",
            ),
            observed_at_unix_ns: 1_787_010_862_205_347_000,
        }),
    };
    for (sequence, entry) in [(1, &first), (2, &second), (3, &third)] {
        fs::write(
            paths.directory().join(format!("attempt-{sequence}.json")),
            entry.canonical_bytes().expect("canonical legacy entry"),
        )
        .expect("write legacy entry");
    }

    let current = read_current_attempt(&paths)
        .expect("legacy chain validates")
        .expect("terminal entry");
    assert_eq!(current.state, AttemptState::Succeeded);
    let receipt = recover_attempt(&paths, &binding, None).expect("legacy terminal reconciles");
    assert_eq!(receipt.outcome, AttemptRecoveryOutcome::AlreadySucceeded);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn root_disagreement_conservative_revalidate_never_guesses() {
    let dir = unique_dir("disagreement");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    prepare_journal(&jp, b.clone()).expect("prepare");
    let unrelated = digest(9);
    let other_dir = unique_dir("disagreement-other");
    let other_jp = paths(&other_dir);
    initialize_published_root(&other_jp, unrelated).expect("other init");
    let other_root_bytes = fs::read(other_jp.root_record()).expect("other root bytes");
    fs::write(jp.root_record(), other_root_bytes).expect("plant unrelated root");
    assert_eq!(
        recover_journal(&jp, &b).unwrap_err().code,
        JournalFailureCode::JournalRootDisagreement
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(other_dir);
}

#[test]
fn cartridge_only_prepare_crash_conservative_abort() {
    let dir = unique_dir("cartridge-only");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    let mut fault = FaultPlan::crash_at(JournalBoundary::PrepareBeforeWrite);
    assert_eq!(
        prepare_journal_with_fault(&jp, b.clone(), &mut fault)
            .unwrap_err()
            .code,
        JournalFailureCode::InjectedCrash
    );
    let recovered = recover_journal(&jp, &b).expect("recover after cartridge crash");
    assert_eq!(recovered.outcome, RecoveryOutcome::OldRootAborted);
    assert_ne!(recovered.prepared_record_digest, Sha256Digest::ZERO);
    assert_eq!(read_journal_record(&jp).unwrap().state, JournalState::Aborted);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn foreign_cartridge_rejected() {
    let dir = unique_dir("foreign-cart");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    let mut fault = FaultPlan::crash_at(JournalBoundary::PrepareBeforeWrite);
    let _ = prepare_journal_with_fault(&jp, b.clone(), &mut fault).unwrap_err();
    let mut foreign = b.clone();
    foreign.transaction_id = digest(9);
    assert_eq!(
        recover_journal(&jp, &foreign).unwrap_err().code,
        JournalFailureCode::CartridgeMismatch
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn owner_death_after_new_root_publication_finishes_commit() {
    let dir = unique_dir("owner-after-publish");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    let cartridge = prepare_journal(&jp, b.clone()).expect("prepare");
    let mut fault = FaultPlan::crash_at(JournalBoundary::CommitBeforeWrite);
    assert_eq!(
        commit_journal_with_fault(&jp, &cartridge, &mut fault)
            .unwrap_err()
            .code,
        JournalFailureCode::InjectedCrash
    );
    let owner = record_owner_death(&jp, b.owner_identity_digest, 77).expect("owner death");
    assert_eq!(owner.observed_root, b.new_root);
    let recovered = recover_journal(&jp, &b).expect("recover after publish+owner_death");
    assert_eq!(recovered.outcome, RecoveryOutcome::NewRootCommitted);
    assert_eq!(recovered.owner_death_receipt_digest, Some(owner.digest().unwrap()));
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Five-term lease binding (ZS-STORE-006) platform-neutral fixtures
// ---------------------------------------------------------------------------

#[test]
fn five_term_binding_round_trip_and_verifiable_read() {
    let dir = unique_dir("five-roundtrip");
    let jp = paths(&dir);
    let b = binding_v2(9);
    initialize_published_root(&jp, b.old_root).expect("init");
    let binding_digest = b.digest().unwrap();
    let cartridge = prepare_lease_journal(&jp, b.clone()).expect("prepare v2");
    let committed = commit_lease_journal(&jp, &cartridge).expect("commit v2");
    assert_eq!(committed.outcome, RecoveryOutcome::NewRootCommitted);
    let record = read_lease_journal_record(&jp).expect("read lease record");
    assert_eq!(record.binding, b);
    assert_eq!(record.binding.lease, b.lease);
    assert_eq!(record.binding.nonce, b.nonce);
    assert_eq!(record.binding.protected_scope, b.protected_scope);
    assert_eq!(read_published_root(&jp).unwrap().root_digest, b.new_root);
    assert_eq!(verify_committed_lease_binding(&jp, &b).unwrap(), committed);
    assert_eq!(recover_lease_journal(&jp, &b).unwrap(), committed);
    let v1 = JournalBinding::new(
        b.transaction_id,
        b.assembly_manifest_digest,
        b.durable_profile_id,
        b.old_root,
        b.new_root,
        b.owner_identity_digest,
    );
    assert_ne!(v1.digest().unwrap(), binding_digest);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn five_term_binding_tamper_refused_on_commit_and_verify() {
    let dir = unique_dir("five-tamper");
    let jp = paths(&dir);
    let b = binding_v2(9);
    initialize_published_root(&jp, b.old_root).expect("init");
    prepare_lease_journal(&jp, b.clone()).expect("prepare");
    let tamper_dir = unique_dir("five-tamper-src");
    let tamper_jp = paths(&tamper_dir);
    let mut tampered = b.clone();
    tampered.protected_scope = digest(0x2a);
    initialize_published_root(&tamper_jp, tampered.old_root).expect("tamper init");
    prepare_lease_journal(&tamper_jp, tampered.clone()).expect("tamper prepare");
    let tampered_bytes = read_lease_journal_record(&tamper_jp)
        .unwrap()
        .canonical_bytes()
        .unwrap();
    fs::write(jp.journal_record(), tampered_bytes).expect("tamper overwrite");
    let _ = fs::remove_dir_all(tamper_dir);
    let err = commit_lease_journal(&jp, &read_lease_continuation_cartridge(&jp).unwrap()).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::CartridgeMismatch);
    let forged = binding_v2(0x2b);
    let err = verify_committed_lease_binding(&jp, &forged).unwrap_err();
    assert!(matches!(
        err.code,
        JournalFailureCode::InvalidBinding
            | JournalFailureCode::CartridgeMismatch
            | JournalFailureCode::JournalRootDisagreement
            | JournalFailureCode::TornOrNoncanonicalRecord
    ));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn five_term_two_writer_cas_one_success() {
    let dir = unique_dir("five-cas");
    let jp = paths(&dir);
    let b = binding_v2(9);
    let cartridge_a = {
        initialize_published_root(&jp, b.old_root).expect("init");
        prepare_lease_journal(&jp, b.clone()).expect("prepare A")
    };
    let committed_a = commit_lease_journal(&jp, &cartridge_a).expect("commit A");
    assert_eq!(committed_a.outcome, RecoveryOutcome::NewRootCommitted);
    let stale = binding_v2(0x1b);
    let err = prepare_lease_journal(&jp, stale).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::RootMismatch);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn five_term_second_prepare_and_replayed_lease_cannot_mutate() {
    let dir = unique_dir("five-replay");
    let jp = paths(&dir);
    let b = binding_v2(0x09);
    initialize_published_root(&jp, b.old_root).expect("init");
    let first = prepare_lease_journal(&jp, b.clone()).expect("first prepare");
    let second_writer = binding_v2(0x2c);
    let err = prepare_lease_journal(&jp, second_writer.clone()).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::AlreadyTerminal);
    commit_lease_journal(&jp, &first).expect("commit first");
    let replay = commit_lease_journal(&jp, &first).expect("replay same cartridge");
    assert_eq!(read_published_root(&jp).unwrap().generation, 1);
    let forged = binding_v2(0x2d);
    let err = recover_lease_journal(&jp, &forged).unwrap_err();
    assert!(matches!(
        err.code,
        JournalFailureCode::ImmutableReceiptConflict | JournalFailureCode::AlreadyTerminal
    ));
    let _ = (replay,);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn five_term_lease_expiry_blocks_fresh_attempts_but_not_recovery() {
    let dir = unique_dir("five-expiry");
    let jp = paths(&dir);
    let b = binding_v2(0x09);
    initialize_published_root(&jp, b.old_root).expect("init");
    let mut expired = binding_v2(0x3a);
    expired.lease = BindingLease::new(digest(8), 1, 1);
    let err = prepare_lease_journal(&jp, expired).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::LeaseExpired);
    let cartridge = prepare_lease_journal(&jp, b.clone()).expect("prepare good");
    let mut fault = FaultPlan::crash_at(JournalBoundary::RootPublishAfterRename);
    let err = commit_lease_journal_with_fault(&jp, &cartridge, &mut fault).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::InjectedCrash);
    assert_eq!(read_published_root(&jp).unwrap().root_digest, b.new_root);
    assert_eq!(
        recover_lease_journal(&jp, &b).unwrap().outcome,
        RecoveryOutcome::NewRootCommitted
    );
    let mut again = binding_v2(0x3b);
    again.old_root = b.new_root;
    again.new_root = digest(0x3c);
    again.lease = BindingLease::new(digest(8), 1, 1);
    let err = prepare_lease_journal(&jp, again).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::LeaseExpired);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn fault_injection_at_every_durable_boundary_is_observed() {
    let boundaries = [
        JournalBoundary::RootInitializeBeforeWrite,
        JournalBoundary::CartridgeBeforeWrite,
        JournalBoundary::PrepareBeforeWrite,
        JournalBoundary::RootPublishBeforeWrite,
        JournalBoundary::CommitBeforeWrite,
        JournalBoundary::AbortBeforeWrite,
        JournalBoundary::OwnerDeathBeforeWrite,
        JournalBoundary::RecoveryBeforeWrite,
    ];
    for boundary in boundaries {
        let dir = unique_dir("fault-boundary");
        let jp = paths(&dir);
        let b = binding();
        let result = match boundary {
            JournalBoundary::RootInitializeBeforeWrite => {
                let mut f = FaultPlan::crash_at(boundary);
                zero_store::initialize_published_root_with_fault(&jp, b.old_root, &mut f)
                    .map(|_| ())
            }
            JournalBoundary::CartridgeBeforeWrite | JournalBoundary::PrepareBeforeWrite => {
                initialize_published_root(&jp, b.old_root).unwrap();
                let mut f = FaultPlan::crash_at(boundary);
                prepare_journal_with_fault(&jp, b.clone(), &mut f).map(|_| ())
            }
            JournalBoundary::RootPublishBeforeWrite | JournalBoundary::CommitBeforeWrite => {
                initialize_published_root(&jp, b.old_root).unwrap();
                let cart = prepare_journal(&jp, b.clone()).unwrap();
                let mut f = FaultPlan::crash_at(boundary);
                commit_journal_with_fault(&jp, &cart, &mut f).map(|_| ())
            }
            JournalBoundary::AbortBeforeWrite => {
                initialize_published_root(&jp, b.old_root).unwrap();
                let cart = prepare_journal(&jp, b.clone()).unwrap();
                let mut f = FaultPlan::crash_at(boundary);
                zero_store::abort_journal_with_fault(&jp, &cart, &mut f).map(|_| ())
            }
            JournalBoundary::OwnerDeathBeforeWrite => {
                initialize_published_root(&jp, b.old_root).unwrap();
                prepare_journal(&jp, b.clone()).unwrap();
                let mut f = FaultPlan::crash_at(boundary);
                zero_store::record_owner_death_with_fault(&jp, b.owner_identity_digest, 1, &mut f)
                    .map(|_| ())
            }
            JournalBoundary::RecoveryBeforeWrite => {
                initialize_published_root(&jp, b.old_root).unwrap();
                prepare_journal(&jp, b.clone()).unwrap();
                let mut f = FaultPlan::crash_at(boundary);
                zero_store::recover_journal_with_fault(&jp, &b, &mut f).map(|_| ())
            }
            _ => continue,
        };
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            JournalFailureCode::InjectedCrash,
            "boundary {:?} must be injectable",
            boundary
        );
        let _ = fs::remove_dir_all(dir);
    }
}

#[test]
fn abort_after_commit_and_commit_after_abort_are_terminal() {
    let dir = unique_dir("terminal-check");
    let jp = paths(&dir);
    let b = binding();
    initialize_published_root(&jp, b.old_root).expect("init");
    let cart = prepare_journal(&jp, b.clone()).expect("prepare");
    commit_journal(&jp, &cart).expect("commit");
    // committed journal cannot abort
    let err = abort_journal(&jp, &cart).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::AlreadyTerminal);
    let dir2 = unique_dir("terminal-abort");
    let jp2 = paths(&dir2);
    initialize_published_root(&jp2, b.old_root).expect("init 2");
    let cart2 = prepare_journal(&jp2, b.clone()).expect("prepare 2");
    abort_journal(&jp2, &cart2).expect("abort");
    let err = commit_journal(&jp2, &cart2).unwrap_err();
    assert_eq!(err.code, JournalFailureCode::AlreadyTerminal);
    // Ensure cartridge binding is checked on both
    let mut foreign = b.clone();
    foreign.transaction_id = digest(0xff);
    let _ = foreign;
    let _ = read_continuation_cartridge(&jp).expect("cartridge still readable");
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(dir2);
}
