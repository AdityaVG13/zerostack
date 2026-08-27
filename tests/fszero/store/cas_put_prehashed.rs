//! F-STORE-403 / F-STORE-045: `put_prehashed` never trusts a caller digest.

use fszero_store::access_log::content_hash_bytes;
use fszero_store::{CasError, CasStore};

fn open_cas() -> (tempfile::TempDir, CasStore) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("blobs")).unwrap();
    let cas = CasStore::for_store_root(dir.path());
    (dir, cas)
}

fn lie_about(hash: &str) -> String {
    let mut bytes = hash.as_bytes().to_vec();
    bytes[0] = if bytes[0] == b'0' { b'1' } else { b'0' };
    String::from_utf8(bytes).unwrap()
}

#[test]
fn matching_digest_puts_and_get_round_trips() {
    let (_dir, cas) = open_cas();
    let bytes = b"cas-prehashed-contract";
    let hash = content_hash_bytes(bytes);

    let outcome = cas.put_prehashed(&hash, bytes).expect("matching digest");
    assert_eq!(outcome.hash, hash);
    assert!(outcome.created);

    let got = cas.get(&hash).expect("get after put");
    assert_eq!(got.as_slice(), bytes);
    assert_eq!(content_hash_bytes(&got), hash);
}

#[test]
fn mismatched_digest_fails_closed_and_stores_nothing() {
    let (_dir, cas) = open_cas();
    let bytes = b"payload-bytes";
    let actual = content_hash_bytes(bytes);
    let lied = lie_about(&actual);
    assert_ne!(lied, actual);

    let err = cas
        .put_prehashed(&lied, bytes)
        .expect_err("mismatched label must fail closed");
    match &err {
        CasError::Malformed(msg) => {
            assert!(msg.contains("put_prehashed") && msg.contains("!="), "{msg}");
        }
        other => panic!("expected Malformed, got {other}"),
    }

    assert!(!cas.contains(&lied), "wrong digest must not be stored");
    assert!(
        !cas.contains(&actual),
        "lied label must not publish under the true digest either"
    );
    assert!(cas.validity_record(&lied).unwrap().is_none());
    assert!(cas.validity_record(&actual).unwrap().is_none());
    match cas.get(&lied) {
        Err(CasError::Missing(_) | CasError::Malformed(_)) => {}
        other => panic!("lied digest must not serve bytes: {other:?}"),
    }
}

#[test]
fn stolen_label_does_not_replace_or_publish_attacker_bytes() {
    let (_dir, cas) = open_cas();
    let original = b"original-blob";
    let attacker = b"attacker-blob";
    let hash = content_hash_bytes(original);
    cas.put_prehashed(&hash, original).unwrap();

    cas.put_prehashed(&hash, attacker)
        .expect_err("stolen label must not replace stored bytes");

    assert_eq!(cas.get(&hash).unwrap(), original);
    assert!(!cas.contains(&content_hash_bytes(attacker)));
}

#[test]
fn empty_blob_round_trips() {
    let (_dir, cas) = open_cas();
    let hash = content_hash_bytes(b"");
    let outcome = cas.put_prehashed(&hash, b"").unwrap();
    assert_eq!(outcome.hash, hash);
    assert_eq!(cas.get(&hash).unwrap(), b"");
}
