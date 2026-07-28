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

/// Write all bytes, fsync the temp, replace onto dest, then fsync parent on Unix.
/// Order is load-bearing: never replace before `sync_all` on the temp.
fn write_sync_replace(
    mut file: File,
    temp: &Path,
    dest: &Path,
    parent: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(temp, dest)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// Atomically publish bytes at dest without ever exposing a truncated file.
///
/// The uniquely-created sibling prevents concurrent processes from sharing a
/// temp path. The temp file is synced before replacement; on Unix the parent
/// directory is synced afterwards so the rename survives a crash.
pub fn atomic_write_file(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = dest.file_name().unwrap_or_else(|| OsStr::new("artifact"));

    let (file, temp) = open_unique_temp(parent, file_name)?;
    let published = write_sync_replace(file, &temp, dest, parent, bytes);
    if published.is_err() {
        let _ = fs::remove_file(&temp);
    }
    published
}
