//! SURF-0013: put reaps stale tmp files and quarantine preserves digest-mismatched bodies.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zero_store::SharedCas;
use zerostack_test_support::sha256_hex;

fn unique_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("zs-{}-{}-{}", prefix, std::process::id(), nanos));
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
fn put_reaps_stale_tmp() {
    let root = unique_root("reap");
    let cas = SharedCas::open(&root);
    let payload = b"surf-0013-reap-payload";
    // Independently computed digest, not via production helper.
    let hash = sha256_hex(payload);
    let parent = cas
        .object_path(&hash)
        .parent()
        .expect("object path has parent")
        .to_path_buf();
    fs::create_dir_all(&parent).expect("create parent shard");

    let stale = parent.join(".tmp-stale-reap-test");
    fs::write(&stale, b"stale-temp-contents").expect("write stale temp");
    assert!(stale.exists(), "stale temp must exist before put");

    // Make file clearly expired (2 hours old) beyond documented reap boundary.
    let old_time = SystemTime::now()
        .checked_sub(Duration::from_secs(7200))
        .expect("old time");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .expect("open stale for utime");
    file.set_times(fs::FileTimes::new().set_modified(old_time))
        .expect("set old mtime");
    drop(file);

    let returned = cas.put(payload).expect("put should succeed");
    assert_eq!(
        returned, hash,
        "put must return independently verified content hash"
    );

    assert!(
        !stale.exists(),
        "put must have reaped stale temp {} in shard {}",
        stale.display(),
        parent.display()
    );

    assert!(
        cas.contains(&hash),
        "object must exist after put at {}",
        cas.object_path(&hash).display()
    );
    // Verify byte-for-byte via verified read.
    assert_eq!(cas.get_verified(&hash).unwrap(), payload);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn quarantine_moves_digest_mismatch() {
    let root = unique_root("quarantine");
    let cas = SharedCas::open(&root);

    let good_bytes = b"surf-0013-good-bytes-for-hash";
    let hash = sha256_hex(good_bytes);
    let dest = cas.object_path(&hash);
    fs::create_dir_all(dest.parent().expect("object parent"))
        .expect("create object parent for quarantine test");

    let corrupt = b"this-body-does-not-hash-to-expected";
    assert_ne!(
        sha256_hex(corrupt),
        hash,
        "corrupt body must mismatch expected hash"
    );
    fs::write(&dest, corrupt).expect("write corrupt object body");
    assert!(dest.exists(), "corrupt object must exist before quarantine");

    // Through the public quarantine path: a digest-mismatched object is removed
    // from the live CAS, retained in gc/quarantine with bytes intact, and not
    // reported as a valid object.
    assert!(
        cas.get_verified(&hash).is_err(),
        "mismatched bytes must fail verification"
    );

    let guard = cas.lock_for_sweep().expect("acquire exclusive sweep lock");
    cas.quarantine_object(&hash, &guard)
        .expect("quarantine digest-mismatched object");

    assert!(
        !dest.exists(),
        "corrupt body must have been moved from object path {}",
        dest.display()
    );
    assert!(
        !cas.contains(&hash),
        "quarantined hash must not be reported as present"
    );

    let quarantine_dir = root.join("gc").join("quarantine");
    assert!(
        quarantine_dir.is_dir(),
        "quarantine dir must exist at {} after quarantine",
        quarantine_dir.display()
    );

    let entries: Vec<String> = fs::read_dir(&quarantine_dir)
        .expect("read quarantine dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let matched = entries
        .iter()
        .find(|name| name.starts_with(&format!("{hash}")))
        .cloned();

    assert!(
        matched.is_some(),
        "quarantine must contain entry for {} found: {:?}",
        hash,
        entries
    );

    let moved_path = quarantine_dir.join(matched.unwrap());
    let moved_bytes = fs::read(&moved_path).expect("read quarantined body");
    assert_eq!(
        moved_bytes,
        corrupt,
        "quarantined body must be the original corrupt bytes at {}",
        moved_path.display()
    );

    // Verified read must still fail after quarantine.
    assert!(cas.get_verified(&hash).is_err());

    let _ = fs::remove_dir_all(&root);
}
