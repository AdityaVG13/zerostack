//! Crash-injection harness for torn writes (fszero-fmz).
//!
//! Damages store files mid-"write" (truncation at dense offsets, byte
//! corruption, garbage tails) and asserts the fszero-ku8 contract on every
//! surface: (1) the store still opens, (2) the damage is DETECTED and
//! REPORTED loudly (integrity_report / typed expand errors), (3) recovery
//! proceeds with everything before the tear intact, (4) corrupted bytes are
//! never served as if valid.
//!
//! The fast subset below runs in CI (dense sample of offsets). The
//! `#[ignore]` sweep walks every byte offset of the pack tail — run it
//! nightly: `cargo test --test crash_injection -- --ignored`.

#[path = "../common/mod.rs"]
mod common;

use common::{TestRoot, env_vars, sha256_hex};
use fs_zero::RecoveryStore;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn blob_ref(data: &[u8]) -> String {
    format!("fz://blob/{}", sha256_hex(data))
}

/// Payloads >= 4096 bytes go to the pack sidecar; below stays inline.
fn big(byte: u8, len: usize) -> Vec<u8> {
    vec![byte; len]
}

fn store_paths(base: &TestRoot) -> (PathBuf, PathBuf) {
    let db = base.join("store/store.sqlite3");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let mut pack = db.as_os_str().to_owned();
    pack.push(".pack");
    (db, PathBuf::from(pack))
}

fn disabled_ref_index(root: &Path) -> common::EnvVarsGuard {
    env_vars(&[
        ("FSZERO_REF_INDEX", Some("0".to_string())),
        (
            "FSZERO_REF_INDEX_PATH",
            Some(root.join("refidx").to_string_lossy().into_owned()),
        ),
    ])
}

#[test]
fn pack_truncation_reported_earlier_payloads_intact() {
    let base = TestRoot::new("crash_pack_trunc");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let first = big(0xA1, 8192);
    let second = big(0xB2, 8192);
    let (r1, r2) = {
        let mut store = RecoveryStore::with_durable(&db);
        let r1 = store.put_content_ref(&first);
        let r2 = store.put_content_ref(&second);
        drop(store);
        (r1, r2)
    };
    let full_len = fs::metadata(&pack).unwrap().len();

    // Dense truncation sample across the second payload's byte range.
    for cut in (full_len - 8192..full_len).step_by(512) {
        let bytes = fs::read(&pack).unwrap();
        fs::write(&pack, &bytes[..cut as usize]).unwrap();
        let store = RecoveryStore::with_durable(&db);
        // (1) opens; (3) everything before the tear intact:
        assert_eq!(
            store.expand(&r1).as_deref(),
            Some(first.as_slice()),
            "cut at {cut}: first payload must survive"
        );
        // (4) the torn payload is never served partially/corrupt:
        assert_eq!(store.expand(&r2), None, "cut at {cut}");
        // (2) the damage is reported, not silently absorbed:
        let (violations, detail) = store.integrity_report();
        assert!(violations >= 1, "cut at {cut}: no integrity report");
        let detail = detail.unwrap();
        assert!(
            detail.contains("torn_pack") || detail.contains("corrupt_payload"),
            "cut at {cut}: {detail}"
        );
        drop(store);
        // Restore for the next iteration.
        let mut restored = bytes;
        restored.truncate(full_len as usize);
        fs::write(&pack, restored).unwrap();
    }
}

#[test]
fn pack_bitrot_never_served_and_reported() {
    let base = TestRoot::new("crash_pack_bitrot");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let payload = big(0xC3, 8192);
    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&payload)
    };
    // Flip one byte inside the packed payload.
    let mut bytes = fs::read(&pack).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(&pack, &bytes).unwrap();

    let store = RecoveryStore::with_durable(&db);
    let res = store.expand_with_tiers(&r);
    assert!(
        res.is_err(),
        "corrupted bytes must never be served: {res:?}"
    );
    let err = res.unwrap_err();
    assert!(
        err.contains("corrupt_payload") || err.contains("ref_unrecoverable"),
        "typed corruption error expected, got: {err}"
    );
    let (violations, detail) = store.integrity_report();
    assert!(violations >= 1);
    assert!(detail.unwrap().contains("sha256 mismatch"));
}

