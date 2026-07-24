//! Canonical ZeroRef v1 content-addressed store.
//!
//! Derived from the GraphZero shared_cas implementation (the strictest of
//! the three engines), generalized behind engine-neutral errors.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::fs_replace::replace_file;

/// Size policy for CAS objects: reads and writes above this are refused as
/// policy_denied so one runaway object cannot wedge shared storage.
pub const CAS_MAX_OBJECT_BYTES: u64 = 256 * 1024 * 1024;

/// Advertised layout template; [SharedCas::object_path] must match it
/// (pinned by a unit test) so capability output cannot drift from code.
pub const CAS_LAYOUT: &str = "blobs/sha256/<hh>/<hash>";
pub const CAS_LAYOUT_VERSION: u64 = 1;

/// Abandoned temp files older than this are reaped opportunistically during
/// put in the same fan-out directory. Younger temps are never touched, so
/// active writers are never raced (a legitimate write lasts milliseconds).
pub const CAS_TEMP_REAP_AGE: Duration = Duration::from_secs(3600);

const TEMP_PREFIX: &str = ".tmp-";

/// Engine-neutral CAS error aligned with the ZeroRef v1 error classes.
#[derive(Debug)]
pub enum CasError {
    /// Object not present in this store.
    NotFound,
    /// Store I/O failed.
    Io(String),
    /// Resolved bytes do not hash to the requested identity.
    DigestMismatch { expected: String, actual: String },
    /// Read or write refused by size or storage policy.
    PolicyDenied(String),
    /// Malformed identity or corrupted store shape (non-regular file,
    /// symlink substitution).
    Malformed(String),
}

impl CasError {
    /// Stable class string aligned with ZeroRef v1 error classes.
    pub fn class(&self) -> &'static str {
        match self {
            Self::NotFound => "missing",
            Self::Io(_) => "io",
            Self::DigestMismatch { .. } => "digest_mismatch",
            Self::PolicyDenied(_) => "policy_denied",
            Self::Malformed(_) => "malformed",
        }
    }
}

impl std::fmt::Display for CasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "missing: object not found"),
            Self::Io(m) => write!(f, "io: {m}"),
            Self::DigestMismatch { expected, actual } => {
                write!(f, "digest_mismatch: expected {expected}, got {actual}")
            }
            Self::PolicyDenied(m) => write!(f, "policy_denied: {m}"),
            Self::Malformed(m) => write!(f, "malformed: {m}"),
        }
    }
}

impl std::error::Error for CasError {}

fn io_err(context: &str, e: impl std::fmt::Display) -> CasError {
    CasError::Io(format!("{context}: {e}"))
}

fn content_hash_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn is_full_lower_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Handle to one CAS root. Cheap to construct; does no I/O until used.
/// Thread-safe: all state is the root path, and the publish protocol makes
/// concurrent identical writers converge on one valid object.
#[derive(Debug, Clone)]
pub struct SharedCas {
    root: PathBuf,
    label: &'static str,
}

