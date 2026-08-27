//! Writer exclusion via an OS advisory file lock on `.graphzero/lock`
//! (FR-012). Readers mmap published snapshots and never take the lock.
//! Uses `std::fs::File::lock` (flock on Unix, LockFileEx on Windows).

use std::fs::{File, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result};

pub struct WriterLock {
    file: File,
}

impl WriterLock {
    /// Acquire the exclusive writer lock, blocking until available.
    pub fn acquire(store_root: &Path) -> Result<Self> {
        let file = Self::open_lock_file(store_root)?;
        file.lock().context("acquire writer lock")?;
        Ok(Self { file })
    }

    /// Try to acquire without blocking; fails if another writer holds it.
    pub fn try_acquire(store_root: &Path) -> Result<Self> {
        let file = Self::open_lock_file(store_root)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("writer lock held by another process")
            }
            Err(std::fs::TryLockError::Error(e)) => Err(e).context("try writer lock"),
        }
    }

    fn open_lock_file(store_root: &Path) -> Result<File> {
        std::fs::create_dir_all(store_root)?;
        let path = store_root.join("lock");
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("open lock file {}", path.display()))
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
