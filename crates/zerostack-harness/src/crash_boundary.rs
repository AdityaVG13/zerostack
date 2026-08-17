//! Named hub-real crash boundaries over journal / CAS / atomic_write.

use serde::{Deserialize, Serialize};
use zero_store::{FaultPlanV1, JournalBoundaryV1};

/// Protocol events on the store publish path. Not byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CrashBoundary {
    BeforeRename,
    AfterTmpWriteBeforeRename,
    AfterRenameBeforeDirSync,
    MidJournalRecover,
    AfterPrepareBeforeCommit,
    CommitAfterFileSync,
    CommitAfterRename,
}

impl CrashBoundary {
    pub const ALL: [Self; 7] = [
        Self::BeforeRename,
        Self::AfterTmpWriteBeforeRename,
        Self::AfterRenameBeforeDirSync,
        Self::MidJournalRecover,
        Self::AfterPrepareBeforeCommit,
        Self::CommitAfterFileSync,
        Self::CommitAfterRename,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeRename => "BeforeRename",
            Self::AfterTmpWriteBeforeRename => "AfterTmpWriteBeforeRename",
            Self::AfterRenameBeforeDirSync => "AfterRenameBeforeDirSync",
            Self::MidJournalRecover => "MidJournalRecover",
            Self::AfterPrepareBeforeCommit => "AfterPrepareBeforeCommit",
            Self::CommitAfterFileSync => "CommitAfterFileSync",
            Self::CommitAfterRename => "CommitAfterRename",
        }
    }

    pub const fn journal_boundary(self) -> JournalBoundaryV1 {
        match self {
            Self::BeforeRename => JournalBoundaryV1::RootInitializeBeforeWrite,
            Self::AfterTmpWriteBeforeRename => JournalBoundaryV1::RootInitializeAfterFileSync,
            Self::AfterRenameBeforeDirSync => JournalBoundaryV1::RootInitializeAfterRename,
            Self::MidJournalRecover => JournalBoundaryV1::RecoveryBeforeWrite,
            Self::AfterPrepareBeforeCommit => JournalBoundaryV1::PrepareAfterRename,
            Self::CommitAfterFileSync => JournalBoundaryV1::CommitAfterFileSync,
            Self::CommitAfterRename => JournalBoundaryV1::CommitAfterRename,
        }
    }

    /// Ack category for the recovery predicate.
    pub const fn ack_class(self) -> AckClass {
        match self {
            Self::BeforeRename | Self::AfterTmpWriteBeforeRename => AckClass::BeforeAck,
            Self::AfterRenameBeforeDirSync
            | Self::AfterPrepareBeforeCommit
            | Self::CommitAfterFileSync => AckClass::AfterAckBeforeDurability,
            Self::CommitAfterRename | Self::MidJournalRecover => AckClass::AfterAckAfterDurability,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckClass {
    BeforeAck,
    AfterAckBeforeDurability,
    AfterAckAfterDurability,
}

pub fn arm_crash_boundary(boundary: CrashBoundary) -> FaultPlanV1 {
    FaultPlanV1::crash_at(boundary.journal_boundary())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_named_boundaries() {
        assert_eq!(CrashBoundary::ALL.len(), 7);
        for boundary in CrashBoundary::ALL {
            let _ = boundary.journal_boundary();
            let _ = boundary.ack_class();
            assert!(!boundary.as_str().is_empty());
        }
    }
}
