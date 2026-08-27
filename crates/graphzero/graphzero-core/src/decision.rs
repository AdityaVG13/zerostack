//! Decision evidence closure: exact / overapprox / heuristic status with gaps.

use std::collections::BTreeSet;

use graphzero_types::ContentHash;

use crate::graph::NodeId;
use crate::truth::TruthClass;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ClosureClass {
    Exact,
    SoundOverapproximation,
    Heuristic,
    Incomplete,
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EvidenceKind {
    Definition,
    Type,
    BuildProfile,
    GeneratedArtifact,
    TestOrVerifier,
    RuntimeEdge,
    UnresolvedGap,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionEvidence {
    pub kind: EvidenceKind,
    pub node: Option<NodeId>,
    pub truth: TruthClass,
    pub digest: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionGap {
    pub kind: EvidenceKind,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionClosure {
    pub task: ContentHash,
    pub class: ClosureClass,
    pub evidence: Vec<DecisionEvidence>,
    pub gaps: Vec<DecisionGap>,
}

impl DecisionClosure {
    #[must_use]
    pub fn assemble(
        task: ContentHash,
        evidence: Vec<DecisionEvidence>,
        gaps: Vec<DecisionGap>,
    ) -> Self {
        let class = if !gaps.is_empty() {
            ClosureClass::Incomplete
        } else if evidence.iter().any(|e| e.truth == TruthClass::Heuristic) {
            ClosureClass::Heuristic
        } else if evidence
            .iter()
            .any(|e| e.truth == TruthClass::SoundOverapproximation)
        {
            ClosureClass::SoundOverapproximation
        } else if evidence.is_empty() {
            ClosureClass::Incomplete
        } else if evidence
            .iter()
            .all(|e| e.truth.is_exact() || e.truth == TruthClass::SyntaxDerived)
        {
            ClosureClass::Exact
        } else {
            ClosureClass::SoundOverapproximation
        };
        Self {
            task,
            class,
            evidence,
            gaps,
        }
    }

    /// Whether the closure is decision-complete enough for strict publication path.
    #[must_use]
    pub fn is_decision_complete(&self) -> bool {
        matches!(
            self.class,
            ClosureClass::Exact | ClosureClass::SoundOverapproximation
        ) && self.gaps.is_empty()
    }

    /// Required evidence kinds present (for golden coverage checks).
    #[must_use]
    pub fn kinds_present(&self) -> BTreeSet<EvidenceKind> {
        self.evidence.iter().map(|e| e.kind).collect()
    }
}
