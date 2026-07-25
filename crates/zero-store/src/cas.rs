//! Canonical ZeroRef v1 content-addressed store.
//!
//! Derived from the GraphZero shared_cas implementation (the strictest of
//! the three engines), generalized behind engine-neutral errors.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::fs_replace::replace_file;
use crate::gc_lock::{StoreLock, LOCK_DEADLINE};

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

/// Directory holding objects swept out of the CAS, relative to the store root.
/// Bodies are moved here rather than unlinked so a wrong collection verdict
/// stays recoverable.
pub const CAS_QUARANTINE_DIR: &str = "quarantine";

/// Historical temp-file shapes from the three engines, all reaped by this
/// crate's reaper.
///
/// Each engine used to reap only its own shape: the prefix form was cleaned by
/// the hub and GraphZero, the suffix form by FSZero, and TokenZero had no
/// reaper at all. On a shared store root that meant abandoned temps from a
/// crashed publisher of one engine were never cleaned by another, so the leak
/// was permanent.
fn is_temp_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX) || name.ends_with(".tmp")
}

/// Outcome of a publish. `created` distinguishes a fresh object from dedup, so
/// callers can report writes without a second existence check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    pub hash: String,
    pub created: bool,
}

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

    /// The store root: the directory holding both `blobs/` and `gc/`. This is
    /// what the coordination lock is scoped to.
    pub fn store_root(&self) -> &Path {
        &self.root
    }

    /// Canonical object path for a full lowercase 64-hex identity.
    ///
    /// Deliberately non-panicking on short input, so a malformed identity
    /// produces a path that simply does not exist and callers report
    /// `missing` or `malformed` rather than aborting the process.
    pub fn object_path(&self, sha256: &str) -> PathBuf {
        self.root
            .join("blobs")
            .join("sha256")
            .join(sha256.get(..2).unwrap_or(sha256))
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
        self.put_outcome(bytes, CAS_MAX_OBJECT_BYTES)
            .map(|o| o.hash)
    }

    /// Publish with an explicit size policy, for hosts enforcing a stricter
    /// cap than [CAS_MAX_OBJECT_BYTES].
    pub fn put_limited(&self, bytes: &[u8], limit: u64) -> Result<String, CasError> {
        self.put_outcome(bytes, limit).map(|o| o.hash)
    }

    /// Publish and report whether the object was newly created.
    ///
    /// Runs under the shared store coordination lock, which is what makes a
    /// publish safe against a concurrent sweep: the sweeper cannot be between
    /// its liveness recheck and its unlink while this call is in flight.
    pub fn put_outcome(&self, bytes: &[u8], limit: u64) -> Result<PutOutcome, CasError> {
        let guard = self.lock_for_publish()?;
        self.put_in_lock(bytes, limit, &guard)
    }

    /// Publish bytes whose digest the caller already computed. The digest is
    /// always re-derived and compared, so a wrong hash writes nothing.
    pub fn put_prehashed(&self, sha256: &str, bytes: &[u8]) -> Result<PutOutcome, CasError> {
        if !is_full_lower_hex(sha256) {
            return Err(CasError::Malformed(format!(
                "identity must be full lowercase 64-hex SHA-256, got '{sha256}'"
            )));
        }
        let actual = content_hash_hex(bytes);
        if actual != sha256 {
            return Err(CasError::DigestMismatch {
                expected: sha256.to_string(),
                actual,
            });
        }
        self.put_outcome(bytes, CAS_MAX_OBJECT_BYTES)
    }

    /// Publish while already holding a publish guard, for callers batching
    /// several objects under one acquisition.
    ///
    /// Panics in debug builds if handed a sweep guard, which would mean the
    /// caller is publishing from inside a collection.
    pub fn put_in_lock(
        &self,
        bytes: &[u8],
        limit: u64,
        guard: &StoreLock,
    ) -> Result<PutOutcome, CasError> {
        debug_assert!(
            !guard.is_exclusive(),
            "publishing under a sweep guard inverts the protocol"
        );
        self.put_with_limit(bytes, limit.min(CAS_MAX_OBJECT_BYTES))
    }

    /// Acquire the shared publish guard for this store.
    pub fn lock_for_publish(&self) -> Result<StoreLock, CasError> {
        StoreLock::publish(&self.root, LOCK_DEADLINE)
            .map_err(|e| io_err("acquire store publish lock", e))
    }

    /// Acquire the exclusive sweep guard for this store. Required by
    /// [SharedCas::remove_object] and [SharedCas::quarantine_object].
    pub fn lock_for_sweep(&self) -> Result<StoreLock, CasError> {
        StoreLock::sweep(&self.root, LOCK_DEADLINE)
            .map_err(|e| io_err("acquire store sweep lock", e))
    }

    /// Non-blocking variant of [SharedCas::lock_for_sweep]; `None` means
    /// another holder is active.
    pub fn try_lock_for_sweep(&self) -> Result<Option<StoreLock>, CasError> {
        StoreLock::try_sweep(&self.root).map_err(|e| io_err("acquire store sweep lock", e))
    }

    fn put_with_limit(&self, bytes: &[u8], limit: u64) -> Result<PutOutcome, CasError> {
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
                // Refresh the mtime: a dedup is a fresh reference, and an
                // age-based retention policy that never sees it will collect a
                // still-referenced object.
                let _ = touch_path(&dest);
                return Ok(PutOutcome {
                    hash,
                    created: false,
                });
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
        Ok(PutOutcome {
            hash,
            created: true,
        })
    }

    /// Refresh an object's modification time without reading it, so an
    /// age-based retention policy observes a reference. Absent objects are
    /// [CasError::NotFound].
    pub fn touch(&self, sha256: &str) -> Result<(), CasError> {
        if !is_full_lower_hex(sha256) {
            return Err(CasError::Malformed(format!(
                "identity must be full lowercase 64-hex SHA-256, got '{sha256}'"
            )));
        }
        let path = self.object_path(sha256);
        if !path.is_file() {
            return Err(CasError::NotFound);
        }
        touch_path(&path).map_err(|e| io_err("touch object", e))
    }

    /// Every published object: exact 64-lowercase-hex regular files under the
    /// fan-out, skipping symlinks, temps, and dotfiles.
    pub fn list_objects(&self) -> Result<Vec<String>, CasError> {
        let sha_root = self.root.join("blobs").join("sha256");
        let mut out = Vec::new();
        let shards = match fs::read_dir(&sha_root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(io_err("read CAS root", e)),
        };
        for shard in shards {
            let shard = shard.map_err(|e| io_err("read CAS shard entry", e))?;
            if !shard.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let objects = fs::read_dir(shard.path()).map_err(|e| io_err("read CAS shard", e))?;
            for object in objects {
                let object = object.map_err(|e| io_err("read CAS object entry", e))?;
                let name = object.file_name().to_string_lossy().to_string();
                if !is_full_lower_hex(&name) || is_temp_name(&name) {
                    continue;
                }
                // Symlinks are not objects; file_type here does not follow.
                if object.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.push(name);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Unlink one object. The exclusive guard is passed in rather than
    /// acquired, so the recheck a sweeper performs and the removal it then
    /// applies are inside one held lock and cannot be split by a publisher.
    pub fn remove_object(&self, sha256: &str, guard: &StoreLock) -> Result<(), CasError> {
        let path = self.sweep_target(sha256, guard)?;
        fs::remove_file(&path).map_err(|e| io_err("remove object", e))
    }

    /// Move one object into `<store_root>/gc/quarantine/<hash>` instead of
    /// unlinking it, so a wrong collection verdict remains recoverable.
    pub fn quarantine_object(&self, sha256: &str, guard: &StoreLock) -> Result<(), CasError> {
        let path = self.sweep_target(sha256, guard)?;
        let dir = self
            .root
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR);
        fs::create_dir_all(&dir).map_err(|e| io_err("create quarantine directory", e))?;
        let dest = dir.join(sha256);
        replace_file(&path, &dest).map_err(|e| io_err("quarantine object", e))?;
        sync_dir(&dir);
        Ok(())
    }

    /// Shared preconditions for any sweep mutation: an exclusive guard, a
    /// well-formed identity, and a real regular file re-stated under the lock
    /// so a symlink cannot be substituted after the decision was made.
    fn sweep_target(&self, sha256: &str, guard: &StoreLock) -> Result<PathBuf, CasError> {
        if !guard.is_exclusive() {
            return Err(CasError::PolicyDenied(
                "removing objects requires the exclusive store sweep lock".to_string(),
            ));
        }
        if !is_full_lower_hex(sha256) {
            return Err(CasError::Malformed(format!(
                "identity must be full lowercase 64-hex SHA-256, got '{sha256}'"
            )));
        }
        let path = self.object_path(sha256);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(CasError::NotFound),
            Err(e) => return Err(io_err("stat object", e)),
        };
        if !meta.file_type().is_file() {
            return Err(CasError::Malformed(
                "sweep target is not a regular file (symlink substitution refused)".to_string(),
            ));
        }
        Ok(path)
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

/// Refresh a path's modification time in place. Opened without truncation, so
/// object content is never altered.
fn touch_path(path: &Path) -> std::io::Result<()> {
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(false)
        .open(path)?;
    file.set_times(fs::FileTimes::new().set_modified(SystemTime::now()))
}

/// Bounded temp cleanup rule: only temp-shaped files inside the given fan-out
/// directory, and only when older than max_age. Best-effort; never errors,
/// never touches younger files, so it cannot race active writers. All three
/// engines' historical temp shapes are recognized (see [is_temp_name]).
fn reap_stale_temps(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !is_temp_name(&name.to_string_lossy()) {
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
        assert!(
            p.ends_with(format!("blobs/sha256/4f/{h}")),
            "{p:?} vs {CAS_LAYOUT}"
        );
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

    /// Publish reports whether it created the object or deduped onto one.
    #[test]
    fn put_outcome_distinguishes_create_from_dedup() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let first = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(first.created);
        let second = cas.put_outcome(b"payload", CAS_MAX_OBJECT_BYTES).unwrap();
        assert!(!second.created);
        assert_eq!(first.hash, second.hash);
    }

    /// A wrong caller-supplied digest writes nothing at all.
    #[test]
    fn put_prehashed_rejects_a_wrong_digest_without_writing() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let real = content_hash_hex(b"payload");
        let wrong = content_hash_hex(b"other");
        let err = cas.put_prehashed(&wrong, b"payload").unwrap_err();
        assert_eq!(err.class(), "digest_mismatch");
        assert!(!cas.contains(&wrong));
        assert!(!cas.contains(&real), "nothing is published on mismatch");
        assert!(cas.put_prehashed(&real, b"payload").unwrap().created);
    }

    /// Deduping refreshes the mtime, because a dedup is a fresh reference and
    /// an age-based retention policy that never sees it collects a live object.
    #[test]
    fn dedup_refreshes_the_modification_time() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        let path = cas.object_path(&hash);
        let old = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
        fs::OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        cas.put(b"payload").unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(after > before, "dedup must refresh mtime");
    }

    #[test]
    fn touch_refreshes_only_existing_objects() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        assert!(cas.touch(&hash).is_ok());
        let absent = content_hash_hex(b"absent");
        assert_eq!(cas.touch(&absent).unwrap_err().class(), "missing");
        assert_eq!(cas.touch("nope").unwrap_err().class(), "malformed");
    }

    #[test]
    fn list_objects_reports_objects_and_ignores_debris() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let a = cas.put(b"a").unwrap();
        let b = cas.put(b"bb").unwrap();
        let shard = cas.object_path(&a).parent().unwrap().to_path_buf();
        fs::write(shard.join(".tmp-deadbeef-1-0"), b"debris").unwrap();
        fs::write(shard.join("not-a-hash"), b"debris").unwrap();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(cas.list_objects().unwrap(), expected);
    }

    #[test]
    fn list_objects_is_empty_for_a_fresh_store() {
        let root = tempdir().unwrap();
        assert!(SharedCas::open(root.path())
            .list_objects()
            .unwrap()
            .is_empty());
    }

    /// Removal is refused without the exclusive guard, so no caller can sweep
    /// while publishers are free to run.
    #[test]
    fn removal_requires_the_exclusive_guard() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"payload").unwrap();
        let publish_guard = cas.lock_for_publish().unwrap();
        let err = cas.remove_object(&hash, &publish_guard).unwrap_err();
        assert_eq!(err.class(), "policy_denied");
        assert!(
            cas.contains(&hash),
            "the object must survive a refused sweep"
        );
        let err = cas.quarantine_object(&hash, &publish_guard).unwrap_err();
        assert_eq!(err.class(), "policy_denied");
        assert!(cas.contains(&hash));
    }

    #[test]
    fn sweeping_under_the_exclusive_guard_removes_and_quarantines() {
        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let doomed = cas.put(b"doomed").unwrap();
        let kept = cas.put(b"kept").unwrap();
        let guard = cas.lock_for_sweep().unwrap();
        cas.remove_object(&doomed, &guard).unwrap();
        assert!(!cas.contains(&doomed));
        cas.quarantine_object(&kept, &guard).unwrap();
        assert!(!cas.contains(&kept));
        let quarantined = root
            .path()
            .join(crate::gc_lock::GC_DIR)
            .join(CAS_QUARANTINE_DIR)
            .join(&kept);
        assert_eq!(
            content_hash_hex(&fs::read(&quarantined).unwrap()),
            kept,
            "a quarantined body stays verifiable, so a wrong verdict is recoverable"
        );
        assert_eq!(
            cas.remove_object(&doomed, &guard).unwrap_err().class(),
            "missing"
        );
    }

    /// The publish/GC race, made deterministic with channel rendezvous rather
    /// than timing.
    ///
    /// A sweeper parks between its liveness decision and its unlink. Before the
    /// coordination lock existed, a publisher could complete inside that window
    /// and have its object deleted immediately afterwards, so publish returned
    /// Ok for an object that no longer existed. Now the publisher is excluded
    /// until the sweep releases, and the republished object survives.
    #[test]
    fn a_publisher_cannot_slip_between_a_sweep_decision_and_its_unlink() {
        use std::sync::mpsc;

        let root = tempdir().unwrap();
        let cas = SharedCas::open(root.path());
        let hash = cas.put(b"contested").unwrap();
        let expected = hash.clone();

        let (parked_tx, parked_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let sweep_root = root.path().to_path_buf();
        let sweep_hash = hash.clone();

        let sweeper = std::thread::spawn(move || {
            let cas = SharedCas::open(&sweep_root);
            let guard = cas.lock_for_sweep().unwrap();
            parked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            cas.remove_object(&sweep_hash, &guard).unwrap();
        });

        parked_rx.recv().unwrap();
        assert!(
            StoreLock::try_publish(root.path()).unwrap().is_none(),
            "a publish must not proceed while a sweep holds the guard"
        );
        release_tx.send(()).unwrap();
        sweeper.join().unwrap();

        assert!(!cas.contains(&hash), "the sweep completed its removal");
        let republished = cas.put(b"contested").unwrap();
        assert_eq!(republished, expected);
        assert_eq!(
            cas.get_verified(&republished).unwrap(),
            b"contested",
            "a publish that returns Ok must leave a readable object behind"
        );
    }

    /// Every historical engine temp shape is reaped, not just this crate's.
    #[test]
    fn all_engine_temp_shapes_are_reaped() {
        let dir = tempdir().unwrap();
        let hash = content_hash_hex(b"x");
        let shapes = [
            format!(".tmp-{}-{}-0", &hash[..8], std::process::id()),
            format!(".tmp-{hash}-1234567890-0.blob"),
            format!("{hash}.{}.0.tmp", std::process::id()),
        ];
        let stale = std::time::SystemTime::now() - CAS_TEMP_REAP_AGE - Duration::from_secs(60);
        for shape in &shapes {
            let path = dir.path().join(shape);
            fs::write(&path, b"debris").unwrap();
            fs::OpenOptions::new()
                .write(true)
                .truncate(false)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(stale))
                .unwrap();
        }
        reap_stale_temps(dir.path(), CAS_TEMP_REAP_AGE);
        for shape in &shapes {
            assert!(
                !dir.path().join(shape).exists(),
                "stale temp left behind: {shape}"
            );
        }
    }
}
