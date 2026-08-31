//! Three-state absence answers derived from snapshot coverage and freshness.
//! Negative answers carry coverage scope; a stale index never proves absence.

use anyhow::Result;

use crate::Tier;

use super::query::Snapshot;

/// Mutually exclusive answer classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerClass {
    Present,
    Absent,
    Unknown,
}

impl AnswerClass {
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerClass::Present => "present",
            AnswerClass::Absent => "absent",
            AnswerClass::Unknown => "unknown",
        }
    }
}

/// Machine-parseable coverage certificate attached to every answer.
#[derive(Clone, Debug, PartialEq)]
pub struct AbsenceCertificate {
    pub tier_a_pct: f64,
    pub tier_b_pct: f64,
    pub tier_c_pct: f64,
    pub freshness_verified: bool,
    pub snapshot_id: u64,
    pub generated_at_secs: u64,
    pub gap_blob_count: usize,
}

/// Structured absence query result for CLI / library JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct AbsenceAnswer {
    pub class: AnswerClass,
    pub query: String,
    pub certificate: AbsenceCertificate,
    pub evidence_ref: Option<String>,
    pub staleness_reason: Option<String>,
    pub summary: String,
}

/// Tier-A coverage fraction required before ABSENT (default 0.99).
#[derive(Clone, Copy, Debug)]
pub struct AbsenceConfig {
    pub tier_a_threshold: f64,
    pub check_freshness: bool,
}

impl Default for AbsenceConfig {
    fn default() -> Self {
        Self {
            tier_a_threshold: 0.99,
            check_freshness: true,
        }
    }
}

/// Classify symbol presence with coverage-bound certificates.
pub fn absence(snapshot: &Snapshot, symbol: &str, config: AbsenceConfig) -> Result<AbsenceAnswer> {
    let query = symbol.trim().to_string();
    let capsule = snapshot.query(&query, 256, config.check_freshness)?;
    let tier_a = capsule.tier_a;
    let tier_b = capsule.tier_b;
    let tier_c = capsule.tier_c;
    let snapshot_id = capsule.snapshot_id;

    // Reuse the staleness verdict the query already computed (graphzero perf) `query_with_repair` runs
    // the full per-file scan when check_freshness is set and records any fault in `freshness.events`.
    // Re-running the scan here doubled the cost of every verify/absence call for the identical answer.
    let stale_reason = if config.check_freshness {
        capsule.freshness.events.first().cloned()
    } else {
        None
    };

    let gap_blob_count = snapshot.unindexed_blob_count(Tier::A);
    let cert = AbsenceCertificate {
        tier_a_pct: tier_a * 100.0,
        tier_b_pct: tier_b * 100.0,
        tier_c_pct: tier_c * 100.0,
        freshness_verified: stale_reason.is_none() && tier_a >= config.tier_a_threshold,
        snapshot_id,
        generated_at_secs: unix_now_secs(),
        gap_blob_count,
    };

    // Presence requires a definition span with evidence. Symbols that exist only as
    // edge targets (e.g. interned by a verification evidence edge) are not "present".
    let evidence_ref = capsule
        .matches
        .iter()
        .flat_map(|m| m.defs.iter())
        .map(|d| d.evidence_ref.as_str())
        .find(|reference| !reference.is_empty())
        .map(str::to_string);
    if let Some(evidence_ref) = evidence_ref {
        let summary = format!(
            "present: symbol {:?} found (snapshot {})",
            query, snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Present,
            query,
            certificate: cert,
            evidence_ref: Some(evidence_ref),
            staleness_reason: None,
            summary,
        });
    }

    if let Some(reason) = stale_reason {
        let summary = format!(
            "unknown: index stale — {}; tier-A {:.1}% (snapshot {})",
            reason, cert.tier_a_pct, snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Unknown,
            query,
            certificate: cert,
            evidence_ref: None,
            staleness_reason: Some(reason),
            summary,
        });
    }

    if tier_a < config.tier_a_threshold {
        let summary = format!(
            "unknown: tier-A coverage {:.1}% below threshold {:.0}% (snapshot {})",
            cert.tier_a_pct,
            config.tier_a_threshold * 100.0,
            snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Unknown,
            query,
            certificate: cert,
            evidence_ref: None,
            staleness_reason: Some("partial_coverage".into()),
            summary,
        });
    }

    let summary = format!(
        "absent: no symbol {:?} under tier-A {:.1}% fresh coverage (snapshot {})",
        query, cert.tier_a_pct, snapshot_id
    );
    Ok(AbsenceAnswer {
        class: AnswerClass::Absent,
        query,
        certificate: cert,
        evidence_ref: None,
        staleness_reason: None,
        summary,
    })
}

impl AbsenceAnswer {
    /// Serialize an absence result as stable JSON.
    pub fn to_json(&self) -> String {
        let class = self.class.as_str();
        let evidence = self
            .evidence_ref
            .as_ref()
            .map(|r| format!(",\"evidence_ref\":\"{}\"", super::expand::json_escape(r)))
            .unwrap_or_default();
        let stale = self
            .staleness_reason
            .as_ref()
            .map(|r| {
                format!(
                    ",\"staleness_reason\":\"{}\"",
                    super::expand::json_escape(r)
                )
            })
            .unwrap_or_default();
        let unknown = if self.class == AnswerClass::Unknown {
            ",\"unknown_reason\":\"coverage_or_freshness\""
        } else {
            ""
        };
        format!(
            "{{\"class\":\"{class}\",\"query\":\"{}\",\"coverage_certificate\":{{\"tier_a_pct\":{:.4},\"tier_b_pct\":{:.4},\"tier_c_pct\":{:.4},\"freshness_verified\":{},\"snapshot_id\":{}}},\"summary\":\"{}\"{evidence}{stale}{unknown}}}",
            super::expand::json_escape(&self.query),
            self.certificate.tier_a_pct,
            self.certificate.tier_b_pct,
            self.certificate.tier_c_pct,
            self.certificate.freshness_verified,
            self.certificate.snapshot_id,
            super::expand::json_escape(&self.summary),
        )
    }

    /// Reject uncertified bare negatives without coverage.
    pub fn validate_certified_negative(&self) -> Result<(), String> {
        match self.class {
            AnswerClass::Present => {
                if self.evidence_ref.as_deref().is_none_or(str::is_empty) {
                    return Err("present answer missing evidence_ref".into());
                }
                Ok(())
            }
            AnswerClass::Absent | AnswerClass::Unknown => {
                if self.certificate.snapshot_id == 0 && self.certificate.tier_a_pct == 0.0 {
                    return Err("bare negative without coverage_certificate".into());
                }
                if self.class == AnswerClass::Unknown
                    && self.staleness_reason.is_none()
                    && self.summary.eq_ignore_ascii_case("not found")
                {
                    return Err("bare not found prose".into());
                }
                Ok(())
            }
        }
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
