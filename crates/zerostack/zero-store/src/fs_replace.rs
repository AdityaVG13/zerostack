//! Durable cross-platform file replacement.
//!
//! replace_file publishes a fully written temp file over a destination.
//! On Unix rename(2) replaces atomically. On Windows MoveFileExW with
//! MOVEFILE_REPLACE_EXISTING (what std::fs::rename issues) can fail
//! transiently while a reader, antivirus scanner, or indexer holds the
//! destination open; those classified sharing errors are retried with
//! bounded backoff. Every other error is returned immediately.
//!
//! The destination is never truncated in place: until the rename lands, the
//! old file remains complete and valid.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Bounded retry policy for classified transient Windows sharing failures.
const REPLACE_ATTEMPTS: u32 = 5;
const REPLACE_BACKOFF_MS: u64 = 10;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// How a durable write treats `sync_all` failures.
///
/// Network mounts (smbfs/nfs) often return ENOTSUP/EPERM from `sync_all`.
/// Engines must keep serving on those mounts; they cannot take
/// [`DurableJournal`](crate::DurableJournal)'s fatal-sync path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SyncPolicy {
    /// `sync_all` errors are fatal, including ENOTSUP/EPERM.
    #[default]
    Required,
    /// ENOTSUP/EPERM (and `ErrorKind::Unsupported` / `PermissionDenied`) succeed.
    /// Every other I/O error is still fatal.
    TolerateUnsupported,
    /// Skip `sync_all`. Use only on the session-delta hot path.
    Never,
}

/// True when `sync_all` failed because the mount cannot fsync, not because
/// the write is in doubt. Matches TokenZero's smbfs/nfs classification:
/// macOS smbfs reports ENOTSUP/EOPNOTSUPP as `Uncategorized`.
pub fn sync_unsupported(err: &io::Error) -> bool {
    if matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    ) {
        return true;
    }
    #[cfg(target_vendor = "apple")]
    const UNSUPPORTED_CODES: &[i32] = &[45, 102]; // ENOTSUP, EOPNOTSUPP
    #[cfg(all(unix, not(target_vendor = "apple")))]
    const UNSUPPORTED_CODES: &[i32] = &[95, 524]; // EOPNOTSUPP, ENOTSUP
    #[cfg(not(unix))]
    const UNSUPPORTED_CODES: &[i32] = &[];

    err.raw_os_error()
        .is_some_and(|code| UNSUPPORTED_CODES.contains(&code))
}

/// Absorb fsync failures that mean "this filesystem cannot fsync".
pub fn tolerate_unsupported_sync(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if sync_unsupported(&err) => Ok(()),
        other => other,
    }
}

fn apply_file_sync(file: &File, policy: SyncPolicy) -> io::Result<()> {
    match policy {
        SyncPolicy::Never => Ok(()),
        SyncPolicy::Required => file.sync_all(),
        SyncPolicy::TolerateUnsupported => tolerate_unsupported_sync(file.sync_all()),
    }
}

/// Windows error codes that indicate another handle briefly blocks the
/// destination: ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32),
/// ERROR_LOCK_VIOLATION (33).
#[cfg(windows)]
fn is_transient_sharing_error(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5) | Some(32) | Some(33))
}

#[cfg(not(windows))]
fn is_transient_sharing_error(_err: &io::Error) -> bool {
    false
}

/// Replace dest with the fully written, synced tmp file.
pub fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(err) if is_transient_sharing_error(&err) && attempt + 1 < REPLACE_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(
                    REPLACE_BACKOFF_MS * u64::from(attempt),
                ));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Create an exclusive sibling temp under `parent` named from `file_name`.
/// Retries only on `AlreadyExists`; every other open error is terminal.
fn open_unique_temp(parent: &Path, file_name: &OsStr) -> io::Result<(File, PathBuf)> {
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let temp = parent.join(temp_name);
        match OpenOptions::new().create_new(true).write(true).open(&temp) {
            Ok(file) => return Ok((file, temp)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Write all bytes, optionally fsync the temp, then replace onto dest.
/// Order is load-bearing: never replace before the chosen sync policy runs.
///
/// The parent directory fsync deliberately lives in [atomic_write_file], after
/// this function has reported that dest is published: once the rename lands the
/// bytes are visible to every reader, so a later directory-sync failure means
/// "present but not proven durable", never "not written".
fn write_sync_replace(
    mut file: File,
    temp: &Path,
    dest: &Path,
    bytes: &[u8],
    policy: SyncPolicy,
) -> io::Result<()> {
    file.write_all(bytes)?;
    apply_file_sync(&file, policy)?;
    drop(file);
    replace_file(temp, dest)
}

/// Fsync a directory so a rename inside it survives a crash.
///
/// Returns the error instead of discarding it; callers decide whether the
/// failure is fatal, which depends on whether the rename already published.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Atomically publish bytes at dest without ever exposing a truncated file.
///
/// Uses [`SyncPolicy::Required`]: the temp is synced before replacement.
pub fn atomic_write_file(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_file_with_sync(dest, bytes, SyncPolicy::Required)
}

/// [`atomic_write_file`] with an explicit fsync policy for optional-fsync mounts.
pub fn atomic_write_file_with_sync(
    dest: &Path,
    bytes: &[u8],
    policy: SyncPolicy,
) -> io::Result<()> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = dest.file_name().unwrap_or_else(|| OsStr::new("artifact"));

    let (file, temp) = open_unique_temp(parent, file_name)?;
    let published = write_sync_replace(file, &temp, dest, bytes, policy);
    if published.is_err() {
        let _ = fs::remove_file(&temp);
        return published;
    }
    // dest is published. A failed directory fsync leaves the new bytes visible,
    // so returning Err here would make callers retry or report a write that in
    // fact succeeded; the weaker durability guarantee is not a failed write.
    if policy != SyncPolicy::Never {
        let _ = match policy {
            SyncPolicy::Required => sync_dir(parent),
            SyncPolicy::TolerateUnsupported => tolerate_unsupported_sync(sync_dir(parent)),
            SyncPolicy::Never => Ok(()),
        };
    }
    Ok(())
}