impl SharedCas {
    /// Open the CAS under root (objects live at root/blobs/sha256/…).
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            label: "shared-cas",
        }
    }

    /// Same as [SharedCas::open] with a distinct trace label, so local and
    /// shared registrations stay distinguishable in resolution traces.
    pub fn open_labeled(root: impl Into<PathBuf>, label: &'static str) -> Self {
        Self {
            root: root.into(),
            label,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    /// Canonical object path for a full lowercase 64-hex identity.
    pub fn object_path(&self, sha256: &str) -> PathBuf {
        self.root
            .join("blobs")
            .join("sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    /// True when the object exists as a regular file at its canonical path.
    pub fn contains(&self, sha256: &str) -> bool {
        is_full_lower_hex(sha256) && self.object_path(sha256).is_file()
    }

    /// Publish complete bytes at their canonical path and return the full
    /// identity. Identical preexisting content is success (dedup); a
    /// preexisting object with different bytes is a loud corruption error and
    /// is never overwritten.
    pub fn put(&self, bytes: &[u8]) -> Result<String, CasError> {
        self.put_with_limit(bytes, CAS_MAX_OBJECT_BYTES)
    }

    /// Publish with an explicit size policy, for hosts enforcing a stricter
    /// cap than [CAS_MAX_OBJECT_BYTES].
    pub fn put_limited(&self, bytes: &[u8], limit: u64) -> Result<String, CasError> {
        self.put_with_limit(bytes, limit.min(CAS_MAX_OBJECT_BYTES))
    }

    fn put_with_limit(&self, bytes: &[u8], limit: u64) -> Result<String, CasError> {
        if bytes.len() as u64 > limit {
            return Err(CasError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({limit} bytes)",
                bytes.len()
            )));
        }
        // Hash the complete bytes before deriving any destination.
        let hash = content_hash_hex(bytes);
        let dest = self.object_path(&hash);

        // Preexisting destination: verify, never overwrite.
        match fs::symlink_metadata(&dest) {
            Ok(meta) => {
                self.check_regular(&meta, &hash)?;
                let existing = self.read_verified_at(&dest, &hash, limit)?;
                debug_assert_eq!(existing.len(), bytes.len());
                return Ok(hash);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("stat existing object", e)),
        }

        let parent = dest.parent().expect("object path always has a parent");
        fs::create_dir_all(parent).map_err(|e| io_err("create object directory", e))?;
        // Refuse symlink substitutions on the directories we publish into.
        for level in [parent, parent.parent().expect("sha256 level")] {
            let meta =
                fs::symlink_metadata(level).map_err(|e| io_err("stat object directory", e))?;
            if !meta.file_type().is_dir() {
                return Err(CasError::Malformed(
                    "object directory is not a real directory (symlink substitution refused)"
                        .to_string(),
                ));
            }
        }
        reap_stale_temps(parent, CAS_TEMP_REAP_AGE);

        // Unique sibling temp file (pid + process-wide sequence, so
        // concurrent writers in one process can never collide), then atomic
        // publish. Only a temp we created ourselves is ever cleaned up.
        static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = parent.join(format!(
            "{TEMP_PREFIX}{}-{}-{}",
            &hash[..8],
            std::process::id(),
            TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err("create temp object", e))?;
        let publish = (|| -> Result<(), CasError> {
            file.write_all(bytes)
                .map_err(|e| io_err("write temp object", e))?;
            file.sync_all().map_err(|e| io_err("sync temp object", e))?;
            // Concurrent identical writers may rename over each other; both
            // orders leave one valid object with these exact bytes.
            if let Err(e) = replace_file(&tmp, &dest) {
                // Destination contention: if a concurrent writer already
                // published a verifying object, converge on it.
                if dest.is_file() && self.read_verified_at(&dest, &hash, limit).is_ok() {
                    return Ok(());
                }
                return Err(io_err("publish object", e));
            }
            sync_dir(parent);
            Ok(())
        })();
        if let Err(e) = publish {
            // The temp is ours (create_new succeeded), so removal races no one.
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        // Converged-on-existing leaves our temp behind; clean it up.
        let _ = fs::remove_file(&tmp);
        Ok(hash)
    }

    /// Open by full digest, enforce the regular-file/size policy, hash the
    /// complete bytes, and only then return data. Digest mismatch is loud and
    /// returns no bytes.
    pub fn get_verified(&self, sha256: &str) -> Result<Vec<u8>, CasError> {
        if !is_full_lower_hex(sha256) {
            return Err(CasError::Malformed(format!(
                "identity must be full lowercase 64-hex SHA-256, got '{sha256}'"
            )));
        }
        let path = self.object_path(sha256);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CasError::NotFound);
            }
            Err(e) => return Err(io_err("stat object", e)),
        };
        self.check_regular(&meta, sha256)?;
        if meta.len() > CAS_MAX_OBJECT_BYTES {
            return Err(CasError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({CAS_MAX_OBJECT_BYTES} bytes)",
                meta.len()
            )));
        }
        self.read_verified_at(&path, sha256, CAS_MAX_OBJECT_BYTES)
    }

    fn check_regular(&self, meta: &fs::Metadata, sha256: &str) -> Result<(), CasError> {
        if !meta.file_type().is_file() {
            return Err(CasError::Malformed(format!(
                "object {} in '{}' is not a regular file",
                &sha256[..8],
                self.label
            )));
        }
        Ok(())
    }

    fn read_verified_at(
        &self,
        path: &Path,
        expected: &str,
        limit: u64,
    ) -> Result<Vec<u8>, CasError> {
        let bytes = fs::read(path).map_err(|e| io_err("read object", e))?;
        if bytes.len() as u64 > limit {
            return Err(CasError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({limit} bytes)",
                bytes.len()
            )));
        }
        let actual = content_hash_hex(&bytes);
        if actual != expected {
            return Err(CasError::DigestMismatch {
                expected: expected.to_string(),
                actual,
            });
        }
        Ok(bytes)
    }
}

