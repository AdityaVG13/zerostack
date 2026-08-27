//! Replication topology + repair-from-replica (ZS-STORE-007 FSZero part).
//!
//! Minimal honest replication for the physical CAS:
//!
//! - A `replication_targets` declaration at `<store_root>/gc/replication.json`
//!   names replica store roots (each a ZeroStack store root holding
//!   `blobs/sha256/...`).
//! - GC consults the declaration **before evicting**: a blob slated for
//!   removal is first published to every declared replica (replicate-before-
//!   evict, via the hub `SharedCas` verified publish path). If any replica
//!   copy fails, the local blob is NOT evicted (fail-loud, zero side
//!   effects) and the refusal lands in the GC report.
//! - `repair_from_replicas` pulls a missing blob from the first replica that
//!   has it and re-publishes it locally through the CAS put path, which
//!   restores L3 validity in the ledger with the same identity (no
//!   rediscovery).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Frozen schema id for the replication declaration (zerostack.cas-replication).
pub const REPLICATION_SCHEMA_VERSION: &str = "zerostack.cas-replication";
/// Declaration file under `<store_root>/gc/`.
pub const REPLICATION_FILE: &str = "replication.json";

/// Typed replication failures.
#[derive(Debug)]
pub enum ReplicationError {
    Io {
        target: String,
        context: String,
        source: std::io::Error,
    },
    /// A declared target cannot serve as a replica store root.
    InvalidTarget(String),
    /// Malformed declaration file.
    Config(String),
}

impl fmt::Display for ReplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplicationError::Io {
                target,
                context,
                source,
            } => write!(f, "replication io (target {target}, {context}): {source}"),
            ReplicationError::InvalidTarget(t) => {
                write!(f, "replication: invalid replica target {t}")
            }
            ReplicationError::Config(msg) => write!(f, "replication config: {msg}"),
        }
    }
}

impl std::error::Error for ReplicationError {}

/// Declared replication topology. All fields serde-defaulted; a missing or
/// empty declaration means "no replication" (legacy behaviour: GC evicts
/// without consulting replicas).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicationConfig {
    pub schema_version: String,
    /// Replica store roots. Relative entries resolve against the local store
    /// root. Each target must hold (or accept) a `blobs/sha256/...` layout.
    pub targets: Vec<String>,
}

impl ReplicationConfig {
    /// Load the declaration from `<store_root>/gc/replication.json`.
    /// Missing file == no replication; malformed file is a loud error (a
    /// broken declaration must never silently disable the eviction hook).
    pub fn load(store_root: &Path) -> Result<Self, ReplicationError> {
        let path = store_root.join("gc").join(REPLICATION_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(Self::default());
        };
        let config: ReplicationConfig = serde_json::from_slice(&bytes)
            .map_err(|e| ReplicationError::Config(format!("{}: {e}", path.display())))?;
        if config.schema_version.is_empty() {
            return Err(ReplicationError::Config(format!(
                "{}: missing schema_version (expected {REPLICATION_SCHEMA_VERSION})",
                path.display()
            )));
        }
        Ok(config)
    }

    pub fn is_declared(&self) -> bool {
        !self.targets.is_empty()
    }

    /// Resolve a declared target against the local store root (relative
    /// entries are store-root relative).
    pub fn resolve_target(&self, store_root: &Path, target: &str) -> PathBuf {
        let path = PathBuf::from(target);
        if path.is_absolute() {
            path
        } else {
            store_root.join(path)
        }
    }

    /// A replica is the local store itself (e.g. store root == target):
    /// replicating to self is a no-op that must not be treated as a failure.
    pub fn is_self(&self, store_root: &Path, target: &str) -> bool {
        let resolved = self.resolve_target(store_root, target);
        match (
            std::fs::canonicalize(&resolved),
            std::fs::canonicalize(store_root),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => resolved == store_root,
        }
    }
}

/// Publish `bytes` (already verified locally, digest matches `hash`) to every
/// declared replica target. Idempotent: a target that already contains the
/// hash is skipped. Returns the number of replicas that newly received the
/// blob. Any failure fails loud; the caller must NOT evict the local copy.
pub fn replicate_before_evict(
    store_root: &Path,
    config: &ReplicationConfig,
    hash: &str,
    bytes: &[u8],
) -> Result<u64, ReplicationError> {
    let mut written = 0u64;
    for target in &config.targets {
        if config.is_self(store_root, target) {
            continue;
        }
        let root = config.resolve_target(store_root, target);
        // A replica store root must exist to hold the coordination lock and
        // the blobs layout; create the layout on first use.
        std::fs::create_dir_all(root.join("blobs")).map_err(|e| ReplicationError::Io {
            target: target.clone(),
            context: "create replica layout".to_string(),
            source: e,
        })?;
        let cas = zero_store::SharedCas::open(&root);
        if cas.contains(hash) {
            continue;
        }
        cas.put_prehashed(hash, bytes)
            .map_err(|e| ReplicationError::Io {
                target: target.clone(),
                context: format!("verified replica publish: {e}"),
                source: std::io::Error::other(format!(
                    "replica publish failed for {hash} at {}: {e}",
                    root.display()
                )),
            })?;
        written += 1;
    }
    Ok(written)
}

/// Result of a repair attempt from replicas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairOutcome {
    /// `true` when a replica held the blob and it was re-published locally
    /// (L3 validity restored via the CAS put path, same identity).
    pub restored: bool,
    /// Number of replicas that were checked before the source was found
    /// (or all were checked and missed).
    pub checked: u64,
}

/// Pull `hash` from the first declared replica that has it and re-publish it
/// locally (restores L3 validity with the same identity; no rediscovery).
/// `restored: false` when no replica has the blob.
pub fn repair_from_replicas(
    store_root: &Path,
    config: &ReplicationConfig,
    hash: &str,
) -> Result<RepairOutcome, ReplicationError> {
    let mut checked = 0u64;
    for target in &config.targets {
        if config.is_self(store_root, target) {
            continue;
        }
        let root = config.resolve_target(store_root, target);
        let cas = zero_store::SharedCas::open(&root);
        checked += 1;
        if !cas.contains(hash) {
            continue;
        }
        let bytes = cas.get_verified(hash).map_err(|e| ReplicationError::Io {
            target: target.clone(),
            context: format!("replica read: {e}"),
            source: std::io::Error::other(format!(
                "replica read failed for {hash} at {}: {e}",
                root.display()
            )),
        })?;
        // Local re-publish goes through the verified CAS put path; the
        // validity ledger restores L3 from the cold record (same identity).
        let local = zero_store::SharedCas::open(store_root);
        local
            .put_prehashed(hash, &bytes)
            .map_err(|e| ReplicationError::Io {
                target: target.clone(),
                context: format!("local restore publish: {e}"),
                source: std::io::Error::other(format!(
                    "local restore failed for {hash} at {}: {e}",
                    store_root.display()
                )),
            })?;
        return Ok(RepairOutcome {
            restored: true,
            checked,
        });
    }
    Ok(RepairOutcome {
        restored: false,
        checked,
    })
}
