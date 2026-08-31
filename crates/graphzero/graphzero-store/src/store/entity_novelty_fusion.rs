//! Persists GraphZero entity digests under the shared novelty store path.
//! Immutable snapshots may be published through [`SharedCas`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::entity::{EntityId, EntityNovelty};
use super::session::SeenScope;
use super::shared_cas::SharedCas;

/// Frozen schema id (TokenZero `schemas/entity-novelty/`).
pub const ENTITY_NOVELTY_SCHEMA_VERSION: &str = "zerostack.entity-novelty";

/// Record type constant.
pub const ENTITY_NOVELTY_RECORD_TYPE: &str = "entity-novelty";

/// Relative directory under a ZeroStack / SharedCas store root.
pub const ENTITY_NOVELTY_REL_DIR: &str = "shared/entity-novelty";

/// Default max entity ids retained per shared novelty scope (disk + CAS snapshot).
pub const DEFAULT_SHARED_ENTITY_NOVELTY_MAX: usize = 50_000;

/// Env override for [`DEFAULT_SHARED_ENTITY_NOVELTY_MAX`] (positive usize).
pub const SHARED_ENTITY_NOVELTY_MAX_ENV: &str = "GRAPHZERO_SHARED_ENTITY_NOVELTY_MAX";

/// Resolved shared novelty entity-id cap.
pub fn shared_entity_novelty_max() -> usize {
    std::env::var(SHARED_ENTITY_NOVELTY_MAX_ENV)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_SHARED_ENTITY_NOVELTY_MAX)
}

/// Shared known-entity novelty set for one scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedEntityNoveltyRecord {
    pub schema_version: String,
    pub record_type: String,
    pub scope_key: String,
    pub entity_ids: Vec<String>,
    pub producing_engine: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cas_digest: Option<String>,
}

impl SharedEntityNoveltyRecord {
    pub fn empty(scope_key: impl Into<String>, producing_engine: &str) -> Self {
        Self {
            schema_version: ENTITY_NOVELTY_SCHEMA_VERSION.to_string(),
            record_type: ENTITY_NOVELTY_RECORD_TYPE.to_string(),
            scope_key: scope_key.into(),
            entity_ids: Vec::new(),
            producing_engine: producing_engine.to_string(),
            updated_at: now_rfc3339(),
            cas_digest: None,
        }
    }

    /// O(log n) membership on the sorted unique `entity_ids` vector.
    pub fn knows(&self, id: &EntityId) -> bool {
        self.entity_ids
            .binary_search_by(|e| e.as_str().cmp(id.as_str()))
            .is_ok()
    }

    /// Drop lowest entity-id hexes until `entity_ids.len() <= max`.
    pub fn enforce_cap(&mut self, max: usize) {
        if max == 0 || self.entity_ids.len() <= max {
            return;
        }
        let drop_n = self.entity_ids.len() - max;
        self.entity_ids.drain(0..drop_n);
    }

    pub fn to_entity_ids(&self) -> Result<Vec<EntityId>, String> {
        self.entity_ids
            .iter()
            .map(|hex| EntityId::parse(hex).map_err(|e| e.to_string()))
            .collect()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ENTITY_NOVELTY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if self.record_type != ENTITY_NOVELTY_RECORD_TYPE {
            return Err(format!("unsupported record_type {}", self.record_type));
        }
        validate_scope_key(&self.scope_key)?;
        match self.producing_engine.as_str() {
            "tokenzero" | "fszero" | "graphzero" => {}
            other => return Err(format!("invalid producing_engine {other}")),
        }
        let mut prev: Option<&str> = None;
        for id in &self.entity_ids {
            EntityId::parse(id).map_err(|e| e.to_string())?;
            if id.contains("://") {
                return Err(format!("entity id must not include a URI scheme: {id}"));
            }
            if let Some(p) = prev {
                if id.as_str() <= p {
                    return Err("entity_ids must be sorted unique".into());
                }
            }
            prev = Some(id.as_str());
        }
        if let Some(digest) = &self.cas_digest {
            EntityId::parse(digest).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// SHA-256 hex of UTF-8 `scope_key`.
pub fn novelty_scope_digest(scope_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(scope_key.as_bytes());
    crate::fast_hex(&hasher.finalize())
}

/// Mutable novelty pointer path under `store_root`.
pub fn shared_entity_novelty_path(store_root: &Path, scope_key: &str) -> PathBuf {
    store_root
        .join(ENTITY_NOVELTY_REL_DIR)
        .join(format!("{}.json", novelty_scope_digest(scope_key)))
}

/// Map a [`SeenScope`] to the shared novelty `scope_key` spelling.
pub fn scope_key_for(scope: &SeenScope) -> String {
    match scope {
        SeenScope::Session(s) => format!("session:{s}"),
        SeenScope::Repo(r) => format!("repo:{r}"),
        SeenScope::Workspace(w) => format!("workspace:{w}"),
        SeenScope::Global => "global".to_string(),
    }
}

/// Load shared novelty; missing file is empty.
pub fn read_shared_entity_novelty(
    store_root: &Path,
    scope_key: &str,
) -> io::Result<SharedEntityNoveltyRecord> {
    let path = shared_entity_novelty_path(store_root, scope_key);
    match fs::read(&path) {
        Ok(bytes) => {
            let record: SharedEntityNoveltyRecord = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            record
                .validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if record.scope_key != scope_key {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "novelty scope_key {} != requested {scope_key}",
                        record.scope_key
                    ),
                ));
            }
            Ok(record)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok(SharedEntityNoveltyRecord::empty(scope_key, "graphzero"))
        }
        Err(err) => Err(err),
    }
}

