//! Append-only delta log: 64MB segments, CRC32c per segment, fdatasync on
//! commit (FR-013). Segment files live under `.graphzero/wal/seg_<id>.log`.
//!
//! Segment layout (little-endian):
//! ```text
//! 0x00 magic        [u8; 4] = "GZDL"
//! 0x04 version      u8      = 0x01
//! 0x05 reserved     [u8;11]
//! 0x10 segment_id   u64
//! 0x18 prev_crc     u32     (CRC32c of previous segment's entries; 0 for first)
//! 0x1C entry_count  u32
//! 0x20 segment_crc  u32     (CRC32c over all entry bytes)
//! 0x24 entries...
//! ```
//! Entry: type u8, blob_hash [u8;32], payload_len u32, payload bytes.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::format::DELTA_MAGIC;
use super::path_safety::{MAX_DELTA_SEGMENT_ENTRIES, file_name_to_str};

pub const SEGMENT_MAX_SIZE: u64 = 64_000_000;
pub const SEGMENT_HEADER_LEN: usize = 0x24;

pub mod entry_type {
    pub const BLOB: u8 = 0;
    pub const SYMBOL: u8 = 1;
    pub const EDGE: u8 = 2;
    pub const TRIGRAM: u8 = 3;
    pub const COVERAGE: u8 = 4;
    /// P5.2 intent reservation audit record
    pub const RESERVATION: u8 = 5;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaEntry {
    pub entry_type: u8,
    pub blob_hash: [u8; 32],
    pub payload: Vec<u8>,
}

impl DeltaEntry {
    fn encoded_len(&self) -> u64 {
        1 + 32 + 4 + self.payload.len() as u64
    }

    fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.entry_type);
        buf.extend_from_slice(&self.blob_hash);
        buf.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.payload);
    }
}

struct OpenSegment {
    id: u64,
    file: Option<File>,
    entry_bytes: Vec<u8>,
    entry_count: u32,
    prev_crc: u32,
}

/// Append-only writer. `append` buffers; `commit` durability-seals the open
/// segment (header + CRC + fdatasync) but **keeps** it open so later
/// appends reuse the same file until [`SEGMENT_MAX_SIZE`] (graphzero-8jxvg).
/// A new `seg_*.log` is only created when the open segment would overflow or
/// after an explicit roll.
pub struct DeltaLog {
    dir: PathBuf,
    current: Option<OpenSegment>,
    last_crc: u32,
}

fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("seg_{id:08}.log"))
}

impl DeltaLog {
    pub fn open(store_root: &Path) -> Result<Self> {
        Self::open_dir(&store_root.join("wal"))
    }

    /// Open a delta log rooted at an explicit segment directory (worktree
    /// overlay layers use per-worktree wal dirs).
    pub fn open_dir(dir: &Path) -> Result<Self> {
        let dir = dir.to_path_buf();
        fs::create_dir_all(&dir)?;
        let segments = read_segment_chain(&dir)?;
        let last_crc = segments.last().map_or(0, |segment| segment.crc);
        Ok(Self {
            dir,
            current: None,
            last_crc,
        })
    }

    pub fn segment_ids(dir: &Path) -> Result<Vec<u64>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name_to_str(&file_name, "delta log segment scan")?;
            if let Some(num) = name
                .strip_prefix("seg_")
                .and_then(|s| s.strip_suffix(".log"))
                && let Ok(id) = num.parse::<u64>()
            {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    pub fn wal_dir(&self) -> &Path {
        &self.dir
    }

    fn next_segment_id(&self) -> Result<u64> {
        Ok(Self::segment_ids(&self.dir)?.last().map_or(0, |id| id + 1))
    }

    fn roll_segment(&mut self) -> Result<()> {
        // Durability-seal then drop the open segment so the next id starts clean.
        self.commit()?;
        if let Some(seg) = self.current.take() {
            if seg.entry_count == 0 {
                let path = segment_path(&self.dir, seg.id);
                drop(seg.file);
                if path.exists() && fs::metadata(&path)?.len() == 0 {
                    fs::remove_file(&path).with_context(|| {
                        format!("remove empty delta segment {}", path.display())
                    })?;
                }
            }
            // Non-empty: file already sealed by commit; drop handle only.
        }
        let id = self.next_segment_id()?;
        let path = segment_path(&self.dir, id);
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("create segment {}", path.display()))?;
        self.current = Some(OpenSegment {
            id,
            file: Some(file),
            entry_bytes: Vec::new(),
            entry_count: 0,
            prev_crc: self.last_crc,
        });
        Ok(())
    }

