//! Canonical ZeroRef v1 content-addressed store (docs/adr/002-zeroref-v1.md §7).
//!
//! Layout, publish protocol, size policy, and layout constants are owned by
//! the canonical hub crate `zero_store` (pinned at
//! `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`); GraphZero no longer duplicates
//! that authority. This module keeps only:
//! - the GraphZero [`SharedCas`] handle: a thin wrapper over
//!   `zero_store::SharedCas` preserving the local API, the `ExternalStore`
//!   adapter, and GraphZero's `ExternalResolveError` taxonomy;
//! - the batch-durability extension `put_nosync` / `put_nosync_prehashed`
//!   (no per-object fsync; pending paths are drained by
//!   `BlobStore::sync_all`).
//!
//! Layout: `<cas_root>/blobs/sha256/<first-two-hex>/<64-lowercase-hex>`,
//! immutable complete objects only. Graph facts, indexes, provenance, and
//! mutable metadata never live in this namespace. A project-local root is the
//! default; a user/team-shared root requires explicit configuration
//! (`GRAPHZERO_SHARED_STORE`/`ZEROSTACK_SHARED_STORE` truthy plus
//! `ZEROSTACK_STORE_ROOT`), and even then only blob bytes are shared — facts
//! stay project-keyed per `store::zerostack_store`.
//!
//! Resolution precedence is deterministic (see `ExpandResolver::resolve_blob`):
//! legacy local `BlobStore` → git OID → project-local CAS → shared CAS
//! (opt-in) → other externals → ref-index. Corruption at any tier is terminal
//! and never falls through.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::expand::{BlobRequest, ExternalResolveError, ExternalStore};
use graphzero_types::ContentHash;
use zero_store::SharedCas as ZeroSharedCas;

/// Canonical layout template, layout version, size policy, and temp-reap age,
/// re-exported from the pinned `zero_store` crate so capability output and
/// runtime behavior cannot drift from the hub contract (each value is pinned
/// by `zero_store`'s own tests).
pub use zero_store::{CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES, CAS_TEMP_REAP_AGE};

const TEMP_PREFIX: &str = ".tmp-";

fn io_err(context: &str, e: impl std::fmt::Display) -> ExternalResolveError {
    ExternalResolveError::Io(format!("{context}: {e}"))
}

/// Map the canonical engine-neutral `zero_store::CasError` onto GraphZero's
/// `ExternalResolveError` taxonomy (class tokens unchanged, see
/// [`ExternalResolveError::class`]).
fn cas_err(e: zero_store::CasError) -> ExternalResolveError {
    match e {
        zero_store::CasError::NotFound => ExternalResolveError::NotFound,
        zero_store::CasError::Io(msg) => ExternalResolveError::Io(msg),
        zero_store::CasError::DigestMismatch { expected, actual } => {
            ExternalResolveError::DigestMismatch { expected, actual }
        }
        zero_store::CasError::PolicyDenied(msg) => ExternalResolveError::PolicyDenied(msg),
        zero_store::CasError::Malformed(msg) => ExternalResolveError::Malformed(msg),
    }
}

/// Handle to one CAS root. Cheap to construct; does no I/O until used.
/// Thread-safe: all state is the root path, and the canonical publish protocol
/// makes concurrent identical writers converge on one valid object.
#[derive(Debug, Clone)]
pub struct SharedCas {
    inner: ZeroSharedCas,
}

