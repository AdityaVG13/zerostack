//! Rooted structural inspection contract for one capsule or snapshot.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::verdict::SafetyVerdict;
use crate::zero_kernel::{AsgrepOptions, StructuralCoverage, StructuralHit, ZeroHandle};

/// Contract version for the internal task-lens ABI. Evolution is bound by
/// this version and by serde's `deny_unknown_fields`, never by a numeric
/// suffix in a symbol.
pub const TASK_LENS_CONTRACT_VERSION: u16 = 1;

/// Rooted lens request: one query over one optional capsule/snapshot pair. `capsule_root` and
/// `required_snapshot` name the roots the caller demands the evidence to cover.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskLensRequest {
    pub query: String,
    pub options: AsgrepOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule_root: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_snapshot: Option<ZeroHandle>,
}

impl TaskLensRequest {
    /// Fail-closed request hygiene: the lens query must not be blank, and every requested root must be
    /// a canonical `z://blob/` handle.
    pub fn validate(&self) -> Result<(), TaskLensError> {
        if self.query.trim().is_empty() {
            return Err(TaskLensError::EmptyQuery);
        }
        for requested in [self.capsule_root.as_ref(), self.required_snapshot.as_ref()] {
            if let Some(root) = requested {
                if ZeroHandle::parse(root.as_str()).is_err() {
                    return Err(TaskLensError::InvalidRequestedRoot(root.clone()));
                }
            }
        }
        Ok(())
    }
}

/// Compiler reverse-impact closure for the lens locus. `complete` states that the reverse impact
/// analysis was exhaustive, not truncated by budget or index gaps. `edge_roots` are the forward
/// edge roots and `reverse_roots` the reverse edge roots of the impact closure, each a content handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskLensCompilerImpact {
    pub complete: bool,
    #[serde(default)]
    pub edge_roots: Vec<ZeroHandle>,
    #[serde(default)]
    pub reverse_roots: Vec<ZeroHandle>,
}

/// One trivalent verdict for one locus. `reasons` must be canonical and equal the verdict reasons.
/// `locus` is the rooted hit when present; proof and evidence refs bind supporting artifacts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaskLensResult {
    pub verdict: SafetyVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locus: Option<StructuralHit>,
    pub impact: TaskLensCompilerImpact,
    #[serde(default)]
    pub proof_support: Vec<ZeroHandle>,
    #[serde(default)]
    pub evidence_roots: Vec<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<StructuralCoverage>,
    pub index_digest: String,
    #[serde(default)]
    pub reasons: Vec<String>,
}

impl TaskLensResult {
    /// Enforce the -lens laws against one request. `Safe` must satisfy every Safe law (1–6) and carry
    /// no reasons. `Unsafe` must be explicit: it carries at least one reason. `Unknown` may carry
    /// reasons but need not.
    pub fn validate(&self, request: &TaskLensRequest) -> Result<(), TaskLensError> {
        if !is_sorted_deduped(&self.reasons) {
            return Err(TaskLensError::UnnormalizedReasons);
        }
        match &self.verdict {
            SafetyVerdict::Safe => self.validate_safe(request),
            SafetyVerdict::Unsafe { reasons } => {
                if reasons.is_empty() {
                    return Err(TaskLensError::UnsafeWithoutReasons);
                }
                if self.reasons.is_empty() || self.reasons != *reasons {
                    return Err(TaskLensError::ReasonMismatch);
                }
                Ok(())
            }
            SafetyVerdict::Unknown { reasons } => {
                if self.reasons != *reasons {
                    return Err(TaskLensError::ReasonMismatch);
                }
                Ok(())
            }
        }
    }

    /// Return a copy with `reasons` (and the verdict's reasons) sorted and
    /// deduplicated. This restores reason hygiene only; it never repairs a missing
    /// reason or a mismatch, so the normalized result may still fail [`TaskLensResult::validate`].
    pub fn normalize(mut self) -> Self {
        self.reasons = sort_dedup(self.reasons);
        self.verdict = match self.verdict {
            SafetyVerdict::Unsafe { reasons } => SafetyVerdict::Unsafe {
                reasons: sort_dedup(reasons),
            },
            SafetyVerdict::Unknown { reasons } => SafetyVerdict::Unknown {
                reasons: sort_dedup(reasons),
            },
            verdict => verdict,
        };
        self
    }

