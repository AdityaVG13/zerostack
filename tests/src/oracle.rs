//! Engine-independent oracle comparisons and reproducible failure evidence.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;

pub const FAILURE_BUNDLE_VERSION: &str = "zerostack.oracle.failure.v1";

/// Stable identities allowed to participate in an oracle comparison.
///
/// Variants are deliberately closed: engine names, this harness, the written
/// specification, and the supported external JSON reference cannot be
/// confused by free-form labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineIdentity {
    TokenZero,
    FsZero,
    GraphZero,
    ConformanceHarness,
    Specification,
    ExternalJq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleMode {
    Spec,
    Property,
    SelfCheck,
    RoundTrip,
    ExternalTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleError {
    SelfComparison { identity: EngineIdentity },
    EmptyFixtureId,
    EmptyReproCommand,
    Serialization(String),
}

impl fmt::Display for OracleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfComparison { identity } => {
                write!(
                    formatter,
                    "subject and oracle must differ, both were {identity:?}"
                )
            }
            Self::EmptyFixtureId => formatter.write_str("fixture_id must not be empty"),
            Self::EmptyReproCommand => {
                formatter.write_str("reproducible command must not be empty")
            }
            Self::Serialization(message) => {
                write!(formatter, "serializing failure payload: {message}")
            }
        }
    }
}

impl Error for OracleError {}

/// Inputs shared by fixture tests and generated/property scenarios.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleScenario {
    mode: OracleMode,
    subject: EngineIdentity,
    oracle: EngineIdentity,
    seed: u64,
    fixture_id: String,
    repro_command: String,
}

impl OracleScenario {
    pub fn new(
        mode: OracleMode,
        subject: EngineIdentity,
        oracle: EngineIdentity,
        seed: u64,
        fixture_id: impl Into<String>,
        repro_command: impl Into<String>,
    ) -> Result<Self, OracleError> {
        validate_identities(subject, oracle)?;
        let fixture_id = fixture_id.into();
        let repro_command = repro_command.into();
        validate_text(&fixture_id, &repro_command)?;
        Ok(Self {
            mode,
            subject,
            oracle,
            seed,
            fixture_id,
            repro_command,
        })
    }