    pub fn append(&mut self, entry: DeltaEntry) -> Result<()> {
        let need_roll = match &self.current {
            None => true,
            Some(seg) => {
                SEGMENT_HEADER_LEN as u64 + seg.entry_bytes.len() as u64 + entry.encoded_len()
                    > SEGMENT_MAX_SIZE
            }
        };
        if need_roll {
            self.roll_segment()?;
        }
        let seg = self.current.as_mut().expect("segment after roll");
        entry.encode_into(&mut seg.entry_bytes);
        seg.entry_count += 1;
        Ok(())
    }

    /// Durability-seal the open segment: rewrite header + entries, CRC32c,
    /// fdatasync. Keeps the segment open so subsequent `append`s reuse the same
    /// file under [`SEGMENT_MAX_SIZE`] (graphzero-8jxvg). Empty open segments
    /// (rolled, no entries) are removed so they never become WAL barriers.
    pub fn commit(&mut self) -> Result<()> {
        let Some(seg) = self.current.as_mut() else {
            return Ok(());
        };
        if seg.entry_count == 0 {
            let path = segment_path(&self.dir, seg.id);
            // Drop empty rolled file; clear current so next append rolls cleanly.
            let id = seg.id;
            let empty = path.exists() && fs::metadata(&path)?.len() == 0;
            // Take only when empty so we can delete without holding the FD.
            if empty {
                let seg = self.current.take().expect("current present");
                drop(seg.file);
                fs::remove_file(&path)
                    .with_context(|| format!("remove empty delta segment {}", path.display()))?;
                let _ = id;
            }
            return Ok(());
        }
        let crc = crc32fast::hash(&seg.entry_bytes);
        let mut header = Vec::with_capacity(SEGMENT_HEADER_LEN);
        header.extend_from_slice(&DELTA_MAGIC);
        header.push(0x01);
        header.extend_from_slice(&[0u8; 11]);
        header.extend_from_slice(&seg.id.to_le_bytes());
        header.extend_from_slice(&seg.prev_crc.to_le_bytes());
        header.extend_from_slice(&seg.entry_count.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        let mut bytes = header;
        bytes.extend_from_slice(&seg.entry_bytes);
        let path = segment_path(&self.dir, seg.id);
        drop(seg.file.take());
        super::atomic_write_file(&path, &bytes)
            .with_context(|| format!("durably publish delta segment {}", path.display()))?;
        seg.file = Some(
            OpenOptions::new()
                .write(true)
                .open(&path)
                .with_context(|| format!("reopen delta segment {}", path.display()))?,
        );
        self.last_crc = crc;
        // Keep `current` so the next append reuses this segment id.
        Ok(())
    }
}

fn is_empty_segment_file(path: &Path) -> Result<bool> {
    Ok(fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len()
        == 0)
}

fn parse_segment_header(buf: &[u8], path: &Path) -> Result<(u64, u32, u32, u32)> {
    if buf.len() < SEGMENT_HEADER_LEN {
        bail!("delta segment truncated: {} bytes", buf.len());
    }
    if buf[0..4] != DELTA_MAGIC {
        bail!("bad delta segment magic");
    }
    if buf[4] != 0x01 {
        bail!("unsupported delta segment version {}", buf[4]);
    }
    let segment_id = u64::from_le_bytes(buf[0x10..0x18].try_into().unwrap());
    let prev_crc = u32::from_le_bytes(buf[0x18..0x1C].try_into().unwrap());
    let entry_count = u32::from_le_bytes(buf[0x1C..0x20].try_into().unwrap());
    if entry_count > MAX_DELTA_SEGMENT_ENTRIES {
        bail!(
            "delta segment entry count {entry_count} exceeds limit {}",
            MAX_DELTA_SEGMENT_ENTRIES
        );
    }
    let stored_crc = u32::from_le_bytes(buf[0x20..0x24].try_into().unwrap());
    let entries_bytes = &buf[SEGMENT_HEADER_LEN..];
    if crc32fast::hash(entries_bytes) != stored_crc {
        bail!("delta segment CRC mismatch: {}", path.display());
    }
    Ok((segment_id, prev_crc, entry_count, stored_crc))
}

fn decode_delta_entries(entries_bytes: &[u8], entry_count: u32) -> Result<Vec<DeltaEntry>> {
    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut at = 0usize;
    for _ in 0..entry_count {
        if at + 37 > entries_bytes.len() {
            bail!("delta segment truncated mid-entry");
        }
        let entry_type = entries_bytes[at];
        let mut blob_hash = [0u8; 32];
        blob_hash.copy_from_slice(&entries_bytes[at + 1..at + 33]);
        let len = u32::from_le_bytes(entries_bytes[at + 33..at + 37].try_into().unwrap()) as usize;
        if at + 37 + len > entries_bytes.len() {
            bail!("delta segment payload truncated");
        }
        entries.push(DeltaEntry {
            entry_type,
            blob_hash,
            payload: entries_bytes[at + 37..at + 37 + len].to_vec(),
        });
        at += 37 + len;
    }
    if at != entries_bytes.len() {
        bail!("delta segment has trailing or reordered entry bytes");
    }
    Ok(entries)
}

struct ValidatedSegment {
    id: u64,
    prev_crc: u32,
    crc: u32,
    entries: Vec<DeltaEntry>,
}

fn read_validated_segment(path: &Path) -> Result<ValidatedSegment> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let (id, prev_crc, entry_count, crc) = parse_segment_header(&buf, path)?;
    let entries = decode_delta_entries(&buf[SEGMENT_HEADER_LEN..], entry_count)?;
    Ok(ValidatedSegment {
        id,
        prev_crc,
        crc,
        entries,
    })
}

