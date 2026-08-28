//! QueryResult — three-answer model with mandatory `CoverageCertificate`.

use crate::certificate::{CoverageCertificate, Gap, GapReason};
use crate::freshness::{LiveBytesProvider, freshness_check};
use crate::index::CoverageIndex;
use graphzero_store::{BlobId, Tier};

/// Evidence reference format: gz://blob/<hash>#B<start>-<end>
pub type EvidenceRef = String;

/// Query answer — one of three mutually exclusive variants.
/// Every variant carries a non-null `CoverageCertificate` (enforced by type).
#[derive(Clone, Debug, PartialEq)]
pub enum QueryResult {
    /// Symbol / edge found; carries evidence and coverage.
    Present {
        evidence_ref: EvidenceRef,
        certificate: CoverageCertificate,
    },
    /// Proven absent at 100 % fresh coverage.
    Absent { certificate: CoverageCertificate },
    /// Unproven — coverage gap or stale blob.
    Unknown { certificate: CoverageCertificate },
}

/// Typed query-result construction error. Carries the computed certificate so
/// callers can report coverage state without fabricating a successful answer.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryBuildError {
    kind: QueryBuildErrorKind,
    certificate: CoverageCertificate,
}

impl QueryBuildError {
    pub fn kind(&self) -> QueryBuildErrorKind {
        self.kind
    }

    pub fn certificate(&self) -> &CoverageCertificate {
        &self.certificate
    }

    fn missing_evidence_ref(certificate: CoverageCertificate) -> Self {
        Self {
            kind: QueryBuildErrorKind::MissingEvidenceRef,
            certificate,
        }
    }
}

/// Stable, non-panicking failure classes for QueryResultBuilder invariants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryBuildErrorKind {
    /// A Present result was requested without a non-empty evidence reference.
    MissingEvidenceRef,
}

/// Builder that generates `QueryResult` from a `CoverageIndex` and query context.
///
/// The builder is the *only* public constructor for `QueryResult`; this lets us
/// enforce the invariant that every answer carries a certificate.
pub struct QueryResultBuilder<'a> {
    index: &'a dyn CoverageIndex,
    tier: Tier,
    found: bool,
    evidence: Option<EvidenceRef>,
}

impl<'a> QueryResultBuilder<'a> {
    pub fn new(index: &'a dyn CoverageIndex, tier: Tier) -> Self {
        Self {
            index,
            tier,
            found: false,
            evidence: None,
        }
    }

    pub fn found(mut self, evidence_ref: EvidenceRef) -> Self {
        self.found = true;
        self.evidence = Some(evidence_ref);
        self
    }

    pub fn not_found(mut self) -> Self {
        self.found = false;
        self.evidence = None;
        self
    }

    /// Build the `QueryResult`, verifying freshness and computing coverage.
    pub fn build<P: LiveBytesProvider>(self, provider: &P) -> QueryResult {
        match self.try_build(provider) {
            Ok(result) => result,
            Err(err) => QueryResult::Unknown {
                certificate: err.certificate,
            },
        }
    }

    /// Build the QueryResult, returning typed errors for invalid builder state.
    pub fn try_build<P: LiveBytesProvider>(
        self,
        provider: &P,
    ) -> Result<QueryResult, QueryBuildError> {
        // One full-repo scan builds the certificate and answers full-tier
        // index coverage (graphzero-wf8za): no second `all_blob_ids` walk.
        let (cert, full_tier_indexed) = build_certificate(self.index, self.tier, provider);

        if self.found {
            let Some(evidence_ref) = self.evidence.filter(|r| !r.is_empty()) else {
                return Err(QueryBuildError::missing_evidence_ref(cert));
            };
            Ok(QueryResult::Present {
                evidence_ref,
                certificate: cert,
            })
        } else if cert.freshness_verified && full_tier_indexed {
            Ok(QueryResult::Absent { certificate: cert })
        } else {
            Ok(QueryResult::Unknown { certificate: cert })
        }
    }
}

fn is_blob_indexed(index: &dyn CoverageIndex, blob_id: &BlobId, tier: Tier) -> bool {
    index
        .read_coverage(blob_id)
        .as_ref()
        .map(|b| b.is_indexed(blob_id, tier))
        .unwrap_or(false)
}

fn blob_is_fresh<P: LiveBytesProvider>(
    index: &dyn CoverageIndex,
    provider: &P,
    blob_id: &BlobId,
) -> bool {
    let stored_hash = index.read_freshness(blob_id);
    let path_hint = index
        .blob_path(blob_id)
        .and_then(|path| path.to_str())
        .unwrap_or("");
    let live = provider.live_bytes(path_hint);
    match (&stored_hash, live) {
        (Some(stored), Ok(live_bytes)) => {
            freshness_check(Some(stored), &live_bytes).unwrap_or(false)
        }
        _ => false,
    }
}

fn assign_tier_coverage_pct(
    cert: &mut CoverageCertificate,
    tier: Tier,
    indexed_count: usize,
    total: usize,
) {
    let pct = if total == 0 {
        0.0
    } else {
        (indexed_count as f64 / total as f64) * 100.0
    };
    match tier {
        Tier::A => cert.tier_a_pct = pct,
        Tier::B => cert.tier_b_pct = pct,
        Tier::C => cert.tier_c_pct = pct,
    }
}

/// Per-blob scan: records gaps and returns whether the blob is indexed and fresh.
fn scan_blob_coverage<P: LiveBytesProvider>(
    index: &dyn CoverageIndex,
    tier: Tier,
    provider: &P,
    blob_id: &BlobId,
    cert: &mut CoverageCertificate,
) -> (bool, bool) {
    if !is_blob_indexed(index, blob_id, tier) {
        cert.gaps
            .push(Gap::new(blob_id.clone(), tier, GapReason::NotIndexed));
        return (false, false);
    }
    if blob_is_fresh(index, provider, blob_id) {
        (true, true)
    } else {
        cert.gaps
            .push(Gap::new(blob_id.clone(), tier, GapReason::Stale));
        (true, false)
    }
}

/// Internal: build a `CoverageCertificate` by scanning the index.
///
/// Returns `(certificate, full_tier_indexed)` where `full_tier_indexed` is true
/// when every tracked blob is indexed at `tier` (non-empty universe). This
/// fuses the former second-pass `is_full_coverage` into the certificate scan.
fn build_certificate<P: LiveBytesProvider>(
    index: &dyn CoverageIndex,
    tier: Tier,
    provider: &P,
) -> (CoverageCertificate, bool) {
    let mut cert = CoverageCertificate::new(crate::now_timestamp());
    let mut total = 0usize;
    let mut fresh_count = 0usize;
    let mut indexed_count = 0usize;

    index.for_each_blob_id(&mut |blob_id| {
        total += 1;
        let (indexed, fresh) = scan_blob_coverage(index, tier, provider, blob_id, &mut cert);
        if indexed {
            indexed_count += 1;
            if fresh {
                fresh_count += 1;
            }
        }
    });

    assign_tier_coverage_pct(&mut cert, tier, indexed_count, total);
    cert.freshness_verified = fresh_count == indexed_count && indexed_count > 0;
    let full_tier_indexed = total > 0 && indexed_count == total;
    (cert, full_tier_indexed)
}
