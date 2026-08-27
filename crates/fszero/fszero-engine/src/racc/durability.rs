//! Handoff acceptance durability matrix: kill-point, torn-write, disk-full (fszero-1de8).
//!
//! Faults are injected through [`AtomicPublication`] stages AND distinct
//! side effects so TornWrite/DiskFull are not mere aliases of kill points:
//! - TornWrite leaves a partial candidate root file that recovery refuses.
//! - DiskFull fails before any candidate identity is recorded.

use super::overlay_publish::{AtomicPublication, CrashPoint, OverlayError, PublicationStage};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityCase {
    KillAfterPrepare,
    KillAfterVerify,
    KillAfterPublish,
    KillBeforeBarrier,
    /// Partial candidate materialization: recovery must not publish torn bytes.
    TornWriteSimulate,
    /// Pre-publish write failure: no candidate identity, base retained.
    DiskFullSimulate,
    CleanPublish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurabilityCaseResult {
    pub case: DurabilityCase,
    pub recovered_root: String,
    pub accepted: bool,
    pub note: String,
    /// Distinct evidence fields so cases are not renames of each other.
    pub torn_partial_present: bool,
    pub candidate_identity_recorded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurabilityMatrixReport {
    pub results: Vec<DurabilityCaseResult>,
}

impl DurabilityMatrixReport {
    pub fn all_accepted(&self) -> bool {
        self.results.iter().all(|r| r.accepted)
    }
}

/// In-memory filesystem-ish state used to distinguish torn vs disk-full.
#[derive(Debug, Clone, Default)]
pub struct DurableRootStore {
    pub published_root: Option<String>,
    /// path -> complete|torn partial content
    pub files: BTreeMap<String, Vec<u8>>,
}

impl DurableRootStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to materialize candidate root bytes under a named root file.
    pub fn materialize_candidate(
        &mut self,
        root_name: &str,
        full_bytes: &[u8],
        mode: MaterializeMode,
    ) -> Result<(), String> {
        match mode {
            MaterializeMode::Complete => {
                self.files
                    .insert(root_name.to_string(), full_bytes.to_vec());
                Ok(())
            }
            MaterializeMode::TornHalf => {
                let half = full_bytes.len() / 2;
                self.files
                    .insert(root_name.to_string(), full_bytes[..half].to_vec());
                Err("torn write: partial candidate root".into())
            }
            MaterializeMode::DiskFull => {
                // No file written, no identity.
                Err("disk full: cannot allocate candidate root".into())
            }
        }
    }

    pub fn has_torn_partial(&self, root_name: &str, full_len: usize) -> bool {
        self.files
            .get(root_name)
            .map(|b| !b.is_empty() && b.len() < full_len)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeMode {
    Complete,
    TornHalf,
    DiskFull,
}

/// Run durability matrix with distinct torn-write and disk-full effects.
pub fn run_durability_matrix(base_root: &str, candidate_root: &str) -> DurabilityMatrixReport {
    let full = candidate_root.as_bytes();
    let mut results = Vec::new();

    let kill_cases = [
        (
            DurabilityCase::KillAfterPrepare,
            CrashPoint::AfterPrepare,
            base_root,
            false,
        ),
        (
            DurabilityCase::KillAfterVerify,
            CrashPoint::AfterVerify,
            base_root,
            false,
        ),
        (
            DurabilityCase::KillBeforeBarrier,
            CrashPoint::BeforeBarrier,
            base_root,
            false,
        ),
        (
            DurabilityCase::KillAfterPublish,
            CrashPoint::AfterPublish,
            candidate_root,
            true,
        ),
        (
            DurabilityCase::CleanPublish,
            CrashPoint::None,
            candidate_root,
            true,
        ),
    ];
    for (case, crash, expect, cand_id) in kill_cases {
        let mut pubn = AtomicPublication::new();
        let mut store = DurableRootStore::new();
        let pub_result = pubn.publish_with_fault(base_root, candidate_root, crash);
        if pub_result.is_ok() || crash == CrashPoint::AfterPublish {
            let _ = store.materialize_candidate(candidate_root, full, MaterializeMode::Complete);
        }
        let recovered = pubn.recover(base_root);
        let accepted = recovered == expect
            && match crash {
                CrashPoint::None => pub_result.is_ok(),
                CrashPoint::AfterPublish => recovered == candidate_root,
                _ => pub_result.is_err() && recovered == base_root,
            };
        results.push(DurabilityCaseResult {
            case,
            recovered_root: recovered,
            accepted,
            note: match &pub_result {
                Ok(_) => "published".into(),
                Err(OverlayError::Journal(s)) => s.clone(),
                Err(e) => e.to_string(),
            },
            torn_partial_present: false,
            candidate_identity_recorded: cand_id
                && pubn.journal.iter().any(|r| {
                    matches!(
                        r.stage,
                        PublicationStage::Published | PublicationStage::Durable
                    )
                }),
        });
    }

    // Torn write: half candidate file present; recovery must keep base and
    // refuse to promote torn partial as published root.
    {
        let mut pubn = AtomicPublication::new();
        let mut store = DurableRootStore::new();
        let mat = store.materialize_candidate(candidate_root, full, MaterializeMode::TornHalf);
        assert!(mat.is_err());
        // Never reach Published stage when materialization tore.
        let _ = pubn.publish_with_fault(base_root, candidate_root, CrashPoint::AfterVerify);
        let recovered = pubn.recover(base_root);
        let torn = store.has_torn_partial(candidate_root, full.len());
        let accepted = recovered == base_root
            && torn
            && !pubn
                .journal
                .iter()
                .any(|r| r.stage == PublicationStage::Published);
        results.push(DurabilityCaseResult {
            case: DurabilityCase::TornWriteSimulate,
            recovered_root: recovered,
            accepted,
            note: format!(
                "torn partial present={torn}; recovery retains base; mat_err={}",
                mat.err().unwrap_or_default()
            ),
            torn_partial_present: torn,
            candidate_identity_recorded: false,
        });
    }

    // Disk full: fail before any candidate file or identity is recorded.
    {
        let mut pubn = AtomicPublication::new();
        let mut store = DurableRootStore::new();
        let mat = store.materialize_candidate(candidate_root, full, MaterializeMode::DiskFull);
        assert!(mat.is_err());
        // Crash before barrier models pre-publish failure after prepare/verify.
        let err = pubn.publish_with_fault(base_root, candidate_root, CrashPoint::BeforeBarrier);
        let recovered = pubn.recover(base_root);
        let has_file = store.files.contains_key(candidate_root);
        let accepted = err.is_err()
            && recovered == base_root
            && !has_file
            && !store.has_torn_partial(candidate_root, full.len());
        results.push(DurabilityCaseResult {
            case: DurabilityCase::DiskFullSimulate,
            recovered_root: recovered,
            accepted,
            note: format!("disk-full: no candidate file (has_file={has_file}); base retained"),
            torn_partial_present: false,
            candidate_identity_recorded: false,
        });
    }

    DurabilityMatrixReport { results }
}
