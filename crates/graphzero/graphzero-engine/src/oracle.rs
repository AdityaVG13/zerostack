//! GraphZero's five-mode oracle adapter and failure-only bundle contract.
//!
//! This module deliberately carries failures, not pass results. It reuses the
//! hub engine identity, release-gate diagnosis fields, and deterministic facts
//! canonicalizer so an oracle receipt cannot silently invent a second protocol.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;
use zero_abi::EngineIdentity;

use crate::deterministic_facts::canonical_json;
use crate::release_gates::{GateFailure, ReleaseGateReport};

/// Version of the GraphZero-owned failure bundle wire shape.
pub const FAILURE_BUNDLE_SCHEMA_VERSION: u32 = 1;

/// The five supported oracle modes. Serde names are part of the wire contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OracleMode {
    Gold,
    Differential,
    Metamorphic,
    Property,
    Mutation,
}

/// Typed validation and encoding failures for [`FailureBundle`].
#[derive(Debug)]
pub enum OracleBundleError {
    Json(serde_json::Error),
    InvalidField { field: &'static str, reason: String },
    SchemaVersion { found: u32 },
    EngineMismatch { found: EngineIdentity },
    DigestMismatch { expected: String, found: String },
}

impl fmt::Display for OracleBundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid failure bundle JSON: {error}"),
            Self::InvalidField { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::SchemaVersion { found } => write!(
                f,
                "unsupported failure bundle schema version {found}; expected {FAILURE_BUNDLE_SCHEMA_VERSION}"
            ),
            Self::EngineMismatch { found } => {
                write!(
                    f,
                    "failure bundle engine must be graphzero, found {found:?}"
                )
            }
            Self::DigestMismatch { expected, found } => {
                write!(
                    f,
                    "contract digest mismatch: expected {expected}, found {found}"
                )
            }
        }
    }
}

impl std::error::Error for OracleBundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for OracleBundleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Failure-only, versioned oracle evidence emitted by GraphZero.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureBundle {
    pub schema_version: u32,
    pub mode: OracleMode,
    pub engine: EngineIdentity,
    pub contract_digest: String,
    pub semantic_contract_version: String,
    pub corpus_identity: String,
    pub source_revision: String,
    pub failures: Vec<GateFailure>,
    pub evidence_refs: Vec<String>,
}

impl FailureBundle {
    /// Build a failure bundle from an existing release-gate report.
    ///
    /// The report owns the digest, semantic version, corpus identity, and all
    /// diagnosis fields. A passed report or empty evidence cannot become a
    /// bundle, so this adapter has no hidden success representation.
    pub fn from_report(
        report: &ReleaseGateReport,
        mode: OracleMode,
        source_revision: impl Into<String>,
        evidence_refs: Vec<String>,
    ) -> Result<Self, OracleBundleError> {
        if report.passed {
            return Err(OracleBundleError::InvalidField {
                field: "report",
                reason: "a passed release-gate report cannot produce a failure bundle".into(),
            });
        }
        let source_revision = source_revision.into();
        let mut bundle = Self {
            schema_version: FAILURE_BUNDLE_SCHEMA_VERSION,
            mode,
            engine: EngineIdentity::GraphZero,
            contract_digest: report.contract_digest.clone(),
            semantic_contract_version: report.semantic_contract_version.clone(),
            corpus_identity: report.corpus_version.clone(),
            source_revision,
            failures: report.failures.clone(),
            evidence_refs,
        };
        bundle.normalize();
        bundle.validate(&bundle.contract_digest.clone())?;
        Ok(bundle)
    }

    /// Alias emphasizing that the source is the release-gate adapter.
    pub fn from_release_gate_report(
        report: &ReleaseGateReport,
        mode: OracleMode,
        source_revision: impl Into<String>,
        evidence_refs: Vec<String>,
    ) -> Result<Self, OracleBundleError> {
        Self::from_report(report, mode, source_revision, evidence_refs)
    }

    /// Build from a release report while checking an independently expected digest.
    pub fn from_report_with_expected_digest(
        report: &ReleaseGateReport,
        mode: OracleMode,
        source_revision: impl Into<String>,
        evidence_refs: Vec<String>,
        expected_digest: &str,
    ) -> Result<Self, OracleBundleError> {
        let bundle = Self::from_report(report, mode, source_revision, evidence_refs)?;
        bundle.validate(expected_digest)?;
        Ok(bundle)
    }

