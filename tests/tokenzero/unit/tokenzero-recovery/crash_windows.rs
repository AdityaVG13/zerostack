//! Phase 6 crash windows: persist / prune / WAL / tmp-rename / lock.
//!
//! In-process refuse/round-trip tests plus Pattern 65 subprocess abort
//! (`TOKENZERO_ARM_CRASH_BOUNDARY`). CrashBoundary names live in
//! `tokenzero_test_support::CrashBoundary`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use tokenzero_core::ContentType;
use tokenzero_recovery::{
    RecoveryStore, STALE_TMP_MAX_AGE, prune_blob_sidecars, prune_recovery_blobs,
    set_ref_index_root_override, sweep_stale_tmp_files,
};

fn wal_path(cache: &Path) -> PathBuf {
    let mut os = cache.as_os_str().to_os_string();
    os.push(".wal");
    PathBuf::from(os)
}

fn sidecar_dir(cache: &Path) -> PathBuf {
    let mut os = cache.as_os_str().to_os_string();
    os.push(".blobs");
    PathBuf::from(os)
}

fn expand_raw(store: &mut RecoveryStore, ref_id: &str) -> tokenzero_recovery::ExpansionResult {
    store.expand(ref_id, Some("raw"), None, None, None, None)
}

fn set_mtime(path: &Path, at: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[test]
fn persist_pending_refuses_unreadable_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let poison = [0xff, 0xfe, 0x00];
    fs::write(&cache, poison).unwrap();
    let mut store = RecoveryStore::new(Some(cache.clone()));
    store
        .persist_pending()
        .expect_err("unreadable snapshot must refuse persist");
    assert_eq!(
        fs::read(&cache).unwrap(),
        poison,
        "persist must not overwrite an unreadable snapshot"
    );
}

#[test]
fn persist_pending_refuses_unparseable_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let poison = b"{not-json";
    fs::write(&cache, poison).unwrap();
    let mut store = RecoveryStore::new(Some(cache.clone()));
    store
        .persist_pending()
        .expect_err("unparseable snapshot must refuse persist");
    assert_eq!(
        fs::read(&cache).unwrap(),
        poison,
        "persist must not overwrite an unparseable snapshot"
    );
}

#[test]
fn expand_unreadable_snapshot_is_not_silent_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    fs::write(&cache, [0xff, 0xfe, 0x00]).unwrap();
    let mut store = RecoveryStore::new(Some(cache));
    let got = expand_raw(&mut store, "tz://blob/deadbeefdeadbeef");
    assert!(!got.found, "unreadable snapshot must not expand as found");
    assert_eq!(
        got.reason, "unreadable-snapshot",
        "unreadable snapshot must fail loud, not a silent miss; got {}",
        got.reason
    );
}

#[test]
fn prune_blob_sidecars_refuses_unreadable_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    fs::write(&cache, [0xff, 0xfe, 0x00]).unwrap();
    let sidecar_dir = sidecar_dir(&cache);
    fs::create_dir_all(&sidecar_dir).unwrap();
    let sidecar = sidecar_dir.join(format!("{}.txt", "a".repeat(64)));
    fs::write(&sidecar, b"payload-bytes-should-survive").unwrap();

    let err =
        prune_blob_sidecars(&cache, 0, false).expect_err("unreadable snapshot must refuse prune");
    assert!(sidecar.is_file(), "sidecar must survive refused prune");
    let msg = err.to_string();
    assert!(
        msg.contains("unreadable"),
        "error must name unreadable snapshot, got {msg}"
    );
}

#[test]
fn prune_recovery_blobs_refuses_unreadable_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    fs::write(&cache, [0xff, 0xfe, 0x00]).unwrap();
    let sidecar_dir = sidecar_dir(&cache);
    fs::create_dir_all(&sidecar_dir).unwrap();
    let sidecar = sidecar_dir.join(format!("{}.txt", "c".repeat(64)));
    fs::write(&sidecar, b"keep-me").unwrap();

    let err = prune_recovery_blobs(&cache, 0, Duration::from_secs(0), false)
        .expect_err("unreadable snapshot must refuse prune");
    assert!(sidecar.is_file(), "sidecar must survive refused prune");
    let msg = err.to_string();
    assert!(
        msg.contains("unreadable"),
        "error must name unreadable snapshot, got {msg}"
    );
}

