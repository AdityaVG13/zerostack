//! Segmented ActionCache index: key -> ref + optional sibling pointers.
//!
//! TokenZero owns the key and artifact ref. FSZero bookmarks and GraphZero
//! dep-closures are stored when present and stay `None` until those surfaces
//! exist. Live and in-grace tombstone entries are GC roots; `serve` pins
//! in-flight artifacts so concurrent eviction cannot dangle a returned ref.
//!
//! Layer discipline (ZS-CACHE-012/013/015):
//! - Blob eviction marks entries L3-cold (L2 validity preserved, needs
//!   refetch) instead of tombstoning them; a refetch of identical bytes
//!   restores L3 without rediscovery, and a tombstone never deletes the
//!   validity record.
//! - `prepare_blob_eviction` consults an `EvictionSlackGuard` (99% retained
//!   valid-mass floor) and fails loudly when the floor would be breached.
//! - Entries carry a `world_id` tenancy scope; resolution is world-filtered
//!   and a live entry is never clobbered by another world's write.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store_schema::{
    SchemaSkewError, StoreSchemaStamp, StoreSchemaVersion, admit_store_schema,
    recover_actioncache_segment, write_actioncache_segment,
};

pub const ACTIONCACHE_REL_DIR: &str = "tokenzero/actions";
/// Grace between index tombstone and CAS blob delete (tokenzero-gvxc).
pub const ACTIONCACHE_GC_GRACE_SECS: u64 = 60;

/// RAII pin: a live serve holds the artifact until drop.
#[derive(Debug)]
pub struct ServedArtifact {
    path: PathBuf,
}

impl Drop for ServedArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Result of index-before-CAS eviction planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobEvictionPlan {
    pub artifact_ref: String,
    /// Keys now marked L3-cold (L2 validity preserved, needs refetch).
    pub cold_keys: Vec<String>,
    pub waiting_grace: Vec<String>,
    pub may_delete_blob: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionCacheEntry {
    pub key: String,
    #[serde(rename = "ref")]
    pub artifact_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fszero_bookmark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dep_closure_ref: Option<String>,
    pub class: String,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tombstone: bool,
    /// Unix seconds when the entry was tombstoned. Required before CAS delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstoned_at_unix: Option<u64>,
    /// L2-valid / L3-cold (ZS-CACHE-013): blob eviction preserves the logical
    /// entry and marks it needs-refetch; a later refetch of identical bytes
    /// restores L3 (`complete_refetch`). Never a tombstone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub l3_cold: bool,
    /// Unix seconds when the entry was marked L3-cold. Required before CAS
    /// delete, mirroring the tombstone grace discipline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold_since_unix: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActionCacheSegment {
    schema: String,
    major: u16,
    minor: u16,
    entry: ActionCacheEntry,
}

#[derive(Debug)]
pub enum ActionCacheError {
    Io(io::Error),
    Json(serde_json::Error),
    Schema(SchemaSkewError),
    InvalidKey(String),
    /// Eviction refused by the 99% slack guard (ZS-CACHE-012): deleting this
    /// artifact's weight would drop retained valid mass below 99% of demanded.
    EvictionRefused {
        resident_mass: u64,
        demanded_mass: u64,
        evict_weight: u64,
        slack_ppm: i64,
    },
    /// A zero demanded mass cannot anchor a 99% slack floor (ZS-CACHE-012).
    InvalidDemandMass,
}

impl std::fmt::Display for ActionCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "actioncache io: {err}"),
            Self::Json(err) => write!(f, "actioncache json: {err}"),
            Self::Schema(err) => write!(f, "actioncache schema: {err}"),
            Self::InvalidKey(key) => write!(f, "actioncache key {key:?} is not 64 lowercase hex"),
            Self::EvictionRefused {
                resident_mass,
                demanded_mass,
                evict_weight,
                slack_ppm,
            } => write!(
                f,
                "actioncache eviction refused: retained valid mass {resident_mass} - {evict_weight} would fall below the 99% floor of demanded {demanded_mass} (slack {slack_ppm}ppm)"
            ),
            Self::InvalidDemandMass => {
                write!(
                    f,
                    "actioncache eviction slack: demanded mass must be nonzero"
                )
            }
        }
    }
}

