//! ZeroStack store schema stamps for TokenZero segments.
//!
//! Shared `.zerostack` layout: stamp major.minor on ActionCache segments,
//! `shadow.jsonl`, and the recovery blobs manifest. Newer major is refused;
//! older minor degrades. `shadow.jsonl` is a fixed ring. ActionCache writes
//! use a commit marker so a torn temp is never promoted.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Current TokenZero store schema (ZeroRef sibling contract).
pub const STORE_SCHEMA_MAJOR: u16 = 1;
pub const STORE_SCHEMA_MINOR: u16 = 0;
pub const STORE_SCHEMA_NAME: &str = "tokenzero.store";
/// Fixed ring size for `shadow.jsonl`. Never unbounded.
pub const SHADOW_JSONL_RING_CAP: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSchemaVersion {
    pub major: u16,
    pub minor: u16,
}

impl StoreSchemaVersion {
    pub const CURRENT: Self = Self {
        major: STORE_SCHEMA_MAJOR,
        minor: STORE_SCHEMA_MINOR,
    };

    pub fn stamp(self) -> StoreSchemaStamp {
        StoreSchemaStamp {
            schema: STORE_SCHEMA_NAME,
            major: self.major,
            minor: self.minor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSchemaStamp {
    pub schema: &'static str,
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaAdmit {
    Accept,
    /// Same major, older or newer compatible minor: read with defaults.
    DegradeMinor,
    /// Older major we can still parse with a degraded reader.
    DegradeOlderMajor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSkewError {
    NewerMajor { found: StoreSchemaVersion },
    MissingStamp,
    WrongSchema { found: String },
}

impl std::fmt::Display for SchemaSkewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerMajor { found } => write!(
                f,
                "tokenzero store schema {}.{} is newer than supported {}.{} (refuse newer major)",
                found.major, found.minor, STORE_SCHEMA_MAJOR, STORE_SCHEMA_MINOR
            ),
            Self::MissingStamp => write!(f, "tokenzero store segment is missing a schema stamp"),
            Self::WrongSchema { found } => {
                write!(
                    f,
                    "tokenzero store schema name {found:?} is not {STORE_SCHEMA_NAME}"
                )
            }
        }
    }
}

impl std::error::Error for SchemaSkewError {}

/// Admit a found stamp against the current TokenZero store schema.
pub fn admit_store_schema(stamp: &StoreSchemaStamp) -> Result<SchemaAdmit, SchemaSkewError> {
    admit_store_schema_against(stamp, StoreSchemaVersion::CURRENT)
}

/// Admit a found stamp against an explicit current version (tests + callers).
pub fn admit_store_schema_against(
    stamp: &StoreSchemaStamp,
    current: StoreSchemaVersion,
) -> Result<SchemaAdmit, SchemaSkewError> {
    if stamp.schema != STORE_SCHEMA_NAME {
        return Err(SchemaSkewError::WrongSchema {
            found: stamp.schema.to_string(),
        });
    }
    let found = StoreSchemaVersion {
        major: stamp.major,
        minor: stamp.minor,
    };
    if found.major > current.major {
        return Err(SchemaSkewError::NewerMajor { found });
    }
    if found.major < current.major {
        return Ok(SchemaAdmit::DegradeOlderMajor);
    }
    if found.minor == current.minor {
        Ok(SchemaAdmit::Accept)
    } else {
        Ok(SchemaAdmit::DegradeMinor)
    }
}

/// Append one shadow line and trim to the fixed ring cap.
pub fn append_shadow_jsonl(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Append plus ring trim is a read-modify-write. Without a lock two
    // writers can each trim a stale snapshot and drop the other's lines.
    let _lock = lock_exclusive(path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    drop(file);
    trim_shadow_ring(path)
}

fn trim_shadow_ring(path: &Path) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() <= SHADOW_JSONL_RING_CAP {
        return Ok(());
    }
    lines = lines.split_off(lines.len() - SHADOW_JSONL_RING_CAP);
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    fs::write(path, out)
}

/// Crash-safe ActionCache segment write: unique temp + commit marker + rename.
/// A per-key flock is held across write-commit-rename so concurrent puts of
/// distinct payloads cannot interleave a shared sidecar pair.
pub fn write_actioncache_segment(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = lock_exclusive(dest)?;
    let tmp = unique_tmp_path(dest);
    let commit = commit_for_tmp(&tmp);
    let result = (|| {
        fs::write(&tmp, bytes)?;
        let digest = hex_sha256(bytes);
        fs::write(&commit, digest.as_bytes())?;
        fs::rename(&tmp, dest)?;
        let _ = fs::remove_file(&commit);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(&commit);
    }
    result
}

/// Recover a segment after crash: promote a committed temp, discard an uncommitted one.
pub fn recover_actioncache_segment(dest: &Path) -> io::Result<Option<PathBuf>> {
    if dest.exists() {
        // dest is the committed file. Do not unlink sidecars: a concurrent
        // put writes unique tmp/commit before rename.
        return Ok(Some(dest.to_path_buf()));
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(None);
    }
    let Some(_lock) = try_lock_key(dest)? else {
        // Writer holds the lock: do not promote or delete its sidecars.
        return Ok(dest.exists().then(|| dest.to_path_buf()));
    };
    if dest.exists() {
        return Ok(Some(dest.to_path_buf()));
    }
    let mut pairs = unique_sidecar_pairs(dest)?;
    pairs.push((legacy_tmp_path(dest), legacy_commit_path(dest)));
    let mut promoted = false;
    for (tmp, commit) in pairs {
        if !promoted && pair_matches_digest(&tmp, &commit)? {
            fs::rename(&tmp, dest)?;
            let _ = fs::remove_file(&commit);
            promoted = true;
        } else {
            let _ = fs::remove_file(&tmp);
            let _ = fs::remove_file(&commit);
        }
    }
    if dest.exists() {
        Ok(Some(dest.to_path_buf()))
    } else {
        Ok(None)
    }
}

struct SegmentLock {
    file: fs::File,
}

impl Drop for SegmentLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn lock_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let mut file_name = dest
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("actioncache"));
    file_name.push(".lock");
    parent.join(file_name)
}

fn lock_exclusive(dest: &Path) -> io::Result<SegmentLock> {
    let path = lock_path(dest);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    FileExt::lock(&file)?;
    Ok(SegmentLock { file })
}

fn try_lock_key(dest: &Path) -> io::Result<Option<SegmentLock>> {
    let path = lock_path(dest);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    match FileExt::try_lock(&file) {
        Ok(()) => Ok(Some(SegmentLock { file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

fn unique_tmp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp_name = OsString::from(".");
    tmp_name.push(
        dest.file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("actioncache")),
    );
    let nonce = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    tmp_name.push(format!(".{}.{nonce}.tmp", std::process::id()));
    parent.join(tmp_name)
}

fn commit_for_tmp(tmp: &Path) -> PathBuf {
    let parent = tmp.parent().unwrap_or_else(|| Path::new("."));
    let name = tmp.file_name().map(OsString::from).unwrap_or_default();
    let lossy = name.to_string_lossy();
    if let Some(stem) = lossy.strip_suffix(".tmp") {
        parent.join(format!("{stem}.commit"))
    } else {
        let mut commit_name = name;
        commit_name.push(".commit");
        parent.join(commit_name)
    }
}

fn unique_sidecar_pairs(dest: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let Some(filename) = dest.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let prefix = format!(".{filename}.");
    let mut stems = BTreeSet::new();
    let rd = match fs::read_dir(parent) {
        Ok(rd) => rd,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    for entry in rd {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        if let Some(stem) = name.strip_suffix(".tmp") {
            stems.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".commit") {
            stems.insert(stem.to_string());
        }
    }
    Ok(stems
        .into_iter()
        .map(|stem| {
            (
                parent.join(format!("{stem}.tmp")),
                parent.join(format!("{stem}.commit")),
            )
        })
        .collect())
}

fn legacy_tmp_path(dest: &Path) -> PathBuf {
    dest.with_extension("tmp")
}

fn legacy_commit_path(dest: &Path) -> PathBuf {
    dest.with_extension("commit")
}

fn pair_matches_digest(tmp: &Path, commit: &Path) -> io::Result<bool> {
    if !tmp.exists() || !commit.exists() {
        return Ok(false);
    }
    let expected = fs::read_to_string(commit)?;
    let bytes = fs::read(tmp)?;
    Ok(hex_sha256(&bytes) == expected.trim())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
