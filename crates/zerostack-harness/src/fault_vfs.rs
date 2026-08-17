//! Declarative fault specs over hub store surfaces. Deterministic seed.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use zero_store::{FaultPlanV1, JournalBoundaryV1};

use crate::crash_boundary::CrashBoundary;

pub const DEFAULT_FAULT_SEED: u64 = 0xD1A6_A3F4_9B17_0C5E;

static FAULTS_INJECTED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultKind {
    TornWrite {
        valid_bytes: usize,
    },
    PartialWrite {
        valid_bytes: usize,
    },
    PowerCut,
    IoError,
    ReadFailure,
    WriteFailure,
    Latency {
        base_millis: u64,
        jitter_millis: u64,
    },
    DiskFull,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultSpec {
    pub id: String,
    pub file_glob: String,
    pub kind: FaultKind,
    pub boundary: CrashBoundary,
    pub max_triggers: u32,
    pub trigger_count: u32,
    pub match_count: u64,
    pub seed: u64,
}

impl FaultSpec {
    pub fn power_cut(id: &str, boundary: CrashBoundary) -> Self {
        Self {
            id: id.to_owned(),
            file_glob: "*.journal.json".into(),
            kind: FaultKind::PowerCut,
            boundary,
            max_triggers: 1,
            trigger_count: 0,
            match_count: 0,
            seed: DEFAULT_FAULT_SEED,
        }
    }

    pub fn leftover_temp(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            file_glob: ".*.tmp-*".into(),
            kind: FaultKind::PartialWrite { valid_bytes: 17 },
            boundary: CrashBoundary::AfterTmpWriteBeforeRename,
            max_triggers: 1,
            trigger_count: 0,
            match_count: 0,
            seed: DEFAULT_FAULT_SEED,
        }
    }

    pub fn arm(&self) -> FaultPlanV1 {
        FaultPlanV1::crash_at(self.boundary.journal_boundary())
    }

    pub fn record_trigger(&mut self, file_path: &str, offset: u64) -> FaultTriggerRecord {
        self.trigger_count = self.trigger_count.saturating_add(1);
        self.match_count = self.match_count.saturating_add(1);
        FAULTS_INJECTED.fetch_add(1, Ordering::Relaxed);
        FaultTriggerRecord {
            spec_id: self.id.clone(),
            file_path: file_path.to_owned(),
            offset,
            kind: self.kind.clone(),
            boundary: self.boundary,
            trigger_index: self.trigger_count,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultTriggerRecord {
    pub spec_id: String,
    pub file_path: String,
    pub offset: u64,
    pub kind: FaultKind,
    pub boundary: CrashBoundary,
    pub trigger_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedProfile {
    pub id: &'static str,
    pub boundary: CrashBoundary,
    pub invariants_preserved: &'static [&'static str],
}

pub const PROFILES: [NamedProfile; 4] = [
    NamedProfile {
        id: "torn-root-init",
        boundary: CrashBoundary::BeforeRename,
        invariants_preserved: &["no_partial_visible_file", "fail_closed"],
    },
    NamedProfile {
        id: "after-tmp-before-rename",
        boundary: CrashBoundary::AfterTmpWriteBeforeRename,
        invariants_preserved: &["no_partial_visible_file", "committed_or_not_committed"],
    },
    NamedProfile {
        id: "mid-journal-recover",
        boundary: CrashBoundary::MidJournalRecover,
        invariants_preserved: &[
            "committed_or_not_committed",
            "loud_journal_error_or_valid_receipt",
        ],
    },
    NamedProfile {
        id: "leftover-temp",
        boundary: CrashBoundary::AfterTmpWriteBeforeRename,
        invariants_preserved: &["dest_complete_or_absent", "leftover_temp_is_not_dest"],
    },
];

pub fn faults_injected_total() -> u64 {
    FAULTS_INJECTED.load(Ordering::Relaxed)
}

pub fn journal_boundary_seed(seed: u64) -> JournalBoundaryV1 {
    let idx = (seed ^ DEFAULT_FAULT_SEED) as usize % CrashBoundary::ALL.len();
    CrashBoundary::ALL[idx].journal_boundary()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_stable() {
        assert_eq!(DEFAULT_FAULT_SEED, 0xD1A6_A3F4_9B17_0C5E);
        assert_eq!(
            journal_boundary_seed(DEFAULT_FAULT_SEED),
            journal_boundary_seed(DEFAULT_FAULT_SEED)
        );
    }

    #[test]
    fn every_profile_names_invariants() {
        for profile in &PROFILES {
            assert!(!profile.invariants_preserved.is_empty());
        }
    }

    #[test]
    fn trigger_records_are_deterministic() {
        let mut a = FaultSpec::power_cut(
            "after-tmp-before-rename",
            CrashBoundary::AfterTmpWriteBeforeRename,
        );
        let mut b = FaultSpec::power_cut(
            "after-tmp-before-rename",
            CrashBoundary::AfterTmpWriteBeforeRename,
        );
        let ra = a.record_trigger("root.json", 0);
        let rb = b.record_trigger("root.json", 0);
        assert_eq!(ra, rb);
        assert!(faults_injected_total() >= 2);
    }
}
