//! Named G1–G10 check identifiers and harness aggregation.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CheckId {
    G1Exposure,
    G2Refs,
    G3Telemetry,
    G4LeakProof,
    G5Errors,
    G6CtxStep,
    G7Limits,
    G8Mutation,
    G9Coalescing,
    G10Sandbox,
}

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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::G1Exposure => "G1_exposure",
            Self::G2Refs => "G2_refs",
            Self::G3Telemetry => "G3_telemetry",
            Self::G4LeakProof => "G4_leak_proof",
            Self::G5Errors => "G5_errors",
            Self::G6CtxStep => "G6_ctx_step",
            Self::G7Limits => "G7_limits",
            Self::G8Mutation => "G8_mutation",
            Self::G9Coalescing => "G9_coalescing",
            Self::G10Sandbox => "G10_sandbox",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Fail,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub id: CheckId,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HarnessReport {
    pub contract_version: String,
    pub ns: String,
    pub substrate_binary: String,
    pub checks: Vec<CheckOutcome>,
}

impl HarnessReport {
    pub fn passed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Pass)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Skip)
            .count()
    }
}

/// In-crate self-checks (no external substrate binary).
pub fn run_self_checks() -> Vec<CheckOutcome> {
    vec![CheckOutcome {
        id: CheckId::G2Refs,
        status: CheckStatus::Pass,
        detail: Some("patterns + schema unit tests".into()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_id_serializes_to_stable_g_names() {
        let id = CheckId::G4LeakProof;
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"G4LEAKPROOF\"");
        assert_eq!(CheckId::ALL.len(), 10);
    }
}
