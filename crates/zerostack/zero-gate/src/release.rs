//! Release claim gate. Every public claim must pass the nine release
//! gates and must cite a CURRENT formulation.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const RELEASE_CONTRACT_VERSION: u16 = 1;

/// The nine public-claim gates.
pub const PUBLIC_CLAIM_GATES: [&str; 9] = [
    "theorem_status_current",
    "provider_fact_date_current",
    "citation_resolved",
    "no_unsupported_q99_substitution",
    "benchmark_evidence_present",
    "checksums_present",
    "negative_results_recorded",
    "claim_scope_declared",
    "artifact_manifest_complete",
];

/// Known superseded formulations (corpus 03 history/corrections). Claims
/// citing these as current authority are rejected.
pub const SUPERSEDED_FORMULATIONS: [&str; 3] = [
    "draft4_rewrite_formula",
    "model_internal_one_token_framing",
    "ambiguous_q99_percentage",
];

/// Fail-closed error for claim and supersession construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseError {
    InvalidClaim(String),
    InvalidSupersession(String),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClaim(detail) => write!(formatter, "invalid release claim: {detail}"),
            Self::InvalidSupersession(detail) => {
                write!(formatter, "invalid supersession table: {detail}")
            }
        }
    }
}

impl Error for ReleaseError {}

/// One supersession record: a formulation was superseded by a newer one with
/// a recorded reason. `None` in `superseded_by` marks the current authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupersessionRecord {
    pub formulation_id: String,
    pub superseded_by: Option<String>,
    pub reason: String,
}

impl SupersessionRecord {
    pub fn new(
        formulation_id: impl Into<String>,
        superseded_by: Option<String>,
        reason: impl Into<String>,
    ) -> Result<Self, ReleaseError> {
        let record = Self {
            formulation_id: formulation_id.into(),
            superseded_by,
            reason: reason.into(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.formulation_id.is_empty() {
            return Err(ReleaseError::InvalidSupersession(
                "formulation_id must be nonempty".into(),
            ));
        }
        if self.reason.is_empty() {
            return Err(ReleaseError::InvalidSupersession(
                "reason must be nonempty".into(),
            ));
        }
        Ok(())
    }
}

/// The current correction/supersession table. Every claim is checked against
/// it before release.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SupersessionTable {
    pub records: Vec<SupersessionRecord>,
}

impl SupersessionTable {
    pub fn new(records: Vec<SupersessionRecord>) -> Result<Self, ReleaseError> {
        let table = Self { records };
        table.validate()?;
        Ok(table)
    }

    pub fn validate(&self) -> Result<(), ReleaseError> {
        let mut ids = std::collections::BTreeSet::new();
        for record in &self.records {
            record.validate()?;
            if !ids.insert(record.formulation_id.clone()) {
                return Err(ReleaseError::InvalidSupersession(format!(
                    "duplicate formulation {}",
                    record.formulation_id
                )));
            }
        }
        Ok(())
    }

    /// Whether a formulation is current authority (no record, or a record
    /// whose `superseded_by` is `None`).
    pub fn is_current(&self, formulation_id: &str) -> bool {
        self.records
            .iter()
            .find(|record| record.formulation_id == formulation_id)
            .map(|record| record.superseded_by.is_none())
            .unwrap_or(true)
    }

    /// Returns the supersession reason, if present.
    pub fn supersession_reason(&self, formulation_id: &str) -> Option<&str> {
        self.records
            .iter()
            .find(|record| record.formulation_id == formulation_id)
            .and_then(|record| {
                record
                    .superseded_by
                    .as_ref()
                    .map(|_| record.reason.as_str())
            })
    }
}

/// One public claim under release review.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicClaim {
    pub claim_version: u16,
    pub claim_id: String,
    /// The formulation this claim cites as current authority.
    pub formulation_id: String,
    /// Nine release gates, keyed by the gate names in [`PUBLIC_CLAIM_GATES`].
    pub gates: std::collections::BTreeMap<String, bool>,
    pub required_artifacts: Vec<String>,
    pub provider_fact_date_unix_ms: Option<u64>,
    pub claim_scope: String,
}

impl PublicClaim {
    pub fn new(
        claim_id: impl Into<String>,
        formulation_id: impl Into<String>,
        gates: std::collections::BTreeMap<String, bool>,
        required_artifacts: Vec<String>,
        provider_fact_date_unix_ms: Option<u64>,
        claim_scope: impl Into<String>,
    ) -> Result<Self, ReleaseError> {
        let claim = Self {
            claim_version: RELEASE_CONTRACT_VERSION,
            claim_id: claim_id.into(),
            formulation_id: formulation_id.into(),
            gates,
            required_artifacts,
            provider_fact_date_unix_ms,
            claim_scope: claim_scope.into(),
        };
        claim.validate()?;
        Ok(claim)
    }

    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.claim_version != RELEASE_CONTRACT_VERSION {
            return Err(ReleaseError::InvalidClaim(format!(
                "unsupported claim version {}",
                self.claim_version
            )));
        }
        if self.claim_id.is_empty() || self.formulation_id.is_empty() {
            return Err(ReleaseError::InvalidClaim(
                "claim_id and formulation_id must be nonempty".into(),
            ));
        }
        if self.claim_scope.is_empty() {
            return Err(ReleaseError::InvalidClaim(
                "claim_scope must be nonempty".into(),
            ));
        }
        Ok(())
    }

    /// All gate names must be declared; unknown gate names are rejected.
    pub fn gate_names(&self) -> Result<(), ReleaseError> {
        for gate in PUBLIC_CLAIM_GATES {
            if !self.gates.contains_key(gate) {
                return Err(ReleaseError::InvalidClaim(format!("missing gate {gate}")));
            }
        }
        for name in self.gates.keys() {
            if !PUBLIC_CLAIM_GATES.contains(&name.as_str()) {
                return Err(ReleaseError::InvalidClaim(format!("unknown gate {name}")));
            }
        }
        Ok(())
    }
}

/// The release verdict: approved, or rejected with deterministic reasons.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseVerdict {
    pub approved: bool,
    pub reasons: Vec<String>,
}

/// The release checker: gates + supersession + artifact/date/scope
/// completeness. One deterministic verdict, nothing inferred.
pub struct ReleaseChecker;

impl ReleaseChecker {
    pub fn check(
        claim: &PublicClaim,
        supersession: &SupersessionTable,
    ) -> Result<ReleaseVerdict, ReleaseError> {
        claim.validate()?;
        claim.gate_names()?;
        supersession.validate()?;

        let mut reasons = Vec::new();
        for gate in PUBLIC_CLAIM_GATES {
            if !claim.gates.get(gate).copied().unwrap_or(false) {
                reasons.push(format!("gate_not_satisfied:{gate}"));
            }
        }
        if claim.required_artifacts.is_empty() {
            reasons.push("required_artifacts_missing".into());
        }
        if claim.provider_fact_date_unix_ms.is_none() {
            reasons.push("provider_fact_date_missing".into());
        }
        if claim.claim_scope.is_empty() {
            reasons.push("claim_scope_missing".into());
        }
        if let Some(reason) = supersession.supersession_reason(&claim.formulation_id) {
            reasons.push(format!(
                "superseded_formulation:{}:{}",
                claim.formulation_id, reason
            ));
        }
        Ok(ReleaseVerdict {
            approved: reasons.is_empty(),
            reasons,
        })
    }
}
