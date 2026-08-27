//! Omission impact: omitted facts that can change actions / obligations / invalidation.

use std::collections::BTreeSet;

use graphzero_types::ContentHash;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum OmissionKind {
    MissingDependencyEdge,
    MissingCoverageRegion,
    MissingVerifier,
    MissingEffectTarget,
    LatentConfig,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum RecoveryTrigger {
    /// Omission can change the action set -- force automatic recovery before publish.
    ForceAutomaticRecovery,
    /// Advisory only; does not block publication alone.
    Advisory,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OmissionImpact {
    pub id: ContentHash,
    pub kind: OmissionKind,
    pub omitted: BTreeSet<ContentHash>,
    pub impacts_actions: bool,
    pub impacts_obligations: bool,
    pub impacts_invalidation: bool,
    pub trigger: RecoveryTrigger,
    pub premises: Vec<String>,
}

impl OmissionImpact {
    /// Classify recovery trigger from impact flags.
    #[must_use]
    pub fn classify(
        id: ContentHash,
        kind: OmissionKind,
        omitted: BTreeSet<ContentHash>,
        impacts_actions: bool,
        impacts_obligations: bool,
        impacts_invalidation: bool,
        premises: Vec<String>,
    ) -> Self {
        let trigger = if impacts_actions || impacts_invalidation {
            RecoveryTrigger::ForceAutomaticRecovery
        } else {
            RecoveryTrigger::Advisory
        };
        Self {
            id,
            kind,
            omitted,
            impacts_actions,
            impacts_obligations,
            impacts_invalidation,
            trigger,
            premises,
        }
    }

    #[must_use]
    pub fn blocks_candidate_publication(&self) -> bool {
        self.trigger == RecoveryTrigger::ForceAutomaticRecovery
    }
}
