//! Session-merge WAL for engine recovery journals.
//!
//! [`DurableJournal`](crate::DurableJournal) is a single-transaction digest
//! 2PC with a 64 KiB record cap, fail-closed torn records, no foreign-writer
//! merge, and fatal `sync_all`. Engines (TokenZero recovery, FS/GZ session
//! journals) need the opposite:
//!
//! - caller-supplied opaque bytes of unbounded size (hard cap is
//!   [`SESSION_WAL_MAX_RECORD_BYTES`], 64 MiB, not 64 KiB)
//! - append-only records; compaction is a snapshot rewrite the caller owns
//! - torn tail fails open (prefix is kept)
//! - foreign writers are detected via [`FileIdentity`]; merge is caller-owned
//! - fsync is optional ([`SyncPolicy`])
//!
//! One writer per WAL unless the caller reloads, merges, and republishes after
//! [`SessionWal::foreign_write_since`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::json;

use crate::fs_replace::{SyncPolicy, atomic_write_file_with_sync, tolerate_unsupported_sync};

/// Hard cap per record. Larger than DurableJournal's 64 KiB so a recovery
/// snapshot (megabytes) can live in one frame.
pub const SESSION_WAL_MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
/// Default sealed-segment count before append asks the caller to compact.
pub const SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS: usize = 4;
/// Floor used when deriving the segment size from a missing/empty snapshot.
pub const SESSION_WAL_MIN_SEGMENT_BYTES: u64 = 64 * 1024;
/// Default replay budget across all segments.
pub const SESSION_WAL_DEFAULT_MAX_REPLAY_BYTES: u64 = 256 * 1024 * 1024;
/// Contract schema for [`session_wal_contract`].
pub const SESSION_WAL_SCHEMA_VERSION: u16 = 1;

const FRAME_OVERHEAD: u64 = 8;
const WAL_SUFFIX: &str = ".wal";

/// Snapshot or WAL file identity used to detect a foreign atomic replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    pub len: u64,
    pub modified: SystemTime,
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
}

impl FileIdentity {
    pub fn capture(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let modified = meta.modified().ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                len: meta.len(),
                modified,
                dev: meta.dev(),
                ino: meta.ino(),
            })
        }
        #[cfg(not(unix))]
        Some(Self {
            len: meta.len(),
            modified,
        })
    }
}

/// Tunables. Defaults match TokenZero's session journal: no fsync on append,
/// tolerate-unsupported fsync on snapshot publish, four sealed segments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionWalConfig {
    pub append_sync: SyncPolicy,
    pub publish_sync: SyncPolicy,
    /// 0 means `max(snapshot_len, SESSION_WAL_MIN_SEGMENT_BYTES)`.
    pub segment_limit: u64,
    pub max_sealed_segments: usize,
    pub max_replay_bytes: u64,
}

