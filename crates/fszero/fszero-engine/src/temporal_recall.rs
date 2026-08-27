//! Journal-native temporal recall with zero model calls (fszero-ute2 / Zero-Mem).
//!
//! Recall is exact journal scan + optional payload rehydrate. It never invents
//! history and never routes through a model or embedding path.

use super::recovery::{MutationRow, RecoveryStore};

/// Query for journal-native temporal recall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalQuery {
    /// Return mutations with seq strictly greater than this (0 = from start).
    pub after_seq: i64,
    /// Optional path filter (exact path).
    pub path: Option<String>,
    /// Max rows to return.
    pub limit: usize,
}

impl TemporalQuery {
    pub fn after(after_seq: i64, limit: usize) -> Self {
        Self {
            after_seq,
            path: None,
            limit,
        }
    }

    pub fn for_path(path: impl Into<String>, after_seq: i64, limit: usize) -> Self {
        Self {
            after_seq,
            path: Some(path.into()),
            limit,
        }
    }
}

/// One exact recall hit from the mutation journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalHit {
    pub seq: i64,
    pub ts: i64,
    pub op: String,
    pub path: String,
    pub pre_ref: String,
    pub post_ref: String,
}

impl From<MutationRow> for TemporalHit {
    fn from(row: MutationRow) -> Self {
        Self {
            seq: row.seq,
            ts: row.ts,
            op: row.op,
            path: row.path,
            pre_ref: row.pre_ref,
            post_ref: row.post_ref,
        }
    }
}

/// Recall mutations without any model/embedding call.
///
/// Uses the durable mutation journal only. Empty result means no matching
/// rows in scope — not "proved absent from the universe".
pub fn recall_mutations(
    store: &RecoveryStore,
    query: &TemporalQuery,
) -> Result<Vec<TemporalHit>, String> {
    let limit = query.limit.max(1);
    let rows = store.query_mutations_after(query.after_seq, limit.saturating_mul(4).max(limit))?;
    let mut hits: Vec<TemporalHit> = rows
        .into_iter()
        .filter(|r| match &query.path {
            Some(p) => r.path == *p,
            None => true,
        })
        .map(TemporalHit::from)
        .take(limit)
        .collect();
    // If path filter emptied a small page, fall back to path-indexed newest-first scan.
    if hits.is_empty() {
        if let Some(path) = &query.path {
            hits = store
                .query_mutations(path, None, limit)
                .into_iter()
                .filter(|r| r.seq > query.after_seq)
                .map(TemporalHit::from)
                .collect();
            // path query returns newest-first; present ascending for temporal order.
            hits.sort_by_key(|h| h.seq);
        }
    }
    Ok(hits)
}

/// True when this recall path uses only journal bytes (zero model calls).
pub fn is_zero_token_recall() -> bool {
    true
}