    /// Compare structured results. A mismatch returns a complete failure bundle;
    /// a match allocates no evidence bundle.
    pub fn compare(&self, expected: Value, actual: Value) -> Result<ComparisonResult, OracleError> {
        validate_identities(self.subject, self.oracle)?;
        if expected == actual {
            return Ok(ComparisonResult {
                matched: true,
                failure: None,
            });
        }
        let failure = FailureBundle::new(
            self.mode,
            self.subject,
            self.oracle,
            self.seed,
            self.fixture_id.clone(),
            self.repro_command.clone(),
            expected,
            actual,
        )?;
        Ok(ComparisonResult {
            matched: false,
            failure: Some(failure),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComparisonResult {
    pub matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureBundle>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailureBundle {
    pub version: &'static str,
    pub id: String,
    pub digest: String,
    pub mode: OracleMode,
    pub subject: EngineIdentity,
    pub oracle: EngineIdentity,
    pub seed: u64,
    pub fixture_id: String,
    pub repro_command: String,
    pub expected: Value,
    pub actual: Value,
}

#[derive(Serialize)]
struct FailurePayload<'a> {
    version: &'static str,
    mode: OracleMode,
    subject: EngineIdentity,
    oracle: EngineIdentity,
    seed: u64,
    fixture_id: &'a str,
    repro_command: &'a str,
    expected: &'a Value,
    actual: &'a Value,
}

impl FailureBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: OracleMode,
        subject: EngineIdentity,
        oracle: EngineIdentity,
        seed: u64,
        fixture_id: impl Into<String>,
        repro_command: impl Into<String>,
        expected: Value,
        actual: Value,
    ) -> Result<Self, OracleError> {
        validate_identities(subject, oracle)?;
        let fixture_id = fixture_id.into();
        let repro_command = repro_command.into();
        validate_text(&fixture_id, &repro_command)?;
        let payload = FailurePayload {
            version: FAILURE_BUNDLE_VERSION,
            mode,
            subject,
            oracle,
            seed,
            fixture_id: &fixture_id,
            repro_command: &repro_command,
            expected: &expected,
            actual: &actual,
        };
        let value = serde_json::to_value(payload)
            .map_err(|error| OracleError::Serialization(error.to_string()))?;
        let canonical = zero_abi::canonical_json(&value);
        let digest = zero_abi::sha256_hex(canonical.as_bytes());
        let id = format!("oracle-v1:{digest}");
        Ok(Self {
            version: FAILURE_BUNDLE_VERSION,
            id,
            digest,
            mode,
            subject,
            oracle,
            seed,
            fixture_id,
            repro_command,
            expected,
            actual,
        })
    }
}

fn validate_identities(subject: EngineIdentity, oracle: EngineIdentity) -> Result<(), OracleError> {
    if subject == oracle {
        return Err(OracleError::SelfComparison { identity: subject });
    }
    Ok(())
}

fn validate_text(fixture_id: &str, repro_command: &str) -> Result<(), OracleError> {
    if fixture_id.trim().is_empty() {
        return Err(OracleError::EmptyFixtureId);
    }
    if repro_command.trim().is_empty() {
        return Err(OracleError::EmptyReproCommand);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scenario(mode: OracleMode, oracle: EngineIdentity, seed: u64) -> OracleScenario {
        OracleScenario::new(
            mode,
            EngineIdentity::TokenZero,
            oracle,
            seed,
            "oracle.unit",
            "cargo test -p zerostack-codemode-conformance --lib --locked oracle_",
        )
        .unwrap()
    }

    #[test]
    fn oracle_spec_uses_committed_canonical_dispatch_golden() {
        let golden: Value =
            serde_json::from_str(include_str!("../fixtures/canonical_dispatch_vectors.json"))
                .unwrap();
        let expected = golden.pointer("/registry_projections/0").unwrap().clone();
        let result = OracleScenario::new(
            OracleMode::Spec,
            EngineIdentity::TokenZero,
            EngineIdentity::Specification,
            0,
            "canonical_dispatch_vectors.registry_projections.token_zero",
            "cargo test -p zerostack-codemode-conformance --test canonical_dispatch --locked",
        )
        .unwrap()
        .compare(expected.clone(), expected)
        .unwrap();
        assert!(result.matched);
        assert!(result.failure.is_none());
    }

    #[test]
    fn oracle_property_preserves_seed_in_failure_evidence() {
        let seed = 0x5eed_u64;
        let generated = seed.rotate_left(7) ^ 0xa5a5;
        let result = scenario(
            OracleMode::Property,
            EngineIdentity::ConformanceHarness,
            seed,
        )
        .compare(
            json!({ "value": generated }),
            json!({ "value": generated + 1 }),
        )
        .unwrap();
        let failure = result.failure.unwrap();
        assert_eq!(failure.seed, seed);
        assert_eq!(failure.mode, OracleMode::Property);
    }

    #[test]
    fn oracle_self_check_compares_distinct_harness_and_engine() {
        let result = scenario(OracleMode::SelfCheck, EngineIdentity::ConformanceHarness, 0)
            .compare(json!({ "valid": true }), json!({ "valid": true }))
            .unwrap();
        assert!(result.matched);
    }

    #[test]
    fn oracle_round_trip_compares_encoded_then_decoded_value() {
        let original = json!({ "engine": "graph_zero", "items": [3, 2, 1] });
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: Value = serde_json::from_slice(&encoded).unwrap();
        let result = scenario(
            OracleMode::RoundTrip,
            EngineIdentity::ConformanceHarness,
            91,
        )
        .compare(original, decoded)
        .unwrap();
        assert!(result.matched);
    }

    #[test]
    fn oracle_external_tool_uses_explicit_reference_result() {
        // This records an identified jq reference result; it does not assert
        // broad external-tool parity.
        let jq_reference = json!(["fs_zero", "graph_zero", "token_zero"]);
        let subject_result = json!(["fs_zero", "graph_zero", "token_zero"]);
        let result = scenario(OracleMode::ExternalTool, EngineIdentity::ExternalJq, 0)
            .compare(jq_reference, subject_result)
            .unwrap();
        assert!(result.matched);
    }

    #[test]
    fn oracle_rejects_self_comparison_before_comparing() {
        let error = OracleScenario::new(
            OracleMode::SelfCheck,
            EngineIdentity::FsZero,
            EngineIdentity::FsZero,
            0,
            "self-check",
            "cargo test oracle_",
        )
        .unwrap_err();
        assert_eq!(
            error,
            OracleError::SelfComparison {
                identity: EngineIdentity::FsZero
            }
        );
    }

    #[test]
    fn oracle_bundle_ids_are_deterministic_and_address_sensitive() {
        let make = |actual: Value| {
            scenario(OracleMode::Property, EngineIdentity::ConformanceHarness, 7)
                .compare(json!({ "answer": 42 }), actual)
                .unwrap()
                .failure
                .unwrap()
        };
        let first = make(json!({ "answer": 41 }));
        let same = make(json!({ "answer": 41 }));
        let changed = make(json!({ "answer": 40 }));
        assert_eq!(first.id, same.id);
        assert_eq!(first.digest, same.digest);
        assert_ne!(first.id, changed.id);
        assert_ne!(first.digest, changed.digest);
        assert_eq!(first.id, format!("oracle-v1:{}", first.digest));
    }

    #[test]
    fn oracle_rejects_empty_fixture_and_repro_fields() {
        let empty_fixture = OracleScenario::new(
            OracleMode::Spec,
            EngineIdentity::GraphZero,
            EngineIdentity::Specification,
            0,
            " ",
            "cargo test oracle_",
        )
        .unwrap_err();
        assert_eq!(empty_fixture, OracleError::EmptyFixtureId);

        let empty_repro = OracleScenario::new(
            OracleMode::Spec,
            EngineIdentity::GraphZero,
            EngineIdentity::Specification,
            0,
            "fixture",
            "\n",
        )
        .unwrap_err();
        assert_eq!(empty_repro, OracleError::EmptyReproCommand);
    }
}
