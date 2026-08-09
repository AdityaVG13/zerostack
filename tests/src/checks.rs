//! Two distinct conformance gate vocabularies, kept strictly separate.
//!
//! - **G1-G10** are the canonical *plan-level* gates. They drive a planner
//!   host that serves `{ns}_execute_code` (JS plan execution, ctx.step,
//!   coalescing, sandbox). They live in `plan.rs` and apply to
//!   `Surface::Planner`. An `*-mcp` artifact is exercised for G1 exposure only.
//! - **RW1-RW10** are the *raw-worker v2* gates. They drive a planner-free
//!   raw-worker binary over the hub raw-v2 wire protocol. They live in
//!   `raw_worker.rs` and apply to `Surface::Codemode`.
//!
//! The two sets are deliberately NOT aliases of each other: a raw worker
//! cannot own planner semantics (ctx.step primitive, aggregate op
//! coalescing, JS sandbox), and a planner run is not a raw-worker boundary
//! check. Tests pin that the id vocabularies never overlap.

use crate::{CheckResult, GateStatus};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Plan-level gates G1-G10 (canonical, unchanged).
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Raw-worker v2 gates RW1-RW10 (distinct from G1-G10).
// ---------------------------------------------------------------------------

/// Raw-worker v2 gate identifiers. These are NOT aliases of the plan-level
/// `CheckId` set: they describe worker-boundary invariants a planner-free
/// raw worker owns, never plan-level semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RawCheckId {
    #[serde(rename = "RW1")]
    Rw1ArtifactExposure,
    #[serde(rename = "RW2")]
    Rw2RecoverableRefs,
    #[serde(rename = "RW3")]
    Rw3TelemetryAccounting,
    #[serde(rename = "RW4")]
    Rw4OutputBounds,
    #[serde(rename = "RW5")]
    Rw5TypedErrors,
    #[serde(rename = "RW6")]
    Rw6SessionContinuity,
    #[serde(rename = "RW7")]
    Rw7FrameLimits,
    #[serde(rename = "RW8")]
    Rw8DomainMutation,
    #[serde(rename = "RW9")]
    Rw9ProcessReuse,
    #[serde(rename = "RW10")]
    Rw10PlannerRefusal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawGateMapping {
    pub id: RawCheckId,
    pub semantic_label: &'static str,
}

pub const RAW_GATE_MAPPINGS: [RawGateMapping; 10] = [
    RawGateMapping {
        id: RawCheckId::Rw1ArtifactExposure,
        semantic_label: "artifact_exposure",
    },
    RawGateMapping {
        id: RawCheckId::Rw2RecoverableRefs,
        semantic_label: "recoverable_refs",
    },
    RawGateMapping {
        id: RawCheckId::Rw3TelemetryAccounting,
        semantic_label: "telemetry_accounting",
    },
    RawGateMapping {
        id: RawCheckId::Rw4OutputBounds,
        semantic_label: "output_bounds",
    },
    RawGateMapping {
        id: RawCheckId::Rw5TypedErrors,
        semantic_label: "typed_errors",
    },
    RawGateMapping {
        id: RawCheckId::Rw6SessionContinuity,
        semantic_label: "session_continuity",
    },
    RawGateMapping {
        id: RawCheckId::Rw7FrameLimits,
        semantic_label: "frame_limits",
    },
    RawGateMapping {
        id: RawCheckId::Rw8DomainMutation,
        semantic_label: "domain_mutation",
    },
    RawGateMapping {
        id: RawCheckId::Rw9ProcessReuse,
        semantic_label: "process_reuse",
    },
    RawGateMapping {
        id: RawCheckId::Rw10PlannerRefusal,
        semantic_label: "planner_refusal",
    },
];

impl RawCheckId {
    pub const ALL: [RawCheckId; 10] = [
        Self::Rw1ArtifactExposure,
        Self::Rw2RecoverableRefs,
        Self::Rw3TelemetryAccounting,
        Self::Rw4OutputBounds,
        Self::Rw5TypedErrors,
        Self::Rw6SessionContinuity,
        Self::Rw7FrameLimits,
        Self::Rw8DomainMutation,
        Self::Rw9ProcessReuse,
        Self::Rw10PlannerRefusal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rw1ArtifactExposure => "RW1",
            Self::Rw2RecoverableRefs => "RW2",
            Self::Rw3TelemetryAccounting => "RW3",
            Self::Rw4OutputBounds => "RW4",
            Self::Rw5TypedErrors => "RW5",
            Self::Rw6SessionContinuity => "RW6",
            Self::Rw7FrameLimits => "RW7",
            Self::Rw8DomainMutation => "RW8",
            Self::Rw9ProcessReuse => "RW9",
            Self::Rw10PlannerRefusal => "RW10",
        }
    }

    pub fn semantic_label(self) -> &'static str {
        RAW_GATE_MAPPINGS
            .iter()
            .find(|mapping| mapping.id == self)
            .expect("every RawCheckId has a mapping")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn plan_and_raw_gate_ids_never_overlap() {
        let plan_ids: HashSet<&str> = GATE_MAPPINGS.iter().map(|m| m.id.as_str()).collect();
        let raw_ids: HashSet<&str> = RAW_GATE_MAPPINGS.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(plan_ids.len(), 10);
        assert_eq!(raw_ids.len(), 10);
        assert!(
            plan_ids.is_disjoint(&raw_ids),
            "plan {plan_ids:?} and raw {raw_ids:?} id sets must be disjoint"
        );
        assert!(
            plan_ids.iter().all(|id| id.starts_with('G'))
                && raw_ids.iter().all(|id| id.starts_with("RW")),
            "plan ids are G*, raw ids are RW*"
        );
    }

    #[test]
    fn raw_gate_labels_are_distinct_and_canonical() {
        let ids: Vec<_> = RAW_GATE_MAPPINGS.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            (1..=10)
                .map(|n| format!("RW{n}"))
                .collect::<Vec<_>>()
        );
        let labels: HashSet<&str> = RAW_GATE_MAPPINGS.iter().map(|m| m.semantic_label).collect();
        assert_eq!(labels.len(), 10);
        assert_eq!(RawCheckId::Rw8DomainMutation.semantic_label(), "domain_mutation");
    }

    #[test]
    fn raw_checkid_serde_emits_rw_form() {
        assert_eq!(
            serde_json::to_string(&RawCheckId::Rw1ArtifactExposure).unwrap(),
            "\"RW1\""
        );
        assert_eq!(
            serde_json::from_str::<RawCheckId>("\"RW10\"").unwrap(),
            RawCheckId::Rw10PlannerRefusal
        );
        assert!(serde_json::from_str::<RawCheckId>("\"G1\"").is_err());
        assert!(serde_json::from_str::<RawCheckId>("\"RW11\"").is_err());
    }
}