#[test]
fn second_process_persist_appends_journal_without_snapshot_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache.json");
    let first = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("alpha\n", ContentType::Unknown).unwrap()
    };
    let snapshot_before = fs::read(&cache).unwrap();

    let second = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("beta\n", ContentType::Unknown).unwrap()
    };
    assert_eq!(
        fs::read(&cache).unwrap(),
        snapshot_before,
        "snapshot must be untouched by a journaled persist"
    );
    assert!(wal_path(&cache).exists(), "session WAL sibling must exist");

    let mut restarted = RecoveryStore::new(Some(cache));
    for (ref_id, text) in [(&first, "alpha\n"), (&second, "beta\n")] {
        let expanded = expand_raw(&mut restarted, ref_id);
        assert!(expanded.found, "lost {ref_id}: {}", expanded.reason);
        assert_eq!(expanded.content, text);
    }
}

#[test]
fn missing_snapshot_replays_wal_persist_does_not_drop_it() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache.json");
    {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("alpha\n", ContentType::Unknown).unwrap();
    }
    let wal_blob = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("from-wal\n", ContentType::Unknown)
            .unwrap()
    };
    assert!(wal_path(&cache).is_file(), "journal append must create WAL");
    fs::remove_file(&cache).unwrap();

    let mut restarted = RecoveryStore::new(Some(cache.clone()));
    let expanded = expand_raw(&mut restarted, &wal_blob);
    assert!(
        expanded.found,
        "missing snapshot must still replay complete WAL; {}",
        expanded.reason
    );
    assert_eq!(expanded.content, "from-wal\n");

    restarted
        .persist_pending()
        .expect("WAL-only recover must be allowed to republish");
    assert!(cache.is_file(), "persist must recreate snapshot from WAL");

    let mut third = RecoveryStore::new(Some(cache));
    let again = expand_raw(&mut third, &wal_blob);
    assert!(again.found, "republish must not drop WAL records");
    assert_eq!(again.content, "from-wal\n");
}

#[test]
fn corrupt_journal_tail_keeps_complete_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache.json");
    {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("alpha\n", ContentType::Unknown).unwrap();
    }
    let good = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("good\n", ContentType::Unknown).unwrap()
    };
    let journal = wal_path(&cache);
    let mut bytes = fs::read(&journal).unwrap();
    bytes.extend_from_slice(b"{\"refs\":[\"tz://blob/torn");
    fs::write(&journal, bytes).unwrap();

    let mut restarted = RecoveryStore::new(Some(cache));
    let expanded = expand_raw(&mut restarted, &good);
    assert!(
        expanded.found,
        "complete journal entry poisoned by torn tail: {}",
        expanded.reason
    );
    assert_eq!(expanded.content, "good\n");
}

#[test]
fn kill_before_rename_keeps_previous_complete_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let blob = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("complete\n", ContentType::Unknown)
            .unwrap()
    };
    let dest_before = fs::read(&cache).unwrap();
    let leftover = dir.path().join(".recovery-cache.json.tmp-99-1");
    fs::write(&leftover, b"partial-new-bytes-must-not-become-dest").unwrap();

    let mut restarted = RecoveryStore::new(Some(cache.clone()));
    let expanded = expand_raw(&mut restarted, &blob);
    assert!(
        expanded.found,
        "kill-before-rename lost dest: {}",
        expanded.reason
    );
    assert_eq!(expanded.content, "complete\n");
    assert_eq!(
        fs::read(&cache).unwrap(),
        dest_before,
        "leftover tmp must not replace the previous complete snapshot"
    );
}

#[test]
fn sweep_stale_tmp_removes_expired_under_lock() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    fs::write(&cache, b"{}\n").unwrap();
    let tmp = dir.path().join(".recovery-cache.json.1.0.tmp");
    fs::write(&tmp, b"stale").unwrap();
    let old = SystemTime::now() - Duration::from_secs(60 * 60 * 2);
    set_mtime(&tmp, old);
    let report = sweep_stale_tmp_files(&cache, STALE_TMP_MAX_AGE, false);
    assert_eq!(report.removed, 1);
    assert!(!tmp.exists(), "expired tmp must be unlinked after lock");
}

#[test]
fn sweep_stale_tmp_reclaims_zero_store_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    fs::write(&cache, b"{}\n").unwrap();
    let dest_before = fs::read(&cache).unwrap();
    let hub_tmp = dir.path().join(".recovery-cache.json.tmp-4242-7");
    fs::write(&hub_tmp, b"kill-before-rename leftover").unwrap();
    let old = SystemTime::now() - Duration::from_secs(60 * 60 * 2);
    set_mtime(&hub_tmp, old);
    let report = sweep_stale_tmp_files(&cache, STALE_TMP_MAX_AGE, false);
    assert_eq!(
        report.removed, 1,
        "hub atomic_write leftover .tmp-pid-seq must be swept"
    );
    assert!(!hub_tmp.exists(), "expired hub tmp must be unlinked");
    assert_eq!(
        fs::read(&cache).unwrap(),
        dest_before,
        "sweep must not touch the previous complete snapshot"
    );
}