impl std::error::Error for ActionCacheError {}

impl From<io::Error> for ActionCacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ActionCacheError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Eviction slack guard (ZS-CACHE-012), mirroring hub `EvictionSlack`:
/// `sigma = W_R - 0.99W`. An eviction that would push retained valid mass
/// below 99% of demanded mass is refused, loudly, before any state change.
#[derive(Clone, Copy, Debug)]
pub struct EvictionSlackGuard {
    resident_mass: u64,
    demanded_mass: u64,
}

impl EvictionSlackGuard {
    pub fn new(resident_mass: u64, demanded_mass: u64) -> Result<Self, ActionCacheError> {
        if demanded_mass == 0 {
            return Err(ActionCacheError::InvalidDemandMass);
        }
        Ok(Self {
            resident_mass,
            demanded_mass,
        })
    }

    /// `sigma = W_R - 0.99W` in PPM of demanded mass (can be negative).
    pub fn slack_ppm(&self) -> i64 {
        let floor = retained_floor(self.demanded_mass).unwrap_or(u64::MAX);
        let floor_ppm = ppm_of(floor, self.demanded_mass);
        let resident_ppm = ppm_of(self.resident_mass, self.demanded_mass);
        let resident = i64::try_from(resident_ppm).unwrap_or(i64::MAX);
        let floor = i64::try_from(floor_ppm).unwrap_or(i64::MAX);
        resident.saturating_sub(floor)
    }

    /// Guard one eviction decision: evicting `evict_weight` must keep
    /// retained valid mass at or above 99% of demanded mass.
    pub fn guard_eviction(&self, evict_weight: u64) -> Result<(), ActionCacheError> {
        let Some(floor) = retained_floor(self.demanded_mass) else {
            return Err(ActionCacheError::EvictionRefused {
                resident_mass: self.resident_mass,
                demanded_mass: self.demanded_mass,
                evict_weight,
                slack_ppm: self.slack_ppm(),
            });
        };
        let after = self.resident_mass.saturating_sub(evict_weight);
        if after < floor {
            return Err(ActionCacheError::EvictionRefused {
                resident_mass: self.resident_mass,
                demanded_mass: self.demanded_mass,
                evict_weight,
                slack_ppm: self.slack_ppm(),
            });
        }
        Ok(())
    }
}

/// On-disk ActionCache under `<store_root>/tokenzero/actions/`.
#[derive(Debug, Clone)]
pub struct ActionCacheIndex {
    root: PathBuf,
}

