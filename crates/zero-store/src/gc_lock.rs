//! The single store coordination lock.
//!
//! There is exactly one lock in this crate, an advisory lock on
//! `<store_root>/gc/coordinator.lock`. Publishers hold it shared; the garbage
//! collector holds it exclusive. Because there is only one lock there is no
//! lock *order* to get wrong and no cycle can exist, so the design is
//! deadlock-free by construction rather than by convention.
//!
//! This closes a critical publish/GC race. Previously every engine published
//! without taking any lock while the sweeper's pre-unlink recheck inspected
//! only `gc/**` metadata, so a publisher could republish or newly reference an
//! object between the sweeper's decision and its `unlink`. The publish
//! returned `Ok`, and the object was then deleted: silent data loss plus a
//! dangling reference from a committed root.
//!
//! The lock file path is deliberately identical to the one TokenZero's
//! `GcCoordLock` already uses, and both `fs4` and `std` map to `flock` on
//! Unix, so a hub-based publisher and a not-yet-migrated TokenZero sweeper
//! interoperate correctly during the cutover.
//!
//! Crash safety needs no stale-lock reclaim: the kernel releases these locks
//! when the holder's file descriptor closes, including on abnormal
//! termination.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// GC namespace directory, relative to a store root.
pub const GC_DIR: &str = "gc";

/// Coordinator lock file name.
pub const COORDINATOR_LOCK: &str = "coordinator.lock";

/// Default acquisition deadline. A publish holds the lock for one write and
/// one rename, so any wait longer than this indicates a wedged holder that is
/// better surfaced as a typed timeout than as a hang.
pub const LOCK_DEADLINE: Duration = Duration::from_secs(30);

const INITIAL_BACKOFF: Duration = Duration::from_micros(200);
const MAX_BACKOFF: Duration = Duration::from_millis(10);

/// `<store_root>/gc/coordinator.lock`.
pub fn coordinator_lock_path(store_root: &Path) -> PathBuf {
    store_root.join(GC_DIR).join(COORDINATOR_LOCK)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Publishers. Mutually compatible, excluded only by a sweep.
    Shared,
    /// The sweeper. Excludes every publisher and every other sweeper.
    Exclusive,
}

/// A held coordination lock. Released on drop and on process death.
///
/// Never acquire a second `StoreLock` while holding one, even in a different
/// mode: advisory locks are per file descriptor, so a second descriptor in the
/// same process blocks against the first exactly as another process would.
/// Functions that mutate the store under a lock therefore take `&StoreLock`
/// instead of acquiring their own.
#[derive(Debug)]
pub struct StoreLock {
    file: File,
    mode: LockMode,
    path: PathBuf,
    store_root: PathBuf,
}

impl StoreLock {
    /// Publish-side guard. Waits for any in-flight sweep, bounded by
    /// `deadline`.
    pub fn publish(store_root: &Path, deadline: Duration) -> io::Result<Self> {
        Self::acquire(store_root, LockMode::Shared, deadline)
    }

    /// Sweep-side guard, held across mark, report, recheck, and unlink so the
    /// recheck and the unlink are one atomic step with respect to publishers.
    pub fn sweep(store_root: &Path, deadline: Duration) -> io::Result<Self> {
        Self::acquire(store_root, LockMode::Exclusive, deadline)
    }

    /// Non-blocking sweep guard. `Ok(None)` means another holder is active.
    pub fn try_sweep(store_root: &Path) -> io::Result<Option<Self>> {
        Self::try_acquire(store_root, LockMode::Exclusive)
    }

    /// Non-blocking publish guard, for callers that prefer to fail fast.
    pub fn try_publish(store_root: &Path) -> io::Result<Option<Self>> {
        Self::try_acquire(store_root, LockMode::Shared)
    }

    pub fn mode(&self) -> LockMode {
        self.mode
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when this guard excludes publishers.
    pub fn is_exclusive(&self) -> bool {
        self.mode == LockMode::Exclusive
    }

    /// The store root this guard was acquired on, normalized.
    ///
    /// A guard excludes only writers of its own store, so mutators compare
    /// against this instead of assuming the caller passed a matching lock.
    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    /// True when this guard coordinates `store_root`, spelling-independently.
    pub fn is_for_store_root(&self, store_root: &Path) -> bool {
        self.store_root == crate::store_root::absolutize(store_root)
    }

    fn acquire(store_root: &Path, mode: LockMode, deadline: Duration) -> io::Result<Self> {
        let (path, file) = open_lock_file(store_root)?;
        let start = Instant::now();
        let mut backoff = INITIAL_BACKOFF;
        loop {
            match try_lock(&file, mode) {
                Ok(()) => {
                    return Ok(Self {
                        file,
                        mode,
                        store_root: crate::store_root::absolutize(store_root),
                        path,
                    });
                }
                Err(TryLockError::WouldBlock) => {
                    if start.elapsed() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!(
                                "store coordination lock at {} still held after {:?}",
                                path.display(),
                                deadline
                            ),
                        ));
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
    }

    fn try_acquire(store_root: &Path, mode: LockMode) -> io::Result<Option<Self>> {
        let (path, file) = open_lock_file(store_root)?;
        match try_lock(&file, mode) {
            Ok(()) => Ok(Some(Self {
                file,
                mode,
                store_root: crate::store_root::absolutize(store_root),
                path,
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Best effort: the kernel also releases on close, which happens next.
        let _ = self.file.unlock();
    }
}

fn try_lock(file: &File, mode: LockMode) -> Result<(), TryLockError> {
    match mode {
        LockMode::Shared => file.try_lock_shared(),
        LockMode::Exclusive => file.try_lock(),
    }
}

/// Open (never truncate) the lock file, creating `gc/` on demand. Truncation
/// would be harmless because the contents are unused, but it would also
/// destroy any future holder metadata written there.
fn open_lock_file(store_root: &Path) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(store_root)?;
    if !fs::symlink_metadata(store_root)?.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "store root is not a real directory: {}",
                store_root.display()
            ),
        ));
    }

    let directory = store_root.join(GC_DIR);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "GC namespace is not a real directory: {}",
                    directory.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "GC namespace is not a real directory: {}",
                        directory.display()
                    ),
                ));
            }
        }
        Err(error) => return Err(error),
    }

    let path = directory.join(COORDINATOR_LOCK);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("coordinator lock is not a real file: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    Ok((path, file))
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-store/unit/gc_lock.rs"]
mod tests;
