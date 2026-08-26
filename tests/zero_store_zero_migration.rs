use std::fs;

use sha2::{Digest, Sha256};
use tempfile::tempdir;
use zero_store::{ZeroCas, import_legacy_store, read_and_verify_manifest};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn importer_verifies_legacy_and_new_bytes_and_signs_manifest() {
    let old = tempdir().unwrap();
    let new = tempdir().unwrap();
    let bytes = b"legacy exact bytes";
    let sha = sha256_hex(bytes);
    let old_path = old.path().join("blobs/sha256").join(&sha[..2]).join(&sha);
    fs::create_dir_all(old_path.parent().unwrap()).unwrap();
    fs::write(&old_path, bytes).unwrap();
    let manifest_path = new.path().join("migration.json");
    let key = [7_u8; 32];

    let manifest = import_legacy_store(old.path(), new.path(), &manifest_path, &key).unwrap();
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].legacy_sha256, sha);
    assert_eq!(manifest.total_bytes, bytes.len() as u64);
    let verified = read_and_verify_manifest(&manifest_path, &key).unwrap();
    assert_eq!(verified, manifest);
    let cas = ZeroCas::open(new.path());
    assert_eq!(cas.get(&manifest.entries[0].zero_handle).unwrap(), bytes);
    assert!(read_and_verify_manifest(&manifest_path, &[8_u8; 32]).is_err());
}

#[test]
fn corrupt_legacy_object_fails_without_manifest() {
    let old = tempdir().unwrap();
    let new = tempdir().unwrap();
    let claimed = "a".repeat(64);
    let path = old.path().join("blobs/sha256").join("aa").join(claimed);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"not a hash match").unwrap();
    let manifest = new.path().join("migration.json");
    assert!(import_legacy_store(old.path(), new.path(), &manifest, &[1; 32]).is_err());
    assert!(!manifest.exists());
}
