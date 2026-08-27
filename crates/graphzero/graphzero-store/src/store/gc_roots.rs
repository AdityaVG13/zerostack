//! Shared-CAS GC reachability-root publisher for GraphZero (zerostack.cas-gc.legacy).
//!
//! Implements the producer side of the frozen multi-engine GC contract under
//! `~/AI/tokenzero/schemas/shared-cas-gc/v1`. GraphZero writes only its own
//! namespace: `<store-root>/gc/roots/graphzero/<project_id>/current.json`.
//! The record lists every blob hash GraphZero currently considers live, so an
//! independent coordinator can collect unreachable shared-CAS objects without
//! deleting blobs retained by graph facts, snapshots, refs, active queries, or
//! legacy stores.
//!
//! Safety invariant: when in doubt, retain. This module never deletes; it
//! only publishes roots. Enumerating too many hashes is conservative; missing
//! one is a bug.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::path_safety::{absolute_path, file_name_to_str};
use super::refs::GzRef;
use crate::fast_hex;

/// Frozen schema version for all shared-CAS GC records.
pub const GC_SCHEMA_VERSION: &str = "zerostack.cas-gc.legacy";

/// Engine namespace for GraphZero.
pub const GC_ENGINE: &str = "graphzero";

/// Record type for a reachability snapshot.
pub const RECORD_TYPE_REACHABILITY: &str = "reachability-snapshot";

/// Record type for a pin record.
pub const RECORD_TYPE_PIN: &str = "pin";

/// A reachability snapshot: the complete live blob-root set for one
/// GraphZero project namespace at a monotonically increasing epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachabilitySnapshot {
    pub schema_version: String,
    pub record_type: String,
    pub engine: String,
    pub project_id: String,
    pub epoch: u64,
    pub published_at: String,
    pub blob_hashes: Vec<String>,
}

/// A pin record: protects one blob independently of reachability snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinRecord {
    pub schema_version: String,
    pub record_type: String,
    pub engine: String,
    pub project_id: String,
    pub pin_id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub blob_hash: String,
}

impl ReachabilitySnapshot {
    /// Build a new reachability snapshot. `blob_hashes` is sorted and deduplicated.
    pub fn new(project_id: String, epoch: u64, mut blob_hashes: Vec<String>) -> Self {
        blob_hashes.sort_unstable();
        blob_hashes.dedup();
        Self {
            schema_version: GC_SCHEMA_VERSION.to_string(),
            record_type: RECORD_TYPE_REACHABILITY.to_string(),
            engine: GC_ENGINE.to_string(),
            project_id,
            epoch,
            published_at: rfc3339_now(),
            blob_hashes,
        }
    }
}

impl PinRecord {
    /// Build a new pin record.
    pub fn new(
        project_id: String,
        pin_id: String,
        blob_hash: String,
        expires_at: Option<String>,
    ) -> Self {
        Self {
            schema_version: GC_SCHEMA_VERSION.to_string(),
            record_type: RECORD_TYPE_PIN.to_string(),
            engine: GC_ENGINE.to_string(),
            project_id,
            pin_id,
            created_at: rfc3339_now(),
            expires_at,
            blob_hash,
        }
    }
}

/// Stable project identity: full lowercase SHA-256 of the canonicalized store root.
///
/// The contract requires a 64-hex project_id. GraphZero derives it from the
/// canonicalized absolute store-root path so the same project always gets the
/// same id, while distinct store roots never collide.
pub fn project_id(store_root: &Path) -> String {
    let canonical = absolute_path(store_root).to_string_lossy().into_owned();
    fast_hex(&Sha256::digest(canonical.as_bytes()))
}

/// Canonical path for the current reachability snapshot.
pub fn reachability_snapshot_path(store_root: &Path, project_id: &str) -> PathBuf {
    store_root
        .join("gc")
        .join("roots")
        .join(GC_ENGINE)
        .join(project_id)
        .join("current.json")
}

/// Canonical path for a pin record.
pub fn pin_record_path(store_root: &Path, project_id: &str, pin_id: &str) -> PathBuf {
    store_root
        .join("gc")
        .join("pins")
        .join(GC_ENGINE)
        .join(project_id)
        .join(format!("{pin_id}.json"))
}

