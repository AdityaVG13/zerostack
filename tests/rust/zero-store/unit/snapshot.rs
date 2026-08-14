//! Snapshot-isolation and branch-aware root race tests (ZS-OPS-002 / V6-R14):
//! concurrent writers from one parent root serialize into exactly one
//! authoritative root (losers observe loud refusals -- `RootMismatch` at
//! prepare or `JournalRootDisagreement` at commit, never a silent
//! overwrite), and stale readers are explicit -- a sealed staleness
//! receipt, never a silent redirect. Writers share the published-root
//! record; each transaction owns its journal/cartridge/recovery files.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use tempfile::tempdir;

use zero_abi::DigestV1;
use crate::durable_journal::{
    BindingLeaseV1, JournalBindingV2, JournalFailureCodeV1, JournalPathsV1, RecoveryOutcomeV1,
    commit_journal_v2, initialize_published_root_v1, prepare_journal_v2, read_published_root_v1,
    recover_journal_v2,
};
use crate::gc_lock::{LOCK_DEADLINE, StoreLock};
use crate::DurableProfileIdV1;

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

/// Per-writer transaction paths. The root record is SHARED across writers
/// (the authoritative root file); everything else is private to one
/// transaction, so two writers from the same parent can both prepare and
/// the parent-root CAS decides the winner at commit.
fn writer_paths(directory: &std::path::Path, tag: &str) -> JournalPathsV1 {
    JournalPathsV1::new(
        directory.join("root.json"),
        directory.join(format!("journal-{tag}.json")),
        directory.join(format!("cartridge-{tag}.json")),
        directory.join(format!("owner-{tag}.json")),
        directory.join(format!("recovery-{tag}.json")),
    )
    .unwrap()
}

fn binding_v2(old_root: DigestV1, new_root: DigestV1, nonce_byte: u8) -> JournalBindingV2 {
    JournalBindingV2::new(
        digest(1),
        digest(2),
        DurableProfileIdV1::PortableStrict,
        old_root,
        new_root,
        digest(5),
        DigestV1::from_bytes([nonce_byte; 32]),
        digest(7),
        BindingLeaseV1::new(digest(8), 1, 4_000_000_000_000_000_000),
    )
}

fn setup() -> (tempfile::TempDir, DigestV1) {
    let directory = tempdir().unwrap();
    let genesis = digest(42);
    initialize_published_root_v1(&writer_paths(directory.path(), "seed"), genesis).unwrap();
    (directory, genesis)
}

/// Deterministic branch race: writer A and writer B both prepare from the
/// same parent root; A commits first; B's commit is a loud
/// `JournalRootDisagreement` (never a silent overwrite or a torn root).
/// Exactly one authoritative root survives.
#[test]
fn branch_race_deterministic_interleaving_one_winner_loud_loser() {
    let (directory, genesis) = setup();
    let paths_a = writer_paths(directory.path(), "a");
    let paths_b = writer_paths(directory.path(), "b");

    let a = binding_v2(genesis, digest(11), 0xA1);
    let b = binding_v2(genesis, digest(12), 0xB1);

    // Both writers prepare while the parent root is still current.
    let cartridge_a = prepare_journal_v2(&paths_a, a.clone()).unwrap();
    let cartridge_b = prepare_journal_v2(&paths_b, b.clone()).unwrap();

    // A commits first: wins the parent.
    let receipt_a = commit_journal_v2(&paths_a, &cartridge_a).unwrap();
    assert_eq!(receipt_a.outcome, RecoveryOutcomeV1::NewRootCommitted);
    assert_eq!(
        read_published_root_v1(&paths_a).unwrap().root_digest,
        digest(11)
    );

    // B commits second: loud JournalRootDisagreement -- the committed
    // journal does not accompany its bound new root because the parent
    // root moved under it. Never a silent overwrite.
    let error_b = commit_journal_v2(&paths_b, &cartridge_b).unwrap_err();
    assert_eq!(error_b.code, JournalFailureCodeV1::JournalRootDisagreement);
    assert!(
        error_b.detail.contains("bound new root"),
        "loud refusal must name the root"
    );

    // The authoritative root is exactly A's; B's root is an unreferenced
    // branch and never becomes current.
    assert_eq!(
        read_published_root_v1(&paths_a).unwrap().root_digest,
        digest(11)
    );

    // Recovery of the losing branch is a loud typed refusal, never a
    // silent re-root of the authoritative root.
    let recovered_b = recover_journal_v2(&paths_b, &b).unwrap_err();
    assert_eq!(recovered_b.code, JournalFailureCodeV1::JournalRootDisagreement);
    assert_eq!(
        read_published_root_v1(&paths_a).unwrap().root_digest,
        digest(11)
    );
}