#[test]
fn ref_index_shard_torn_tail_valid_lines_survive() {
    // A torn shard tail (half-written JSONL line + garbage) must not break
    // lookups of the valid prefix, and the damage must be reported.
    let index_root = TestRoot::new("crash_refidx_root");
    let stores = TestRoot::new("crash_refidx_stores");
    let _env = env_vars(&[
        ("FSZERO_REF_INDEX", Some("1".to_string())),
        (
            "FSZERO_REF_INDEX_PATH",
            Some(index_root.path().to_string_lossy().into_owned()),
        ),
    ]);
    let payload = big(0xD4, 8192);
    let r = blob_ref(&payload);
    let db = stores.join("origin/store.sqlite3");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    {
        let mut store = RecoveryStore::with_durable(&db);
        assert_eq!(store.put_content_ref(&payload), r);
    }
    // Find the shard file the append created and tear its tail.
    let shard = walk_one_ndjson(index_root.path()).expect("shard written");
    let mut text = fs::read_to_string(&shard).unwrap();
    text.push_str("{\"ref_id\":\"fz://blob/deadbeef\",\"store_pa"); // torn line
    fs::write(&shard, &text).unwrap();

    // A different session (empty local store) recovers via the ref index.
    let other_db = stores.join("other/store.sqlite3");
    fs::create_dir_all(other_db.parent().unwrap()).unwrap();
    let store = RecoveryStore::with_durable(&other_db);
    assert_eq!(
        store.expand(&r).as_deref(),
        Some(payload.as_slice()),
        "valid prefix of a torn shard must keep serving"
    );
    let (violations, detail) = store.integrity_report();
    assert!(violations >= 1, "torn shard line must be reported");
    assert!(detail.unwrap().contains("ref_index_damaged"));
}

#[test]
fn sqlite_db_truncation_never_panics() {
    // Torn main DB: dense truncation sample. Contract: never a panic; either
    // a clean open (sqlite recovers via WAL discipline) or a loud error the
    // session layer turns into durable_degraded fallback.
    let base = TestRoot::new("crash_db_trunc");
    let _env = disabled_ref_index(base.path());
    let (db, _pack) = store_paths(&base);
    {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_key("k", b"v");
    }
    let bytes = fs::read(&db).unwrap();
    let len = bytes.len();
    for cut in (0..len).step_by((len / 16).max(1)) {
        fs::write(&db, &bytes[..cut]).unwrap();
        // Must not panic; Err is acceptable (loud, session degrades).
        let _ = RecoveryStore::try_with_durable(&db);
        fs::write(&db, &bytes).unwrap();
    }
}

/// Nightly full sweep: every byte offset of the pack tail (fszero-fmz).
#[test]
#[ignore = "full byte-offset sweep; run nightly via -- --ignored"]
fn pack_truncation_full_sweep() {
    let base = TestRoot::new("crash_pack_sweep");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let first = big(0xE5, 4096);
    let second = big(0xF6, 4096);
    let (r1, r2) = {
        let mut store = RecoveryStore::with_durable(&db);
        (
            store.put_content_ref(&first),
            store.put_content_ref(&second),
        )
    };
    let bytes = fs::read(&pack).unwrap();
    let full = bytes.len();
    for cut in full - 4096..full {
        fs::write(&pack, &bytes[..cut]).unwrap();
        let store = RecoveryStore::with_durable(&db);
        assert_eq!(store.expand(&r1).as_deref(), Some(first.as_slice()));
        assert_eq!(store.expand(&r2), None, "cut at {cut}");
        assert!(store.integrity_report().0 >= 1, "cut at {cut}");
    }
    fs::write(&pack, &bytes).unwrap();
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&r2).as_deref(), Some(second.as_slice()));
}

fn walk_one_ndjson(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "ndjson") {
                return Some(path);
            }
        }
    }
    None
}

