//! Canonical ZeroRef v1 content-addressed store.
//!
//! Derived from the GraphZero shared_cas implementation (the strictest of
//! the three engines), generalized behind engine-neutral errors.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use zero_ref::{content_hash_hex, is_full_lower_hex};

use crate::fs_replace::{replace_file, sync_dir};
use crate::gc_lock::{LOCK_DEADLINE, StoreLock};

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
const TEMP_CREATE_ATTEMPTS: usize = 5;

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

/// True when `name` is a published CAS identity: full lowercase 64-hex and not a temp.
#[inline]
fn is_listable_object_name(name: &str) -> bool {
    is_full_lower_hex(name) && !is_temp_name(name)
}

/// Ensure fan-out parent dirs exist and are real directories (no symlink substitution).
fn ensure_object_publish_dirs(parent: &Path) -> Result<(), CasError> {
    fs::create_dir_all(parent).map_err(|e| io_err("create object directory", e))?;
    let sha256_level = parent.parent().ok_or_else(|| {
        CasError::Malformed("object path is missing the sha256 directory level".into())
    })?;
    for level in [parent, sha256_level] {
        let meta = fs::symlink_metadata(level).map_err(|e| io_err("stat object directory", e))?;
        if !meta.file_type().is_dir() {
            return Err(CasError::Malformed(
                "object directory is not a real directory (symlink substitution refused)"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Collect listable regular-file object names from one fan-out shard directory.
fn push_objects_from_shard(
    shard_path: &Path,
    out: &mut Vec<String>,
    max_objects: Option<usize>,
) -> Result<(), CasError> {
    for object in fs::read_dir(shard_path).map_err(|e| io_err("read CAS shard", e))? {
        let object = object.map_err(|e| io_err("read CAS object entry", e))?;
        let name = object.file_name().to_string_lossy().into_owned();
        // Combined name predicates + regular-file gate; file_type does not follow.
        if is_listable_object_name(&name)
            && object.file_type().map(|t| t.is_file()).unwrap_or(false)
        {
            if let Some(max_objects) = max_objects
                && out.len() >= max_objects
            {
                return Err(CasError::Malformed(format!(
                    "CAS object enumeration exceeds {max_objects} objects"
                )));
            }
            out.push(name);
        }
    }
    Ok(())
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

/// True only for a regular file at exactly this path.
///
/// `Path::is_file` follows symlinks, so a symlink pointing at a regular file
/// reports present and then fails verification under [SharedCas::get_verified],
/// which stats the link itself. Every presence check uses this instead.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

fn io_err(context: &str, e: impl std::fmt::Display) -> CasError {
    CasError::Io(format!("{context}: {e}"))
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
        is_full_lower_hex(sha256) && is_regular_file(&self.object_path(sha256))
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

    /// Publish while already holding this store's coordinator lock.
    ///
    /// Normal batches use a shared publish guard. Atomic object-plus-lease
    /// transactions use an exclusive guard so the object and protection record
    /// become visible as one collector boundary.
    /// # Multi-object contract
    ///
    /// There is no cross-object barrier. Each call is individually atomic (a
    /// reader sees an object either absent or complete and verifying), but a
    /// batch is **not** all-or-nothing: if the third of five calls fails, or the
    /// process dies mid-batch, the objects already published stay published.
    ///
    /// This is safe because CAS objects are content-addressed and immutable, so
    /// a partial batch is a subset of the intended objects and never a corrupt
    /// or half-written one. Callers that need set-level atomicity must get it
    /// from the artifact that names the set: publish every member first, and
    /// only then publish the manifest/root that references them, so a crash
    /// leaves unreferenced garbage for the sweeper rather than a dangling
    /// reference. Do not treat an error from this method as "nothing was
    /// written".
    pub fn put_in_lock(
        &self,
        bytes: &[u8],
        limit: u64,
        guard: &StoreLock,
    ) -> Result<PutOutcome, CasError> {
        self.check_guard_root(guard)?;
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
        static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        self.put_with_limit_and_sequence(bytes, limit, || {
            TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        })
    }

    fn put_with_limit_and_sequence(
        &self,
        bytes: &[u8],
        limit: u64,
        next_sequence: impl FnMut() -> u64,
    ) -> Result<PutOutcome, CasError> {
        if bytes.len() as u64 > limit {
            return Err(CasError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({limit} bytes)",
                bytes.len()
            )));
        }
        // Hash the complete bytes before deriving any destination.
        let hash = content_hash_hex(bytes);
        let dest = self.object_path(&hash);

        if let Some(outcome) = self.try_return_existing_object(&dest, &hash, bytes.len(), limit)? {
            return Ok(outcome);
        }

        let parent = dest.parent().ok_or_else(|| {
            CasError::Malformed("object path is missing a parent directory".into())
        })?;
        ensure_object_publish_dirs(parent)?;
        reap_stale_temps(parent, CAS_TEMP_REAP_AGE);
        // Converging on a concurrent publisher's identical object is a dedup,
        // not a creation, exactly as the preexisting-destination path reports it.
        let created = self.publish_new_object_via_temp_with_sequence(
            parent,
            &dest,
            &hash,
            bytes,
            limit,
            next_sequence,
        )?;
        Ok(PutOutcome { hash, created })
    }

    /// Preexisting destination: verify, touch mtime, never overwrite.
    /// `None` means NotFound — the normal create path.
    fn try_return_existing_object(
        &self,
        dest: &Path,
        hash: &str,
        bytes_len: usize,
        limit: u64,
    ) -> Result<Option<PutOutcome>, CasError> {
        match fs::symlink_metadata(dest) {
            Ok(meta) => {
                self.check_regular(&meta, hash)?;
                let existing = self.read_verified_at(dest, hash, limit)?;
                debug_assert_eq!(existing.len(), bytes_len);
                // Refresh the mtime: a dedup is a fresh reference, and an
                // age-based retention policy that never sees it will collect a
                // still-referenced object.
                let _ = touch_path(dest);
                Ok(Some(PutOutcome {
                    hash: hash.to_string(),
                    created: false,
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("stat existing object", e)),
        }
    }

    /// Unique sibling temp, write+fsync, atomic replace with race converge.
    /// Only a temp we created ourselves is ever cleaned up.
    fn publish_new_object_via_temp_with_sequence(
        &self,
        parent: &Path,
        dest: &Path,
        hash: &str,
        bytes: &[u8],
        limit: u64,
        mut next_sequence: impl FnMut() -> u64,
    ) -> Result<bool, CasError> {
        let (file, tmp) = Self::create_temp_object(parent, hash, &mut next_sequence)?;
        let publish = self.publish_temp_object(file, &tmp, parent, dest, hash, bytes, limit);
        // create_new established ownership, so removal races no other publisher.
        // Successful rename has already consumed the path; convergence leaves it.
        let _ = fs::remove_file(tmp);
        publish
    }

    fn create_temp_object(
        parent: &Path,
        hash: &str,
        next_sequence: &mut impl FnMut() -> u64,
    ) -> Result<(fs::File, PathBuf), CasError> {
        for attempt in 0..TEMP_CREATE_ATTEMPTS {
            let tmp = parent.join(format!(
                "{TEMP_PREFIX}{}-{}-{}",
                &hash[..8],
                std::process::id(),
                next_sequence()
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
            {
                Ok(file) => return Ok((file, tmp)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        && attempt + 1 < TEMP_CREATE_ATTEMPTS => {}
                Err(error) => {
                    return Err(io_err(
                        &format!(
                            "create temp object after {} unique-name attempt(s)",
                            attempt + 1
                        ),
                        error,
                    ));
                }
            }
        }
        Err(CasError::Io(
            "create temp object: no unique name after all attempts".into(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_temp_object(
        &self,
        mut file: fs::File,
        tmp: &Path,
        parent: &Path,
        dest: &Path,
        hash: &str,
        bytes: &[u8],
        limit: u64,
    ) -> Result<bool, CasError> {
        file.write_all(bytes)
            .map_err(|e| io_err("write temp object", e))?;
        file.sync_all().map_err(|e| io_err("sync temp object", e))?;
        // Concurrent identical writers may rename over each other; both
        // orders leave one valid object with these exact bytes.
        if let Err(e) = replace_file(tmp, dest) {
            // Destination contention: if a concurrent writer already published
            // a verifying object, converge. Order is load-bearing: stat first.
            if is_regular_file(dest) && self.read_verified_at(dest, hash, limit).is_ok() {
                return Ok(false);
            }
            return Err(io_err("publish object", e));
        }
        // A directory fsync failure downgrades durability after publication. It
        // is accepted only when the destination still verifies, as before.
        if sync_dir(parent).is_err() && self.read_verified_at(dest, hash, limit).is_err() {
            return Err(CasError::Io(format!(
                "sync object directory after publishing {} in '{}'",
                &hash[..8],
                self.label
            )));
        }
        Ok(true)
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
        if !is_regular_file(&path) {
            return Err(CasError::NotFound);
        }
        touch_path(&path).map_err(|e| io_err("touch object", e))
    }

    /// Every published object: exact 64-lowercase-hex regular files under the
    /// fan-out, skipping symlinks, temps, and dotfiles.
    pub fn list_objects(&self) -> Result<Vec<String>, CasError> {
        self.list_objects_with_limit(None)
    }

    /// List at most `max_objects` published objects for a bounded GC pass.
    pub(crate) fn list_objects_bounded(&self, max_objects: usize) -> Result<Vec<String>, CasError> {
        self.list_objects_with_limit(Some(max_objects))
    }

    fn list_objects_with_limit(&self, max_objects: Option<usize>) -> Result<Vec<String>, CasError> {
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
            push_objects_from_shard(&shard.path(), &mut out, max_objects)?;
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
        // Post-rename: the object is already out of the object tree, so a
        // directory fsync failure is a durability warning, not a failed move.
        let _ = sync_dir(&dir);
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
        self.check_guard_root(guard)?;
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

}

/// Authorization gate for verified CAS reads (ZS-SEC-005). A capability
/// gate names the exact content a reader is authorized to fetch; a refusal
/// is fail-closed: no object lookup happens and no bytes are returned.
pub trait CasReadGate {
    /// Refuse the read with `PolicyDenied` unless the reader is authorized
    /// to fetch exactly `sha256`. Called before any object lookup, so a
    /// refused read cannot leak object existence either.
    fn authorize_read(&self, sha256: &str) -> Result<(), CasError>;
}

impl SharedCas {

    /// Open by full digest, enforce the regular-file/size policy, hash the
    /// complete bytes, and only then return data. Digest mismatch is loud and
    /// returns no bytes.
    pub fn get_verified(&self, sha256: &str) -> Result<Vec<u8>, CasError> {
        self.get_verified_limited(sha256, CAS_MAX_OBJECT_BYTES)
    }

    /// Verified read behind an authorization gate. The gate is consulted
    /// BEFORE any object lookup, so a refused read returns no bytes and cannot
    /// distinguish object existence; content still hashes to the requested
    /// identity before bytes are returned.
    pub fn get_verified_gated(
        &self,
        sha256: &str,
        gate: &dyn CasReadGate,
    ) -> Result<Vec<u8>, CasError> {
        gate.authorize_read(sha256)?;
        self.get_verified(sha256)
    }

    /// Verified read under a caller-supplied bound no weaker than the CAS policy.
    /// Metadata is checked before allocation so strict format profiles can impose
    /// a smaller ceiling than the shared CAS maximum.
    pub fn get_verified_limited(&self, sha256: &str, limit: u64) -> Result<Vec<u8>, CasError> {
        if !is_full_lower_hex(sha256) {
            return Err(CasError::Malformed(format!(
                "identity must be full lowercase 64-hex SHA-256, got '{sha256}'"
            )));
        }
        let effective_limit = limit.min(CAS_MAX_OBJECT_BYTES);
        let path = self.object_path(sha256);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CasError::NotFound);
            }
            Err(e) => return Err(io_err("stat object", e)),
        };
        self.check_regular(&meta, sha256)?;
        if meta.len() > effective_limit {
            return Err(CasError::PolicyDenied(format!(
                "object of {} bytes exceeds the CAS size policy ({effective_limit} bytes)",
                meta.len()
            )));
        }
        self.read_verified_at(&path, sha256, effective_limit)
    }

    /// A guard only excludes writers of the store it was taken on, so using one
    /// store's lock while mutating another provides no exclusion at all. Refuse
    /// it rather than run an unsynchronized publish or sweep.
    fn check_guard_root(&self, guard: &StoreLock) -> Result<(), CasError> {
        if guard.is_for_store_root(&self.root) {
            return Ok(());
        }
        Err(CasError::PolicyDenied(format!(
            "store lock was taken on a different store root than '{}'",
            self.label
        )))
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
#[cfg(test)]
#[path = "../../../tests/rust/zero-store/unit/cas.rs"]
mod tests;
