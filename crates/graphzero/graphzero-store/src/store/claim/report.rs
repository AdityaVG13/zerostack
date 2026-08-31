use serde::{Deserialize, Serialize};

use super::super::absence::AbsenceAnswer;

/// One graph-backed counterexample when a claim is false.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurvivingSpan {
    pub kind: String,
    pub from_symbol: String,
    pub to_symbol: String,
    pub evidence_ref: String,
    pub confidence: f64,
    pub source: String,
}

/// Coverage certificate attached to every verify result (analogue).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimCertificate {
    pub tier_a_pct: f64,
    pub tier_b_pct: f64,
    pub tier_c_pct: f64,
    pub freshness_verified: bool,
    pub snapshot_id: u64,
    pub gap_blob_count: usize,
}

/// Structured claim-verification result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimVerifyResult {
    pub schema_version: u32,
    pub verified: bool,
    pub claim_kind: String,
    pub target: String,
    pub summary: String,
    pub certificate: ClaimCertificate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surviving_spans: Vec<SurvivingSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
    /// Optional provenance rows for evidence references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<crate::ProvenanceRecord>,
}

pub(super) fn claim_certificate_from_absence(presence: &AbsenceAnswer) -> ClaimCertificate {
    ClaimCertificate {
        tier_a_pct: presence.certificate.tier_a_pct,
        tier_b_pct: presence.certificate.tier_b_pct,
        tier_c_pct: presence.certificate.tier_c_pct,
        freshness_verified: presence.certificate.freshness_verified,
        snapshot_id: presence.certificate.snapshot_id,
        gap_blob_count: presence.certificate.gap_blob_count,
    }
}

pub(super) fn claim_result_unknown_coverage(
    claim_kind: &str,
    target: &str,
    cert: ClaimCertificate,
    presence: &AbsenceAnswer,
) -> ClaimVerifyResult {
    ClaimVerifyResult {
        schema_version: 1,
        verified: false,
        claim_kind: claim_kind.to_string(),
        target: target.to_string(),
        summary: format!("unknown: cannot verify {:?} — {}", target, presence.summary),
        certificate: cert,
        evidence_ref: None,
        surviving_spans: Vec::new(),
        unknown_reason: presence
            .staleness_reason
            .clone()
            .or_else(|| Some("partial_coverage_or_stale".into())),
        provenance: Vec::new(),
    }
}

pub(super) fn claim_result_target_not_found(
    claim_kind: &str,
    target: &str,
    cert: ClaimCertificate,
    summary: &str,
    reason: &str,
) -> ClaimVerifyResult {
    ClaimVerifyResult {
        schema_version: 1,
        verified: false,
        claim_kind: claim_kind.to_string(),
        target: target.to_string(),
        summary: summary.to_string(),
        certificate: cert,
        evidence_ref: None,
        surviving_spans: Vec::new(),
        unknown_reason: Some(reason.into()),
        provenance: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn claim_result_from_survivors(
    claim_kind: &str,
    target: &str,
    cert: ClaimCertificate,
    surviving: Vec<SurvivingSpan>,
    verified_summary: &str,
    refuted_label: &str,
    target_evidence_ref: Option<String>,
) -> ClaimVerifyResult {
    if surviving.is_empty() {
        return ClaimVerifyResult {
            schema_version: 1,
            verified: true,
            claim_kind: claim_kind.to_string(),
            target: target.to_string(),
            summary: verified_summary.to_string(),
            certificate: cert,
            evidence_ref: target_evidence_ref,
            surviving_spans: Vec::new(),
            unknown_reason: None,
            provenance: Vec::new(),
        };
    }

    ClaimVerifyResult {
        schema_version: 1,
        verified: false,
        claim_kind: claim_kind.to_string(),
        target: target.to_string(),
        summary: format!(
            "refuted: {} surviving {refuted_label} involving {:?}",
            surviving.len(),
            target
        ),
        certificate: cert,
        evidence_ref: surviving
            .first()
            .map(|span| span.evidence_ref.clone())
            .or(target_evidence_ref),
        surviving_spans: surviving,
        unknown_reason: None,
        provenance: Vec::new(),
    }
}
