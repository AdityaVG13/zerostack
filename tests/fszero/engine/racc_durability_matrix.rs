//! RACC durability matrix for fz:// refs (fszero-g0i8 / 1tp).
//!
//! Covers transactional mint (persist-then-ack): same-call expand after mint,
//! reopen after ack, packed and inline artifact classes, named keys, memory.
//!
//! Absolute-durable barrier class is documented in `docs/durability.md`
//! (WAL + synchronous=FULL; pack sync_all before locator). This suite is the
//! fixture matrix for mint classes. Stack-wide MMR inclusion proofs remain a
//! cross-engine follow-up (not required for per-store transactional mint).

#[path = "../common/mod.rs"]
mod common;

use fs_zero::RecoveryStore;
use std::fs;

fn big(seed: u8, n: usize) -> Vec<u8> {
    (0..n).map(|i| seed.wrapping_add((i % 251) as u8)).collect()
}

#[test]
fn same_call_expand_after_inline_mint() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let mut store = RecoveryStore::with_durable(&db);
    let data = b"inline-same-call-expand";
    let r = store.try_put_content_ref(data).expect("mint");
    assert!(r.starts_with("fz://blob/"));
    // Same call / same handle: expand must see bytes immediately (persist-then-ack).
    assert_eq!(store.expand(&r).as_deref(), Some(data.as_slice()));
}

#[test]
fn same_call_expand_after_packed_mint() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let data = big(0x5A, 8192);
    let mut store = RecoveryStore::with_durable(&db);
    let r = store.try_put_content_ref(&data).expect("packed mint");
    assert_eq!(
        store.expand(&r).as_deref(),
        Some(data.as_slice()),
        "same-call expand after packed mint"
    );
    // Pack sidecar: store.sqlite3.pack
    let pack = dir.path().join("store.sqlite3.pack");
    assert!(pack.exists(), "pack sidecar must exist after packed mint");
}

#[test]
fn acked_mint_survives_reopen_inline_and_packed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let inline = b"tiny-acked".to_vec();
    let packed = big(0xB1, 8192);
    let (r_inline, r_packed) = {
        let mut store = RecoveryStore::with_durable(&db);
        let a = store.try_put_content_ref(&inline).unwrap();
        let b = store.try_put_content_ref(&packed).unwrap();
        (a, b)
    };
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(store.expand(&r_inline).as_deref(), Some(inline.as_slice()));
    assert_eq!(store.expand(&r_packed).as_deref(), Some(packed.as_slice()));
}

#[test]
fn named_key_put_same_call_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let body = b"named-key-payload-v1";
    {
        let mut store = RecoveryStore::with_durable(&db);
        store.try_put_key("matrix/named", body).expect("put_key");
        assert_eq!(
            store.expand("matrix/named").as_deref(),
            Some(body.as_slice()),
            "same-call expand of named key"
        );
    }
    let store = RecoveryStore::with_durable(&db);
    assert_eq!(
        store.expand("matrix/named").as_deref(),
        Some(body.as_slice())
    );
}

#[test]
fn session_memory_put_mint_expand_roundtrip() {
    use fs_zero::FSZeroSession;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("README"), b"ws").unwrap();
    let mut s = FSZeroSession::with_repo_store(dir.path());
    let (tok, ok, _) = s.execute('M', Some("put:matrix/m1|hello-racc"));
    assert!(ok, "memory put: {tok}");
    let (tok, ok, detail) = s.execute('M', Some("get:matrix/m1"));
    assert!(ok, "memory get: {tok}");
    let detail = detail.unwrap_or_default();
    let blob_ref = detail
        .split_whitespace()
        .find(|t| t.starts_with("ref=fz://blob/"))
        .map(|t| t.trim_start_matches("ref=").to_string())
        .expect("get must mint fz://blob ref");
    let get_bytes = s.expand("memory").expect("get body under memory");
    assert_eq!(String::from_utf8_lossy(&get_bytes), "hello-racc");
    assert_eq!(s.expand(&blob_ref).expect("blob expand"), get_bytes);
}

#[test]
fn digest_identity_matches_content_address() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let data = b"address-binding";
    let mut store = RecoveryStore::with_durable(&db);
    let r = store.try_put_content_ref(data).unwrap();
    let expect = format!("fz://blob/{}", common::sha256_hex(data));
    assert_eq!(r, expect);
    assert_eq!(store.expand(&r).as_deref(), Some(data.as_slice()));
}

#[test]
fn dual_mint_idempotent_same_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.sqlite3");
    let data = b"idempotent-mint";
    let mut store = RecoveryStore::with_durable(&db);
    let r1 = store.try_put_content_ref(data).unwrap();
    let r2 = store.try_put_content_ref(data).unwrap();
    assert_eq!(r1, r2);
    assert_eq!(store.expand(&r1).as_deref(), Some(data.as_slice()));
}
