//! Embeddable FSZero store handle (fszero-c6q.8).
//!
//! `FsZeroStore` is a non-global, in-process handle that owns a durable
//! recovery store and an optional shared CAS tier. It is intended for the
//! single ZeroStack binary where each engine (FSZero, GraphZero, TokenZero)
//! holds its own isolated store handle while sharing the same canonical CAS
//! layout under a single store root.
//!
//! The descriptor and contract produced by an embedded handle are byte-for-byte
//! identical to those produced by the standalone CLI session (`FSZeroSession`).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::capability::capability_descriptor_from_recovery;
use super::filesystem_contract::filesystem_contract_descriptor;
use super::recovery::RecoveryStore;
use super::zerostack_store;

/// Non-global embeddable FSZero store handle.
///
/// * Two handles with different workspace roots share the same CAS if both
///   point at the same store root (i.e. the same `.zerostack` directory).
/// * Two handles with different workspace roots but different store roots
///   are fully isolated in their durable SQLite stores and CAS attachment.
/// * The capability descriptor is identical to the standalone CLI session
///   because it is derived from the same `RecoveryStore` state.
pub struct FsZeroStore {
    pub root: Option<PathBuf>,
    recovery: RecoveryStore,
    pub durable_degraded: bool,
}

impl FsZeroStore {
    /// Open an embedded handle using the same durable-store discovery as the
    /// CLI session: `<root>/.fszero` or `<root>/.zerostack/fszero`.
    /// Attaches a shared CAS when a `.zerostack/blobs` directory exists.
    pub fn open(root: impl AsRef<Path>) -> Self {
        let root_path = root.as_ref().to_path_buf();
        match Self::try_open(&root_path) {
            Ok(s) => s,
            Err(_e) => {
                let recovery = RecoveryStore::new();
                Self {
                    root: Some(root_path),
                    recovery,
                    durable_degraded: true,
                }
            }
        }
    }

    /// Open the handle, returning `Err` rather than degrading to in-memory.
    pub fn try_open(root: impl AsRef<Path>) -> Result<Self, String> {
        let root_path = root.as_ref().to_path_buf();
        let mut recovery = super::session::prepare_repo_store(&root_path)
            .and_then(RecoveryStore::try_with_durable)?;
        recovery.attach_cas_if_detected(&root_path);
        Ok(Self {
            root: Some(root_path),
            recovery,
            durable_degraded: false,
        })
    }

    /// Create an in-memory store handle. Useful for tests and ephemeral
    /// sibling-engine contexts that do not need a durable store.
    pub fn in_memory() -> Self {
        Self {
            root: None,
            recovery: RecoveryStore::new(),
            durable_degraded: false,
        }
    }

    /// Access the underlying recovery store. This is an escape hatch; most
    /// callers should use the typed methods on this handle.
    pub fn recovery(&self) -> &RecoveryStore {
        &self.recovery
    }

    /// Mutable access to the underlying recovery store.
    pub fn recovery_mut(&mut self) -> &mut RecoveryStore {
        &mut self.recovery
    }

    /// ZeroRef v1 capability descriptor — identical to the standalone CLI.
    pub fn capability_descriptor(&self) -> Value {
        capability_descriptor_from_recovery(&self.recovery, self.durable_degraded)
    }

    /// Normative filesystem contract descriptor — identical to the standalone CLI.
    pub fn filesystem_contract_descriptor(&self) -> Value {
        filesystem_contract_descriptor().clone()
    }

    /// Store root for this handle (parent of the SQLite DB, or unified root).
    pub fn store_root(&self) -> Option<PathBuf> {
        zerostack_store::store_root_from_db_path(self.recovery.store_db_path()?)
    }

    /// Persist a byte payload and return a durable `fz://blob/<sha256>` ref.
    /// Honors `max_object_bytes` if supplied; uses the shared CAS when attached.
    pub fn put(&mut self, bytes: &[u8], max_object_bytes: Option<u64>) -> Result<String, String> {
        if let Some(limit) = max_object_bytes {
            if bytes.len() as u64 > limit {
                return Err(format!(
                    "payload {} bytes exceeds limit {limit}",
                    bytes.len()
                ));
            }
        }
        self.recovery.try_put_content_ref(bytes)
    }

    /// Expand a ref to its exact bytes. Returns `None` for unknown refs.
    pub fn expand(&self, r: &str) -> Option<Vec<u8>> {
        self.recovery.expand(r)
    }

    /// Store health, migration, and peer-incompatibility summary for telemetry
    /// and the single-binary router. Does not leak absolute private paths.
    pub fn root_report(&self) -> Value {
        let store_root = self
            .store_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "memory".to_string());
        let store_db = self
            .recovery
            .store_db_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "memory".to_string());
        let effective_root_mode = zerostack_store::effective_root_mode(&store_root);
        let cap = self.capability_descriptor();
        let (
            layout_version,
            store_health,
            migration_legacy,
            peer_incompatibility,
            last_integrity_error,
        ) = self
            .recovery
            .root_report_store_fragments(self.durable_degraded, &cap);
        serde_json::json!({
            "workspace_root": self.root.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(none)".to_string()), "store_root": store_root,
            "store_db": store_db, "durable_degraded": self.durable_degraded,
            "effective_root_mode": effective_root_mode, "layout_version": layout_version,
            "store_health": store_health, "migration_legacy": migration_legacy,
            "peer_incompatibility": peer_incompatibility, "capabilities": cap,
            "filesystem_contract": self.filesystem_contract_descriptor(), "last_integrity_error": last_integrity_error,
        })
    }

    /// Publish the capability and filesystem-contract descriptors into the
    /// store so CodeMode / MCP peers can expand them. Best-effort; never blocks.
    pub fn publish_capabilities(&mut self) {
        super::capability::publish_capability_store_keys(&mut self.recovery, self.durable_degraded);
    }
}
