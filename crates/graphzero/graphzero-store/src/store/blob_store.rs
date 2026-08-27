//! Content-addressed blob store under the project store root.
//!
//! **Write layout (graphzero-56s1t):** new puts materialize a single physical
//! object in the ZeroRef fan-out path
//! `blobs/sha256/<hh>/<64-hex>` (via [`super::shared_cas::SharedCas`]). The
//! legacy flat `blobs/<64-hex>` path is no longer dual-written (eliminates
//! ~2x write amplification and single-dir densification pressure).
//!
//! **Read layout:** exact lookups try legacy flat first (old stores), then
//! cas-local fan-out. Prefix scans cover both namespaces.
//!
//! Also owns the sha256 -> git OID secondary-key map used by expand fallback.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossbeam_queue::SegQueue;
use rayon::prelude::*;

use crate::ContentHash;

use super::path_safety::{file_name_to_str, validate_blob_hash_component};

/// Stored bytes did not hash to the requested identity. Typed so the expand
/// fallback chain can tell corruption (terminal, INV-001) apart from a plain
/// miss instead of collapsing both into an untyped read error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobDigestMismatch {
    pub expected: String,
    pub actual: String,
    pub path: PathBuf,
}

impl std::fmt::Display for BlobDigestMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "blob {} at {} failed content verification (got {})",
            &self.expected[..8.min(self.expected.len())],
            self.path.display(),
            &self.actual[..8.min(self.actual.len())]
        )
    }
}

impl std::error::Error for BlobDigestMismatch {}

/// One blob path awaiting the [`BlobStore::sync_all`] durability barrier.
struct PendingFsync {
    path: PathBuf,
    /// Write handle retained across rename so the barrier can `sync_data`
    /// without re-opening the final path (graphzero-3olfy). `None` when only
    /// the path is known (e.g. SharedCas dual-write pending).
    file: Option<fs::File>,
}

pub struct BlobStore {
    root: PathBuf,
    /// Final blob paths written by [`Self::put_nosync`] (cas-local fan-out)
    /// that still need per-file `sync_data` before a durable publish (INV-DUR-1).
    ///
    /// Lock-free queue so rayon `put_nosync` paths never take a process-wide
    /// Mutex on every new blob (graphzero-bn9e3). Drained once in [`Self::sync_all`].
    pending_fsync: SegQueue<PendingFsync>,
}

impl BlobStore {
    /// `root` is the `.graphzero` directory.
    pub fn open(root: &Path) -> Result<Self> {
        let blobs = root.join("blobs");
        fs::create_dir_all(&blobs)?;
        Ok(Self {
            root: root.to_path_buf(),
            pending_fsync: SegQueue::new(),
        })
    }

    /// Legacy flat path `blobs/<64-hex>` (read-only for pre-migration stores).
    fn blob_path(&self, hash_hex: &str) -> Result<PathBuf> {
        validate_blob_hash_component(hash_hex, "blob store lookup")?;
        Ok(self.root.join("blobs").join(hash_hex))
    }

    /// Canonical fan-out path `blobs/sha256/<hh>/<64-hex>` (write target).
    fn cas_path(&self, hash_hex: &str) -> Result<PathBuf> {
        validate_blob_hash_component(hash_hex, "blob store cas lookup")?;
        Ok(
            super::shared_cas::SharedCas::open_labeled(&self.root, "cas-local")
                .object_path(hash_hex),
        )
    }

    /// Store bytes by content sha256; returns the hash. Idempotent.
    pub fn put(&self, data: &[u8]) -> Result<ContentHash> {
        self.put_with_sync(ContentHash::of(data), data, true)
    }

    /// Store bytes without per-blob fdatasync. Callers must call [`Self::sync_all`]
    /// before publishing a durable snapshot.
    pub fn put_nosync(&self, data: &[u8]) -> Result<ContentHash> {
        self.put_with_sync(ContentHash::of(data), data, false)
    }