#[test]
fn concurrent_persistence_preserves_all_thread_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store.store_blob("seed\n", ContentType::Unknown).unwrap();
    }
    let cache_a = cache.clone();
    let cache_b = cache.clone();
    let t1 = std::thread::spawn(move || {
        let mut store = RecoveryStore::new(Some(cache_a));
        store.store_blob("from-a\n", ContentType::Unknown).unwrap()
    });
    let t2 = std::thread::spawn(move || {
        let mut store = RecoveryStore::new(Some(cache_b));
        store.store_blob("from-b\n", ContentType::Unknown).unwrap()
    });
    let a = t1.join().expect("thread a");
    let b = t2.join().expect("thread b");
    let mut restarted = RecoveryStore::new(Some(cache));
    for (ref_id, text) in [(a, "from-a\n"), (b, "from-b\n")] {
        let got = expand_raw(&mut restarted, &ref_id);
        assert!(got.found, "lost {ref_id}: {}", got.reason);
        assert_eq!(got.content, text);
    }
}

/// Must match `BLOB_EXTERNALIZE_MIN_BYTES` in recovery persist.
const BLOB_EXTERNALIZE_MIN_BYTES: usize = 64 * 1024;

fn snapshot_text(cache: &Path) -> String {
    String::from_utf8_lossy(&fs::read(cache).unwrap()).into_owned()
}

#[test]
fn small_blob_persist_keeps_inline_even_after_cas_publish() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let payload = "small-inline-body\n";
    let mut store = RecoveryStore::new(Some(cache.clone()));
    let ref_id = store.store_blob(payload, ContentType::Unknown).unwrap();
    store
        .publish_pending_cas()
        .expect("small blobs still publish to CAS for full-hash expand");
    let snap = snapshot_text(&cache);
    assert!(
        snap.contains("small-inline-body"),
        "bodies below the externalize floor must stay inline in the snapshot"
    );
    assert!(
        !snap.contains("tzx:v1:"),
        "small bodies must not be replaced with a CAS marker"
    );
    let expanded = expand_raw(&mut store, &ref_id);
    assert!(expanded.found, "{}", expanded.reason);
    assert_eq!(expanded.content, payload);
}

#[test]
fn large_blob_persist_replaces_inline_with_cas_marker() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let payload = "L".repeat(BLOB_EXTERNALIZE_MIN_BYTES);
    let mut store = RecoveryStore::new(Some(cache.clone()));
    let ref_id = store
        .store_blob(&payload, ContentType::Unknown)
        .expect("large blob persist");
    let snap = snapshot_text(&cache);
    assert!(
        snap.contains("tzx:v1:"),
        "persist must marker-replace blobs at the externalize floor; snapshot still inline"
    );
    assert!(
        !snap.contains(&payload),
        "snapshot must not still carry the megabyte inline body"
    );
    let expanded = expand_raw(&mut store, &ref_id);
    assert!(
        expanded.found,
        "marker expand lost bytes: {}",
        expanded.reason
    );
    assert_eq!(expanded.content, payload);

    let mut restarted = RecoveryStore::new(Some(cache));
    let again = expand_raw(&mut restarted, &ref_id);
    assert!(
        again.found,
        "restart expand lost marker blob: {}",
        again.reason
    );
    assert_eq!(again.content, payload);
}