/// Read the current reachability snapshot, if one exists and is valid.
pub fn read_reachability_snapshot(store_root: &Path) -> Result<Option<ReachabilitySnapshot>> {
    let path = reachability_snapshot_path(store_root, &project_id(store_root));
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read reachability snapshot {}", path.display()))?;
    let snap: ReachabilitySnapshot = serde_json::from_str(&text)
        .with_context(|| format!("parse reachability snapshot {}", path.display()))?;
    Ok(Some(snap))
}

/// Read the current reachability snapshot regardless of namespace path.
pub fn read_reachability_snapshot_at(path: &Path) -> Result<ReachabilitySnapshot> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read reachability snapshot {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parse reachability snapshot {}", path.display()))
}

/// Exclusive publisher lock for one GC namespace directory.
///
/// The GC record is a read-modify-write (read current epoch, increment,
/// rename over `current.json`), so atomic rename alone is not enough: two
/// unsynchronised publishers can read the same epoch and the later rename
/// silently discards the other's blob set. This lock is scoped to the
/// namespace directory, not the store writer lock, so publishing roots never
/// blocks indexing.
struct GcNamespaceLock {
    file: fs::File,
}

impl GcNamespaceLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(".publish.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("open GC publisher lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("acquire GC publisher lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for GcNamespaceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Publish a reachability snapshot atomically on the same filesystem.
///
/// Protocol: take the namespace publisher lock, refuse any epoch that does not
/// strictly exceed the currently published one, then write a uniquely named
/// sibling temp file, flush it, and rename it over `current.json`. Never
/// modifies `current.json` in place.
///
/// Strict monotonicity is a safety property, not bookkeeping: a snapshot that
/// declared a blob set reachable must never be replaced by an older-epoch
/// snapshot that omits it, or a sweeper on the shared root becomes entitled to
/// collect still-live objects.
pub fn publish_reachability_snapshot(
    store_root: &Path,
    epoch: u64,
    blob_hashes: &[String],
) -> Result<PathBuf> {
    let pid = project_id(store_root);
    let dir = store_root
        .join("gc")
        .join("roots")
        .join(GC_ENGINE)
        .join(&pid);
    fs::create_dir_all(&dir)
        .with_context(|| format!("create reachability snapshot dir {}", dir.display()))?;

    let _lock = GcNamespaceLock::acquire(&dir)?;
    publish_reachability_snapshot_locked(&dir, &pid, epoch, blob_hashes)
}

/// Snapshot-publish body; the caller must already hold the namespace lock for
/// `dir`. `flock` blocks per-fd even within one process, so re-acquiring here
/// would self-deadlock.
fn publish_reachability_snapshot_locked(
    dir: &Path,
    pid: &str,
    epoch: u64,
    blob_hashes: &[String],
) -> Result<PathBuf> {
    let dest = dir.join("current.json");
    if dest.exists() {
        let current = read_reachability_snapshot_at(&dest)?;
        anyhow::ensure!(
            epoch > current.epoch,
            "refusing non-monotonic reachability snapshot for {pid}: epoch {epoch} does not exceed published epoch {}",
            current.epoch
        );
    }

    let snapshot = ReachabilitySnapshot::new(pid.to_string(), epoch, blob_hashes.to_vec());
    let text =
        serde_json::to_string_pretty(&snapshot).context("serialize reachability snapshot")?;

    let tmp = dir.join(format!(".current.{}.tmp", process_nonce()));
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create temp snapshot {}", tmp.display()))?;
        f.write_all(text.as_bytes())
            .with_context(|| format!("write temp snapshot {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("sync temp snapshot {}", tmp.display()))?;
    }
    fs::rename(&tmp, &dest)
        .with_context(|| format!("publish snapshot {} -> {}", tmp.display(), dest.display()))?;
    // Best-effort directory sync for durability.
    let _ = sync_dir(&dir);
    Ok(dest)
}

/// Publish a pin record atomically on the same filesystem.
pub fn publish_pin_record(store_root: &Path, pin: &PinRecord) -> Result<PathBuf> {
    let dir = store_root
        .join("gc")
        .join("pins")
        .join(GC_ENGINE)
        .join(&pin.project_id);
    fs::create_dir_all(&dir).with_context(|| format!("create pin dir {}", dir.display()))?;
    let text = serde_json::to_string_pretty(pin).context("serialize pin record")?;
    let dest = dir.join(format!("{}.json", pin.pin_id));
    let tmp = dest.with_extension(format!("{}.tmp", process_nonce()));
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create temp pin {}", tmp.display()))?;
        f.write_all(text.as_bytes())
            .with_context(|| format!("write temp pin {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("sync temp pin {}", tmp.display()))?;
    }
    fs::rename(&tmp, &dest)
        .with_context(|| format!("publish pin {} -> {}", tmp.display(), dest.display()))?;
    let _ = sync_dir(&dir);
    Ok(dest)
}

/// Enumerate every blob hash GraphZero considers live at `store_root`.
///
/// Sources:
///   - the canonical shared-CAS object directory (`blobs/sha256/...`)
///   - the legacy local BlobStore (`blobs/`, flat 64-hex files)
///   - the records sidecar (`records_latest.jsonl`)
///   - the paths sidecar for every published snapshot (`shards/paths_*.txt`)
///   - the per-user ref-index entries that point back at this store_root
///     and name `gz://blob/<hash>` refs.
///
/// This is intentionally conservative: it may include hashes that are no
/// longer referenced by current graph facts, but it must never drop a hash
/// that is.
pub fn enumerate_live_blob_hashes(store_root: &Path) -> Result<BTreeSet<String>> {
    let mut hashes = BTreeSet::new();
    collect_cas_object_hashes(store_root, &mut hashes)?;
    collect_legacy_blob_hashes(store_root, &mut hashes)?;
    collect_records_sidecar_hashes(store_root, &mut hashes)?;
    collect_paths_sidecar_hashes(store_root, &mut hashes)?;
    collect_ref_index_hashes(store_root, &mut hashes)?;
    Ok(hashes)
}

/// Publish a new reachability snapshot whose epoch is one greater than the
/// current snapshot, or `1` if none exists.
pub fn publish_live_roots(store_root: &Path) -> Result<ReachabilitySnapshot> {
    let pid = project_id(store_root);
    let dir = store_root
        .join("gc")
        .join("roots")
        .join(GC_ENGINE)
        .join(&pid);
    fs::create_dir_all(&dir)
        .with_context(|| format!("create reachability snapshot dir {}", dir.display()))?;
    // Hold the namespace lock across read-epoch -> publish so two concurrent
    // publishers cannot read the same epoch and have the loser's blob set
    // silently dropped by the winning rename.
    let _lock = GcNamespaceLock::acquire(&dir)?;

    let prior = read_reachability_snapshot(store_root)?;
    let epoch = prior.map(|s| s.epoch.saturating_add(1)).unwrap_or(1);
    let hashes: Vec<String> = enumerate_live_blob_hashes(store_root)?
        .into_iter()
        .collect();
    publish_reachability_snapshot_locked(&dir, &pid, epoch, &hashes)?;
    Ok(ReachabilitySnapshot::new(pid, epoch, hashes))
}

fn collect_cas_object_hashes(store_root: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let cas = store_root.join("blobs").join("sha256");
    if !cas.is_dir() {
        return Ok(());
    }
    for fanout in fs::read_dir(&cas)
        .with_context(|| format!("read CAS fan-out dir {}", cas.display()))?
        .flatten()
    {
        let fanout_path = fanout.path();
        if !fanout_path.is_dir() {
            continue;
        }
        collect_hex_hash_files(
            &fanout_path,
            out,
            &format!("read CAS fan-out entry {}", fanout_path.display()),
            "CAS object file",
            |_| false,
        )?;
    }
    Ok(())
}

fn collect_legacy_blob_hashes(store_root: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let legacy = store_root.join("blobs");
    if !legacy.is_dir() {
        return Ok(());
    }
    collect_hex_hash_files(
        &legacy,
        out,
        &format!("read legacy blob dir {}", legacy.display()),
        "legacy blob file",
        |name| name == "oidmap" || name == "sha256",
    )
}

fn collect_hex_hash_files(
    dir: &Path,
    out: &mut BTreeSet<String>,
    read_ctx: &str,
    label: &str,
    skip: impl Fn(&str) -> bool,
) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| read_ctx.to_string())?
        .flatten()
    {
        let file_name = entry.file_name();
        let name = file_name_to_str(&file_name, label)?;
        if skip(name) {
            continue;
        }
        let is_file = entry.path().is_file();
        if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) && is_file {
            out.insert(name.to_lowercase());
        }
    }
    Ok(())
}

fn collect_records_sidecar_hashes(store_root: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let path = store_root.join("records_latest.jsonl");
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read records sidecar {}", path.display()))?;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("parse records sidecar line in {}", path.display()))?;
        if let Some(files) = entry.get("files").and_then(|v| v.as_array()) {
            for file in files {
                if let Some(hash) = file.get("hash").and_then(|v| v.as_str())
                    && is_full_hash(hash)
                {
                    out.insert(hash.to_lowercase());
                }
            }
        }
    }
    Ok(())
}

