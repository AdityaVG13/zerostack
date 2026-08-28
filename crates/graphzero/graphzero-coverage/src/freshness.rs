//! Lazy freshness verification: compare live blob bytes to stored content hash.

use graphzero_store::ContentHash;
use std::fmt;

/// Error during freshness check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FreshnessError {
    Io(String),
    MissingStoredHash,
    HashMismatch { stored: String, live: String },
}

impl fmt::Display for FreshnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FreshnessError::Io(s) => write!(f, "io error: {}", s),
            FreshnessError::MissingStoredHash => write!(f, "no stored hash"),
            FreshnessError::HashMismatch { stored, live } => {
                write!(f, "hash mismatch stored={} live={}", stored, live)
            }
        }
    }
}

impl std::error::Error for FreshnessError {}

/// Trait for providing live bytes of a blob.
pub trait LiveBytesProvider: Send + Sync {
    fn live_bytes(&self, _blob_path_hint: &str) -> Result<Vec<u8>, FreshnessError> {
        Ok(Vec::new())
    }
}

/// Default provider that returns empty bytes (for tests / warm mode where
/// freshness is already verified).
pub struct EmptyProvider;
impl LiveBytesProvider for EmptyProvider {}

/// File-system provider that reads the file at the given path.
pub struct FsProvider;
impl LiveBytesProvider for FsProvider {
    fn live_bytes(&self, blob_path_hint: &str) -> Result<Vec<u8>, FreshnessError> {
        std::fs::read(blob_path_hint).map_err(|e| FreshnessError::Io(e.to_string()))
    }
}

/// Check whether live bytes match the stored content hash.
///
/// Returns `Ok(true)` when fresh, `Ok(false)` when stale, `Err` when the
/// check could not be performed.
pub fn freshness_check(
    stored: Option<&ContentHash>,
    live_bytes: &[u8],
) -> Result<bool, FreshnessError> {
    let stored = stored.ok_or(FreshnessError::MissingStoredHash)?;
    let live_hash = compute_hash(live_bytes);
    Ok(stored.0 == live_hash.0)
}

/// Compute the SHA-256 content hash of bytes.
pub fn compute_hash(bytes: &[u8]) -> ContentHash {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    ContentHash::from_bytes(out)
}