/// Bounded temp cleanup rule: only files named .tmp-* inside the given
/// fan-out directory, and only when older than max_age. Best-effort; never
/// errors, never touches younger files, so it cannot race active writers.
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

/// Durability for the published rename where the platform supports it.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_get_roundtrip_and_dedup() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let h1 = cas.put(b"hello zerostack").unwrap();
        let h2 = cas.put(b"hello zerostack").unwrap();
        assert_eq!(h1, h2);
        assert!(cas.contains(&h1));
        assert_eq!(cas.get_verified(&h1).unwrap(), b"hello zerostack");
    }

    #[test]
    fn layout_constant_matches_object_path() {
        let cas = SharedCas::open("/store");
        let h = "4fdbc441ea7b546100e086ac1e4fc5ae6749b7314311c99db05be450eca12996";
        let p = cas.object_path(h);
        assert!(p.ends_with(format!("blobs/sha256/4f/{h}")), "{p:?} vs {CAS_LAYOUT}");
    }

    #[test]
    fn size_policy_is_enforced_on_put() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let err = cas
            .put_with_limit(b"tiny object over a tiny limit", 4)
            .expect_err("over-limit put");
        assert_eq!(err.class(), "policy_denied");
    }

    #[test]
    fn corrupted_object_is_loud_and_returns_no_bytes() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let h = cas.put(b"original bytes").unwrap();
        std::fs::write(cas.object_path(&h), b"tampered").unwrap();
        let err = cas.get_verified(&h).expect_err("tampered object");
        assert_eq!(err.class(), "digest_mismatch");
    }

    #[test]
    fn missing_and_malformed_identities() {
        let dir = tempdir().unwrap();
        let cas = SharedCas::open(dir.path());
        let absent = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(cas.get_verified(absent).unwrap_err().class(), "missing");
        assert_eq!(cas.get_verified("SHORT").unwrap_err().class(), "malformed");
    }

    #[test]
    fn stale_temps_are_reaped_but_fresh_ones_survive() {
        let dir = tempdir().unwrap();
        let fresh = dir.path().join(".tmp-fresh");
        let plain = dir.path().join("not-a-temp");
        std::fs::write(&fresh, b"active writer").unwrap();
        std::fs::write(&plain, b"object-like").unwrap();

        reap_stale_temps(dir.path(), Duration::ZERO);
        assert!(!fresh.exists(), "stale temp must be reaped");
        assert!(plain.exists(), "non-temp names are never touched");

        let active = dir.path().join(".tmp-active");
        std::fs::write(&active, b"active").unwrap();
        reap_stale_temps(dir.path(), CAS_TEMP_REAP_AGE);
        assert!(active.exists(), "young temps are never raced");
    }
}