fn collect_paths_sidecar_hashes(store_root: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let shards = store_root.join("shards");
    if !shards.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&shards)
        .with_context(|| format!("read shards dir {}", shards.display()))?
        .flatten()
    {
        let file_name = entry.file_name();
        let name = file_name_to_str(&file_name, "paths sidecar file")?;
        if !name.starts_with("paths_") || !name.ends_with(".txt") {
            continue;
        }
        insert_hashes_from_paths_sidecar(&entry.path(), out)?;
    }
    Ok(())
}

fn insert_hashes_from_paths_sidecar(path: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read paths sidecar {}", path.display()))?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let Some(hash) = parts.next()
            && is_full_hash(hash)
        {
            out.insert(hash.to_lowercase());
        }
    }
    Ok(())
}

fn collect_ref_index_hashes(store_root: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let Some(index_dir) = ref_index_dir() else {
        return Ok(());
    };
    if !index_dir.is_dir() {
        return Ok(());
    }
    let store_canonical = absolute_path(store_root).to_string_lossy().to_string();
    for entry in fs::read_dir(&index_dir)
        .with_context(|| format!("read ref-index dir {}", index_dir.display()))?
        .flatten()
    {
        let file_name = entry.file_name();
        let name = file_name_to_str(&file_name, "ref-index shard")?;
        if !name.ends_with(".ndjson") {
            continue;
        }
        insert_hashes_from_ref_index_shard(&entry.path(), &store_canonical, out)?;
    }
    Ok(())
}

