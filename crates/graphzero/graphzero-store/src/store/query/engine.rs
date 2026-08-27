//! Convenience facade used by the CLI and benchmarks.

use std::path::Path;

use super::snapshot::Snapshot;
use super::types::Capsule;

pub struct QueryEngine;

impl QueryEngine {
    /// Cold query: open, query with freshness check, drop (FR-009).
    pub fn cold(
        store_root: &Path,
        repo_root: Option<&Path>,
        symbol: &str,
        budget: usize,
    ) -> anyhow::Result<Capsule> {
        let snapshot = Snapshot::open(store_root, repo_root)?;
        snapshot.query_with_repair(symbol, budget, true, true)
    }

    /// Warm query against an already-open snapshot (FR-008).
    pub fn warm(snapshot: &Snapshot, symbol: &str, budget: usize) -> anyhow::Result<Capsule> {
        snapshot.query(symbol, budget, false)
    }
}
