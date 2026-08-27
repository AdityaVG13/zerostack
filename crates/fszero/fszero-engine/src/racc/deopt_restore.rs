//! Candidate abort and exact baseline rehydration (fszero-d90z).

use super::exact_snapshot::ExactSnapshot;
use super::safepoint::RawBaselineSafepoint;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeoptRestoreError {
    SafepointMismatch(String),
    MissingBaselineFile(String),
}

impl std::fmt::Display for DeoptRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SafepointMismatch(s) => write!(f, "deopt safepoint mismatch: {s}"),
            Self::MissingBaselineFile(p) => write!(f, "missing baseline file: {p}"),
        }
    }
}
impl std::error::Error for DeoptRestoreError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeoptRestoreReceipt {
    pub safepoint_id: String,
    pub restored_root: String,
    pub files_restored: usize,
    /// Explicit non-claim: external DBs/processes/services not restored.
    pub external_restore_claimed: bool,
}

/// Rehydrate project-side baseline bytes from a safepoint + baseline file map.
pub fn rehydrate_from_safepoint(
    safepoint: &RawBaselineSafepoint,
    baseline_snapshot: &ExactSnapshot,
    baseline_files: &BTreeMap<String, Vec<u8>>,
) -> Result<(BTreeMap<String, Vec<u8>>, DeoptRestoreReceipt), DeoptRestoreError> {
    safepoint
        .assert_matches_snapshot(baseline_snapshot)
        .map_err(|e| DeoptRestoreError::SafepointMismatch(e.to_string()))?;
    let mut out = BTreeMap::new();
    for (path, rec) in baseline_snapshot.records() {
        let bytes = baseline_files
            .get(path)
            .ok_or_else(|| DeoptRestoreError::MissingBaselineFile(path.clone()))?;
        if bytes.len() as u64 != rec.len {
            return Err(DeoptRestoreError::SafepointMismatch(format!(
                "length mismatch for {path}"
            )));
        }
        out.insert(path.clone(), bytes.clone());
    }
    let receipt = DeoptRestoreReceipt {
        safepoint_id: safepoint.safepoint_id.clone(),
        restored_root: safepoint.snapshot_root.clone(),
        files_restored: out.len(),
        external_restore_claimed: false,
    };
    Ok((out, receipt))
}
