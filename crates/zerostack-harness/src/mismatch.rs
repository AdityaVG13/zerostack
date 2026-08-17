//! Mismatch classification. CI fails only on [`MismatchClassification::TrueDivergence`].

use serde::{Deserialize, Serialize};

/// Why two answers disagreed. Greenfield variants are triage, not CI red.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MismatchClassification {
    TrueDivergence {
        description: String,
    },
    OrderDependentDifference,
    TypeAffinityDifference,
    NullHandlingDifference,
    FloatingPointDifference {
        max_epsilon_str: String,
    },
    FalsePositive {
        reason: String,
    },
    SpecConflict {
        source_a: String,
        source_b: String,
        tag: String,
    },
    UnverifiedSpecTag {
        tag: String,
        source: String,
    },
    SeedContractViolation {
        regression_file: String,
        line: u32,
        replay_status: String,
    },
    SurfaceDrift {
        direction: String,
        item: String,
    },
}

impl MismatchClassification {
    /// CI / conformal gate: only a true subject≠oracle divergence is red.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::TrueDivergence { .. })
    }

    pub fn triage_priority(&self) -> u8 {
        match self {
            Self::TrueDivergence { .. } => 0,
            Self::NullHandlingDifference => 1,
            Self::TypeAffinityDifference => 2,
            Self::FloatingPointDifference { .. } => 3,
            Self::OrderDependentDifference => 4,
            Self::FalsePositive { .. } => 5,
            Self::SpecConflict { .. } => 6,
            Self::UnverifiedSpecTag { .. } => 7,
            Self::SeedContractViolation { .. } => 8,
            Self::SurfaceDrift { .. } => 9,
        }
    }

    pub fn discriminant_name(&self) -> &'static str {
        match self {
            Self::TrueDivergence { .. } => "TrueDivergence",
            Self::OrderDependentDifference => "OrderDependentDifference",
            Self::TypeAffinityDifference => "TypeAffinityDifference",
            Self::NullHandlingDifference => "NullHandlingDifference",
            Self::FloatingPointDifference { .. } => "FloatingPointDifference",
            Self::FalsePositive { .. } => "FalsePositive",
            Self::SpecConflict { .. } => "SpecConflict",
            Self::UnverifiedSpecTag { .. } => "UnverifiedSpecTag",
            Self::SeedContractViolation { .. } => "SeedContractViolation",
            Self::SurfaceDrift { .. } => "SurfaceDrift",
        }
    }
}

/// CI-equivalent gate. Other classes flow into triage.
pub fn ci_fails(class: &MismatchClassification) -> bool {
    class.is_actionable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_fails_only_on_true_divergence() {
        let red = MismatchClassification::TrueDivergence {
            description: "subject ok, oracle err".into(),
        };
        assert!(ci_fails(&red));
        assert_eq!(red.triage_priority(), 0);

        let triage = [
            MismatchClassification::OrderDependentDifference,
            MismatchClassification::TypeAffinityDifference,
            MismatchClassification::NullHandlingDifference,
            MismatchClassification::FloatingPointDifference {
                max_epsilon_str: "1ulp".into(),
            },
            MismatchClassification::FalsePositive {
                reason: "cosmetic snapshot".into(),
            },
            MismatchClassification::SpecConflict {
                source_a: "CONTRACT.md".into(),
                source_b: "AGENTS.md".into(),
                tag: "SPEC-RES-001".into(),
            },
            MismatchClassification::UnverifiedSpecTag {
                tag: "SPEC-EE-042".into(),
                source: "docs/spec".into(),
            },
            MismatchClassification::SeedContractViolation {
                regression_file: "proptest-regressions/x.txt".into(),
                line: 17,
                replay_status: "missed".into(),
            },
            MismatchClassification::SurfaceDrift {
                direction: "cli-only".into(),
                item: "analyze".into(),
            },
        ];
        for class in &triage {
            assert!(
                !ci_fails(class),
                "CI must not fail on {}",
                class.discriminant_name()
            );
            assert!(class.triage_priority() > 0);
        }
    }
}