    /// Like [`Self::put_nosync`], with a typed caller-supplied content hash.
    ///
    /// `ContentHash` excludes malformed raw digest strings, but does not prove
    /// correspondence to `data`. Re-hash before any final-path or shared-CAS
    /// mutation so an untrusted caller cannot publish bytes under a false key.
    pub fn put_nosync_prehashed(&self, hash: ContentHash, data: &[u8]) -> Result<ContentHash> {
        let actual = ContentHash::of(data);
        if actual != hash {
            let expected = hash.to_hex();
            return Err(BlobDigestMismatch {
                path: self.blob_path(&expected)?,
                expected,
                actual: actual.to_hex(),
            }
            .into());
        }
        self.put_with_sync(hash, data, false)
    }

    fn put_with_sync(&self, hash: ContentHash, data: &[u8], sync: bool) -> Result<ContentHash> {
        // Single physical write into cas-local fan-out (graphzero-56s1t).
        // Legacy flat `blobs/<hash>` is not written; get falls back for old stores.
        //
        // Honor the caller's sync flag: put_nosync must not pay per-object
        // SharedCas sync_all + dir fsync (graphzero-o1td4). Unsynced CAS
        // paths join `pending_fsync` and drain in [`Self::sync_all`].
        let cas = super::shared_cas::SharedCas::open_labeled(&self.root, "cas-local");
        // The no-sync batch extension trusts this pre-verified ContentHash and
        // avoids rehashing (graphzero-qf9y5). Canonical synced publish verifies it.
        if sync {
            cas.put_prehashed(hash, data)
                .map_err(|e| anyhow::anyhow!("cas-local blob put failed: {e}"))?;
        } else {
            let (_hash, cas_pending) = cas
                .put_nosync_prehashed(hash, data)
                .map_err(|e| anyhow::anyhow!("cas-local blob put failed: {e}"))?;
            if let Some(cas_path) = cas_pending {
                self.pending_fsync.push(PendingFsync {
                    path: cas_path,
                    file: None,
                });
            }
        }
        // Bulk indexer uses nosync puts; recording every blob into the global
        // NDJSON ref-index turns cold index into a shard-growth/CPU storm and
        // makes later expands scan fat shards. Cross-root mint still uses
        // sync `put()` (and explicit record sites).
        if sync {
            super::ref_index::record_ref(&format!("gz://blob/{}", hash.to_hex()), &self.root)?;
        }
        Ok(hash)
    }