#[test]
#[cfg(unix)]
fn bulk_build_survives_unreadable_file_mid_ingest() {
    // fszero-coo: one unreadable file must not poison the single-txn bulk
    // build — the parallel ingest skips it (ingest_one_file returns None),
    // every other file lands in the index, and the build commits cleanly.
    use fs_zero::FSZeroSession;
    use std::os::unix::fs::PermissionsExt;
    let base = TestRoot::new("coo_unreadable");
    let _env = disabled_ref_index(base.path());
    for i in 0..80 {
        base.write(&format!("src/m{i}.rs"), &format!("pub fn f_{i}() {{}}\n"));
    }
    let poisoned = base.join("src/m40.rs");
    fs::set_permissions(&poisoned, fs::Permissions::from_mode(0o000)).unwrap();

    let mut s = FSZeroSession::with_repo_store(&base);
    let payload =
        |s: &FSZeroSession| String::from_utf8(s.expand("search").expect("search payload")).unwrap();
    let (_, ok, detail) = s.execute('S', Some("f_39"));
    assert!(ok, "{detail:?}");
    assert!(
        payload(&s).contains("m39.rs"),
        "readable neighbors must be indexed: {}",
        payload(&s)
    );
    let (_, ok, _) = s.execute('S', Some("f_41"));
    assert!(ok, "build continued past the unreadable file");
    assert!(payload(&s).contains("m41.rs"));
    // The poisoned file is absent, not a crash and not a phantom entry.
    let (_, _, _) = s.execute('S', Some("f_40"));
    assert!(
        !payload(&s).contains("DEF: src/m40.rs"),
        "unreadable file must not appear as indexed"
    );
    fs::set_permissions(&poisoned, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn pack_gc_reclaims_dead_bytes_and_preserves_live_payloads() {
    // fszero-qzt: superseded payloads leave dead pack ranges; compaction
    // moves live bytes to a new pack generation atomically and every live
    // payload still expands byte-exact (digest-verified) across reopen.
    let base = TestRoot::new("pack_gc");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let live_a = big(0x11, 8192);
    let live_b = big(0x22, 8192);
    let (ra, rb) = {
        let mut store = RecoveryStore::with_durable(&db);
        // Dead weight: the same KEY overwritten 5 times leaves 4 dead ranges.
        for i in 0..5u8 {
            store.put_key("churn", &big(0x30 + i, 8192));
        }
        (
            store.put_content_ref(&live_a),
            store.put_content_ref(&live_b),
        )
    };
    let before = fs::metadata(&pack).unwrap().len();

    let mut store = RecoveryStore::with_durable(&db);
    let (old_len, new_len) = store.compact_pack().expect("compact");
    assert_eq!(old_len, before);
    assert!(
        new_len < old_len,
        "compaction must reclaim dead bytes ({old_len} -> {new_len})"
    );
    // Live payloads intact through the new generation (digest-verified).
    assert_eq!(store.expand(&ra).as_deref(), Some(live_a.as_slice()));
    assert_eq!(store.expand(&rb).as_deref(), Some(live_b.as_slice()));
    assert_eq!(store.payload("churn").unwrap(), big(0x34, 8192));
    drop(store);

    // Old pack file replaced by the generation file; reopen resolves it.
    assert!(!pack.exists(), "legacy gen-0 pack must be cleaned");
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&ra).as_deref(), Some(live_a.as_slice()));
    assert_eq!(store.expand(&rb).as_deref(), Some(live_b.as_slice()));

    // A second compaction is idempotent-ish and keeps everything live.
    let mut store = store;
    let (l2, n2) = store.compact_pack().expect("compact 2");
    assert!(n2 <= l2);
    assert_eq!(store.expand(&ra).as_deref(), Some(live_a.as_slice()));
}

#[test]
fn pack_gc_stale_new_generation_from_crash_is_harmless() {
    // Crash BETWEEN writing the new generation file and committing the
    // locator txn: the store keeps serving from the old pack; the orphaned
    // file is overwritten by the next compaction.
    let base = TestRoot::new("pack_gc_crash");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let payload = big(0x55, 8192);
    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&payload)
    };
    // Simulate the torn compaction: a garbage next-gen file, txn never ran.
    let mut g1 = pack.clone().into_os_string();
    g1.push(".g1");
    fs::write(std::path::PathBuf::from(g1), b"torn garbage from a crash").unwrap();

    let mut store = RecoveryStore::with_durable(&db);
    assert_eq!(
        store.expand(&r).as_deref(),
        Some(payload.as_slice()),
        "old generation must keep serving after a torn compaction"
    );
    let (_, new_len) = store.compact_pack().expect("compact over stale file");
    assert_eq!(new_len, 8192, "stale garbage replaced, live bytes exact");
    assert_eq!(store.expand(&r).as_deref(), Some(payload.as_slice()));
}

