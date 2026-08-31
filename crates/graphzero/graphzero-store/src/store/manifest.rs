//! Snapshot manifest with atomic publish: write `.manifest.tmp`, fdatasync, rename to `.manifest`,
//! fsync the directory.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::format::MANIFEST_MAGIC;
use super::path_safety::{MAX_MANIFEST_SNAPSHOT_COUNT, MAX_MANIFEST_VEC_COUNT};

const MANIFEST_VERSION: u8 = 0x01;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub snapshot_id: u64,
    pub timestamp_nanos: i64,
    pub global_hash: u64,
    pub shard_hashes: Vec<u64>,
    pub segment_ids: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub snapshots: Vec<SnapshotEntry>,
}

impl SnapshotEntry {
    fn encode_into(&self, buf: &mut Vec<u8>) {
        let start = buf.len();
        buf.extend_from_slice(&self.snapshot_id.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_nanos.to_le_bytes());
        buf.extend_from_slice(&self.global_hash.to_le_bytes());
        buf.extend_from_slice(&(self.shard_hashes.len() as u32).to_le_bytes());
        for h in &self.shard_hashes {
            buf.extend_from_slice(&h.to_le_bytes());
        }
        buf.extend_from_slice(&(self.segment_ids.len() as u32).to_le_bytes());
        for id in &self.segment_ids {
            buf.extend_from_slice(&id.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf[start..]);
        buf.extend_from_slice(&crc.to_le_bytes());
    }

    fn decode(buf: &[u8], at: &mut usize) -> Result<Self> {
        let start = *at;
        let (snapshot_id, timestamp_nanos, global_hash) = decode_entry_header(buf, at)?;
        let shard_hashes = read_u64_vec(buf, at)?;
        let segment_ids = read_u64_vec(buf, at)?;
        let body_end = *at;
        let crc = read_u32(buf, at)?;
        if crc32fast::hash(&buf[start..body_end]) != crc {
            bail!("manifest snapshot entry CRC mismatch");
        }
        Ok(Self {
            snapshot_id,
            timestamp_nanos,
            global_hash,
            shard_hashes,
            segment_ids,
        })
    }
}

fn validate_manifest_version(version: u8) -> Result<()> {
    match version {
        MANIFEST_VERSION => Ok(()),
        0 => bail!("unsupported legacy manifest version 0; no migration path is defined"),
        future if future > MANIFEST_VERSION => bail!(
            "unsupported future manifest version {future}; maximum supported is {MANIFEST_VERSION}"
        ),
        other => bail!("unsupported manifest version {other}"),
    }
}

fn read_u32(buf: &[u8], at: &mut usize) -> Result<u32> {
    if *at + 4 > buf.len() {
        bail!("manifest truncated");
    }
    let v = u32::from_le_bytes(buf[*at..*at + 4].try_into().unwrap());
    *at += 4;
    Ok(v)
}

fn read_u64(buf: &[u8], at: &mut usize) -> Result<u64> {
    if *at + 8 > buf.len() {
        bail!("manifest truncated");
    }
    let v = u64::from_le_bytes(buf[*at..*at + 8].try_into().unwrap());
    *at += 8;
    Ok(v)
}

fn decode_entry_header(buf: &[u8], at: &mut usize) -> Result<(u64, i64, u64)> {
    let snapshot_id = read_u64(buf, at)?;
    let timestamp_nanos = read_u64(buf, at)? as i64;
    let global_hash = read_u64(buf, at)?;
    Ok((snapshot_id, timestamp_nanos, global_hash))
}

fn read_u64_vec(buf: &[u8], at: &mut usize) -> Result<Vec<u64>> {
    let count = read_u32(buf, at)? as usize;
    if count > MAX_MANIFEST_VEC_COUNT {
        bail!("manifest vector count {count} exceeds limit");
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_u64(buf, at)?);
    }
    Ok(out)
}

pub fn manifest_path(store_root: &Path) -> PathBuf {
    store_root.join(".manifest")
}

pub fn manifest_tmp_path(store_root: &Path) -> PathBuf {
    store_root.join(".manifest.tmp")
}

pub fn manifest_prev_path(store_root: &Path) -> PathBuf {
    store_root.join(".manifest.prev")
}

impl Manifest {
    pub fn latest(&self) -> Option<&SnapshotEntry> {
        self.snapshots.iter().max_by_key(|s| s.snapshot_id)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&MANIFEST_MAGIC);
        buf.push(MANIFEST_VERSION);
        buf.extend_from_slice(&(self.snapshots.len() as u32).to_le_bytes());
        for snap in &self.snapshots {
            snap.encode_into(&mut buf);
        }
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < 13 {
            bail!("manifest too small");
        }
        if buf[0..4] != MANIFEST_MAGIC {
            bail!("bad manifest magic");
        }
        validate_manifest_version(buf[4])?;
        let stored_crc = u32::from_le_bytes(buf[buf.len() - 4..].try_into().unwrap());
        if crc32fast::hash(&buf[..buf.len() - 4]) != stored_crc {
            bail!("manifest CRC mismatch");
        }
        let mut at = 5usize;
        let count = read_u32(buf, &mut at)? as usize;
        if count > MAX_MANIFEST_SNAPSHOT_COUNT {
            bail!("manifest snapshot count {count} exceeds limit");
        }
        let mut snapshots = Vec::with_capacity(count);
        for _ in 0..count {
            snapshots.push(SnapshotEntry::decode(buf, &mut at)?);
        }
        Ok(Self { snapshots })
    }

    /// Load the manifest after interrupted publication.
    /// Readers ignore `.manifest.tmp`, and writers truncate it.
    /// A corrupt `.manifest` falls back to `.manifest.prev`.
    pub fn load(store_root: &Path) -> Result<Self> {
        let path = manifest_path(store_root);
        match fs::read(&path) {
            Ok(buf) => match Self::decode(&buf) {
                Ok(m) => Ok(m),
                Err(_) => Self::load_prev(store_root),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("read manifest"),
        }
    }

    fn load_prev(store_root: &Path) -> Result<Self> {
        let prev = manifest_prev_path(store_root);
        let buf = fs::read(&prev).context("manifest corrupt and no .manifest.prev")?;
        Self::decode(&buf)
    }

    /// Atomic publish: tmp write + fdatasync + rename + dir fsync.
    pub fn atomic_publish(&self, store_root: &Path) -> Result<()> {
        fs::create_dir_all(store_root)?;
        let path = manifest_path(store_root);
        let tmp = manifest_tmp_path(store_root);
        // Keep the previous manifest as the recovery fallback.
        if path.exists() {
            fs::copy(&path, manifest_prev_path(store_root))?;
        }
        let bytes = self.encode();
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_data()?;
        drop(f);
        fs::rename(&tmp, &path)?;
        // fsync the directory so the rename itself is durable.
        if let Ok(dir) = File::open(store_root) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

/// Read raw manifest bytes (diagnostics).
pub fn read_manifest_bytes(store_root: &Path) -> Result<Vec<u8>> {
    let mut f = File::open(manifest_path(store_root))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}
