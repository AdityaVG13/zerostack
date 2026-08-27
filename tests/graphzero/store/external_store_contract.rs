//! ExternalStore adapter-boundary contract (ZeroRef v1 §5/§6,
//! docs/adr/002-zeroref-v1.md): exact-hash requests, typed fallible results,
//! digest verification before fragment/presentation code, and terminal
//! corruption that no lower tier can mask.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::refs::GzRef;
use graphzero_store::{
    BlobRequest, ContentHash, ExpandResolver, ExternalResolveError, ExternalStore,
    LocalBlobStoreAdapter,
};
use tempfile::tempdir;

const ALPHA: &[u8] = b"alpha\nbeta\ngamma\n";

struct FixedResponse {
    label: &'static str,
    response: fn() -> Result<Vec<u8>, ExternalResolveError>,
}

impl ExternalStore for FixedResponse {
    fn name(&self) -> &'static str {
        self.label
    }

    fn get(&self, _request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError> {
        (self.response)()
    }
}

struct PanicIfCalled {
    called: &'static AtomicBool,
}

impl ExternalStore for PanicIfCalled {
    fn name(&self) -> &'static str {
        "panic-if-called"
    }

    fn get(&self, _request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError> {
        self.called.store(true, Ordering::SeqCst);
        Err(ExternalResolveError::NotFound)
    }
}

fn resolver_with(store_root: &std::path::Path, ext: Box<dyn ExternalStore>) -> ExpandResolver {
    let mut resolver = ExpandResolver::new(store_root, None).expect("open resolver");
    resolver.register_external(ext);
    resolver
}

fn unknown_hash() -> String {
    ContentHash::of(b"external-store-contract-absent-sentinel").to_hex()
}

#[test]
fn blob_request_requires_full_lowercase_hex() {
    let full = ContentHash::of(ALPHA).to_hex();
    assert!(BlobRequest::exact(&full).is_ok());
    for bad in [
        &full[..12],
        &full[..63],
        &full.to_uppercase()[..],
        "zz00000000000000000000000000000000000000000000000000000000000000",
        "",
    ] {
        let err = BlobRequest::exact(bad).expect_err("must reject non-exact identity");
        assert_eq!(err.class(), "malformed", "input {bad:?}");
    }
}

#[test]
fn miss_falls_through_documented_tier_order() {
    let dir = tempdir().unwrap();
    let resolver = resolver_with(
        dir.path(),
        Box::new(FixedResponse {
            label: "ext-miss",
            response: || Err(ExternalResolveError::NotFound),
        }),
    );
    let hash = unknown_hash();
    let err = resolver
        .resolve_blob(&hash, &format!("gz://blob/{hash}"))
        .expect_err("nothing holds this blob");
    assert!(
        err.reason.starts_with("not_found"),
        "reason: {}",
        err.reason
    );
    let order: Vec<&str> = err.trace.iter().map(|s| s.store).collect();
    assert_eq!(
        order,
        vec!["graphzero", "git", "cas-local", "ext-miss", "ref-index"],
        "chain must consult tiers in the documented order"
    );
    assert!(err.trace.iter().all(|s| s.result == "miss"));
}

#[test]
fn malicious_adapter_wrong_bytes_is_terminal_digest_mismatch() {
    let dir = tempdir().unwrap();
    let resolver = resolver_with(
        dir.path(),
        Box::new(FixedResponse {
            label: "ext-evil",
            response: || Ok(b"substituted contents".to_vec()),
        }),
    );
    let hash = ContentHash::of(ALPHA).to_hex();

    // Whole-blob resolution is rejected.
    let err = resolver
        .resolve_blob(&hash, &format!("gz://blob/{hash}"))
        .expect_err("wrong bytes must never resolve");
    assert!(
        err.reason.starts_with("digest_mismatch"),
        "reason: {}",
        err.reason
    );
    assert!(
        !err.reason.contains("substituted"),
        "error must not leak blob contents"
    );
    assert_eq!(err.trace.last().unwrap().result, "digest_mismatch");
    assert!(
        !err.trace.iter().any(|s| s.store == "ref-index"),
        "corruption is terminal; later tiers must not mask it"
    );

    // Fragment resolution never sees the bytes either.
    let reference = format!("gz://blob/{hash}#B0-5");
    let gz = GzRef::parse(&reference).unwrap();
    let err = resolver
        .resolve(&gz, &reference)
        .expect_err("no fragment output from unverified bytes");
    assert!(
        err.reason.starts_with("digest_mismatch"),
        "reason: {}",
        err.reason
    );
}