#[test]
fn stale_process_handle_rebinds_after_pack_rotation() {
    // A long-lived process may open generation 0 before another process
    // rotates to generation 1. Its next acknowledged packed put must refresh
    // the durable generation rather than append to the retired inode.
    let base = TestRoot::new("pack_gc_stale_process");
    let _env = disabled_ref_index(base.path());
    let (db, _) = store_paths(&base);
    let initial = big(0x61, 8192);
    let after_rotation = big(0x72, 8192);
    let mut stale_process = RecoveryStore::with_durable(&db);
    let mut rotator = RecoveryStore::with_durable(&db);
    let initial_ref = stale_process.put_content_ref(&initial);
    rotator.compact_pack().expect("rotate active pack");

    let late_ref = stale_process
        .try_put_content_ref(&after_rotation)
        .expect("stale process packed put");
    drop(stale_process);
    drop(rotator);

    let restarted = RecoveryStore::with_durable(&db);
    assert_eq!(
        restarted.expand(&initial_ref).as_deref(),
        Some(initial.as_slice())
    );
    assert_eq!(
        restarted.expand(&late_ref).as_deref(),
        Some(after_rotation.as_slice()),
        "acknowledged post-rotation put must survive process restart"
    );
}

#[test]
fn durable_store_pragma_synchronous_is_full() {
    // PRAGMA synchronous is per-connection: a fresh fsqlite open of the db
    // file defaults to NORMAL and must not be used as the store class.
    // RecoveryStore sets FULL on its live connection (unit-tested). This
    // integration check proves the durable open path creates a rehydratable
    // store under that configuration.
    let base = TestRoot::new("durable_pragma_full");
    let _env = disabled_ref_index(base.path());
    let (db, _) = store_paths(&base);
    let payload = big(0x44, 8192);
    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&payload)
    };
    assert!(db.is_file());
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&r).as_deref(), Some(payload.as_slice()));
}

#[test]
fn acked_packed_put_survives_reopen() {
    let base = TestRoot::new("durable_acked_reopen");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let payload = big(0xA1, 8192);
    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&payload)
    };
    assert!(pack.exists(), "pack sidecar must exist after packed put");
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(
        store.expand(&r).as_deref(),
        Some(payload.as_slice()),
        "acked packed put must survive reopen"
    );
}

#[test]
fn mid_pack_orphan_tail_without_locator_is_harmless() {
    let base = TestRoot::new("durable_orphan_tail");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let first = big(0x11, 8192);
    let r1 = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&first)
    };
    let before = fs::metadata(&pack).unwrap().len();
    let mut f = fs::OpenOptions::new().append(true).open(&pack).unwrap();
    f.write_all(&big(0x22, 8192)).unwrap();
    f.sync_all().unwrap();
    drop(f);
    assert!(fs::metadata(&pack).unwrap().len() > before);

    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&r1).as_deref(), Some(first.as_slice()));
    let orphan_ref = blob_ref(&big(0x22, 8192));
    assert_eq!(
        store.expand(&orphan_ref),
        None,
        "orphan pack tail must not become a recoverable blob"
    );
}