/// Persist the mutable novelty pointer.
pub fn write_shared_entity_novelty(
    store_root: &Path,
    record: &SharedEntityNoveltyRecord,
) -> io::Result<PathBuf> {
    record
        .validate()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let path = shared_entity_novelty_path(store_root, &record.scope_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Union `ids` into the shared novelty set and publish a CAS snapshot.
/// Returns the updated record (with `cas_digest` set when publish succeeds).
pub fn merge_shared_entity_novelty(
    store_root: &Path,
    scope_key: &str,
    ids: &[EntityId],
    publish_cas: bool,
) -> io::Result<SharedEntityNoveltyRecord> {
    let mut record = read_shared_entity_novelty(store_root, scope_key)?;
    let mut set: std::collections::BTreeSet<String> = record.entity_ids.iter().cloned().collect();
    for id in ids {
        set.insert(id.as_str().to_ascii_lowercase());
    }
    record.entity_ids = set.into_iter().collect();
    // Soft cap: retain highest hex ids so disk + CAS snapshot stay bounded.
    record.enforce_cap(shared_entity_novelty_max());
    record.producing_engine = "graphzero".to_string();
    record.updated_at = now_rfc3339();

    if publish_cas {
        let snapshot = serde_json::to_vec(&record)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        match SharedCas::open(store_root).put(&snapshot) {
            Ok(digest) => record.cas_digest = Some(digest),
            Err(err) => {
                return Err(io::Error::other(format!(
                    "shared CAS publish failed: {err}"
                )));
            }
        }
    }

    write_shared_entity_novelty(store_root, &record)?;
    Ok(record)
}

/// Hydrate an in-memory [`EntityNovelty`] from the shared store pointer.
pub fn hydrate_entity_novelty(
    store_root: &Path,
    scope_key: &str,
    novelty: &mut EntityNovelty,
) -> io::Result<usize> {
    let record = read_shared_entity_novelty(store_root, scope_key)?;
    let ids = record
        .to_entity_ids()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(novelty.merge_ids(ids))
}

/// Flush known ids from memory into the shared store + CAS snapshot.
pub fn flush_entity_novelty(
    store_root: &Path,
    scope_key: &str,
    novelty: &EntityNovelty,
) -> io::Result<SharedEntityNoveltyRecord> {
    merge_shared_entity_novelty(store_root, scope_key, &novelty.known_ids(), true)
}

/// Resolve store root for fusion from env (`ZEROSTACK_STORE_ROOT` first).
pub fn fusion_store_root_from_env() -> Option<PathBuf> {
    for key in [
        "ZEROSTACK_STORE_ROOT",
        "ZERO_STACK_STORE_ROOT",
        "GRAPHZERO_STORE_ROOT",
    ] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return Some(PathBuf::from(v));
            }
        }
    }
    None
}

fn validate_scope_key(scope_key: &str) -> Result<(), String> {
    let ok = scope_key == "global"
        || scope_key
            .strip_prefix("session:")
            .is_some_and(|s| !s.is_empty())
        || scope_key
            .strip_prefix("repo:")
            .is_some_and(|s| !s.is_empty())
        || scope_key
            .strip_prefix("workspace:")
            .is_some_and(|s| !s.is_empty());
    if ok {
        Ok(())
    } else {
        Err(format!("invalid scope_key {scope_key}"))
    }
}

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (y, m, d) = unix_days_to_ymd(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn unix_days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