fn read_segment_chain(wal_dir: &Path) -> Result<Vec<ValidatedSegment>> {
    let mut segments: Vec<ValidatedSegment> = Vec::new();
    for expected_id in DeltaLog::segment_ids(wal_dir)? {
        let path = segment_path(wal_dir, expected_id);
        if is_empty_segment_file(&path)? {
            continue;
        }
        let segment = read_validated_segment(&path)?;
        if segment.id != expected_id {
            bail!(
                "delta segment id/path mismatch: header {} path {}",
                segment.id,
                expected_id
            );
        }
        if let Some(previous) = segments.last()
            && segment.prev_crc != previous.crc
        {
            bail!(
                "delta segment prev_crc mismatch: segment {} expected {} got {}",
                segment.id,
                previous.crc,
                segment.prev_crc
            );
        }
        segments.push(segment);
    }
    Ok(segments)
}

/// Read and validate one segment; returns (entries, segment_crc).
/// Truncated, trailing, corrupt, or path-reordered segments fail loudly.
pub fn read_segment(path: &Path) -> Result<(Vec<DeltaEntry>, u32)> {
    let segment = read_validated_segment(path)?;
    if let Some(name) = path.file_name().and_then(|name| name.to_str())
        && let Some(expected_id) = name
            .strip_prefix("seg_")
            .and_then(|value| value.strip_suffix(".log"))
            .and_then(|value| value.parse::<u64>().ok())
        && segment.id != expected_id
    {
        bail!(
            "delta segment id/path mismatch: header {} path {}",
            segment.id,
            expected_id
        );
    }
    Ok((segment.entries, segment.crc))
}

/// Read all committed segments in id order and validate their CRC chain.
/// Any truncation, corruption, path reordering, or `prev_crc` mismatch fails.
pub fn read_all_segments(wal_dir: &Path) -> Result<Vec<(u64, Vec<DeltaEntry>)>> {
    read_segment_chain(wal_dir).map(|segments| {
        segments
            .into_iter()
            .map(|segment| (segment.id, segment.entries))
            .collect()
    })
}