/// Transient `fz://seq/` rows must never divert to the pack sidecar
/// (fszero-5u7).
///
/// The pack's ordering invariant costs an extra `sync_all` barrier before the
/// locator may commit. On macOS that barrier is `F_FULLFSYNC` (~4ms measured
/// on this disk), and it was the single largest fixed cost of every CodeMode
/// plan, which writes its response bundle under exactly this key prefix.
///
/// Diverting these rows to the pack bought nothing. This key class is
/// explicitly execution-scoped: `expand_with_tiers` refuses every `://seq/`
/// ref up front ("Execution-scoped refs are never durable"), and a transient
/// put does not survive a reopen at any size, before or after this change.
/// So the pack's durability-ordering and space-amplification arguments both
/// apply to guarantees this key class does not offer. What the pack did buy
/// was one guaranteed disk barrier per plan.
#[test]
fn transient_put_stays_inline_off_the_pack() {
    let base = TestRoot::new("durable_transient_inline");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);

    // Comfortably over PACK_MIN_BYTES (4096): a non-transient payload this
    // size is packed, so any diversion here would be visible in the sidecar.
    let payload = big(0xC7, 8192);

    let mut store = RecoveryStore::with_durable(&db);
    let r = store.put("codemode/response", &payload);
    assert!(
        r.starts_with("fz://seq/"),
        "expected a transient ref, got {r}"
    );

    let pack_len = if pack.exists() {
        fs::metadata(&pack).unwrap().len()
    } else {
        0
    };
    assert_eq!(
        pack_len, 0,
        "transient payload must not be written to the pack sidecar (pack grew to {pack_len} bytes)"
    );
}

/// A non-transient payload of the same size must still take the pack, so the
/// fix above cannot silently disable packing for everything.
#[test]
fn non_transient_put_of_same_size_still_packs() {
    let base = TestRoot::new("durable_nontransient_packs");
    let _env = disabled_ref_index(base.path());
    let (db, pack) = store_paths(&base);
    let payload = big(0xC8, 8192);

    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        store.put_content_ref(&payload)
    };

    assert!(pack.exists(), "pack sidecar must exist after a packed put");
    assert!(
        fs::metadata(&pack).unwrap().len() >= payload.len() as u64,
        "non-transient payload of this size must still divert to the pack"
    );

    // And it must still survive reopen: this is the durable key class.
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&r).as_deref(), Some(payload.as_slice()));
}

/// The relaxed transient commit must leave the store at `synchronous=FULL`
/// (fszero-5u7).
///
/// Transient-only commits drop to NORMAL to skip an F_FULLFSYNC, which is only
/// sound if the relaxation is strictly scoped to that one COMMIT. If it ever
/// leaked, every later durable commit would silently lose its barrier -- a
/// durability regression that no latency number would reveal. Assert the
/// pragma directly, before and after a transient put, and after a durable one.
#[test]
fn transient_commit_restores_synchronous_full() {
    let base = TestRoot::new("durable_sync_restored");
    let _env = disabled_ref_index(base.path());
    let (db, _pack) = store_paths(&base);

    let mut store = RecoveryStore::with_durable(&db);
    assert_eq!(
        store.synchronous_pragma(),
        Some(2),
        "store must start at FULL"
    );

    let r = store.put("codemode/response", &big(0xE1, 5036));
    assert!(r.starts_with("fz://seq/"));
    assert_eq!(
        store.synchronous_pragma(),
        Some(2),
        "a transient commit must restore synchronous=FULL"
    );

    store.put_content_ref(&big(0xE2, 8192));
    assert_eq!(
        store.synchronous_pragma(),
        Some(2),
        "a durable commit must run and remain at synchronous=FULL"
    );
}

/// A plan that only reads may relax its commit barrier; a plan that mutates
/// must not (fszero-5u7).
///
/// `commit_exec_txn` drops to `synchronous=NORMAL` when the transaction wrote
/// nothing outside the CodeMode execution-scoped key class. The whole safety
/// argument is that a real mutation flips `exec_txn_durable_dirty` and keeps
/// FULL. Assert that flag directly through the store API: a `write-post` row
/// (what a file mutation records) must mark the transaction dirty, and a
/// CodeMode artifact must not.
#[test]
fn only_execution_scoped_writes_keep_the_exec_txn_clean() {
    let base = TestRoot::new("durable_exec_dirty");
    let _env = disabled_ref_index(base.path());
    let (db, _pack) = store_paths(&base);
    let mut store = RecoveryStore::with_durable(&db);

    // CodeMode's own audit trail: clean, so the barrier may be relaxed.
    assert!(store.begin_exec_txn(), "exec txn must open");
    store.put_key("codemode/response", b"{}");
    store.put_key("codemode/telemetry", b"{}");
    store.put_key("fz://codemode/execution/abc/steps", b"[]");
    assert!(
        !store.exec_txn_durable_dirty_for_test(),
        "CodeMode execution artifacts alone must leave the exec txn clean"
    );
    store.commit_exec_txn(true);
    assert_eq!(
        store.synchronous_pragma(),
        Some(2),
        "commit must restore FULL"
    );

    // A file mutation records write-post: durable, so the barrier must stay.
    assert!(store.begin_exec_txn(), "exec txn must reopen");
    store.put_key("write-post", b"content");
    assert!(
        store.exec_txn_durable_dirty_for_test(),
        "a mutation row must mark the exec txn durable-dirty"
    );
    store.commit_exec_txn(true);
    assert_eq!(
        store.synchronous_pragma(),
        Some(2),
        "durable commit stays FULL"
    );
}

