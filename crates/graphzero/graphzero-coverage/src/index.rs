//! Coverage index integration with snapshot storage.

use crate::bitmap::Bitmap;
use graphzero_store::{BlobId, ContentHash};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Error type for coverage index operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageError {
    Io(String),
    BlobNotFound,
    VersionMismatch { expected: u8, found: u8 },
}

impl fmt::Display for CoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoverageError::Io(s) => write!(f, "io error: {}", s),
            CoverageError::BlobNotFound => write!(f, "blob not found in index"),
            CoverageError::VersionMismatch { expected, found } => {
                write!(
                    f,
                    "unsupported format version {}, expected {}",
                    found, expected
                )
            }
        }
    }
}

impl std::error::Error for CoverageError {}

/// Coverage storage implemented by shard readers and writers.
pub trait CoverageIndex {
    /// Read the coverage bitmap for a blob.
    fn read_coverage(&self, blob_id: &BlobId) -> Option<Bitmap>;

    /// Write a coverage bitmap for a blob.
    fn write_coverage(&mut self, blob_id: BlobId, bitmap: Bitmap) -> Result<(), CoverageError>;

    /// Read the stored content hash for freshness comparison.
    fn read_freshness(&self, blob_id: &BlobId) -> Option<ContentHash>;

    /// Write a stored content hash.
    fn write_freshness(&mut self, blob_id: BlobId, hash: ContentHash) -> Result<(), CoverageError>;

    /// Return all tracked blob ids.
    fn all_blob_ids(&self) -> Vec<BlobId>;

    /// Visit tracked blob ids without cloning the key set.
    fn for_each_blob_id(&self, visitor: &mut dyn FnMut(&BlobId)) {
        for blob_id in self.all_blob_ids() {
            visitor(&blob_id);
        }
    }

    /// Return the live file path for a blob, when freshness should read from disk.
    fn blob_path(&self, _blob_id: &BlobId) -> Option<&Path> {
        None
    }
}

/// In-memory mock implementation for testing.
#[derive(Clone, Debug, Default)]
pub struct MockCoverageIndex {
    coverage: HashMap<BlobId, Bitmap>,
    hashes: HashMap<BlobId, ContentHash>,
    paths: HashMap<BlobId, PathBuf>,
    order: Vec<BlobId>,
}

impl MockCoverageIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.coverage.len()
    }

    pub fn is_empty(&self) -> bool {
        self.coverage.is_empty()
    }

    pub fn write_blob_path(&mut self, blob_id: BlobId, path: impl Into<PathBuf>) {
        self.paths.insert(blob_id, path.into());
    }
}

impl CoverageIndex for MockCoverageIndex {
    fn read_coverage(&self, blob_id: &BlobId) -> Option<Bitmap> {
        self.coverage.get(blob_id).cloned()
    }

    fn write_coverage(&mut self, blob_id: BlobId, bitmap: Bitmap) -> Result<(), CoverageError> {
        if !self.coverage.contains_key(&blob_id) {
            self.order.push(blob_id.clone());
        }
        self.coverage.insert(blob_id, bitmap);
        Ok(())
    }

    fn read_freshness(&self, blob_id: &BlobId) -> Option<ContentHash> {
        self.hashes.get(blob_id).copied()
    }

    fn write_freshness(&mut self, blob_id: BlobId, hash: ContentHash) -> Result<(), CoverageError> {
        self.hashes.insert(blob_id, hash);
        Ok(())
    }

    fn all_blob_ids(&self) -> Vec<BlobId> {
        self.order.clone()
    }

    fn for_each_blob_id(&self, visitor: &mut dyn FnMut(&BlobId)) {
        for blob_id in &self.order {
            visitor(blob_id);
        }
    }

    fn blob_path(&self, blob_id: &BlobId) -> Option<&Path> {
        self.paths.get(blob_id).map(PathBuf::as_path)
    }
}
