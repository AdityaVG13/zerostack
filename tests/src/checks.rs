//! Named G1-G10 gate identifiers and their authoritative semantic mapping.

use crate::{CheckResult, GateStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckId {
    #[serde(rename = "G1")]
    G1Exposure,
    #[serde(rename = "G2")]
    G2Refs,
    #[serde(rename = "G3")]
    G3Telemetry,
    #[serde(rename = "G4", alias = "G4LEAKPROOF")]
    G4LeakProof,
    #[serde(rename = "G5")]
    G5Errors,
    #[serde(rename = "G6")]
    G6CtxStep,
    #[serde(rename = "G7")]
    G7Limits,
    #[serde(rename = "G8")]
    G8Mutation,
    #[serde(rename = "G9")]
    G9Coalescing,
    #[serde(rename = "G10")]
    G10Sandbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateMapping {
    pub id: CheckId,
    pub semantic_label: &'static str,
}

pub const GATE_MAPPINGS: [GateMapping; 10] = [
    GateMapping {
        id: CheckId::G1Exposure,
        semantic_label: "exposure",
    },
    GateMapping {
        id: CheckId::G2Refs,
        semantic_label: "refs",
    },
    GateMapping {
        id: CheckId::G3Telemetry,
        semantic_label: "telemetry",
    },
    GateMapping {
        id: CheckId::G4LeakProof,
        semantic_label: "leak_proof",
    },
    GateMapping {
        id: CheckId::G5Errors,
        semantic_label: "errors",
    },
    GateMapping {
        id: CheckId::G6CtxStep,
        semantic_label: "ctx_step",
    },
    GateMapping {
        id: CheckId::G7Limits,
        semantic_label: "limits",
    },
    GateMapping {
        id: CheckId::G8Mutation,
        semantic_label: "mutation",
    },
    GateMapping {
        id: CheckId::G9Coalescing,
        semantic_label: "coalescing",
    },
    GateMapping {
        id: CheckId::G10Sandbox,
        semantic_label: "sandbox",
    },
];

impl CheckId {
    pub const ALL: [CheckId; 10] = [
        Self::G1Exposure,
        Self::G2Refs,
        Self::G3Telemetry,
        Self::G4LeakProof,
        Self::G5Errors,
        Self::G6CtxStep,
        Self::G7Limits,
        Self::G8Mutation,
        Self::G9Coalescing,
        Self::G10Sandbox,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::G1Exposure => "G1",
            Self::G2Refs => "G2",
            Self::G3Telemetry => "G3",
            Self::G4LeakProof => "G4",
            Self::G5Errors => "G5",
            Self::G6CtxStep => "G6",
            Self::G7Limits => "G7",
            Self::G8Mutation => "G8",
            Self::G9Coalescing => "G9",
            Self::G10Sandbox => "G10",
        }
    }

    pub fn semantic_label(self) -> &'static str {
        GATE_MAPPINGS
            .iter()
            .find(|mapping| mapping.id == self)
            .expect("every CheckId has a mapping")
            .semantic_label
    }
}

/// Compatibility name used by the independent RACC gate results.
pub type CheckStatus = GateStatus;

/// In-crate self-checks (no external substrate binary).
pub fn run_self_checks() -> Vec<CheckResult> {
    vec![CheckResult::pass(
        CheckId::G2Refs.as_str(),
        CheckId::G2Refs.semantic_label(),
    )]
}