/// The mutation a plan performed must survive a reopen (fszero-5u7).
///
/// End-to-end backstop for the flag test above: whatever the barrier logic
/// decides, user data written through a plan-shaped transaction has to be
/// readable from a fresh store.
#[test]
fn mutation_written_in_an_exec_txn_survives_reopen() {
    let base = TestRoot::new("durable_exec_mutation");
    let _env = disabled_ref_index(base.path());
    let (db, _pack) = store_paths(&base);
    let payload = big(0xF3, 8192);

    let r = {
        let mut store = RecoveryStore::with_durable(&db);
        assert!(store.begin_exec_txn());
        store.put_key("codemode/response", b"{}");
        let r = store.put_content_ref(&payload);
        store.commit_exec_txn(true);
        r
    };

    let store = RecoveryStore::with_durable(&db);
    assert_eq!(
        store.expand(&r).as_deref(),
        Some(payload.as_slice()),
        "content written inside an exec txn must survive reopen"
    );
}

fn journal_store_path(root: &Path) -> PathBuf {
    let db = root.join("store/store.sqlite3");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    db
}

fn mutation_count(session: &mut fs_zero::FSZeroSession, path: &str) -> usize {
    let (_, ok, detail) = session.execute('H', Some(path));
    assert!(ok, "history failed: {detail:?}");
    detail
        .as_deref()
        .and_then(|text| {
            text.split_whitespace()
                .find_map(|word| word.strip_prefix("n="))
        })
        .and_then(|value| value.parse().ok())
        .expect("history detail must contain n=<count>")
}

fn spawn_journal_crash(
    root: &Path,
    db: &Path,
    action: &str,
    stage: &str,
) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("current test executable");
    std::process::Command::new(exe)
        .args(["--exact", "journal_crash_child_entrypoint", "--nocapture"])
        .env("FSZERO_JOURNAL_CRASH_ROOT", root)
        .env("FSZERO_JOURNAL_CRASH_DB", db)
        .env("FSZERO_JOURNAL_CRASH_ACTION", action)
        .env("FSZERO_CRASH_MUTATION_AT", stage)
        .env("FSZERO_REF_INDEX", "0")
        .status()
        .expect("spawn journal crash child")
}

/// Child-only entrypoint for T-JU-01..03. The parent selects one product
/// boundary through Command::env; the product abort hook terminates this process.
#[test]
fn journal_crash_child_entrypoint() {
    let Some(root) = std::env::var_os("FSZERO_JOURNAL_CRASH_ROOT").map(PathBuf::from) else {
        return;
    };
    let db = PathBuf::from(std::env::var_os("FSZERO_JOURNAL_CRASH_DB").expect("child DB"));
    let action = std::env::var("FSZERO_JOURNAL_CRASH_ACTION").expect("child action");
    let mut session = fs_zero::FSZeroSession::with_durable_root(&root, &db);
    match action.as_str() {
        "edit" => {
            let _ = session.execute_edit_parts("file.txt", "old", "new");
        }
        "undo" => {
            let _ = session.execute('U', Some("file.txt"));
        }
        other => panic!("unknown child action {other}"),
    }
    panic!("crash stage was not reached");
}

