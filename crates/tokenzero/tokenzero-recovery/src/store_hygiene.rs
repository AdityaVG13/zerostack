//! Recovery temp-file sweep and blob-sidecar prune.
//!
//! Split out of `lib.rs` so the persist/expand core is not also the GC
//! surface. Callers and tests keep the same crate-root names.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::{
    BlobEntry, PersistLock, RecoveryConfig, RecoveryError, RecoveryState, RecoveryStore,
    blob_sidecar_dir, load_state_if_present, parse_blob_marker, recovery_lock_path,
    ref_index_blob_lru,
};

/// Age after which an abandoned atomic-write temp file is reclaimable. A
/// live persist holds its temp file for milliseconds; an hour-old one belongs
/// to a process that died mid-write.
pub const STALE_TMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Outcome of a stale temp-file sweep. With `dry_run`, `removed*` counts what
/// would be reclaimed without unlinking anything.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TmpSweepReport {
    pub dry_run: bool,
    pub scanned: usize,
    pub removed: usize,
    pub removed_bytes: u64,
    pub failed: usize,
}

/// Remove matching recovery temp files older than `max_age`.
/// Dry runs report candidates; per-file failures are counted and fail open.
pub fn sweep_stale_tmp_files(
    cache_path: &Path,
    max_age: Duration,
    dry_run: bool,
) -> TmpSweepReport {
    let mut report = TmpSweepReport {
        dry_run,
        ..TmpSweepReport::default()
    };
    // Persist writes a unique tmp then renames under PersistLock. Sweeping
    // without that lock can unlink a live writer's tmp (TOCTOU). Lock timeout
    // skips the sweep: fail closed for deletes.
    let Ok(_lock) = PersistLock::acquire(recovery_lock_path(cache_path)) else {
        return report;
    };
    let Some((parent, cache_name)) = cache_path
        .parent()
        .zip(cache_path.file_name().and_then(|name| name.to_str()))
    else {
        return report;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return report;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_recovery_tmp_name(name, cache_name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        report.scanned += 1;
        let expired = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if !expired {
            continue;
        }
        if dry_run || fs::remove_file(path).is_ok() {
            report.removed += 1;
            report.removed_bytes += meta.len();
        } else {
            report.failed += 1;
        }
    }
    report
}

/// Persist temps from this crate (`.{cache}.{pid}.{nonce}.tmp`) and hub
/// `atomic_write_file` leftovers (`.{cache}.tmp-{pid}-{seq}`). Kill-before-rename
/// leaves the dest complete; sweeping the leftover must not miss the hub name.
fn is_recovery_tmp_name(name: &str, cache_name: &str) -> bool {
    if !name.contains(cache_name) {
        return false;
    }
    if name.ends_with(".tmp") {
        return true;
    }
    let Some((_, rest)) = name.rsplit_once(".tmp-") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

/// Result of enforcing the legacy recovery sidecar byte budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobSidecarPruneReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub removed_files: usize,
    pub retained_referenced: usize,
}

/// Load the snapshot used as a prune root set. A missing cache is empty; an
/// existing but unreadable cache is not -- treating that as empty would make
/// every sidecar look unreferenced and delete it.
fn load_prune_snapshot(
    cache_path: &Path,
    config: &RecoveryConfig,
) -> Result<RecoveryState, RecoveryError> {
    Ok(load_state_if_present(cache_path, config)?.unwrap_or_else(|| RecoveryState::empty(config)))
}

/// Remove oldest unreferenced legacy blob sidecars until the budget is met.
/// Sidecars referenced by the authoritative snapshot or journal are retained.
pub fn prune_blob_sidecars(
    cache_path: &Path,
    max_bytes: u64,
    dry_run: bool,
) -> Result<BlobSidecarPruneReport, RecoveryError> {
    let _lock = PersistLock::acquire(recovery_lock_path(cache_path))?;
    let config = RecoveryConfig::default();
    let state = match load_prune_snapshot(cache_path, &config) {
        Ok(state) => state,
        Err(err) => {
            crate::crash_inject::maybe_crash(crate::crash_inject::BEFORE_PRUNE_UNREADABLE);
            return Err(err);
        }
    };
    let referenced: HashSet<String> = state
        .blobs
        .values()
        .filter_map(|entry| match entry {
            BlobEntry::Inline(value) => parse_blob_marker(value).map(|(hash, _)| hash.to_string()),
            BlobEntry::FileRef { .. } => None,
        })
        .collect();
    let directory = blob_sidecar_dir(cache_path);
    let mut files = Vec::new();
    let mut bytes_before = 0_u64;
    let mut retained_referenced = 0_usize;
    match fs::read_dir(&directory) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if !metadata.is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(hash) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                let len = metadata.len();
                bytes_before = bytes_before.saturating_add(len);
                if referenced.contains(hash) {
                    retained_referenced += 1;
                } else {
                    let Ok(modified) = metadata.modified() else {
                        continue;
                    };
                    files.push((modified, path, len));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    let mut bytes_after = bytes_before;
    let mut removed_files = 0_usize;
    for (_, path, len) in files {
        if bytes_after <= max_bytes {
            break;
        }
        if !dry_run {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        bytes_after = bytes_after.saturating_sub(len);
        removed_files += 1;
    }
    Ok(BlobSidecarPruneReport {
        bytes_before,
        bytes_after,
        removed_files,
        retained_referenced,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryBlobPruneReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub freed_bytes: u64,
    pub removed_files: usize,
    pub removed_referenced: usize,
    pub expired_files: usize,
    pub max_bytes: u64,
    pub max_age_seconds: u64,
    pub dry_run: bool,
}

/// Enforce byte and age bounds over the complete recovery sidecar store.
/// Never-expanded blobs are selected before expanded blobs; expanded blobs use
/// their durable ref-index last-expand timestamp as the LRU key.
pub fn prune_recovery_blobs(
    cache_path: &Path,
    max_bytes: u64,
    max_age: Duration,
    dry_run: bool,
) -> Result<RecoveryBlobPruneReport, RecoveryError> {
    let _lock = PersistLock::acquire(recovery_lock_path(cache_path))?;
    let _ = load_prune_snapshot(cache_path, &RecoveryConfig::default())?;
    let directory = blob_sidecar_dir(cache_path);
    let mut store = RecoveryStore::new(Some(cache_path.to_path_buf()));
    let mut files = Vec::new();
    let mut bytes_before = 0_u64;
    let now = SystemTime::now();
    match fs::read_dir(&directory) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let metadata = entry.metadata()?;
                if !metadata.is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(hash) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                let ref_id = format!("tz://blob/{hash}");
                let len = metadata.len();
                bytes_before = bytes_before.saturating_add(len);
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                let expired = now.duration_since(modified).unwrap_or_default() >= max_age;
                let (expansion_count, last_expanded) = ref_index_blob_lru(&ref_id);
                let referenced = store.state.blobs.contains_key(&ref_id);
                files.push((
                    !expired,
                    expansion_count > 0,
                    last_expanded,
                    modified,
                    path,
                    ref_id,
                    len,
                    referenced,
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    files.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
            .then(left.3.cmp(&right.3))
            .then(left.5.cmp(&right.5))
    });
    let mut bytes_after = bytes_before;
    let mut victims = Vec::new();
    let mut expired_files = 0_usize;
    for (not_expired, _, _, _, path, ref_id, len, referenced) in files {
        if not_expired && bytes_after <= max_bytes {
            continue;
        }
        if !not_expired {
            expired_files += 1;
        }
        bytes_after = bytes_after.saturating_sub(len);
        victims.push((path, ref_id, len, referenced));
    }
    let removed_referenced = victims.iter().filter(|item| item.3).count();
    if !dry_run {
        for (_, ref_id, _, referenced) in &victims {
            if *referenced {
                let aliases: Vec<_> = store
                    .state
                    .aliases
                    .iter()
                    .filter(|(_, target)| *target == ref_id)
                    .map(|(alias, _)| alias.clone())
                    .collect();
                for alias in aliases {
                    store.remove_alias(&alias);
                }
                store.remove_blob(ref_id);
            }
        }
        store.persist_assuming_locked()?;
        let published = load_prune_snapshot(cache_path, &store.config)?;
        for (_, ref_id, _, _) in &victims {
            if published.blobs.contains_key(ref_id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("snapshot still names {ref_id}; refusing unlink"),
                )
                .into());
            }
        }
        for (path, _, _, _) in &victims {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(RecoveryBlobPruneReport {
        bytes_before,
        bytes_after,
        freed_bytes: bytes_before.saturating_sub(bytes_after),
        removed_files: victims.len(),
        removed_referenced,
        expired_files,
        max_bytes,
        max_age_seconds: max_age.as_secs(),
        dry_run,
    })
}

pub fn recovery_blob_status(cache_path: &Path) -> serde_json::Value {
    let bytes = fs::read_dir(blob_sidecar_dir(cache_path))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .fold(0_u64, |total, metadata| {
            total.saturating_add(metadata.len())
        });
    serde_json::json!({"bytes": bytes, "freed_bytes": 0, "path": blob_sidecar_dir(cache_path)})
}