    /// Durability barrier for relaxed blob puts (INV-DUR-1).
    ///
    /// Fsyncs each pending blob file's data, then fsyncs the `blobs/` directory
    /// so both file contents and directory entries are durable. Directory-only
    /// sync is insufficient on APFS/macOS (and other platforms) for guaranteeing
    /// blob bytes survive a crash before manifest publish.
    pub fn sync_all(&self) -> Result<()> {
        // Drain the lock-free queue into a local Vec for parallel fsync.
        // Concurrent put_nosync during sync_all is not a supported publish
        // pattern; items pushed after drain will remain for a later barrier.
        let mut pending = Vec::new();
        while let Some(item) = self.pending_fsync.pop() {
            pending.push(item);
        }
        pending.par_iter().try_for_each(|item| -> Result<()> {
            if let Some(file) = item.file.as_ref() {
                file.sync_data()
                    .with_context(|| format!("fsync blob {}", item.path.display()))?;
                return Ok(());
            }
            match fs::OpenOptions::new().write(true).open(&item.path) {
                Ok(file) => file.sync_data()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    return Err(error).context(format!("fsync blob {}", item.path.display()));
                }
            }
            Ok(())
        })?;
        // std::fs::File cannot open directory handles on Windows without
        // FILE_FLAG_BACKUP_SEMANTICS. Blob files were flushed above; skip the
        // unsupported directory metadata barrier on that platform.
        //
        // Barrier covers cas-local fan-out parents (and any legacy flat paths
        // still queued from older dual-write code paths).
        #[cfg(not(windows))]
        {
            use std::collections::HashSet;
            let mut dirs: HashSet<PathBuf> = HashSet::new();
            dirs.insert(self.root.join("blobs"));
            for item in &pending {
                if let Some(parent) = item.path.parent() {
                    dirs.insert(parent.to_path_buf());
                }
            }
            for dir in dirs {
                if dir.is_dir() {
                    let handle = fs::File::open(&dir)
                        .with_context(|| format!("open blob dir {}", dir.display()))?;
                    handle.sync_all()?;
                }
            }
        }
        Ok(())
    }

    /// Number of blob paths still awaiting the [`Self::sync_all`] barrier.
    pub fn pending_unsynced_count(&self) -> usize {
        self.pending_fsync.len()
    }

    /// Exact-hash lookup.
    pub fn get(&self, hash: &ContentHash) -> Result<Option<Vec<u8>>> {
        self.get_hex(&hash.to_hex())
    }

    /// Lookup by full or prefix hex hash. Prefix must be unambiguous.
    pub fn get_hex(&self, hash_hex: &str) -> Result<Option<Vec<u8>>> {
        if hash_hex.len() == 64 {
            // Legacy flat first (pre-56s1t stores), then cas-local fan-out.
            if let Some(data) = self.read_blob_at_path(&self.blob_path(hash_hex)?, hash_hex)? {
                return Ok(Some(data));
            }
            return self.read_blob_at_path(&self.cas_path(hash_hex)?, hash_hex);
        }
        validate_blob_hash_component(hash_hex, "blob store prefix lookup")?;
        self.get_hex_by_prefix(hash_hex)
    }

    fn read_blob_at_path(&self, path: &Path, expected_hex: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(data) => {
                let actual = ContentHash::of(&data).to_hex();
                if actual != expected_hex {
                    return Err(BlobDigestMismatch {
                        expected: expected_hex.to_string(),
                        actual,
                        path: path.to_path_buf(),
                    }
                    .into());
                }
                Ok(Some(data))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn get_hex_by_prefix(&self, hash_hex: &str) -> Result<Option<Vec<u8>>> {
        let mut found: Option<(PathBuf, String)> = None;
        let mut consider = |path: PathBuf, name: String| -> Result<()> {
            if !name.starts_with(hash_hex) || name.len() != 64 {
                return Ok(());
            }
            anyhow::ensure!(found.is_none(), "ambiguous blob hash prefix: {hash_hex}");
            found = Some((path, name));
            Ok(())
        };

        // Legacy flat siblings under blobs/.
        let blobs = self.root.join("blobs");
        if blobs.is_dir() {
            for entry in fs::read_dir(&blobs)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let name = file_name_to_str(&file_name, "blob store prefix lookup")?;
                // Skip fan-out control dirs and non-hash names (sha256/, oidmap, temps).
                if name.len() != 64 {
                    continue;
                }
                consider(entry.path(), name.to_string())?;
            }
        }

        // Fan-out: blobs/sha256/<hh>/<hash> — only hh dirs that can match prefix.
        let fanout_root = blobs.join("sha256");
        if fanout_root.is_dir() {
            for entry in fs::read_dir(&fanout_root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let hh = entry.file_name();
                let hh = file_name_to_str(&hh, "blob store fan-out bucket")?;
                if hh.len() != 2 {
                    continue;
                }
                // When prefix length >= 2, only scan the matching fan-out bucket.
                if hash_hex.len() >= 2 && &hash_hex[..2] != hh {
                    continue;
                }
                for obj in fs::read_dir(entry.path())? {
                    let obj = obj?;
                    let file_name = obj.file_name();
                    let name = file_name_to_str(&file_name, "blob store fan-out object")?;
                    if name.len() != 64 {
                        continue;
                    }
                    consider(obj.path(), name.to_string())?;
                }
            }
        }

        match found {
            Some((path, name)) => self.read_blob_at_path(&path, &name),
            None => Ok(None),
        }
    }

    /// Record the git OID secondary key for a content hash.
    pub fn record_git_oid(&self, hash: &ContentHash, git_oid: &str) -> Result<()> {
        let path = self.root.join("blobs").join("oidmap");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{} {}", hash.to_hex(), git_oid)?;
        Ok(())
    }

    /// Look up the git OID for a content hash (or prefix).
    pub fn git_oid_for(&self, hash_hex: &str) -> Result<Option<String>> {
        let path = self.root.join("blobs").join("oidmap");
        let data = match fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("read oidmap"),
        };
        for line in data.lines() {
            if let Some((sha, oid)) = line.split_once(' ')
                && sha.starts_with(hash_hex)
            {
                return Ok(Some(oid.to_string()));
            }
        }
        Ok(None)
    }
}
