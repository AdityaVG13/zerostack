//! CoverageCertificate and Gap types.

use graphzero_store::{BlobId, Tier};

/// Seconds since Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

/// Why a blob is not trusted for a tier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GapReason {
    NotIndexed,
    Stale,
    Unparseable,
}

impl std::fmt::Display for GapReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GapReason::NotIndexed => write!(f, "not_indexed"),
            GapReason::Stale => write!(f, "stale"),
            GapReason::Unparseable => write!(f, "unparseable"),
        }
    }
}

/// A single coverage gap entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Gap {
    pub blob_id: BlobId,
    pub tier: Tier,
    pub reason: GapReason,
}

impl Gap {
    pub fn new(blob_id: BlobId, tier: Tier, reason: GapReason) -> Self {
        Self {
            blob_id,
            tier,
            reason,
        }
    }
}

/// Certificate attached to every query answer.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageCertificate {
    pub tier_a_pct: f64,
    pub tier_b_pct: f64,
    pub tier_c_pct: f64,
    pub freshness_verified: bool,
    pub gaps: Vec<Gap>,
    pub generated_at: Timestamp,
}

impl CoverageCertificate {
    pub fn new(generated_at: Timestamp) -> Self {
        Self {
            tier_a_pct: 0.0,
            tier_b_pct: 0.0,
            tier_c_pct: 0.0,
            freshness_verified: false,
            gaps: Vec::new(),
            generated_at,
        }
    }

    /// Estimate serialized size (upper bound).
    pub fn estimated_size(&self) -> usize {
        // rough upper bound: 3 f64 + bool + timestamp + gap overhead
        let base = 8 * 3 + 1 + 8 + 24; // Vec overhead
        let per_gap = 32 + 8 + 16; // BlobId (~32), Tier (8), GapReason (~16)
        base + self.gaps.len() * per_gap
    }
}
