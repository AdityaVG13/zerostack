//! Cross-root ref index (ndjson shards under `~/.fszero/ref-index`).
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

pub(super) const REF_INDEX_SHARD_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct RefIndexEntry {
    ref_id: String,
    pub(super) store_path: PathBuf,
    ts: u128,
    order: u64,
}

pub(super) fn ref_index_enabled() -> bool {
    std::env::var("FSZERO_REF_INDEX").ok().as_deref() != Some("0")
}

/// Keys that carry a durable identity worth cross-root recovery.
/// Blob content refs + CodeMode execution artifacts (post-edit readback).
/// Transient `fz://seq/` and bare named keys (`read`, `search`) stay local.
pub(super) fn ref_indexable(key: &str) -> bool {
    if key.starts_with("fz://seq/") {
        return false;
    }
    key.starts_with("fz://blob/") || key.starts_with("fz://codemode/")
}

pub(super) fn ref_index_root() -> Option<PathBuf> {
    if !ref_index_enabled() {
        return None;
    }
    if let Some(path) = std::env::var_os("FSZERO_REF_INDEX_PATH") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".fszero/ref-index"))
}

pub(super) fn ref_index_shard_path(ref_id: &str) -> Option<PathBuf> {
    let root = ref_index_root()?;
    let id = ref_id
        .strip_prefix("fz://blob/")
        .or_else(|| ref_id.strip_prefix("tz://blob/"))
        .or_else(|| ref_id.strip_prefix("gz://blob/"));
    let shard = if let Some(hash) = id {
        let mut s = hash
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(2)
            .collect::<String>()
            .to_ascii_lowercase();
        if s.len() < 2 {
            s = "__".to_string();
        }
        s
    } else {
        let mut h = Sha256::new();
        h.update(ref_id.as_bytes());
        fszero_core::hexutil::sha256_hex_of(h.finalize().into())[..2].to_string()
    };
    Some(root.join(format!("{shard}.ndjson")))
}

/// One locator covers every immutable artifact below an execution base. The
/// exact payload key still selects and verifies bytes in the located store.
pub(super) fn codemode_execution_base_ref(ref_id: &str) -> Option<String> {
    const PREFIX: &str = "fz://codemode/execution/";
    let rest = ref_id.strip_prefix(PREFIX)?;
    let (execution_id, _) = rest.split_once('/')?;
    if execution_id.is_empty() {
        return None;
    }
    Some(format!("{PREFIX}{execution_id}"))
}

pub(super) fn ensure_ref_index_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub(super) fn parse_ref_index_line(line: &str, order: u64) -> Option<RefIndexEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ref_id = v.get("ref_id")?.as_str()?.to_string();
    let store_path = PathBuf::from(v.get("store_path")?.as_str()?);
    let ts = v
        .get("ts")
        .and_then(|v| v.as_u64())
        .map(u128::from)
        .unwrap_or(0);
    Some(RefIndexEntry {
        ref_id,
        store_path,
        ts,
        order,
    })
}

/// Returns parsed entries plus the count of damaged (unparseable) lines —
/// a torn shard tail parses up to the tear; the damage count is reported by
/// callers, never silently absorbed (fszero-ku8).
pub(super) fn read_ref_index_entries_reporting(shard: &Path) -> (Vec<RefIndexEntry>, usize) {
    let Ok(text) = std::fs::read_to_string(shard) else {
        return (Vec::new(), 0);
    };
    let mut damaged = 0usize;
    let entries = text
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let parsed = parse_ref_index_line(line, idx as u64);
            if parsed.is_none() && !line.trim().is_empty() {
                damaged += 1;
            }
            parsed
        })
        .collect();
    (entries, damaged)
}

pub(super) fn read_ref_index_entries(shard: &Path) -> Vec<RefIndexEntry> {
    read_ref_index_entries_reporting(shard).0
}

/// Process-global in-memory map of ref-index shards (fszero-lim).
/// Keyed by shard path; invalidated on append/compact when mtime/len changes.
pub(super) struct RefIndexShardCache {
    mtime: Option<SystemTime>,
    len: u64,
    by_ref: HashMap<String, RefIndexEntry>,
}

