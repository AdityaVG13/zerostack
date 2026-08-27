//! Per-user recovery ref index for cross-root `gz://` expansion.
//!
//! The index records only ref id -> store root pointers. Payload bytes remain
//! in the store that minted the ref.
//!
//! Platform notes (bead 1ghi.5): shards are append-only NDJSON whose readers
//! tolerate a torn final line, so appends need no lock on any OS. Compaction
//! writes a uniquely named sibling temp file, syncs it, and publishes it via
//! [`super::replace_file`], which retries classified transient
//! Windows sharing violations with bounded backoff. The old shard stays
//! complete and valid until the replacement lands. On Unix the index files
//! are created `0o600`/`0o700`; on Windows they rely on the user-profile
//! directory ACLs — Unix modes are never faked.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::replace_file;

const MAX_SHARD_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RefIndexEntry {
    pub ref_id: String,
    pub store_path: String,
    pub ts: u64,
}

pub fn enabled() -> bool {
    match std::env::var("GRAPHZERO_REF_INDEX") {
        Ok(v) => v != "0",
        Err(_) => true,
    }
}

pub fn record_ref(ref_id: &str, store_root: &Path) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let Some(index_dir) = index_dir() else {
        return Ok(());
    };
    append_entry(&index_dir, ref_id, store_root).with_context(|| {
        format!(
            "record ref-index entry for {ref_id} in {}",
            index_dir.display()
        )
    })
}

pub fn lookup_store(ref_id: &str) -> Option<PathBuf> {
    if !enabled() {
        return None;
    }
    let index_dir = index_dir()?;
    let shard = shard_path(&index_dir, ref_id);
    let entries = read_entries(&shard);
    let mut newest: Option<RefIndexEntry> = None;
    let mut saw_stale = false;
    for entry in entries {
        if !Path::new(&entry.store_path).exists() {
            saw_stale = true;
            continue;
        }
        if entry.ref_id == ref_id
            && newest
                .as_ref()
                .map(|old| entry.ts >= old.ts)
                .unwrap_or(true)
        {
            newest = Some(entry);
        }
    }
    if saw_stale {
        let _ = compact_shard(&shard);
    }
    newest.map(|entry| PathBuf::from(entry.store_path))
}

pub fn compact_all_for_tests() {
    if let Some(index_dir) = index_dir()
        && let Ok(entries) = fs::read_dir(index_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("ndjson") {
                let _ = compact_shard(&path);
            }
        }
    }
}

/// Open for private appends: `0o600` on Unix, user-profile ACLs on Windows.
fn open_private_append(shard: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(shard)
}