/// A writer that prepares only after the parent root moved observes
/// `RootMismatch` at prepare (the parent-root CAS).
#[test]
fn late_prepare_after_branch_commit_is_loud_root_mismatch() {
    let (directory, genesis) = setup();
    let paths_a = writer_paths(directory.path(), "a");
    let paths_b = writer_paths(directory.path(), "b");

    let a = binding_v2(genesis, digest(13), 0xA2);
    let cartridge_a = prepare_journal_v2(&paths_a, a.clone()).unwrap();
    commit_journal_v2(&paths_a, &cartridge_a).unwrap();

    // B prepares against the now-stale parent: loud RootMismatch.
    let b = binding_v2(genesis, digest(14), 0xB2);
    let error = prepare_journal_v2(&paths_b, b).unwrap_err();
    assert_eq!(error.code, JournalFailureCodeV1::RootMismatch);
    assert_eq!(
        read_published_root_v1(&paths_a).unwrap().root_digest,
        digest(13)
    );
}

/// Concurrent branch race: two threads prepare+commit from the same parent
/// under the store's exclusive commit lock (the production protocol: the
/// root record is metadata published under the exclusive coordination
/// lock). Whatever the interleaving, exactly one commit wins, the loser
/// fails loud (`RootMismatch` at prepare or `JournalRootDisagreement` at
/// commit), and the final root is exactly the winner's new root.
///
/// Without the exclusive lock the root-record check-then-rename is a TOCTOU
/// (two concurrent commits from one parent can both report success, final
/// root = last rename) -- that is exactly why the publish protocol requires
/// the exclusive lock and why the hub serializes commit boundaries
/// (`CODE_COMMIT_RACE` in zsx-node). The lock is the serialization point;
/// the fixture exercises the real protocol.
#[test]
fn branch_race_concurrent_writers_serialize_to_one_authoritative_root() {
    let (directory, genesis) = setup();
    let directory = Arc::new(directory);

    let wins = Arc::new(AtomicUsize::new(0));
    let refusals = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();
    for (tag, nonce, new_root) in [("c", 0xC1u8, digest(21)), ("d", 0xD1u8, digest(22))] {
        let directory = Arc::clone(&directory);
        let wins = Arc::clone(&wins);
        let refusals = Arc::clone(&refusals);
        threads.push(thread::spawn(move || {
            let paths = writer_paths(directory.path(), tag);
            let binding = binding_v2(genesis, new_root, nonce);
            let cartridge = match prepare_journal_v2(&paths, binding.clone()) {
                Ok(cartridge) => cartridge,
                Err(error) => {
                    assert_eq!(error.code, JournalFailureCodeV1::RootMismatch);
                    refusals.fetch_add(1, Ordering::SeqCst);
                    return;
                }
            };
            // Commit under the exclusive store coordination lock: the root
            // publication is metadata and the contract says metadata is
            // published exclusively, so two racing commits serialize here.
            let _guard = StoreLock::sweep(directory.path(), LOCK_DEADLINE).unwrap();
            match commit_journal_v2(&paths, &cartridge) {
                Ok(_) => {
                    wins.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) => {
                    assert_eq!(error.code, JournalFailureCodeV1::JournalRootDisagreement);
                    refusals.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for handle in threads {
        handle.join().unwrap();
    }

    // Exactly one winner; the loser refused loudly; final root is the
    // winner's new root (serializable authoritative roots).
    assert_eq!(wins.load(Ordering::SeqCst), 1);
    assert_eq!(refusals.load(Ordering::SeqCst), 1);
    let root = read_published_root_v1(&writer_paths(directory.path(), "a"))
        .unwrap()
        .root_digest;
    assert!(
        root == digest(21) || root == digest(22),
        "final root must be exactly one winner's new root, got {root}"
    );
}

/// Explicit stale-reader semantics: a reader snapshotting before a commit
/// resolves stale afterward and receives a sealed receipt naming both
/// roots -- the store never silently advances or mixes roots.
#[test]
fn stale_reader_is_explicit_and_never_silently_advanced() {
    let (directory, genesis) = setup();
    let paths = writer_paths(directory.path(), "writer");

    // Reader takes a snapshot at the genesis root.
    let view = take_root_snapshot_v1(&paths).unwrap();
    assert_eq!(view.root, genesis);

    // A writer commits a new root.
    let writer = binding_v2(genesis, digest(31), 0xE1);
    let cartridge = prepare_journal_v2(&paths, writer.clone()).unwrap();
    commit_journal_v2(&paths, &cartridge).unwrap();

    // The reader resolves: stale is explicit with a sealed receipt.
    let resolution = resolve_snapshot_read_v1(&paths, view).unwrap();
    assert!(resolution.stale);
    assert_eq!(resolution.current_root, digest(31));
    assert_eq!(resolution.receipt.view_root, genesis);
    assert_eq!(resolution.receipt.current_root, digest(31));
    assert_eq!(resolution.receipt.stale, true);
    assert_ne!(resolution.receipt.digest(), DigestV1::ZERO);

    // A fresh snapshot is consistent: stale == false, receipt seals it.
    let fresh = take_root_snapshot_v1(&paths).unwrap();
    let consistent = resolve_snapshot_read_v1(&paths, fresh).unwrap();
    assert!(!consistent.stale);
    assert_eq!(consistent.receipt.view_root, consistent.receipt.current_root);

    // Staleness receipts are deterministic and tamper-sensitive.
    let replay = resolve_snapshot_read_v1(&paths, view).unwrap();
    assert_eq!(replay.receipt, resolution.receipt);
    let original_receipt = resolution.receipt.clone();
    let mut tampered = resolution.receipt;
    tampered.view_root = digest(99);
    assert_ne!(tampered.digest(), original_receipt.digest());
}

/// A reader that never re-snapshots keeps seeing the sealed staleness
/// receipt across any number of commits -- reads are served from the
/// snapshot root or refused, never redirected.
#[test]
fn snapshot_view_is_immutable_across_many_commits() {
    let (directory, genesis) = setup();
    let view = take_root_snapshot_v1(&writer_paths(directory.path(), "seed")).unwrap();

    let mut current = genesis;
    for index in 0..3u8 {
        let tag = format!("tx{index}");
        let paths = writer_paths(directory.path(), &tag);
        let next = digest(40 + index);
        let writer = binding_v2(current, next, 0xF0 + index);
        let cartridge = prepare_journal_v2(&paths, writer.clone()).unwrap();
        commit_journal_v2(&paths, &cartridge).unwrap();
        current = next;
    }

    let resolution = resolve_snapshot_read_v1(
        &writer_paths(directory.path(), "seed"),
        view,
    )
    .unwrap();
    assert!(resolution.stale);
    assert_eq!(resolution.receipt.view_root, genesis);
    assert_eq!(resolution.receipt.current_root, current);
}

/// The contract manifest freezes snapshot-isolation semantics.
#[test]
fn contract_manifest_freezes_isolation_semantics() {
    let manifest = snapshot_isolation_contract_v1();
    assert_eq!(manifest["schema_version"], SNAPSHOT_SCHEMA_VERSION_V1);
    assert_eq!(
        manifest["reader"]["stale_reader"],
        serde_json::json!("explicit sealed receipt (view root vs current root); never silent")
    );
    assert_eq!(
        manifest["writer"]["commit"],
        serde_json::json!("parent-root CAS; one winner per parent; losers observe RootMismatch")
    );
}