    /// Validate all wire invariants, including the expected contract digest.
    pub fn validate(&self, expected_digest: &str) -> Result<(), OracleBundleError> {
        if self.schema_version != FAILURE_BUNDLE_SCHEMA_VERSION {
            return Err(OracleBundleError::SchemaVersion {
                found: self.schema_version,
            });
        }
        if self.engine != EngineIdentity::GraphZero {
            return Err(OracleBundleError::EngineMismatch { found: self.engine });
        }
        validate_hex(
            "contract_digest",
            &self.contract_digest,
            64,
            "lowercase hexadecimal",
        )?;
        validate_hex(
            "expected_digest",
            expected_digest,
            64,
            "lowercase hexadecimal",
        )?;
        if self.contract_digest != expected_digest {
            return Err(OracleBundleError::DigestMismatch {
                expected: expected_digest.to_owned(),
                found: self.contract_digest.clone(),
            });
        }
        validate_nonempty_text("semantic_contract_version", &self.semantic_contract_version)?;
        validate_nonempty_text("corpus_identity", &self.corpus_identity)?;
        validate_hex(
            "source_revision",
            &self.source_revision,
            40,
            "lowercase hexadecimal",
        )?;
        if self.failures.is_empty() {
            return Err(OracleBundleError::InvalidField {
                field: "failures",
                reason: "must contain at least one GateFailure".into(),
            });
        }
        if self.evidence_refs.is_empty() {
            return Err(OracleBundleError::InvalidField {
                field: "evidence_refs",
                reason: "must contain at least one reference".into(),
            });
        }
        for evidence_ref in &self.evidence_refs {
            validate_nonempty_text("evidence_refs", evidence_ref)?;
            if evidence_ref.chars().any(char::is_control) {
                return Err(OracleBundleError::InvalidField {
                    field: "evidence_refs",
                    reason: "references must not contain control characters".into(),
                });
            }
        }
        if self
            .evidence_refs
            .windows(2)
            .any(|window| window[0] >= window[1])
        {
            return Err(OracleBundleError::InvalidField {
                field: "evidence_refs",
                reason: "references must be sorted and deduplicated".into(),
            });
        }
        Ok(())
    }

    /// Parse and fail closed against the expected contract digest.
    pub fn from_json(input: &str, expected_digest: &str) -> Result<Self, OracleBundleError> {
        let bundle: Self = serde_json::from_str(input)?;
        bundle.validate(expected_digest)?;
        Ok(bundle)
    }

    /// Alias for callers that name the operation as parsing.
    pub fn parse_json(input: &str, expected_digest: &str) -> Result<Self, OracleBundleError> {
        Self::from_json(input, expected_digest)
    }

    /// Return deterministic JSON bytes using GraphZero's existing canonicalizer.
    pub fn canonical_json(&self) -> Result<String, OracleBundleError> {
        let expected = self.contract_digest.clone();
        self.validate(&expected)?;
        let mut normalized = self.clone();
        normalized.normalize();
        let value = serde_json::to_value(&normalized)?;
        Ok(canonical_json(&value))
    }

    /// Return the UTF-8 bytes of [`Self::canonical_json`].
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, OracleBundleError> {
        Ok(self.canonical_json()?.into_bytes())
    }

    fn normalize(&mut self) {
        self.failures
            .sort_by(|left, right| diagnosis_identity(left).cmp(&diagnosis_identity(right)));
        self.evidence_refs.sort();
        self.evidence_refs.dedup();
    }
}

fn diagnosis_identity(failure: &GateFailure) -> String {
    canonical_json(&json!({
        "gate": failure.gate,
        "tier": failure.tier,
        "operation": failure.operation,
        "surface": failure.surface,
        "normalized_diff": failure.normalized_diff,
        "planner_owner": failure.planner_owner,
        "compression_owner": failure.compression_owner,
        "latency_stage": failure.latency_stage,
        "message": failure.message,
    }))
}

fn validate_nonempty_text(field: &'static str, value: &str) -> Result<(), OracleBundleError> {
    if value.trim().is_empty() {
        return Err(OracleBundleError::InvalidField {
            field,
            reason: "must be nonempty".into(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(OracleBundleError::InvalidField {
            field,
            reason: "must not contain control characters".into(),
        });
    }
    Ok(())
}

fn validate_hex(
    field: &'static str,
    value: &str,
    length: usize,
    description: &'static str,
) -> Result<(), OracleBundleError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OracleBundleError::InvalidField {
            field,
            reason: format!("must be {length} characters of {description}"),
        });
    }
    Ok(())
}
