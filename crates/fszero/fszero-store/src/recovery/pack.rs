//! Append-only pack sidecar + locator encoding.
use fsqlite::Connection;
use std::path::Path;
use std::time::Instant;

/// Payloads at or above this size go to the append-only pack sidecar instead
/// of the sqlite btree. Large cells overflow into chained pages and every
/// insert churns the btree cursor's slot cache (profiled: 32% of a fresh
/// 208MB index inside CellSlotCache::insert_slow even after sorted, batched
/// writes). The pack file turns those into sequential appends; sqlite keeps
/// only a fixed-size locator row.
pub const PACK_MIN_BYTES: usize = 4096;
/// Row-format tags (first byte of `payloads.value`). Rows written before the
/// pack existed carry neither tag and are treated as legacy inline bytes.
pub const PAYLOAD_TAG_INLINE: u8 = 0x01;
pub const PAYLOAD_TAG_PACKED: u8 = 0x00;

/// Append-only blob sidecar. Locators live in SQLite; bytes live here.
/// Durable stores call `PackFile::sync_all` (via `path::full_sync_file`)
/// before the SQLite row that points at the new extent is committed, so a
/// crash cannot leave a locator at
/// short/zeroed bytes. An orphaned pack tail (append without a committed
/// locator) is harmless garbage. Deleted payloads leave holes; generation
/// compaction reclaims them.
pub struct PackFile {
    file: std::fs::File,
    pub len: u64,
}

impl PackFile {
    fn from_opened(file: std::fs::File) -> Option<Self> {
        let len = file.metadata().ok()?.len();
        Some(Self { file, len })
    }

    pub fn open(path: &Path) -> Option<Self> {
        Self::from_opened(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(path)
                .ok()?,
        )
    }

    pub fn open_existing(path: &Path) -> Option<Self> {
        Self::from_opened(
            std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(path)
                .ok()?,
        )
    }

    /// Create an empty generation without ever appending to a stale orphan.
    pub fn create_fresh(path: &Path) -> Option<Self> {
        Self::from_opened(
            std::fs::OpenOptions::new()
                .create_new(true)
                .append(true)
                .read(true)
                .open(path)
                .ok()?,
        )
    }

    /// Take exclusive pack flock (blocking).
    ///
    /// **Policy (fszero-pack-lock-unbounded-qgpd):** blocking forever is intentional.
    /// Multi-process durable writers serialize pack appends/rotations on this lock
    /// while already holding SQLite write order (SQLite → pack). A try_lock +
    /// deadline would force callers to invent a second busy class next to store
    /// busy_timeout; the pack generation must not tear mid-append. Wait cost is
    /// available via `runtime_metrics::lock_wait_snapshot` (wall of `File::lock`, including
    /// uncontended acquires so dual-writer off-CPU wait is visible as elevated us).
    pub fn lock_exclusive(&self) -> Result<(), String> {
        let t0 = Instant::now();
        std::fs::File::lock(&self.file).map_err(|e| format!("pack lock failed: {e}"))?;
        let us = t0.elapsed().as_micros() as u64;
        crate::runtime_metrics::record_pack_lock_wait(us);
        Ok(())
    }

    pub fn unlock(&self) {
        let _ = std::fs::File::unlock(&self.file);
    }

    pub fn current_len(&self) -> Result<u64, String> {
        self.file
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|e| format!("pack metadata failed: {e}"))
    }

    /// Caller holds the pack lock. Refreshing EOF here makes locators correct
    /// even when another process opened this generation first.
    pub fn append_locked(&mut self, data: &[u8]) -> Option<(u64, u32)> {
        use std::io::Write;
        let offset = self.file.metadata().ok()?.len();
        self.file.write_all(data).ok()?;
        self.len = offset.saturating_add(data.len() as u64);
        Some((offset, data.len() as u32))
    }

    pub fn append(&mut self, data: &[u8]) -> Option<(u64, u32)> {
        self.lock_exclusive().ok()?;
        let appended = self.append_locked(data);
        self.unlock();
        appended
    }

    /// Durability barrier: pack bytes + metadata must hit stable storage
    /// before any SQLite locator that references them is committed.
    /// Uses [`crate::path::full_sync_file`] (`sync_all` + macOS
    /// `F_FULLFSYNC`) so the barrier matches the absolute-durable class
    /// marketed in `docs/durability.md`. `sync_data` alone is insufficient.
    pub fn sync_all(&mut self) -> Result<(), String> {
        crate::path::full_sync_file(&self.file).map_err(|e| format!("pack sync_all failed: {e}"))
    }

    pub fn read(&self, offset: u64, len: u32) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len as usize];
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(&mut buf, offset).ok()?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut filled = 0;
            while filled < buf.len() {
                let read = self
                    .file
                    .seek_read(&mut buf[filled..], offset + filled as u64)
                    .ok()?;
                if read == 0 {
                    return None;
                }
                filled += read;
            }
        }
        Some(buf)
    }
}

/// Persist a newly-created generation's directory entry before SQLite can
/// publish locators that name it. Windows has no portable directory fsync;
/// the atomic file+SQLite ordering still applies there.
pub fn sync_parent_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| format!("pack path has no parent: {}", path.display()))?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| format!("pack parent sync failed for {}: {e}", parent.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn encode_packed_locator(offset: u64, len: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(13);
    v.push(PAYLOAD_TAG_PACKED);
    v.extend_from_slice(&offset.to_le_bytes());
    v.extend_from_slice(&len.to_le_bytes());
    v
}

pub fn decode_packed_locator(row: &[u8]) -> Option<(u64, u32)> {
    if row.len() != 13 || row[0] != PAYLOAD_TAG_PACKED {
        return None;
    }
    let offset = u64::from_le_bytes(row[1..9].try_into().ok()?);
    let len = u32::from_le_bytes(row[9..13].try_into().ok()?);
    Some((offset, len))
}

pub fn pack_path_for_db(db_path: &Path) -> std::path::PathBuf {
    let mut os = db_path.as_os_str().to_owned();
    os.push(".pack");
    std::path::PathBuf::from(os)
}

/// Pack GC (fszero-qzt): compaction writes a NEW pack generation and
/// switches to it atomically inside the locator-update txn (meta key
/// `pack_gen`). Gen 0 is the legacy unsuffixed file.
pub fn pack_gen_path(db_path: &Path, generation: i64) -> std::path::PathBuf {
    let base = pack_path_for_db(db_path);
    if generation <= 0 {
        return base;
    }
    let mut os = base.into_os_string();
    os.push(format!(".g{generation}"));
    std::path::PathBuf::from(os)
}

pub fn load_pack_gen(conn: &Connection) -> i64 {
    super::meta_i64(conn, "pack_gen").unwrap_or(0)
}
