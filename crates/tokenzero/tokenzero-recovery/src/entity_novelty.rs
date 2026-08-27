//! Shared entity novelty (`zerostack.entity-novelty`).
//!
//! Contract owner: ZeroStack. GraphZero owns `EntityId` minting;
//! this module only stores/loads 64-hex digests and always displays them as
//! `gz://entity/<id>`. There is no `tz://entity/` namespace.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokenzero_core::sha256_hex;

/// Frozen schema id.
pub const ENTITY_NOVELTY_SCHEMA_VERSION: &str = "zerostack.entity-novelty";

/// Record type constant.
pub const ENTITY_NOVELTY_RECORD_TYPE: &str = "entity-novelty";

/// Relative directory under a ZeroStack store root.
pub const ENTITY_NOVELTY_REL_DIR: &str = "shared/entity-novelty/v1";

/// Engines that may write the shared novelty pointer.
pub const PRODUCING_ENGINES: &[&str] = &["tokenzero", "fszero", "graphzero"];

/// Shared known-entity novelty set for one scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityNoveltyRecord {
    pub schema_version: String,
    pub record_type: String,
    pub scope_key: String,
    pub entity_ids: Vec<String>,
    pub producing_engine: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cas_digest: Option<String>,
}

impl EntityNoveltyRecord {
    /// Build an empty validated record for `scope_key`.
    pub fn empty(
        scope_key: impl Into<String>,
        producing_engine: &str,
    ) -> Result<Self, NoveltyError> {
        let scope_key = scope_key.into();
        validate_scope_key(&scope_key)?;
        validate_engine(producing_engine)?;
        Ok(Self {
            schema_version: ENTITY_NOVELTY_SCHEMA_VERSION.to_string(),
            record_type: ENTITY_NOVELTY_RECORD_TYPE.to_string(),
            scope_key,
            entity_ids: Vec::new(),
            producing_engine: producing_engine.to_string(),
            updated_at: now_rfc3339(),
            cas_digest: None,
        })
    }

    /// Canonical GraphZero-owned ref for a stored id (`gz://entity/<id>` only).
    pub fn entity_ref(entity_id: &str) -> Result<String, NoveltyError> {
        validate_entity_id(entity_id)?;
        Ok(format!("gz://entity/{entity_id}"))
    }

    pub fn knows(&self, entity_id: &str) -> bool {
        self.entity_ids.iter().any(|id| id == entity_id)
    }

    /// Union `ids` into this record (sorted unique). Rejects scheme-prefixed values.
    pub fn merge_ids(
        &mut self,
        ids: impl IntoIterator<Item = impl AsRef<str>>,
        producing_engine: &str,
    ) -> Result<usize, NoveltyError> {
        validate_engine(producing_engine)?;
        let mut set: BTreeSet<String> = self.entity_ids.iter().cloned().collect();
        let before = set.len();
        for id in ids {
            let id = id.as_ref();
            validate_entity_id(id)?;
            set.insert(id.to_ascii_lowercase());
        }
        let added = set.len().saturating_sub(before);
        self.entity_ids = set.into_iter().collect();
        self.producing_engine = producing_engine.to_string();
        self.updated_at = now_rfc3339();
        Ok(added)
    }

    pub fn validate(&self) -> Result<(), NoveltyError> {
        if self.schema_version != ENTITY_NOVELTY_SCHEMA_VERSION {
            return Err(NoveltyError::Schema(self.schema_version.clone()));
        }
        if self.record_type != ENTITY_NOVELTY_RECORD_TYPE {
            return Err(NoveltyError::RecordType(self.record_type.clone()));
        }
        validate_scope_key(&self.scope_key)?;
        validate_engine(&self.producing_engine)?;
        let mut seen = BTreeSet::new();
        for id in &self.entity_ids {
            validate_entity_id(id)?;
            if !seen.insert(id.clone()) {
                return Err(NoveltyError::DuplicateId(id.clone()));
            }
        }
        if let Some(digest) = &self.cas_digest {
            validate_entity_id(digest)?;
        }
        Ok(())
    }
}

/// Errors for the shared novelty contract.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NoveltyError {
    #[error("unsupported entity-novelty schema_version: {0}")]
    Schema(String),
    #[error("unsupported entity-novelty record_type: {0}")]
    RecordType(String),
    #[error("invalid scope_key: {0}")]
    Scope(String),
    #[error("invalid producing_engine: {0}")]
    Engine(String),
    #[error("entity id must be 64 lowercase hex (no scheme); got {0}")]
    EntityId(String),
    #[error("duplicate entity id: {0}")]
    DuplicateId(String),
    #[error("entity refs must use gz://entity/; refused {0}")]
    ForbiddenScheme(String),
    #[error("io: {0}")]
    Io(String),
    #[error("json: {0}")]
    Json(String),
}