    fn validate_safe(&self, request: &TaskLensRequest) -> Result<(), TaskLensError> {
        // Law 1: exactly one rooted locus.
        let locus = self.locus.as_ref().ok_or(TaskLensError::MissingLocus)?;
        if locus.evidence.is_none() && locus.source.is_none() {
            return Err(TaskLensError::UnrootedLocus);
        }
        for root in [locus.evidence.as_ref(), locus.source.as_ref()]
            .into_iter()
            .flatten()
        {
            if !is_canonical_handle(root) {
                return Err(TaskLensError::MalformedLocusRoot(root.clone()));
            }
        }
        // Law 2: complete compiler reverse impact, rooted on both sides.
        // A boolean `complete` without rooted compiler evidence is not proof.
        if !self.impact.complete
            || self.impact.edge_roots.is_empty()
            || self.impact.reverse_roots.is_empty()
        {
            return Err(TaskLensError::IncompleteImpact);
        }
        for root in self
            .impact
            .edge_roots
            .iter()
            .chain(self.impact.reverse_roots.iter())
        {
            if !is_canonical_handle(root) {
                return Err(TaskLensError::MalformedImpactRoot(root.clone()));
            }
        }
        // Law 3: non-empty fresh proof support.
        if self.proof_support.is_empty() {
            return Err(TaskLensError::MissingProofSupport);
        }
        for root in &self.proof_support {
            if !is_canonical_handle(root) {
                return Err(TaskLensError::MalformedProofRoot(root.clone()));
            }
        }
        let coverage = self
            .coverage
            .as_ref()
            .ok_or(TaskLensError::MissingCoverage)?;
        if !coverage.freshness_verified {
            return Err(TaskLensError::StaleCoverage);
        }
        // Law 4: complete coverage and freshness. Tiers B and C are
        // independent coverages; completeness is tier A >= 99%.
        if coverage.tier_a_pct < 99.0 {
            return Err(TaskLensError::IncompleteCoverage);
        }
        // Evidence hygiene: index_digest is a live content digest.
        if !is_live_digest(&self.index_digest) {
            return Err(TaskLensError::MalformedIndexDigest);
        }
        // Law 5: matching requested snapshot/capsule roots.
        for root in &self.evidence_roots {
            if !is_canonical_handle(root) {
                return Err(TaskLensError::MalformedEvidenceRoot(root.clone()));
            }
        }
        for requested in [
            request.capsule_root.as_ref(),
            request.required_snapshot.as_ref(),
        ] {
            if let Some(root) = requested {
                if !self.evidence_roots.contains(root) {
                    return Err(TaskLensError::MissingEvidenceRoot(root.clone()));
                }
            }
        }
        // Law 6: no semantic choice gap. Any explicit semantic choice or
        // conflict is a reason; a Safe verdict must carry none.
        if !self.reasons.is_empty() {
            return Err(TaskLensError::SafeWithReasons);
        }
        Ok(())
    }
}