/// T-JU-01: kill after file publication but before edit evidence. Reopen uses
/// the prepared intent to restore the preimage and leaves no history row.
#[test]
fn journal_t_ju_01_edit_kill_before_history_rolls_back() {
    let root = TestRoot::new("journal_t_ju_01");
    let _env = disabled_ref_index(root.path());
    root.write("file.txt", "old");
    let db = journal_store_path(root.path());
    let status = spawn_journal_crash(root.path(), &db, "edit", "edit_after_publish");
    assert!(!status.success(), "child must abort at edit journal window");

    let mut reopened = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"old");
    assert_eq!(mutation_count(&mut reopened, "file.txt"), 0);
    drop(reopened);

    // Reconciliation is idempotent across another restart.
    let mut reopened_again = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"old");
    assert_eq!(mutation_count(&mut reopened_again, "file.txt"), 0);
}

fn prepare_journaled_edit(root: &TestRoot, db: &Path) {
    root.write("file.txt", "old");
    let mut session = fs_zero::FSZeroSession::with_durable_root(root.path(), db);
    let (_, ok, detail) = session.execute_edit_parts("file.txt", "old", "new");
    assert!(ok, "prepare edit failed: {detail:?}");
    assert_eq!(mutation_count(&mut session, "file.txt"), 1);
}

/// T-JU-02: kill after undo changes the file but before reverse history.
/// Reopen restores the still-journaled postimage; a later undo remains valid.
#[test]
fn journal_t_ju_02_undo_kill_after_publish_rolls_back() {
    let root = TestRoot::new("journal_t_ju_02");
    let _env = disabled_ref_index(root.path());
    let db = journal_store_path(root.path());
    prepare_journaled_edit(&root, &db);
    let status = spawn_journal_crash(root.path(), &db, "undo", "undo_after_publish");
    assert!(!status.success(), "child must abort after undo publication");

    let mut reopened = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"new");
    assert_eq!(mutation_count(&mut reopened, "file.txt"), 1);
    let (_, ok, detail) = reopened.execute('U', Some("file.txt"));
    assert!(ok, "undo after recovery failed: {detail:?}");
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"old");
}

/// T-JU-03: kill after staging reverse history inside its SQLite transaction.
/// The uncommitted evidence and file change both roll back on reopen.
#[test]
fn journal_t_ju_03_undo_kill_before_evidence_commit_rolls_back() {
    let root = TestRoot::new("journal_t_ju_03");
    let _env = disabled_ref_index(root.path());
    let db = journal_store_path(root.path());
    prepare_journaled_edit(&root, &db);
    let status = spawn_journal_crash(root.path(), &db, "undo", "undo_before_commit");
    assert!(
        !status.success(),
        "child must abort inside undo evidence transaction"
    );

    let mut reopened = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"new");
    assert_eq!(mutation_count(&mut reopened, "file.txt"), 1);
    drop(reopened);
    let mut reopened_again = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"new");
    assert_eq!(mutation_count(&mut reopened_again, "file.txt"), 1);
}

/// T-JU-04: a torn packed preimage is never materialized by undo. The live
/// postimage and history stay unchanged and the error names the missing ref.
#[test]
fn journal_t_ju_04_undo_rejects_torn_packed_preimage() {
    let root = TestRoot::new("journal_t_ju_04");
    let _env = disabled_ref_index(root.path());
    let db = journal_store_path(root.path());
    let old = "a".repeat(8192);
    let new = "b".repeat(8192);
    root.write("file.txt", &old);
    {
        let mut session = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
        let (_, ok, detail) = session.execute_edit_parts("file.txt", &old, &new);
        assert!(ok, "prepare packed edit failed: {detail:?}");
    }
    let pack = PathBuf::from(format!("{}.pack", db.display()));
    assert!(fs::metadata(&pack).unwrap().len() >= 8192);
    fs::OpenOptions::new()
        .write(true)
        .open(&pack)
        .unwrap()
        .set_len(0)
        .unwrap();

    let mut reopened = fs_zero::FSZeroSession::with_durable_root(root.path(), &db);
    let (_, ok, detail) = reopened.execute('U', Some("file.txt"));
    assert!(!ok, "undo must reject a torn preimage");
    let detail = detail.unwrap_or_default();
    assert!(detail.contains("pre-content unrecoverable"), "{detail}");
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), new.as_bytes());
    assert_eq!(mutation_count(&mut reopened, "file.txt"), 1);
}