impl From<io::Error> for NoveltyError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for NoveltyError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err.to_string())
    }
}

/// SHA-256 hex of UTF-8 `scope_key` (filename under the shared novelty dir).
pub fn scope_digest(scope_key: &str) -> String {
    sha256_hex(scope_key)
}

/// Path to the mutable novelty pointer for `scope_key`.
pub fn entity_novelty_path(store_root: &Path, scope_key: &str) -> PathBuf {
    store_root
        .join(ENTITY_NOVELTY_REL_DIR)
        .join(format!("{}.json", scope_digest(scope_key)))
}

/// Load a novelty record; missing file yields an empty validated record.
pub fn read_entity_novelty(
    store_root: &Path,
    scope_key: &str,
) -> Result<EntityNoveltyRecord, NoveltyError> {
    let path = entity_novelty_path(store_root, scope_key);
    match fs::read(&path) {
        Ok(bytes) => {
            let record: EntityNoveltyRecord = serde_json::from_slice(&bytes)?;
            record.validate()?;
            if record.scope_key != scope_key {
                return Err(NoveltyError::Scope(format!(
                    "file scope_key {} != requested {scope_key}",
                    record.scope_key
                )));
            }
            Ok(record)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            EntityNoveltyRecord::empty(scope_key, "tokenzero")
        }
        Err(err) => Err(err.into()),
    }
}

/// Persist a validated novelty pointer (creates parent dirs).
pub fn write_entity_novelty(
    store_root: &Path,
    record: &EntityNoveltyRecord,
) -> Result<PathBuf, NoveltyError> {
    record.validate()?;
    let path = entity_novelty_path(store_root, &record.scope_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(record)?;
    // Unique tmp + rename: a shared `*.json.tmp` plus `fs::write` can clobber
    // dest when two writers race, and kill-after-truncate of that tmp is not
    // dest-safe either. Hub replace never truncates dest in place.
    zero_store::atomic_write_file(&path, &bytes)?;
    Ok(path)
}

/// Union `ids` into the on-disk novelty set for `scope_key`.
pub fn merge_entity_novelty(
    store_root: &Path,
    scope_key: &str,
    ids: &[String],
    producing_engine: &str,
    cas_digest: Option<&str>,
) -> Result<EntityNoveltyRecord, NoveltyError> {
    let mut record = read_entity_novelty(store_root, scope_key)?;
    // Fresh empty from missing file may carry tokenzero; overwrite engine on merge.
    record.merge_ids(ids.iter().map(String::as_str), producing_engine)?;
    if let Some(digest) = cas_digest {
        validate_entity_id(digest)?;
        record.cas_digest = Some(digest.to_ascii_lowercase());
    }
    write_entity_novelty(store_root, &record)?;
    Ok(record)
}

/// Refuse any entity URI that is not the GraphZero-owned `gz://entity/` form.
pub fn parse_entity_ref(reference: &str) -> Result<String, NoveltyError> {
    if let Some(rest) = reference.strip_prefix("gz://entity/") {
        validate_entity_id(rest)?;
        return Ok(rest.to_ascii_lowercase());
    }
    if reference.starts_with("tz://entity/") || reference.starts_with("fz://entity/") {
        return Err(NoveltyError::ForbiddenScheme(reference.to_string()));
    }
    // Bare 64-hex is accepted as an EntityId digest.
    validate_entity_id(reference)?;
    Ok(reference.to_ascii_lowercase())
}

fn validate_scope_key(scope_key: &str) -> Result<(), NoveltyError> {
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
        Err(NoveltyError::Scope(scope_key.to_string()))
    }
}

fn validate_engine(engine: &str) -> Result<(), NoveltyError> {
    if PRODUCING_ENGINES.contains(&engine) {
        Ok(())
    } else {
        Err(NoveltyError::Engine(engine.to_string()))
    }
}

fn validate_entity_id(hex: &str) -> Result<(), NoveltyError> {
    if hex.contains("://") {
        return Err(NoveltyError::EntityId(hex.to_string()));
    }
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(NoveltyError::EntityId(hex.to_string()));
    }
    if hex.bytes().any(|b| b.is_ascii_uppercase()) {
        // Normalize path accepts uppercase only via to_ascii_lowercase callers;
        // stored form must be lowercase.
        return Err(NoveltyError::EntityId(hex.to_string()));
    }
    Ok(())
}

fn now_rfc3339() -> String {
    crate::shared_cas::format_system_time(SystemTime::now())
}