static REF_INDEX_SHARD_CACHE: LazyLock<Mutex<HashMap<PathBuf, RefIndexShardCache>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn invalidate_ref_index_shard_cache(shard: &Path) {
    if let Ok(mut g) = REF_INDEX_SHARD_CACHE.lock() {
        g.remove(shard);
    }
}

pub(super) fn load_ref_index_shard_map(shard: &Path) -> HashMap<String, RefIndexEntry> {
    let entries = read_ref_index_entries(shard);
    let mut by_ref: HashMap<String, RefIndexEntry> = HashMap::new();
    for entry in entries {
        match by_ref.get(&entry.ref_id) {
            Some(prev) if (prev.ts, prev.order) > (entry.ts, entry.order) => {}
            _ => {
                by_ref.insert(entry.ref_id.clone(), entry);
            }
        }
    }
    by_ref
}

pub(super) fn write_compacted_ref_index_shard(
    shard: &Path,
    prune_missing: bool,
) -> std::io::Result<()> {
    let entries = read_ref_index_entries(shard);
    let mut newest: BTreeMap<String, RefIndexEntry> = BTreeMap::new();
    for entry in entries {
        if prune_missing && !entry.store_path.is_file() {
            continue;
        }
        match newest.get(&entry.ref_id) {
            Some(prev) if (prev.ts, prev.order) > (entry.ts, entry.order) => {}
            _ => {
                newest.insert(entry.ref_id.clone(), entry);
            }
        }
    }
    let tmp = shard.with_extension("ndjson.tmp");
    let mut out = String::new();
    for entry in newest.into_values() {
        let line = serde_json::json!({ "ref_id": entry.ref_id, "store_path": entry.store_path, "ts": entry.ts, });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    std::fs::write(&tmp, out)?;
    std::fs::rename(tmp, shard)?;
    invalidate_ref_index_shard_cache(shard);
    Ok(())
}

fn lookup_ref_index_entry_exact(ref_id: &str) -> Option<RefIndexEntry> {
    let shard = ref_index_shard_path(ref_id)?;
    let meta = std::fs::metadata(&shard).ok();
    let (mtime, len) = match &meta {
        Some(m) => (m.modified().ok(), m.len()),
        None => return None,
    };
    // Hit path: hold the mutex only for map lookup (no disk I/O).
    if let Ok(cache) = REF_INDEX_SHARD_CACHE.lock() {
        if let Some(hit) = cache.get(&shard) {
            if hit.mtime == mtime && hit.len == len {
                return hit.by_ref.get(ref_id).cloned();
            }
        }
    }
    // Miss/stale: load shard map **outside** the mutex so concurrent expand of
    // other shards (or the same shard) is not serialized on disk read/parse
    // (fszero-ref-index-cache-mutex-io-1qgf). Double-check before insert.
    let by_ref = load_ref_index_shard_map(&shard);
    let result = by_ref.get(ref_id).cloned();
    if let Ok(mut cache) = REF_INDEX_SHARD_CACHE.lock() {
        if let Some(hit) = cache.get(&shard) {
            if hit.mtime == mtime && hit.len == len {
                return hit.by_ref.get(ref_id).cloned();
            }
        }
        cache.insert(shard, RefIndexShardCache { mtime, len, by_ref });
        return result;
    }
    // Lock poisoned: return the map we already loaded uncached.
    result
}

pub(super) fn lookup_ref_index_entry(ref_id: &str) -> Option<RefIndexEntry> {
    lookup_ref_index_entry_exact(ref_id).or_else(|| {
        codemode_execution_base_ref(ref_id)
            .as_deref()
            .and_then(lookup_ref_index_entry_exact)
    })
}

pub(super) fn prune_missing_ref_index_entries_for(ref_id: &str) {
    let mut refs = vec![ref_id.to_string()];
    if let Some(base) = codemode_execution_base_ref(ref_id) {
        refs.push(base);
    }
    for reference in refs {
        if let Some(shard) = ref_index_shard_path(&reference) {
            let _ = write_compacted_ref_index_shard(&shard, true);
        }
    }
}
