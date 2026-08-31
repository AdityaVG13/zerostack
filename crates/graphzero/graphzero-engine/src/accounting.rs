//! Prevented-read accounting shared by query surfaces and blast reports.

use std::collections::BTreeSet;

use graphzero_store::Snapshot;
use serde::{Deserialize, Serialize};

pub const PREVENTED_READ_ACCOUNTING_SCHEMA_VERSION: u32 = 1;

/// Ledger-compatible counters for bytes/files GraphZero lets a caller avoid reading.
/// required_* counts the unique indexed files that the graph-selected answer says are
/// relevant. prevented_* is the complement inside the current indexed repository snapshot.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreventedReadAccounting {
    pub schema_version: u32,
    pub scope: String,
    pub indexed_files: usize,
    pub indexed_bytes: u64,
    pub required_files: usize,
    pub required_bytes: u64,
    pub prevented_files: usize,
    pub prevented_bytes: u64,
    pub reason: String,
}

pub fn accounting_for_evidence_refs(
    snapshot: &Snapshot,
    scope: &str,
    evidence_refs: impl IntoIterator<Item = impl AsRef<str>>,
    reason: &str,
) -> PreventedReadAccounting {
    let mut required_hashes = BTreeSet::new();
    for reference in evidence_refs {
        if let Some(hash) = blob_hash_from_ref(reference.as_ref())
            && snapshot.path_for_blob(&hash).is_some()
        {
            required_hashes.insert(hash);
        }
    }

    let mut indexed_files = 0usize;
    let mut indexed_bytes = 0u64;
    let mut required_files = 0usize;
    let mut required_bytes = 0u64;
    for (hash, rec) in snapshot.path_records() {
        indexed_files += 1;
        indexed_bytes = indexed_bytes.saturating_add(rec.size);
        let hash_hex = hash.to_hex();
        if required_hashes.contains(&hash_hex) {
            required_files += 1;
            required_bytes = required_bytes.saturating_add(rec.size);
        }
    }

    PreventedReadAccounting {
        schema_version: PREVENTED_READ_ACCOUNTING_SCHEMA_VERSION,
        scope: scope.to_string(),
        indexed_files,
        indexed_bytes,
        required_files,
        required_bytes,
        prevented_files: indexed_files.saturating_sub(required_files),
        prevented_bytes: indexed_bytes.saturating_sub(required_bytes),
        reason: reason.to_string(),
    }
}

fn blob_hash_from_ref(reference: &str) -> Option<String> {
    let rest = reference.strip_prefix("z://blob/")?;
    let hash = rest.split(['#', '/', '?']).next().unwrap_or(rest);
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}