/// Persist used to `panic!` after a durable WAL append when ref-index compact
/// hit EACCES/ENOSPC. Compact is a secondary-index rewrite; dest stays intact.
#[cfg(unix)]
#[test]
fn persist_pending_does_not_panic_when_ref_index_compact_fails() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let index = dir.path().join("ref-index");
    fs::create_dir_all(&index).unwrap();
    set_ref_index_root_override(Some(index.clone()));
    struct RestoreOverride;
    impl Drop for RestoreOverride {
        fn drop(&mut self) {
            set_ref_index_root_override(None);
        }
    }
    let _restore_override = RestoreOverride;

    let first = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("seed-ref-index\n", ContentType::Unknown)
            .unwrap()
    };
    let hash = first
        .strip_prefix("tz://blob/")
        .expect("canonical blob ref");
    let prefix = &hash[..3];
    let shard = index.join(format!("{prefix}.ndjson"));
    assert!(shard.is_file(), "persist must create the ref-index shard");

    let pad_line = format!(r#"{{"ref_id":"{first}","store_path":"/stale","ts":1}}"#);
    let mut pad = fs::read(&shard).unwrap();
    while pad.len() <= 1_048_576 {
        pad.extend_from_slice(pad_line.as_bytes());
        pad.push(b'\n');
    }
    fs::write(&shard, &pad).unwrap();

    let prev_mode = fs::metadata(&index).unwrap().permissions();
    fs::set_permissions(&index, fs::Permissions::from_mode(0o555)).unwrap();
    struct RestorePerms {
        dir: PathBuf,
        mode: fs::Permissions,
    }
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.dir, self.mode.clone());
        }
    }
    let _restore_perms = RestorePerms {
        dir: index.clone(),
        mode: prev_mode,
    };

    let collide = {
        let mut probe = RecoveryStore::new(None);
        let mut n = 0u32;
        loop {
            n += 1;
            assert!(n < 50_000, "no sha256 prefix collision for {prefix}");
            let text = format!("collide-{n}\n");
            let id = probe.store_blob_deferred(&text, ContentType::Unknown);
            if id["tz://blob/".len()..].starts_with(prefix) {
                break text;
            }
        }
    };

    let mut store = RecoveryStore::new(Some(cache.clone()));
    let second = store
        .store_blob(&collide, ContentType::Unknown)
        .expect("persist must survive ref-index compact IO failure");
    assert!(
        second["tz://blob/".len()..].starts_with(prefix),
        "collision blob must share the padded shard"
    );
    let shard_after = fs::read_to_string(&shard).unwrap();
    assert!(
        shard_after.contains(&second),
        "append reached the fat shard, so compact ran and must not have panicked"
    );

    fs::set_permissions(&index, fs::Permissions::from_mode(0o755)).unwrap();
    let mut restarted = RecoveryStore::new(Some(cache));
    let got = expand_raw(&mut restarted, &second);
    assert!(
        got.found,
        "durable persist must keep the blob after compact IO failure: {}",
        got.reason
    );
    assert_eq!(got.content, collide);
}

fn crash_child_mode() -> bool {
    std::env::var("TOKENZERO_CRASH_CHILD").as_deref() == Ok("1")
}

fn spawn_armed_child(
    test_fn: &str,
    cache: &Path,
    boundary: &str,
    payload: Option<&str>,
) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("crash_windows test binary");
    let mut cmd = Command::new(exe);
    cmd.arg("--exact")
        .arg(test_fn)
        .arg("--test-threads")
        .arg("1")
        .env("TOKENZERO_CRASH_CHILD", "1")
        .env(tokenzero_recovery::ARM_ENV, boundary)
        .env("TOKENZERO_CRASH_CACHE", cache);
    if let Some(text) = payload {
        cmd.env("TOKENZERO_CRASH_PAYLOAD", text);
    }
    cmd.status().expect("spawn Pattern 65 child")
}

fn child_persist_payload() {
    let cache = std::env::var("TOKENZERO_CRASH_CACHE").expect("TOKENZERO_CRASH_CACHE");
    let payload =
        std::env::var("TOKENZERO_CRASH_PAYLOAD").unwrap_or_else(|_| "child\n".to_string());
    let mut store = RecoveryStore::new(Some(PathBuf::from(cache)));
    let _ = store.store_blob(&payload, ContentType::Unknown);
    let _ = store.persist_pending();
}

fn child_persist_empty() {
    let cache = std::env::var("TOKENZERO_CRASH_CACHE").expect("TOKENZERO_CRASH_CACHE");
    let mut store = RecoveryStore::new(Some(PathBuf::from(cache)));
    let _ = store.persist_pending();
}

fn child_prune() {
    let cache = std::env::var("TOKENZERO_CRASH_CACHE").expect("TOKENZERO_CRASH_CACHE");
    let _ = prune_blob_sidecars(Path::new(&cache), u64::MAX, false);
}

fn json_or_absent_is_not_torn(path: &Path) {
    if !path.exists() {
        return;
    }
    let bytes = fs::read(path).unwrap_or_default();
    if bytes.is_empty() {
        return;
    }
    if bytes.first() == Some(&b'{') {
        assert!(
            bytes.contains(&b'}'),
            "snapshot must not be a torn JSON object after abort"
        );
    }
}

