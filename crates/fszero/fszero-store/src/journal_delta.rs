//! Portable, ordered mutation-journal deltas. A byte range is half-open in each image:
//! `start..before_end` is replaced by `start..after_end`. This makes unequal-length replacements
//! explicit.

use crate::recovery::RecoveryStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const JOURNAL_DELTA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalDeltaOp {
    Upsert,
    Remove,
}

/// Minimal changed span between the exact preimage and postimage bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalByteRange {
    pub start: u64,
    pub before_end: u64,
    pub after_end: u64,
}

/// Serde-neutral wire data. `replacement` is the exact byte slice inserted at
/// `byte_range.start`, so consumers need neither workspace reads nor FSZero refs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalDelta {
    pub version: u32,
    pub seq: i64,
    pub op: JournalDeltaOp,
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
    pub byte_range: JournalByteRange,
    pub replacement: Vec<u8>,
}

impl JournalDelta {
    pub fn upsert(seq: i64, path: impl Into<String>, before: &[u8], after: &[u8]) -> Self {
        let byte_range = changed_span(before, after);
        let start = byte_range.start as usize;
        let after_end = byte_range.after_end as usize;
        Self {
            version: JOURNAL_DELTA_VERSION,
            seq,
            op: JournalDeltaOp::Upsert,
            path: path.into(),
            before_hash: hash_bytes(before),
            after_hash: hash_bytes(after),
            byte_range,
            replacement: after[start..after_end].to_vec(),
        }
    }

    pub fn remove(seq: i64, path: impl Into<String>, before: &[u8]) -> Self {
        Self {
            version: JOURNAL_DELTA_VERSION,
            seq,
            op: JournalDeltaOp::Remove,
            path: path.into(),
            before_hash: hash_bytes(before),
            after_hash: hash_bytes(&[]),
            byte_range: JournalByteRange {
                start: 0,
                before_end: before.len() as u64,
                after_end: 0,
            },
            replacement: Vec::new(),
        }
    }
}

impl RecoveryStore {
    /// Return one bounded journal page after `after_seq`. The page starts at
    /// exactly `after_seq + 1` and is gapless, or the entire call fails.
    pub fn mutation_deltas(
        &self,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<JournalDelta>, String> {
        let rows = self.query_mutations_after(after_seq, limit)?;
        let mut expected = after_seq
            .checked_add(1)
            .ok_or_else(|| "journal delta cursor overflow".to_string())?;
        let mut deltas = Vec::with_capacity(rows.len());

        for row in rows {
            if row.seq != expected {
                return Err(format!(
                    "journal delta sequence gap: expected {expected}, got {}",
                    row.seq
                ));
            }
            expected = expected
                .checked_add(1)
                .ok_or_else(|| "journal delta sequence overflow".to_string())?;

            let before = if row.pre_ref.is_empty() {
                if row.created {
                    Vec::new()
                } else {
                    return Err(format!("journal delta {} preimage ref missing", row.seq));
                }
            } else {
                self.expand_with_tiers(&row.pre_ref).map_err(|error| {
                    format!("journal delta {} preimage unavailable: {error}", row.seq)
                })?
            };

            let delta = if row.post_ref.is_empty() {
                JournalDelta::remove(row.seq, row.path, &before)
            } else {
                let after = self.expand_with_tiers(&row.post_ref).map_err(|error| {
                    format!("journal delta {} postimage unavailable: {error}", row.seq)
                })?;
                JournalDelta::upsert(row.seq, row.path, &before, &after)
            };
            deltas.push(delta);
        }
        Ok(deltas)
    }
}

/// Atomically integrate a page into caller-owned logical state. No
/// workspace path or engine-specific ref is read. Every touched preimage,
/// replacement, sequence, operation, and range is validated before publication.
pub fn integrate_journal_deltas(
    state: &mut BTreeMap<String, Vec<u8>>,
    after_seq: i64,
    deltas: &[JournalDelta],
) -> Result<(), String> {
    let mut staged = state.clone();
    let mut expected = after_seq
        .checked_add(1)
        .ok_or_else(|| "journal delta cursor overflow".to_string())?;

    for delta in deltas {
        if delta.version != JOURNAL_DELTA_VERSION {
            return Err(format!(
                "journal delta {} unsupported version {}",
                delta.seq, delta.version
            ));
        }
        if delta.seq != expected {
            return Err(format!(
                "journal delta sequence gap: expected {expected}, got {}",
                delta.seq
            ));
        }
        expected = expected
            .checked_add(1)
            .ok_or_else(|| "journal delta sequence overflow".to_string())?;
        if delta.path.is_empty() {
            return Err(format!("journal delta {} path is empty", delta.seq));
        }

        let before = staged
            .get(&delta.path)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if hash_bytes(before) != delta.before_hash {
            return Err(format!("journal delta {} before_hash mismatch", delta.seq));
        }

        match delta.op {
            JournalDeltaOp::Upsert => {
                let start = usize::try_from(delta.byte_range.start)
                    .map_err(|_| format!("journal delta {} range overflow", delta.seq))?;
                let before_end = usize::try_from(delta.byte_range.before_end)
                    .map_err(|_| format!("journal delta {} range overflow", delta.seq))?;
                let after_end = usize::try_from(delta.byte_range.after_end)
                    .map_err(|_| format!("journal delta {} range overflow", delta.seq))?;
                if start > before_end
                    || before_end > before.len()
                    || after_end < start
                    || after_end - start != delta.replacement.len()
                {
                    return Err(format!("journal delta {} invalid byte_range", delta.seq));
                }
                let mut after = Vec::with_capacity(
                    start + delta.replacement.len() + before.len().saturating_sub(before_end),
                );
                after.extend_from_slice(&before[..start]);
                after.extend_from_slice(&delta.replacement);
                after.extend_from_slice(&before[before_end..]);
                if hash_bytes(&after) != delta.after_hash
                    || changed_span(before, &after) != delta.byte_range
                {
                    return Err(format!(
                        "journal delta {} after_hash or byte_range mismatch",
                        delta.seq
                    ));
                }
                staged.insert(delta.path.clone(), after);
            }
            JournalDeltaOp::Remove => {
                if !delta.replacement.is_empty()
                    || delta.after_hash != hash_bytes(&[])
                    || delta.byte_range
                        != (JournalByteRange {
                            start: 0,
                            before_end: before.len() as u64,
                            after_end: 0,
                        })
                {
                    return Err(format!("journal delta {} malformed remove", delta.seq));
                }
                staged.remove(&delta.path);
            }
        }
    }

    *state = staged;
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> String {
    fszero_core::hexutil::sha256_hex_of(Sha256::digest(bytes).into())
}

fn changed_span(before: &[u8], after: &[u8]) -> JournalByteRange {
    let shared = before.len().min(after.len());
    let mut start = 0;
    while start < shared && before[start] == after[start] {
        start += 1;
    }

    let mut suffix = 0;
    while suffix < before.len().saturating_sub(start)
        && suffix < after.len().saturating_sub(start)
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }

    JournalByteRange {
        start: start as u64,
        before_end: (before.len() - suffix) as u64,
        after_end: (after.len() - suffix) as u64,
    }
}