/// Open a fresh private temp file for compaction output.
fn open_private_truncate(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn append_entry(index_dir: &Path, ref_id: &str, store_root: &Path) -> Result<()> {
    ensure_private_dir(index_dir)
        .with_context(|| format!("create ref-index dir {}", index_dir.display()))?;
    let store_path = super::path_safety::absolute_path(store_root);
    let entry = RefIndexEntry {
        ref_id: ref_id.to_string(),
        store_path: store_path.to_string_lossy().to_string(),
        ts: now_millis(),
    };
    let shard = shard_path(index_dir, ref_id);
    let line = serde_json::to_string(&entry).context("serialize ref-index entry")?;
    {
        let mut file = open_private_append(&shard)
            .with_context(|| format!("open ref-index shard {}", shard.display()))?;
        // Single O_APPEND line write; readers tolerate a torn final line.
        let mut record = line.into_bytes();
        record.push(b'\n');
        file.write_all(&record)
            .with_context(|| format!("write ref-index shard {}", shard.display()))?;
    }
    if fs::metadata(&shard)
        .map(|m| m.len() > MAX_SHARD_BYTES)
        .unwrap_or(false)
    {
        compact_shard(&shard)
            .with_context(|| format!("compact oversized ref-index shard {}", shard.display()))?;
    }
    Ok(())
}

fn compact_shard(shard: &Path) -> Result<()> {
    let values = newest_live_entries(read_entries(shard));
    if let Some(parent) = shard.parent() {
        ensure_private_dir(parent)
            .with_context(|| format!("create ref-index shard parent {}", parent.display()))?;
    }
    // Unique sibling temp: concurrent compactors never share a temp file, so
    // a rename always publishes a file its writer completed. Stale temps from
    // crashed compactors are reaped opportunistically once old enough.
    reap_stale_compaction_temps(shard);
    let tmp = unique_compaction_temp(shard);
    let write = write_compacted_shard(&tmp, shard, &values);
    if write.is_err() {
        // The temp is ours; the old shard is still complete and valid.
        let _ = fs::remove_file(&tmp);
    }
    write
}

fn newest_live_entries(entries: Vec<RefIndexEntry>) -> Vec<RefIndexEntry> {
    let mut newest: HashMap<String, RefIndexEntry> = HashMap::new();
    for entry in entries {
        if !Path::new(&entry.store_path).exists() {
            continue;
        }
        match newest.get(&entry.ref_id) {
            Some(old) if old.ts > entry.ts => {}
            _ => {
                newest.insert(entry.ref_id.clone(), entry);
            }
        }
    }
    let mut values: Vec<_> = newest.into_values().collect();
    values.sort_by(|a, b| a.ref_id.cmp(&b.ref_id));
    values
}

fn unique_compaction_temp(shard: &Path) -> PathBuf {
    static COMPACT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    shard.with_extension(format!(
        "ndjson.tmp-{}-{}",
        std::process::id(),
        COMPACT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

fn write_compacted_shard(tmp: &Path, shard: &Path, values: &[RefIndexEntry]) -> Result<()> {
    write_compacted_lines(tmp, values)?;
    replace_file(tmp, shard)
        .with_context(|| format!("replace ref-index shard {}", shard.display()))?;
    Ok(())
}

fn write_compacted_lines(tmp: &Path, values: &[RefIndexEntry]) -> Result<()> {
    let mut file = open_private_truncate(tmp)
        .with_context(|| format!("open compacted ref-index shard {}", tmp.display()))?;
    for entry in values {
        let line = serde_json::to_string(entry).context("serialize compacted ref-index entry")?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .with_context(|| format!("write compacted ref-index shard {}", tmp.display()))?;
    }
    file.sync_all()
        .with_context(|| format!("sync compacted ref-index shard {}", tmp.display()))?;
    Ok(())
}

/// Bounded cleanup: only `<shard>.tmp-*` siblings older than one hour are
/// removed, so an active compactor is never raced.
fn reap_stale_compaction_temps(shard: &Path) {
    let Some(parent) = shard.parent() else {
        return;
    };
    let Some(name) = shard.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{name}.tmp-");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&prefix) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age.as_secs() >= 3600);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn read_entries(shard: &Path) -> Vec<RefIndexEntry> {
    let mut text = String::new();
    let mut file = match fs::File::open(shard) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    text.lines()
        .filter_map(|line| serde_json::from_str::<RefIndexEntry>(line).ok())
        .collect()
}

fn index_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GRAPHZERO_REF_INDEX_PATH") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".graphzero").join("ref-index"))
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn shard_path(index_dir: &Path, ref_id: &str) -> PathBuf {
    index_dir.join(format!("{}.ndjson", shard_key(ref_id)))
}

fn shard_key(ref_id: &str) -> String {
    let id = ref_identity(ref_id);
    let mut key = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_ascii_lowercase();
    while key.len() < 2 {
        key.push('x');
    }
    key
}

fn ref_identity(ref_id: &str) -> &str {
    if let Some(id) = ref_id.strip_prefix("q:") {
        return id;
    }
    let without_fragment = ref_id.split('#').next().unwrap_or(ref_id);
    without_fragment
        .rsplit('/')
        .next()
        .unwrap_or(without_fragment)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