fn insert_hashes_from_ref_index_shard(
    path: &Path,
    store_canonical: &str,
    out: &mut BTreeSet<String>,
) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read ref-index shard {}", path.display()))?;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let entry: RefIndexEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue, // tolerate torn final line
        };
        let entry_path = absolute_path(Path::new(&entry.store_path))
            .to_string_lossy()
            .to_string();
        if entry_path != store_canonical {
            continue;
        }
        if let Some(hash) = blob_hash_from_ref(&entry.ref_id) {
            out.insert(hash);
        }
    }
    Ok(())
}

fn blob_hash_from_ref(ref_id: &str) -> Option<String> {
    let gz = GzRef::parse(ref_id).ok()?;
    if let GzRef::Blob { hash, .. } = gz {
        Some(hash.to_lowercase())
    } else {
        None
    }
}

fn is_full_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn rfc3339_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    // UTC, no leap seconds, simple formatting.
    let days = secs / 86_400;
    let (y, m, d) = days_to_ymd(days as i64);
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{nanos:09}Z")
}

fn days_to_ymd(mut days: i64) -> (i64, u8, u8) {
    // Algorithm from civil-from-days (Howard Hinnant).
    days += 719_468;
    let era = if days >= 0 {
        days / 146_097
    } else {
        (days - 146_096) / 146_097
    };
    let doe = days - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + if m <= 2 { 1 } else { 0 }, m as u8, d as u8)
}

fn process_nonce() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id() as u64;
    (pid << 32) | seq
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    // Prefer File::sync_all over libc::fsync (graphzero-esig8 / site-0019):
    // isomorphic durability, no unsafe, matches hub atomic_write/indexer.
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RefIndexEntry {
    ref_id: String,
    store_path: String,
    #[allow(dead_code)]
    ts: u64,
}

fn ref_index_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GRAPHZERO_REF_INDEX_PATH") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".graphzero").join("ref-index"))
}
