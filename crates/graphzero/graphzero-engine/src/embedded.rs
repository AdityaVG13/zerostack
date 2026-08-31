//! Embedding API for the single zerostack binary. The host owns the store path
//! and passes it explicitly; this module keeps no process-global state, so
//! multiple embedded instances can target different shared stores in one process.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::indexer::index_repo as index_repo_into_store;
use graphzero_store::{ContentHash, SharedCas, Snapshot};
use serde::{Deserialize, Serialize};

use crate::blast::{BlastError, BlastRadiusCapsule, blast_radius, blast_radius_with_depth};

/// Sibling-scheme spellings of one content-addressed identity.
/// A sibling ref resolves only when the same bytes are reachable and digest-verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedBlobRef {
    pub hash: String,
    pub gz_ref: String,
    pub fz_ref: String,
    pub tz_ref: String,
}

impl SharedBlobRef {
    pub fn for_hash(hash: impl Into<String>) -> Self {
        let hash = hash.into();
        Self {
            gz_ref: format!("z://blob/{hash}"),
            fz_ref: format!("z://blob/{hash}"),
            tz_ref: format!("z://blob/{hash}"),
            hash,
        }
    }

    pub fn for_bytes(bytes: &[u8]) -> Self {
        Self::for_hash(ContentHash::of(bytes).to_hex())
    }
}

#[derive(Debug, Clone)]
pub struct SharedGraphZeroStore {
    root: PathBuf,
}

impl SharedGraphZeroStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blob_store(&self) -> Result<BlobStore> {
        BlobStore::open(&self.root)
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddedGraphZero {
    store_root: PathBuf,
    repo_root: Option<PathBuf>,
}

impl EmbeddedGraphZero {
    pub fn new(store_root: impl Into<PathBuf>, repo_root: Option<impl Into<PathBuf>>) -> Self {
        Self {
            store_root: store_root.into(),
            repo_root: repo_root.map(Into::into),
        }
    }

    pub fn from_shared_store(
        store: SharedGraphZeroStore,
        repo_root: Option<impl Into<PathBuf>>,
    ) -> Self {
        Self {
            store_root: store.root,
            repo_root: repo_root.map(Into::into),
        }
    }

    pub fn shared_store(&self) -> SharedGraphZeroStore {
        SharedGraphZeroStore::new(self.store_root.clone())
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    pub fn repo_root(&self) -> Option<&Path> {
        self.repo_root.as_deref()
    }

    pub fn snapshot(&self) -> Result<Arc<Snapshot>> {
        Snapshot::open_cached(&self.store_root, self.repo_root.as_deref())
            .with_context(|| format!("open graphzero snapshot at {}", self.store_root.display()))
    }

    pub fn index_repo(&self) -> Result<()> {
        let repo_root = self
            .repo_root
            .as_deref()
            .ok_or_else(|| anyhow!("index_repo requires repo_root"))?;
        index_repo_into_store(repo_root, &self.store_root).map(|_| ())
    }

    pub fn blast_radius(
        &self,
        intent: &str,
        budget: usize,
    ) -> Result<BlastRadiusCapsule, BlastError> {
        let snapshot = self
            .snapshot()
            .map_err(|e| BlastError::Store(e.to_string()))?;
        blast_radius(&snapshot, intent, budget)
    }

    pub fn blast_radius_with_depth(
        &self,
        intent: &str,
        budget: usize,
        max_depth: u32,
    ) -> Result<BlastRadiusCapsule, BlastError> {
        let snapshot = self
            .snapshot()
            .map_err(|e| BlastError::Store(e.to_string()))?;
        blast_radius_with_depth(&snapshot, intent, budget, max_depth)
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<SharedBlobRef> {
        let store = self.shared_store().blob_store()?;
        let hash = store.put(bytes)?;
        Ok(SharedBlobRef::for_hash(hash.to_hex()))
    }

    /// Handle to the canonical ZeroRef CAS under this store root
    /// (`blobs/sha256/<hh>/<hash>`, ADR 002 §7).
    pub fn shared_cas(&self) -> SharedCas {
        SharedCas::open(&self.store_root)
    }

    /// ZeroRef rollout path: publish bytes into the canonical CAS layout and emit the full-hash sibling
    /// refs. `put_blob` remains the flat-store write; hosts opt in storage by calling this method instead.
    pub fn put_blob_cas(&self, bytes: &[u8]) -> Result<SharedBlobRef> {
        let hash = self
            .shared_cas()
            .put(bytes)
            .map_err(|e| anyhow!("cas put: {e}"))?;
        Ok(SharedBlobRef::for_hash(hash))
    }
}