#[test]
fn io_failure_is_terminal_and_distinct_from_miss() {
    let dir = tempdir().unwrap();
    let resolver = resolver_with(
        dir.path(),
        Box::new(FixedResponse {
            label: "ext-io",
            response: || Err(ExternalResolveError::Io("disk on fire".to_string())),
        }),
    );
    let hash = unknown_hash();
    let err = resolver
        .resolve_blob(&hash, &format!("gz://blob/{hash}"))
        .expect_err("io failure must surface");
    assert!(
        err.reason.starts_with("io:") && err.reason.contains("disk on fire"),
        "reason: {}",
        err.reason
    );
    assert_eq!(err.trace.last().unwrap().result, "io");
    assert!(
        !err.trace.iter().any(|s| s.store == "ref-index"),
        "fallback happens only on an explicit not-found"
    );
}

#[test]
fn policy_denied_and_unsupported_propagate_their_classes() {
    for (label, response, class) in [
        (
            "ext-policy",
            (|| {
                Err(ExternalResolveError::PolicyDenied(
                    "shared root not opted in".to_string(),
                ))
            }) as fn() -> Result<Vec<u8>, ExternalResolveError>,
            "policy_denied",
        ),
        (
            "ext-unsupported",
            || {
                Err(ExternalResolveError::Unsupported(
                    "scheme not served".to_string(),
                ))
            },
            "unsupported",
        ),
    ] {
        let dir = tempdir().unwrap();
        let resolver = resolver_with(dir.path(), Box::new(FixedResponse { label, response }));
        let hash = unknown_hash();
        let err = resolver
            .resolve_blob(&hash, &format!("gz://blob/{hash}"))
            .expect_err("terminal class must surface");
        assert!(err.reason.contains(class), "reason: {}", err.reason);
        assert_eq!(err.trace.last().unwrap().result, class);
    }
}

#[test]
fn prefix_requests_never_reach_exact_hash_adapters() {
    static CALLED: AtomicBool = AtomicBool::new(false);
    let dir = tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    let hash = store.put(ALPHA).unwrap().to_hex();
    let resolver = resolver_with(dir.path(), Box::new(PanicIfCalled { called: &CALLED }));

    // Local prefix hit resolves without consulting externals.
    let prefix = &hash[..16];
    let hit = resolver
        .resolve_blob(prefix, &format!("gz://blob/{prefix}"))
        .expect("local prefix hit");
    assert_eq!(hit.bytes, ALPHA);
    assert_eq!(hit.source, "graphzero");
    assert!(!CALLED.load(Ordering::SeqCst));

    // A missing prefix records that exact-hash adapters were skipped.
    let absent = &unknown_hash()[..16];
    let err = resolver
        .resolve_blob(absent, &format!("gz://blob/{absent}"))
        .expect_err("absent prefix");
    assert!(
        !CALLED.load(Ordering::SeqCst),
        "adapter saw a prefix request"
    );
    assert!(
        err.trace
            .iter()
            .any(|s| s.store == "external" && s.result == "skipped_prefix"),
        "trace: {:?}",
        err.trace
    );
}

#[test]
fn local_blob_store_adapter_resolves_foreign_root_with_verification() {
    let home = tempdir().unwrap();
    let foreign = tempdir().unwrap();
    let foreign_store = BlobStore::open(foreign.path()).unwrap();
    let hash = foreign_store.put(ALPHA).unwrap().to_hex();

    let resolver = resolver_with(
        home.path(),
        Box::new(LocalBlobStoreAdapter {
            root: PathBuf::from(foreign.path()),
            label: "legacy-root",
        }),
    );

    let hit = resolver
        .resolve_blob(&hash, &format!("gz://blob/{hash}"))
        .expect("adapter resolves foreign root");
    assert_eq!(hit.bytes, ALPHA);
    assert_eq!(hit.source, "legacy-root");

    // Verified bytes flow into fragment selection.
    let reference = format!("gz://blob/{hash}#B0-5");
    let gz = GzRef::parse(&reference).unwrap();
    let fragment = resolver.resolve(&gz, &reference).expect("fragment");
    assert_eq!(fragment.bytes, b"alpha");
}

#[test]
fn corrupt_local_blob_is_terminal_digest_mismatch() {
    let dir = tempdir().unwrap();
    let store = BlobStore::open(dir.path()).unwrap();
    let hash = store.put(ALPHA).unwrap().to_hex();
    std::fs::write(dir.path().join("blobs").join(&hash), b"rotten bytes").unwrap();

    let resolver = ExpandResolver::new(dir.path(), None).expect("open resolver");
    let err = resolver
        .resolve_blob(&hash, &format!("gz://blob/{hash}"))
        .expect_err("corrupt local blob must not resolve");
    assert!(
        err.reason.starts_with("digest_mismatch"),
        "reason: {}",
        err.reason
    );
    assert_eq!(err.trace.last().unwrap().store, "graphzero");
    assert_eq!(err.trace.last().unwrap().result, "digest_mismatch");
}