impl Default for SessionWalConfig {
    fn default() -> Self {
        Self {
            append_sync: SyncPolicy::Never,
            publish_sync: SyncPolicy::TolerateUnsupported,
            segment_limit: 0,
            max_sealed_segments: SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS,
            max_replay_bytes: SESSION_WAL_DEFAULT_MAX_REPLAY_BYTES,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionWalError {
    #[error("session WAL record is {len} bytes; max is {max}")]
    RecordTooLarge { len: u64, max: u64 },
    #[error("session WAL snapshot path must name a file")]
    InvalidPath,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Appended,
    NeedsCompaction,
}

/// Complete records from a fail-open replay. `truncated` is true when a torn
/// or invalid tail stopped the scan; the prefix in `records` is still valid.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Replay {
    pub records: Vec<Vec<u8>>,
    pub truncated: bool,
}

/// Append-only session WAL bound to one snapshot path.
#[derive(Clone, Debug)]
pub struct SessionWal {
    snapshot: PathBuf,
    config: SessionWalConfig,
}

impl SessionWal {
    pub fn new(
        snapshot: impl Into<PathBuf>,
        config: SessionWalConfig,
    ) -> Result<Self, SessionWalError> {
        let snapshot = snapshot.into();
        if snapshot.file_name().is_none() {
            return Err(SessionWalError::InvalidPath);
        }
        Ok(Self { snapshot, config })
    }

    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot
    }

    pub fn wal_path(&self) -> PathBuf {
        sibling(&self.snapshot, WAL_SUFFIX)
    }

    pub fn snapshot_identity(&self) -> Option<FileIdentity> {
        FileIdentity::capture(&self.snapshot)
    }

    pub fn wal_identity(&self) -> Option<FileIdentity> {
        FileIdentity::capture(&self.wal_path())
    }

    /// True when the snapshot or active WAL no longer matches the caller's
    /// last write. The caller must reload, merge, and republish; this crate
    /// does not interpret payload bytes.
    pub fn foreign_write_since(
        &self,
        snapshot: Option<FileIdentity>,
        wal: Option<FileIdentity>,
    ) -> bool {
        self.snapshot_identity() != snapshot || self.wal_identity() != wal
    }

    pub fn append(&self, record: &[u8]) -> Result<AppendOutcome, SessionWalError> {
        let len = record.len() as u64;
        if len > SESSION_WAL_MAX_RECORD_BYTES {
            return Err(SessionWalError::RecordTooLarge {
                len,
                max: SESSION_WAL_MAX_RECORD_BYTES,
            });
        }
        let framed = framed_len(record.len());
        let limit = self.segment_limit();
        if framed > limit {
            return Ok(AppendOutcome::NeedsCompaction);
        }

        let active = self.wal_path();
        if let Some(parent) = active.parent() {
            fs::create_dir_all(parent)?;
        }
        let active_len = fs::metadata(&active).map(|meta| meta.len()).unwrap_or(0);
        if active_len > 0 && active_len.saturating_add(framed) > limit {
            if self.sealed_path(self.config.max_sealed_segments).exists() {
                return Ok(AppendOutcome::NeedsCompaction);
            }
            self.rotate_segments()?;
        }

        let mut file = OpenOptions::new().create(true).append(true).open(&active)?;
        write_frame(&mut file, record)?;
        apply_append_sync(&file, self.config.append_sync)?;
        Ok(AppendOutcome::Appended)
    }

    /// Replay complete frames oldest-to-newest. Stops at the first torn or
    /// non-canonical tail without failing the call.
    pub fn replay(&self) -> Result<Replay, SessionWalError> {
        let mut remaining = self.config.max_replay_bytes;
        let mut replay = Replay::default();
        let segments = (1..=self.config.max_sealed_segments)
            .rev()
            .map(|generation| self.sealed_path(generation))
            .chain(std::iter::once(self.wal_path()));
        for path in segments {
            match replay_segment(&path, &mut remaining, &mut replay)? {
                SegmentStatus::Missing => {}
                SegmentStatus::Consumed => {}
                SegmentStatus::Stopped => return Ok(replay),
            }
        }
        Ok(replay)
    }

    /// Atomically publish snapshot bytes and delete WAL segments.
    pub fn publish_snapshot(&self, bytes: &[u8]) -> Result<(), SessionWalError> {
        atomic_write_file_with_sync(&self.snapshot, bytes, self.config.publish_sync)?;
        self.clear_wal();
        Ok(())
    }

    pub fn clear_wal(&self) {
        let _ = fs::remove_file(self.wal_path());
        for generation in 1..=self.config.max_sealed_segments {
            let _ = fs::remove_file(self.sealed_path(generation));
        }
    }

    fn segment_limit(&self) -> u64 {
        if self.config.segment_limit > 0 {
            return self.config.segment_limit;
        }
        FileIdentity::capture(&self.snapshot)
            .map(|identity| identity.len)
            .unwrap_or(0)
            .max(SESSION_WAL_MIN_SEGMENT_BYTES)
    }

    fn sealed_path(&self, generation: usize) -> PathBuf {
        sibling(&self.wal_path(), &format!(".{generation}"))
    }

    fn rotate_segments(&self) -> io::Result<()> {
        let active = self.wal_path();
        for generation in (1..self.config.max_sealed_segments).rev() {
            let from = self.sealed_path(generation);
            if from.exists() {
                fs::rename(from, self.sealed_path(generation + 1))?;
            }
        }
        fs::rename(&active, self.sealed_path(1))
    }
}

/// Machine-readable contract used by conformance generators.
pub fn session_wal_contract() -> serde_json::Value {
    json!({
        "schema_version": SESSION_WAL_SCHEMA_VERSION,
        "max_record_bytes": SESSION_WAL_MAX_RECORD_BYTES,
        "min_segment_bytes": SESSION_WAL_MIN_SEGMENT_BYTES,
        "default_max_sealed_segments": SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS,
        "default_max_replay_bytes": SESSION_WAL_DEFAULT_MAX_REPLAY_BYTES,
        "framing": "u32le_len_payload_u32le_len",
        "torn_tail": "fail_open",
        "merge": "caller_owned",
        "foreign_writer": "file_identity",
        "sync_policies": ["required", "tolerate_unsupported", "never"],
        "not": "durable_journal",
    })
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn framed_len(payload_len: usize) -> u64 {
    FRAME_OVERHEAD.saturating_add(payload_len as u64)
}

fn write_frame(file: &mut File, record: &[u8]) -> io::Result<()> {
    let len = u32::try_from(record.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "session WAL record exceeds u32 length",
        )
    })?;
    file.write_all(&len.to_le_bytes())?;
    file.write_all(record)?;
    file.write_all(&len.to_le_bytes())?;
    Ok(())
}

