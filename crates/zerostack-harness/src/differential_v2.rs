//! Differential V2 envelope. `artifact_id` is SHA-256 of canonical JSON
//! excluding `run_id`.

use serde::Serialize;

use crate::engine_identity::{EngineIdentity, SUBJECT_IDENTITY_LABEL};
use crate::repo::sha256_hex;

pub const FORMAT_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize)]
pub struct EngineVersions {
    pub subject_identity: String,
    pub reference_identity: String,
}

impl EngineVersions {
    pub fn new(oracle: &EngineIdentity) -> Self {
        Self {
            subject_identity: SUBJECT_IDENTITY_LABEL.to_owned(),
            reference_identity: oracle.label.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HarnessConfig {
    pub spec_contract_path: String,
    pub subject_binary: String,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            spec_contract_path: "conformance/contracts/spec_version_contract.toml".into(),
            subject_binary: "zsx".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CanonicalizationRules {
    pub float_tolerance: String,
    pub unordered_results_as_multiset: bool,
    pub error_match_by_category: bool,
    pub normalize_whitespace: bool,
}

impl Default for CanonicalizationRules {
    fn default() -> Self {
        Self {
            float_tolerance: "0".into(),
            unordered_results_as_multiset: false,
            error_match_by_category: true,
            normalize_whitespace: true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutionEnvelope {
    pub format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub scenario_id: String,
    pub seed: u64,
    pub engines: EngineVersions,
    pub config: HarnessConfig,
    pub schema: Vec<String>,
    pub workload: Vec<String>,
    pub canonicalization: CanonicalizationRules,
}

#[derive(Serialize)]
struct CanonicalEnvelope<'a> {
    format_version: u32,
    scenario_id: &'a str,
    seed: u64,
    engines: &'a EngineVersions,
    config: &'a HarnessConfig,
    schema: &'a [String],
    workload: &'a [String],
    canonicalization: &'a CanonicalizationRules,
}

impl ExecutionEnvelope {
    pub fn new(scenario_id: impl Into<String>, oracle: &EngineIdentity) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            run_id: None,
            scenario_id: scenario_id.into(),
            seed: 0,
            engines: EngineVersions::new(oracle),
            config: HarnessConfig::default(),
            schema: Vec::new(),
            workload: Vec::new(),
            canonicalization: CanonicalizationRules::default(),
        }
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// SHA-256 of canonical JSON excluding `run_id`.
    pub fn artifact_id(&self) -> String {
        let canonical = CanonicalEnvelope {
            format_version: self.format_version,
            scenario_id: &self.scenario_id,
            seed: self.seed,
            engines: &self.engines,
            config: &self.config,
            schema: &self.schema,
            workload: &self.workload,
            canonicalization: &self.canonicalization,
        };
        let json = serde_json::to_string(&canonical).expect("envelope serialization must not fail");
        sha256_hex(json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_identity::EngineIdentity;

    #[test]
    fn artifact_id_ignores_run_id() {
        let oracle = EngineIdentity::oracle("spec-v1");
        let a = ExecutionEnvelope::new("smoke", &oracle).with_run_id("run-a");
        let b = ExecutionEnvelope::new("smoke", &oracle).with_run_id("run-b");
        assert_eq!(a.artifact_id(), b.artifact_id());
        assert_ne!(a.run_id, b.run_id);
    }

    #[test]
    fn artifact_id_changes_when_workload_changes() {
        let oracle = EngineIdentity::oracle("spec-v1");
        let mut a = ExecutionEnvelope::new("smoke", &oracle);
        let b = ExecutionEnvelope::new("smoke", &oracle);
        a.workload.push("extra".into());
        assert_ne!(a.artifact_id(), b.artifact_id());
    }
}
