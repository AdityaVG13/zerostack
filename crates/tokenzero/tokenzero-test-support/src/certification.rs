//! Strict-conformant-release.v1 assessment.
//!
//! `CERTIFICATION_MIN_VERIFICATION_PCT = 100.0` is catalog evidence
//! completeness (every obligation Satisfied), not "conformal lower bound
//! equals 1.0". A Beta(1,1) lower bound is strictly below 1.0 for every
//! finite sample; comparing it to 1.0 made certification unreachable.
//! The conformal lower bound remains the ratchet metric.

use crate::conformal::{DEFAULT_CONFIDENCE, ParityScorecard};
use crate::invariant_catalog::{
    BaseGate, CloseDecision, ContractStatus, InvariantCatalog, close_decision,
    seal_satisfied_hashes,
};
use crate::parity_taxonomy::{FeatureUniverse, truncate_score};

use std::path::Path;

pub const CERTIFICATION_MIN_VERIFICATION_PCT: f64 = 100.0;
pub const CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT: f64 = 100.0;
pub const CERTIFICATION_MAX_HIGH_SEVERITY_COUNTEREXAMPLES: u32 = 0;
pub const CERTIFICATION_SCHEMA: &str = "tokenzero.certification-assessment.v1";

/// Conformal lower == 1.0 is unreachable under the uniform prior.
pub const CONFORMAL_LOWER_ONE_UNREACHABLE: bool = true;

#[derive(Debug, Clone, PartialEq)]
pub enum CertificationVerdict {
    Ready,
    Hold { reasons: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CertificationAssessment {
    pub schema_version: String,
    pub catalog_status: ContractStatus,
    pub verification_pct: f64,
    pub suite_pass_rate_pct: f64,
    pub high_severity_counterexamples: u32,
    pub conformal_lower: f64,
    pub conformal_point: f64,
    pub conformal_used_as: &'static str,
    pub close: CloseDecision,
    pub verdict: CertificationVerdict,
}

impl CertificationAssessment {
    pub fn is_ready(&self) -> bool {
        matches!(self.verdict, CertificationVerdict::Ready)
    }
}

pub fn catalog_verification_pct(catalog: &InvariantCatalog) -> f64 {
    let obligations: Vec<_> = catalog
        .invariants()
        .iter()
        .flat_map(|inv| inv.proof_obligations.iter())
        .collect();
    if obligations.is_empty() {
        return 0.0;
    }
    let satisfied = obligations.iter().filter(|o| o.status.is_met()).count();
    truncate_score(100.0 * satisfied as f64 / obligations.len() as f64)
}

/// Catalog Pass + 100% verification + 100% required suite + zero high-severity
/// counterexamples. Conformal lower is the ratchet input, not a 1.0 gate.
pub fn assess_certification(
    catalog: &mut InvariantCatalog,
    universe: &FeatureUniverse,
    repo_root: &Path,
    suite_pass_rate_pct: f64,
    high_severity_counterexamples: u32,
) -> CertificationAssessment {
    seal_satisfied_hashes(catalog, repo_root);
    let catalog_status = catalog.contract_status(repo_root);
    let verification_pct = catalog_verification_pct(catalog);
    let close = close_decision(catalog_status, BaseGate::Allowed);
    let scorecard: ParityScorecard = universe.conformal_scorecard_at(DEFAULT_CONFIDENCE, &[]);
    let mut reasons = Vec::new();
    if close != CloseDecision::Close {
        reasons.push(format!("catalog {catalog_status:?} is not Close"));
    }
    if verification_pct < CERTIFICATION_MIN_VERIFICATION_PCT {
        reasons.push(format!(
            "verification_pct {verification_pct} < {CERTIFICATION_MIN_VERIFICATION_PCT}"
        ));
    }
    if suite_pass_rate_pct < CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT {
        reasons.push(format!(
            "suite_pass_rate_pct {suite_pass_rate_pct} < {CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT}"
        ));
    }
    if high_severity_counterexamples > CERTIFICATION_MAX_HIGH_SEVERITY_COUNTEREXAMPLES {
        reasons.push(format!(
            "high_severity_counterexamples {high_severity_counterexamples} > 0"
        ));
    }
    let verdict = if reasons.is_empty() {
        CertificationVerdict::Ready
    } else {
        CertificationVerdict::Hold { reasons }
    };
    CertificationAssessment {
        schema_version: CERTIFICATION_SCHEMA.to_string(),
        catalog_status,
        verification_pct,
        suite_pass_rate_pct: truncate_score(suite_pass_rate_pct),
        high_severity_counterexamples,
        conformal_lower: scorecard.global_lower,
        conformal_point: scorecard.global_point,
        conformal_used_as: "ratchet_high_water",
        close,
        verdict,
    }
}

#[cfg(test)]
#[path = "../../../../tests/tokenzero/unit/tokenzero-test-support/certification_tests.rs"]
mod tests;
