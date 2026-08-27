//! GraphZero P0.3 Coverage Machinery.
//!
//! Per-blob per-tier bitmaps, lazy freshness verification, and three-answer
//! query results (PRESENT / ABSENT / UNKNOWN) with `CoverageCertificate`.

pub mod bitmap;
pub mod certificate;
pub mod confidence_algebra;
pub mod freshness;
pub mod index;
pub mod query;

pub use bitmap::{Bitmap, CATEGORY_INDEXED};
pub use certificate::{CoverageCertificate, Gap, GapReason, Timestamp};
pub use freshness::{FreshnessError, LiveBytesProvider, freshness_check};
pub use index::{CoverageError, CoverageIndex, MockCoverageIndex};
pub use query::{QueryBuildError, QueryBuildErrorKind, QueryResult, QueryResultBuilder};

// Re-export dependency types used by tests and benchmarks.
pub use graphzero_store::{BlobId, Tier};

use std::time::{SystemTime, UNIX_EPOCH};

/// Current bitmap layout version.
pub const BITMAP_VERSION: u8 = 1;

/// Helper: generate a SHA-256 hex hash of bytes (hub digest primitive).
pub fn sha256_hex(bytes: &[u8]) -> String {
    zero_abi::sha256_hex(bytes)
}

/// Current wall-clock timestamp (seconds since unix epoch).
pub fn now_timestamp() -> Timestamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Timestamp(secs)
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-coverage/lib_tests.rs"]
mod tests;