fn apply_append_sync(file: &File, policy: SyncPolicy) -> io::Result<()> {
    match policy {
        SyncPolicy::Never => Ok(()),
        SyncPolicy::Required => file.sync_all(),
        SyncPolicy::TolerateUnsupported => tolerate_unsupported_sync(file.sync_all()),
    }
}

enum SegmentStatus {
    Missing,
    Consumed,
    Stopped,
}

fn replay_segment(
    path: &Path,
    remaining: &mut u64,
    replay: &mut Replay,
) -> io::Result<SegmentStatus> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(SegmentStatus::Missing),
        Err(err) => return Err(err),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > *remaining {
        replay.truncated = true;
        return Ok(SegmentStatus::Stopped);
    }
    *remaining -= meta.len();
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)?;
    decode_frames(&bytes, replay);
    if replay.truncated {
        return Ok(SegmentStatus::Stopped);
    }
    Ok(SegmentStatus::Consumed)
}

fn decode_frames(bytes: &[u8], replay: &mut Replay) {
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(frame) = read_frame(bytes, offset) else {
            replay.truncated = true;
            return;
        };
        match frame {
            Frame::Complete { payload, next } => {
                replay.records.push(payload.to_vec());
                offset = next;
            }
            Frame::Torn => {
                replay.truncated = true;
                return;
            }
        }
    }
}

enum Frame<'a> {
    Complete { payload: &'a [u8], next: usize },
    Torn,
}

fn read_frame(bytes: &[u8], offset: usize) -> Option<Frame<'_>> {
    let rest = bytes.get(offset..)?;
    if rest.is_empty() {
        return None;
    }
    if rest.len() < 4 {
        return Some(Frame::Torn);
    }
    let len = u32::from_le_bytes(rest[..4].try_into().ok()?) as usize;
    if len as u64 > SESSION_WAL_MAX_RECORD_BYTES {
        return Some(Frame::Torn);
    }
    let payload_end = 4usize.checked_add(len)?;
    let trailer_end = payload_end.checked_add(4)?;
    if rest.len() < trailer_end {
        return Some(Frame::Torn);
    }
    let payload = rest.get(4..payload_end)?;
    let trailer = u32::from_le_bytes(rest[payload_end..trailer_end].try_into().ok()?);
    if trailer as usize != len {
        return Some(Frame::Torn);
    }
    Some(Frame::Complete {
        payload,
        next: offset + trailer_end,
    })
}