impl SharedCas {
    /// Open the CAS under `root` (objects live at `root/blobs/sha256/…`).
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: ZeroSharedCas::open(root),
        }
    }

    /// Same as [`SharedCas::open`] with a distinct trace label, so local and
    /// shared registrations stay distinguishable in resolution traces.
    pub fn open_labeled(root: impl Into<PathBuf>, label: &'static str) -> Self {
        Self {
            inner: ZeroSharedCas::open_labeled(root, label),
        }
    }

    pub fn root(&self) -> &Path {
        self.inner.root()
    }

    /// Canonical object path for a full lowercase 64-hex identity (layout
    /// owned by `zero_store`; non-panicking on short input).
    pub fn object_path(&self, sha256: &str) -> PathBuf {
        self.inner.object_path(sha256)
    }

    /// Publish complete bytes at their canonical path and return the full
    /// identity. Delegated to the canonical publish protocol: dedup,
    /// corruption-is-loud, shared store-coordination lock, fsync + directory
    /// sync.
    pub fn put(&self, bytes: &[u8]) -> Result<String, ExternalResolveError> {
        self.inner.put(bytes).map_err(cas_err)
    }

    /// Publish with an explicit size policy, for hosts enforcing a stricter
    /// cap than [`CAS_MAX_OBJECT_BYTES`]. Delegated to the canonical store.
    pub fn put_limited(&self, bytes: &[u8], limit: u64) -> Result<String, ExternalResolveError> {
        self.inner.put_limited(bytes, limit).map_err(cas_err)
    }

    /// Publish bytes whose digest the caller already computed. The canonical
    /// store re-derives and compares the digest before writing anything, so a
    /// wrong hash publishes nothing (fail closed).
    pub fn put_prehashed(
        &self,
        hash: ContentHash,
        bytes: &[u8],
    ) -> Result<String, ExternalResolveError> {
        let sha256 = hash.to_hex();
        self.inner
            .put_prehashed(&sha256, bytes)
            .map(|outcome| outcome.hash)
            .map_err(cas_err)
    }

    /// GraphZero batch-durability extension (not part of the canonical
    /// publish protocol): publish without per-object fsync, returning the
    /// path that still needs a durability barrier ([`BlobStore::sync_all`]).
    pub fn put_nosync(
        &self,
        bytes: &[u8],
    ) -> Result<(String, Option<PathBuf>), ExternalResolveError> {
        self.put_nosync_prehashed(ContentHash::of(bytes), bytes)
    }

    /// Like [`Self::put_nosync`] with a caller-supplied digest. The digest is
    /// **not** re-derived here (caller pre-verified identity, graphzero-qf9y5);
    /// full verification stays on the `get_verified` read path.
    pub fn put_nosync_prehashed(
        &self,
        hash: ContentHash,
        bytes: &[u8],
    ) -> Result<(String, Option<PathBuf>), ExternalResolveError> {
        if bytes.len() as u64 > CAS_MAX_OBJECT_BYTES {
            return Err(ExternalResolveError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({CAS_MAX_OBJECT_BYTES} bytes)",
                bytes.len()
            )));
        }
        let hash_hex = hash.to_hex();
        let dest = self.object_path(&hash_hex);

        // Same publish-side coordination as the canonical protocol, acquired
        // BEFORE the existence check and held through publish: a concurrent
        // sweeper cannot unlink between publish and reference. Fail closed —
        // a wedged holder surfaces as a typed error rather than an
        // uncoordinated write.
        let _publish_guard = self.inner.lock_for_publish().map_err(cas_err)?;

        if let Some(existing_hash) = self.try_reuse_existing(&dest, &hash_hex, bytes)? {
            return Ok((existing_hash, None));
        }

        let parent = dest.parent().expect("object path always has a parent");
        self.ensure_object_dirs(parent)?;
        reap_stale_temps(parent, CAS_TEMP_REAP_AGE);

        let tmp = unique_cas_temp(parent, &hash_hex);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err("create temp object", e))?;
        if let Err(e) = self.publish_temp_object(&mut file, bytes, &tmp, &dest, &hash_hex) {
            // The temp is ours (create_new succeeded), so removal races no one.
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        // Converged-on-existing leaves our temp behind; clean it up.
        let _ = fs::remove_file(&tmp);
        Ok((hash_hex, Some(dest)))
    }

    /// Open by full digest, enforce the regular-file/size policy, hash the
    /// complete bytes, and only then return data. Delegated to the canonical
    /// store; digest mismatch is loud and returns no bytes.
    pub fn get_verified(&self, sha256: &str) -> Result<Vec<u8>, ExternalResolveError> {
        self.inner.get_verified(sha256).map_err(cas_err)
    }

    fn try_reuse_existing(
        &self,
        dest: &Path,
        hash: &str,
        bytes: &[u8],
    ) -> Result<Option<String>, ExternalResolveError> {
        match fs::symlink_metadata(dest) {
            Ok(meta) => {
                if !meta.file_type().is_file() {
                    return Err(ExternalResolveError::Malformed(format!(
                        "object {} in '{}' is not a regular file",
                        &hash[..8],
                        self.inner.label()
                    )));
                }
                if meta.len() as u64 > CAS_MAX_OBJECT_BYTES {
                    return Err(ExternalResolveError::PolicyDenied(format!(
                        "object of {} bytes exceeds the CAS size policy ({CAS_MAX_OBJECT_BYTES} bytes)",
                        meta.len()
                    )));
                }
                // Put-path reuse: size + regular-file check only. Full
                // `read_verified_at` re-read+rehash stays on get (qf9y5).
                // CAS objects are immutable once published; length mismatch
                // is treated as corruption, not silent overwrite.
                if meta.len() != bytes.len() as u64 {
                    return Err(ExternalResolveError::DigestMismatch {
                        expected: hash.to_string(),
                        actual: format!("existing-len={} put-len={}", meta.len(), bytes.len()),
                    });
                }
                Ok(Some(hash.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("stat existing object", e)),
        }
    }

    fn ensure_object_dirs(&self, parent: &Path) -> Result<(), ExternalResolveError> {
        fs::create_dir_all(parent).map_err(|e| io_err("create object directory", e))?;
        // Refuse symlink substitutions on the directories we publish into.
        for level in [parent, parent.parent().expect("sha256 level")] {
            let meta =
                fs::symlink_metadata(level).map_err(|e| io_err("stat object directory", e))?;
            if !meta.file_type().is_dir() {
                return Err(ExternalResolveError::Malformed(
                    "object directory is not a real directory (symlink substitution refused)"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    fn publish_temp_object(
        &self,
        file: &mut fs::File,
        bytes: &[u8],
        tmp: &Path,
        dest: &Path,
        hash: &str,
    ) -> Result<(), ExternalResolveError> {
        file.write_all(bytes)
            .map_err(|e| io_err("write temp object", e))?;
        // No per-object fsync: the durability barrier is BlobStore::sync_all.
        // Concurrent identical writers may rename over each other; both
        // orders leave one valid object with these exact bytes. On Windows,
        // transient sharing violations are retried with bounded backoff inside
        // replace_file.
        if let Err(e) = super::replace_file(tmp, dest) {
            // Destination contention: if a concurrent writer already
            // published a verifying object, converge on it.
            if dest.is_file() && self.inner.get_verified(hash).is_ok() {
                return Ok(());
            }
            return Err(io_err("publish object", e));
        }
        Ok(())
    }
}

impl ExternalStore for SharedCas {
    fn name(&self) -> &'static str {
        self.inner.label()
    }

    fn get(&self, request: &BlobRequest<'_>) -> Result<Vec<u8>, ExternalResolveError> {
        self.get_verified(request.sha256())
    }
}

/// Bounded temp cleanup rule: only files named `.tmp-*` inside the given
/// fan-out directory, and only when older than `max_age`. Best-effort; never
/// errors, never touches younger files, so it cannot race active writers.
fn unique_cas_temp(parent: &Path, hash: &str) -> PathBuf {
    // Unique sibling temp file (pid + process-wide sequence, so
    // concurrent writers in one process can never collide), then atomic
    // publish. Only a temp we created ourselves is ever cleaned up.
    static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    parent.join(format!(
        "{TEMP_PREFIX}{}-{}-{}",
        &hash[..8],
        std::process::id(),
        TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

fn reap_stale_temps(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(TEMP_PREFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age >= max_age);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}