impl ActionCacheIndex {
    pub fn open(store_root: impl Into<PathBuf>) -> Self {
        Self {
            root: store_root.into().join(ACTIONCACHE_REL_DIR),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn put(&self, entry: ActionCacheEntry) -> Result<(), ActionCacheError> {
        validate_key(&entry.key)?;
        if entry.artifact_ref.is_empty() {
            return Err(ActionCacheError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "actioncache artifact_ref must be non-empty",
            )));
        }
        if crate::unexpanded_tilde_path(&self.root) {
            return Err(ActionCacheError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpanded ~ store path: {}", self.root.display()),
            )));
        }
        // Tenancy (ZS-CACHE-015): never clobber another world's live validity
        // record. A live entry written under a different world stays in place;
        // the write-through caller still recorded its own decision.
        if let Some(existing) = self.load_raw(&entry.key)? {
            if !existing.tombstone && existing.world_id != entry.world_id {
                return Ok(());
            }
        }
        let stamp = StoreSchemaVersion::CURRENT.stamp();
        let segment = ActionCacheSegment {
            schema: stamp.schema.to_string(),
            major: stamp.major,
            minor: stamp.minor,
            entry,
        };
        let bytes = serde_json::to_vec(&segment)?;
        write_actioncache_segment(&self.segment_path(&segment.entry.key), &bytes)?;
        Ok(())
    }

    /// Pin a live entry for serve. GC cannot delete the artifact while
    /// the returned guard is held.
    pub fn serve(
        &self,
        key: &str,
    ) -> Result<Option<(ActionCacheEntry, ServedArtifact)>, ActionCacheError> {
        let Some(entry) = self.get(key)? else {
            return Ok(None);
        };
        if entry.l3_cold {
            // L2-valid / L3-cold: the blob is gone; refetch before use and
            // never hand out a ref to an evicted blob.
            return Ok(None);
        }
        let path = self.serve_path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"1")?;
        // Own the pin immediately so a later get() error still unlinks it.
        // Recheck after the pin is visible so a concurrent eviction that
        // marked L3-cold (or tombstoned) between the first get and the pin
        // cannot hand out a dangling artifact_ref.
        let pin = ServedArtifact { path };
        match self.get(key) {
            Ok(Some(fresh)) if !fresh.l3_cold => Ok(Some((fresh, pin))),
            Ok(_) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<ActionCacheEntry>, ActionCacheError> {
        validate_key(key)?;
        let path = self.segment_path(key);
        recover_actioncache_segment(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let segment: ActionCacheSegment = serde_json::from_slice(&bytes)?;
        admit_loaded_stamp(&segment)?;
        if segment.entry.tombstone {
            return Ok(None);
        }
        Ok(Some(segment.entry))
    }

    pub fn tombstone(&self, key: &str) -> Result<bool, ActionCacheError> {
        self.tombstone_at(key, unix_now())
    }

    pub fn tombstone_at(&self, key: &str, now_unix: u64) -> Result<bool, ActionCacheError> {
        let Some(mut entry) = self.get(key)? else {
            return Ok(false);
        };
        entry.tombstone = true;
        entry.tombstoned_at_unix = Some(now_unix);
        self.put(entry)?;
        Ok(true)
    }

    /// L3 loss (ZS-CACHE-013): preserve the entry's L2 validity and mark it
    /// needs-refetch. Mirror of hub `LayerValidityLedger::mark_l3_loss`;
    /// never a tombstone, and the validity record is never deleted. Returns
    /// false when the key has no live entry.
    pub fn mark_l3_loss(&self, key: &str, now_unix: u64) -> Result<bool, ActionCacheError> {
        let Some(mut entry) = self.get(key)? else {
            return Ok(false);
        };
        if entry.l3_cold {
            // Idempotent: preserve the original cold timestamp so repeated
            // planning calls do not extend the grace window.
            return Ok(true);
        }
        entry.l3_cold = true;
        entry.cold_since_unix = Some(now_unix);
        self.put(entry)?;
        Ok(true)
    }

    /// Complete a refetch of identical bytes (ZS-CACHE-013): L3 restored,
    /// the causal identity never rediscovered. Mirror of hub
    /// `LayerValidityLedger::complete_refetch`. Returns false when the key
    /// is missing or not marked L3-cold.
    pub fn complete_refetch(&self, key: &str) -> Result<bool, ActionCacheError> {
        let Some(mut entry) = self.get(key)? else {
            return Ok(false);
        };
        if !entry.l3_cold {
            return Ok(false);
        }
        entry.l3_cold = false;
        entry.cold_since_unix = None;
        self.put(entry)?;
        Ok(true)
    }

    /// World-scoped logical resolution (ZS-CACHE-015): an entry written under
    /// one world must not resolve under another. Unscoped (legacy) entries
    /// resolve for any caller; a scoped entry never leaks to an unscoped one.
    pub fn resolve(
        &self,
        key: &str,
        world_id: Option<&str>,
    ) -> Result<Option<ActionCacheEntry>, ActionCacheError> {
        let Some(entry) = self.get(key)? else {
            return Ok(None);
        };
        if !world_matches(entry.world_id.as_deref(), world_id) {
            return Ok(None);
        }
        Ok(Some(entry))
    }

    /// Eviction ordering: mark referencing entries L3-cold first (L2 validity
    /// preserved, needs-refetch; blob eviction never tombstones). The blob may
    /// be deleted only after every referencing entry is L3-cold (or
    /// tombstoned), grace has elapsed, no serve is in flight, and the
    /// eviction-slack guard approves the mass impact (ZS-CACHE-012/013).
    pub fn prepare_blob_eviction(
        &self,
        artifact_ref: &str,
        now_unix: u64,
        grace_secs: u64,
        slack: EvictionSlackGuard,
        evict_weight: u64,
    ) -> Result<BlobEvictionPlan, ActionCacheError> {
        // Consult the slack guard before any state change: a refused eviction
        // fails loudly with zero side effects on the index.
        slack.guard_eviction(evict_weight)?;
        let live = self.keys_for_artifact(artifact_ref, false)?;
        let mut cold_keys = Vec::new();
        for key in &live {
            if self.mark_l3_loss(key, now_unix)? {
                cold_keys.push(key.clone());
            }
        }
        let mut waiting_grace = Vec::new();
        let mut may_delete_blob = true;
        for key in self.keys_for_artifact(artifact_ref, true)? {
            if self.serve_path(&key).exists() {
                may_delete_blob = false;
            }
        }
        for key in self.keys_for_artifact(artifact_ref, true)? {
            let Some(entry) = self.load_raw(&key)? else {
                continue;
            };
            if !entry.tombstone && !entry.l3_cold {
                may_delete_blob = false;
                continue;
            }
            let since = if entry.tombstone {
                entry.tombstoned_at_unix
            } else {
                entry.cold_since_unix
            };
            match since {
                Some(at) if now_unix.saturating_sub(at) >= grace_secs => {}
                _ => {
                    may_delete_blob = false;
                    waiting_grace.push(key);
                }
            }
        }
        Ok(BlobEvictionPlan {
            artifact_ref: artifact_ref.to_string(),
            cold_keys,
            waiting_grace,
            may_delete_blob,
        })
    }

    pub fn keys_for_artifact(
        &self,
        artifact_ref: &str,
        include_tombstones: bool,
    ) -> Result<Vec<String>, ActionCacheError> {
        let mut keys = Vec::new();
        for key in self.all_keys()? {
            let Some(entry) = self.load_raw(&key)? else {
                continue;
            };
            if entry.artifact_ref != artifact_ref {
                continue;
            }
            if entry.tombstone && !include_tombstones {
                continue;
            }
            keys.push(key);
        }
        keys.sort();
        Ok(keys)
    }

    fn load_raw(&self, key: &str) -> Result<Option<ActionCacheEntry>, ActionCacheError> {
        validate_key(key)?;
        let path = self.segment_path(key);
        recover_actioncache_segment(&path)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        let segment: ActionCacheSegment = serde_json::from_slice(&bytes)?;
        admit_loaded_stamp(&segment)?;
        Ok(Some(segment.entry))
    }

    fn all_keys(&self) -> Result<Vec<String>, ActionCacheError> {
        let mut keys = Vec::new();
        if !self.root.exists() {
            return Ok(keys);
        }
        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            if shard.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            for file in fs::read_dir(shard.path())? {
                let file = file?;
                let name = file.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(key) = name.strip_suffix(".json") else {
                    continue;
                };
                if validate_key(key).is_ok() {
                    keys.push(key.to_string());
                }
            }
        }
        keys.sort();
        Ok(keys)
    }

    /// Live artifact refs for GC root-set consumption.
    pub fn live_artifact_refs(&self) -> Result<Vec<String>, ActionCacheError> {
        let mut refs = Vec::new();
        for key in self.live_keys()? {
            if let Some(entry) = self.get(&key)? {
                refs.push(entry.artifact_ref);
            }
        }
        refs.sort();
        refs.dedup();
        Ok(refs)
    }

    /// Whether ActionCache still protects this CAS hash. Live entries,
    /// L3-cold entries, and tombstones inside the grace window all pin;
    /// an approved cold eviction or tombstone past grace releases. Unreadable
    /// indexes report protected so a sweep cannot collect through a damaged
    /// root set.
    pub fn protects_hash(
        &self,
        full_hash: &str,
        now_unix: u64,
        grace_secs: u64,
    ) -> Result<bool, ActionCacheError> {
        for key in self.all_keys()? {
            let Some(entry) = self.load_raw(&key)? else {
                continue;
            };
            if artifact_full_hash(&entry.artifact_ref) != Some(full_hash) {
                continue;
            }
            if !entry.tombstone && !entry.l3_cold {
                return Ok(true);
            }
            let since = if entry.tombstone {
                entry.tombstoned_at_unix
            } else {
                entry.cold_since_unix
            };
            match since {
                Some(at) if now_unix.saturating_sub(at) >= grace_secs => {}
                _ => return Ok(true),
            }
        }
        Ok(false)
    }

    pub fn live_keys(&self) -> Result<Vec<String>, ActionCacheError> {
        let mut keys = Vec::new();
        for key in self.all_keys()? {
            if self.get(&key)?.is_some() {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    fn segment_path(&self, key: &str) -> PathBuf {
        self.root.join(&key[..2]).join(format!("{key}.json"))
    }

    fn serve_path(&self, key: &str) -> PathBuf {
        self.root.join(".serves").join(key)
    }

    pub fn has_in_flight_serve(&self, key: &str) -> bool {
        validate_key(key).is_ok() && self.serve_path(key).exists()
    }
}

fn admit_loaded_stamp(segment: &ActionCacheSegment) -> Result<(), ActionCacheError> {
    if segment.schema != crate::store_schema::STORE_SCHEMA_NAME {
        return Err(ActionCacheError::Schema(SchemaSkewError::WrongSchema {
            found: segment.schema.clone(),
        }));
    }
    admit_store_schema(&StoreSchemaStamp {
        schema: crate::store_schema::STORE_SCHEMA_NAME,
        major: segment.major,
        minor: segment.minor,
    })
    .map(|_| ())
    .map_err(ActionCacheError::Schema)
}

/// Full CAS hash protected by a live ActionCache entry.
pub fn artifact_full_hash(artifact_ref: &str) -> Option<&str> {
    let rest = artifact_ref.strip_prefix("tz://blob/")?;
    let ok = rest.len() == 64
        && rest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    ok.then_some(rest)
}

/// Whether ActionCache still protects this CAS hash. Live entries and
/// tombstones inside the grace window both pin. Unreadable indexes report
/// protected so a sweep cannot collect through a damaged root set.
pub fn action_cache_protects_hash(store_root: &Path, full_hash: &str) -> bool {
    let index = ActionCacheIndex::open(store_root);
    index
        .protects_hash(full_hash, unix_now(), ACTIONCACHE_GC_GRACE_SECS)
        .unwrap_or(true)
}

/// Tenancy match (ZS-CACHE-015): unscoped (legacy) entries are global; a
/// scoped entry resolves only under its own world and never for an unscoped
/// resolver.
fn world_matches(entry_world: Option<&str>, resolver_world: Option<&str>) -> bool {
    match (entry_world, resolver_world) {
        (None, _) => true,
        (Some(entry), Some(resolver)) => entry == resolver,
        (Some(_), None) => false,
    }
}

/// PPM helper: `numerator / denominator * 1_000_000`, saturating.
fn ppm_of(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    numerator.saturating_mul(1_000_000) / denominator
}

/// 99% retained-mass floor. `None` when `demanded_mass * 99` does not fit in u64.
fn retained_floor(demanded_mass: u64) -> Option<u64> {
    demanded_mass.checked_mul(99).map(|n| n / 100)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn validate_key(key: &str) -> Result<(), ActionCacheError> {
    let ok = key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if ok {
        Ok(())
    } else {
        Err(ActionCacheError::InvalidKey(key.to_string()))
    }
}
