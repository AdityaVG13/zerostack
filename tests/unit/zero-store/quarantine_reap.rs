//! SURF-0013: put reaps stale tmp files older than CAS_TEMP_REAP_AGE and
//! quarantine moves digest-mismatched bodies into gc/quarantine.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zero_ref::content_hash_hex;
use zero_store::{SharedCas, CAS_QUARANTINE_DIR, CAS_TEMP_REAP_AGE};

fn unique_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "zs-{}-{}-{}",
        prefix,
        std::process::id(),
        nanos
    ));
    // Ensure uniqueness even if nanos collide within same pid.
    let mut candidate = base.clone();
    let mut suffix = 0u64;
    loop {
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
                candidate = std::env::temp_dir().join(format!(
                    "zs-{}-{}-{}-{}",
                    prefix,
                    std::process::id(),
                    nanos,
                    suffix
                ));
            }
            Err(e) => panic!("create isolated test root {}: {e}", candidate.display()),
        }
    }
}

#[test]
fn put_reaps_stale_tmp_and_quarantine_moves_digest_mismatch() {
    // Both constants must be referenced in the test so the test fails if they are removed.
    assert_eq!(
        CAS_TEMP_REAP_AGE,
        Duration::from_secs(3600),
        "CAS_TEMP_REAP_AGE must be 3600s"
    );
    assert_eq!(
        CAS_QUARANTINE_DIR, "quarantine",
        "CAS_QUARANTINE_DIR must be quarantine"
    );

    // ---- Part 1: put reaps stale .tmp-* files older than CAS_TEMP_REAP_AGE ----
    {
        let root = unique_root("reap");
        let cas = SharedCas::open(&root);
        let payload = b"surf-0013-reap-payload";
        let hash = content_hash_hex(payload);
        let parent = cas
            .object_path(&hash)
            .parent()
            .expect("object path has parent")
            .to_path_buf();
        fs::create_dir_all(&parent).expect("create parent shard");

        let stale = parent.join(".tmp-stale-reap-test");
        fs::write(&stale, b"stale-temp-contents").expect("write stale temp");
        assert!(stale.exists(), "stale temp must exist before put");

        // Make the file older than CAS_TEMP_REAP_AGE so reap_stale_temps will remove it.
        let old_time = SystemTime::now()
            .checked_sub(CAS_TEMP_REAP_AGE + Duration::from_secs(10))
            .expect("old time");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .expect("open stale for utime");
        file.set_times(fs::FileTimes::new().set_modified(old_time))
            .expect("set old mtime");
        drop(file);

        // Put must reap the stale file in the same fan-out directory.
        let returned = cas.put(payload).expect("put should succeed");
        assert_eq!(returned, hash, "put must return content hash");

        assert!(
            !stale.exists(),
            "put must have reaped stale temp {} via CAS_TEMP_REAP_AGE={:?} in shard {}",
            stale.display(),
            CAS_TEMP_REAP_AGE,
            parent.display()
        );

        // Publish created the object.
        assert!(
            cas.contains(&hash),
            "object must exist after put at {}",
            cas.object_path(&hash).display()
        );

        let _ = fs::remove_dir_all(&root);
    }

    // ---- Part 2: quarantine moves digest-mismatched body into gc/quarantine ----
    {
        let root = unique_root("quarantine");
        let cas = SharedCas::open(&root);

        // Choose a hash derived from good bytes, then write a corrupt body at that path.
        let good_bytes = b"surf-0013-good-bytes-for-hash";
        let hash = content_hash_hex(good_bytes);
        let dest = cas.object_path(&hash);
        fs::create_dir_all(dest.parent().expect("object parent"))
            .expect("create object parent for quarantine test");

        let corrupt = b"this-body-does-not-hash-to-expected";
        // Sanity: corrupt must indeed mismatch the hash.
        assert_ne!(
            content_hash_hex(corrupt),
            hash,
            "corrupt body must mismatch expected hash"
        );
        fs::write(&dest, corrupt).expect("write corrupt object body");
        assert!(dest.exists(), "corrupt object must exist before quarantine");

        // quarantine_object requires the exclusive sweep lock.
        let guard = cas.lock_for_sweep().expect("acquire exclusive sweep lock");
        cas.quarantine_object(&hash, &guard)
            .expect("quarantine digest-mismatched object");

        assert!(
            !dest.exists(),
            "corrupt body must have been moved from object path {}",
            dest.display()
        );

        let quarantine_dir = root.join("gc").join(CAS_QUARANTINE_DIR);
        assert!(
            quarantine_dir.is_dir(),
            "quarantine dir must exist at {} after quarantine_object (CAS_QUARANTINE_DIR={})",
            quarantine_dir.display(),
            CAS_QUARANTINE_DIR
        );

        let entries: Vec<String> = fs::read_dir(&quarantine_dir)
            .expect("read quarantine dir")
            .map(|e| e.expect("dir entry").file_name().to_string_lossy().to_string())
            .collect();

        let matched = entries
            .iter()
            .find(|name| name.starts_with(&format!("{hash}.corrupt-")))
            .cloned();

        assert!(
            matched.is_some(),
            "quarantine must contain <hash>.corrupt-* for {} (CAS_QUARANTINE_DIR={}), found: {:?}",
            hash,
            CAS_QUARANTINE_DIR,
            entries
        );

        let moved_path = quarantine_dir.join(matched.unwrap());
        let moved_bytes = fs::read(&moved_path).expect("read quarantined body");
        assert_eq!(
            moved_bytes, corrupt,
            "quarantined body must be the original corrupt bytes at {}",
            moved_path.display()
        );

        let _ = fs::remove_dir_all(&root);
    }
}