#[test]
fn subprocess_abort_after_wal_append() {
    if crash_child_mode() {
        child_persist_payload();
        panic!("Pattern 65 child must abort at AfterWalAppendSession");
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let first = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("alpha\n", ContentType::Unknown)
            .expect("seed persist")
    };
    let status = spawn_armed_child(
        "subprocess_abort_after_wal_append",
        &cache,
        tokenzero_recovery::AFTER_WAL_APPEND,
        Some("beta\n"),
    );
    assert!(!status.success(), "child must abort, got {status}");
    json_or_absent_is_not_torn(&cache);
    let mut recovered = RecoveryStore::new(Some(cache));
    let a = expand_raw(&mut recovered, &first);
    assert!(
        a.found,
        "acknowledged alpha must survive abort: {}",
        a.reason
    );
    assert_eq!(a.content, "alpha\n");
}

#[test]
fn subprocess_abort_after_journal_append() {
    if crash_child_mode() {
        child_persist_payload();
        panic!("Pattern 65 child must abort at AfterJournalAppendBeforeSnapshotRewrite");
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let first = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("alpha\n", ContentType::Unknown)
            .expect("seed persist")
    };
    let status = spawn_armed_child(
        "subprocess_abort_after_journal_append",
        &cache,
        tokenzero_recovery::AFTER_JOURNAL_APPEND,
        Some("beta\n"),
    );
    assert!(!status.success(), "child must abort, got {status}");
    json_or_absent_is_not_torn(&cache);
    let mut recovered = RecoveryStore::new(Some(cache));
    let a = expand_raw(&mut recovered, &first);
    assert!(
        a.found,
        "acknowledged alpha must survive abort: {}",
        a.reason
    );
    assert_eq!(a.content, "alpha\n");
}

#[test]
fn subprocess_abort_after_tmp_before_rename() {
    if crash_child_mode() {
        child_persist_payload();
        panic!("Pattern 65 child must abort at AfterTmpWriteBeforeRename");
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let first = {
        let mut store = RecoveryStore::new(Some(cache.clone()));
        store
            .store_blob("alpha\n", ContentType::Unknown)
            .expect("seed persist")
    };
    let dest_before = fs::read(&cache).ok();
    let status = spawn_armed_child(
        "subprocess_abort_after_tmp_before_rename",
        &cache,
        tokenzero_recovery::AFTER_TMP_BEFORE_RENAME,
        Some("beta\n"),
    );
    assert!(!status.success(), "child must abort, got {status}");
    if let Some(before) = dest_before {
        if cache.exists() {
            let after = fs::read(&cache).unwrap();
            if after != before {
                json_or_absent_is_not_torn(&cache);
            }
        }
    }
    let mut recovered = RecoveryStore::new(Some(cache));
    let a = expand_raw(&mut recovered, &first);
    assert!(
        a.found,
        "acknowledged alpha must survive abort: {}",
        a.reason
    );
    assert_eq!(a.content, "alpha\n");
}

#[test]
fn subprocess_abort_before_persist_on_unreadable() {
    if crash_child_mode() {
        child_persist_empty();
        panic!("Pattern 65 child must abort at BeforePersistOnUnreadableSnapshot");
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let poison = [0xffu8, 0xfe, 0x00];
    fs::write(&cache, poison).unwrap();
    let status = spawn_armed_child(
        "subprocess_abort_before_persist_on_unreadable",
        &cache,
        tokenzero_recovery::BEFORE_PERSIST_UNREADABLE,
        None,
    );
    assert!(!status.success(), "child must abort, got {status}");
    assert_eq!(
        fs::read(&cache).unwrap(),
        poison,
        "poison snapshot must stay"
    );
}

#[test]
fn subprocess_abort_before_prune_on_unreadable() {
    if crash_child_mode() {
        child_prune();
        panic!("Pattern 65 child must abort at BeforePruneOnUnreadableSnapshot");
    }
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("recovery-cache.json");
    let poison = [0xffu8, 0xfe, 0x00];
    fs::write(&cache, poison).unwrap();
    let status = spawn_armed_child(
        "subprocess_abort_before_prune_on_unreadable",
        &cache,
        tokenzero_recovery::BEFORE_PRUNE_UNREADABLE,
        None,
    );
    assert!(!status.success(), "child must abort, got {status}");
    assert_eq!(
        fs::read(&cache).unwrap(),
        poison,
        "poison snapshot must stay"
    );
}