/// Fail-closed error for task-lens contract violations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskLensError {
    /// The lens query is blank.
    EmptyQuery,
    /// A requested `capsule_root`/`required_snapshot` is not a canonical
    /// `z://blob/` handle.
    InvalidRequestedRoot(ZeroHandle),
    /// `reasons` is not sorted and deduplicated.
    UnnormalizedReasons,
    /// Safe law 1: the result carries no rooted locus.
    MissingLocus,
    /// Safe law 1: the locus is not anchored to a content handle.
    UnrootedLocus,
    /// Safe law 2: the compiler reverse impact is incomplete or not rooted
    /// on both the edge and reverse sides.
    IncompleteImpact,
    /// Safe law 3: there is no proof support.
    MissingProofSupport,
    /// Safe laws 3/4: there is no coverage snapshot.
    MissingCoverage,
    /// Safe laws 3/4: the coverage snapshot is not freshness-verified.
    StaleCoverage,
    /// Safe law 4: tier A coverage is below the 99% complete law.
    IncompleteCoverage,
    /// Safe law 5: a requested snapshot/capsule root is missing from the
    /// evidence roots.
    MissingEvidenceRoot(ZeroHandle),
    /// Safe law 1: a rooted locus handle is malformed.
    MalformedLocusRoot(ZeroHandle),
    /// Safe law 2: a rooted impact handle is malformed.
    MalformedImpactRoot(ZeroHandle),
    /// Safe law 3: a proof support handle is malformed.
    MalformedProofRoot(ZeroHandle),
    /// Safe law 5: a rooted evidence handle is malformed.
    MalformedEvidenceRoot(ZeroHandle),
    /// Safe evidence hygiene: `index_digest` is not exactly 64 lowercase
    /// hexadecimal characters (the canonical handle digest domain).
    MalformedIndexDigest,
    /// Safe law 6: the verdict carries reasons, i.e. an explicit semantic
    /// choice or conflict was laundered into `Safe`.
    SafeWithReasons,
    /// Explicit `Unsafe` must carry at least one reason.
    UnsafeWithoutReasons,
    /// The result `reasons` differ from the reasons carried by `verdict`.
    ReasonMismatch,
}

impl fmt::Display for TaskLensError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyQuery => write!(formatter, "lens query must not be blank"),
            Self::InvalidRequestedRoot(root) => {
                write!(
                    formatter,
                    "requested root {root} is not a canonical z://blob handle"
                )
            }
            Self::UnnormalizedReasons => {
                write!(formatter, "reasons must be sorted and deduplicated")
            }
            Self::MissingLocus => {
                write!(formatter, "safe result requires exactly one rooted locus")
            }
            Self::UnrootedLocus => {
                write!(formatter, "safe locus must be anchored to a content handle")
            }
            Self::IncompleteImpact => {
                write!(
                    formatter,
                    "safe result requires complete, rooted compiler reverse impact"
                )
            }
            Self::MissingProofSupport => {
                write!(formatter, "safe result requires non-empty proof support")
            }
            Self::MissingCoverage => {
                write!(formatter, "safe result requires a coverage snapshot")
            }
            Self::StaleCoverage => {
                write!(
                    formatter,
                    "safe result requires freshness-verified coverage"
                )
            }
            Self::IncompleteCoverage => {
                write!(
                    formatter,
                    "safe result requires tier A coverage of at least 99%"
                )
            }
            Self::MissingEvidenceRoot(root) => {
                write!(formatter, "safe evidence must cover requested root {root}")
            }
            Self::MalformedLocusRoot(root) => {
                write!(
                    formatter,
                    "locus root {root} is not a canonical z://blob handle"
                )
            }
            Self::MalformedImpactRoot(root) => {
                write!(
                    formatter,
                    "impact root {root} is not a canonical z://blob handle"
                )
            }
            Self::MalformedProofRoot(root) => {
                write!(
                    formatter,
                    "proof support root {root} is not a canonical z://blob handle"
                )
            }
            Self::MalformedEvidenceRoot(root) => {
                write!(
                    formatter,
                    "evidence root {root} is not a canonical z://blob handle"
                )
            }
            Self::MalformedIndexDigest => {
                write!(
                    formatter,
                    "index_digest must be exactly 64 lowercase hexadecimal characters"
                )
            }
            Self::SafeWithReasons => {
                write!(
                    formatter,
                    "safe result must carry no semantic choice gap reasons"
                )
            }
            Self::UnsafeWithoutReasons => {
                write!(formatter, "explicit unsafe requires at least one reason")
            }
            Self::ReasonMismatch => {
                write!(formatter, "result reasons differ from verdict reasons")
            }
        }
    }
}

impl std::error::Error for TaskLensError {}

fn is_sorted_deduped(reasons: &[String]) -> bool {
    reasons.windows(2).all(|pair| pair[0] < pair[1])
}

fn sort_dedup(mut reasons: Vec<String>) -> Vec<String> {
    reasons.sort();
    reasons.dedup();
    reasons
}
fn is_canonical_handle(handle: &ZeroHandle) -> bool {
    ZeroHandle::parse(handle.as_str()).is_ok()
}

fn is_live_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
