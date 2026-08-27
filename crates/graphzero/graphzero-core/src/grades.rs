//! Evidence-grade upgrade/revocation ledger (RACC row ZS-GRAPH-003).
//!
//! Grades are fixed at construction ([`GradeLedger::declare_grade`] is
//! one-shot per artifact; [`crate::truth::TruthClass::may_upgrade_to_exact`]
//! remains always false -- no proximity-driven promotion). A grade may only
//! move upward through an explicit, evidence-carrying
//! [`GradeUpgradeRecord`]; it may only move downward when the evidence that
//! justified an upgrade is invalidated ([`GradeLedger::revoke_evidence`]) or
//! when an upgrade record itself is revoked ([`GradeLedger::revoke_upgrade`]).
//!
//! Revocation is fail-closed and cascades:
//! - every record whose evidence was invalidated is revoked, and the
//!   artifact's grade rests at the pre-upgrade grade (`from`) -- never at the
//!   upgraded grade;
//! - dependents are found two ways: explicit
//!   [`GradeEvidence::PriorUpgrade`] citations, and an implicit sound
//!   over-approximation -- any later same-artifact record whose `from` is the
//!   revoked record's `to` (invalidation may revoke too much, never too
//!   little). Unrelated artifacts and evidence are never touched.
//!
//! All records are append-only: revoked records stay in the ledger with
//! their revocation notes, and [`GradeLedger::history`] returns the full
//! ordered event sequence per artifact.
//!
//! The wire types at the bottom of this module ([`GradeName`],
//! [`HubGradeName`], [`hub_equivalent`], [`from`]) implement the
//! composition-boundary mapping documented in ZeroStack
//! `racc/v6/research/GRADE_NAME_MAPPING.md`. The mapping is enforced only
//! here, at the boundary -- never inside either authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::graph::CoverageClass;

/// GraphZero-side grade vocabulary at the composition boundary.
///
/// Matches the GraphZero column of `GRADE_NAME_MAPPING.md` (Complete,
/// SoundOverapproximation, ObservedOnly, Unknown). `Unknown` is
/// terminal-epistemic: nothing upgrades it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeName {
    /// Full proof of the property (maps to V6 `Proved`).
    Complete,
    /// Complete within a declared bound (maps to V6 `BoundedComplete` for
    /// positive claims only -- never certifies absence).
    SoundOverapproximation,
    /// Observed on the current state, no bound (maps to V6 `Observed`).
    ObservedOnly,
    /// No usable evidence; terminal-epistemic (maps to V6 `Unknown`).
    Unknown,
}

impl GradeName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::SoundOverapproximation => "sound_overapproximation",
            Self::ObservedOnly => "observed_only",
            Self::Unknown => "unknown",
        }
    }

    /// Whether an upgrade from `self` to `to` is a lattice-valid,
    /// evidence-eligible edge.
    ///
    /// `Unknown` is terminal (never upgrades), `Complete` is top, and grades
    /// never move downward through this API. Upgrading always still requires
    /// evidence -- the lattice check alone never authorizes.
    #[must_use]
    pub const fn may_upgrade_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::ObservedOnly, Self::SoundOverapproximation)
                | (Self::ObservedOnly, Self::Complete)
                | (Self::SoundOverapproximation, Self::Complete)
        )
    }
}

/// Lossy boundary mapping of the graph's declared coverage class onto the
/// wire grade vocabulary. `Partial` coverage is weaker than `ObservedOnly`;
/// mapping it there is lossy but never upgrades.
impl From<CoverageClass> for GradeName {
    fn from(coverage: CoverageClass) -> Self {
        match coverage {
            CoverageClass::Complete => Self::Complete,
            CoverageClass::SoundOverapproximation => Self::SoundOverapproximation,
            CoverageClass::ObservedOnly | CoverageClass::Partial => Self::ObservedOnly,
            CoverageClass::Unknown => Self::Unknown,
        }
    }
}

/// Evidence that justifies a grade upgrade. Every variant carries a
/// reference (`digest`, `run_ref`, or prior record id) that revocation can
/// target; a record with an empty reference is rejected.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GradeEvidence {
    /// A verification receipt digest (e.g. a formal-verifier output hash).
    VerificationReceipt { digest: String },
    /// A test-run reference (e.g. `run://ci/1234`).
    TestRun { run_ref: String },
    /// A bounded analysis certificate.
    BoundedAnalysis { bound: String, digest: String },
    /// Explicit dependency on a prior upgrade record: revoking that record
    /// cascades to this one.
    PriorUpgrade { record: u64 },
}

impl GradeEvidence {
    /// Verification receipt evidence; rejects an empty digest.
    pub fn verification_receipt(digest: &str) -> Result<Self, GradeError> {
        let evidence = Self::VerificationReceipt {
            digest: digest.to_owned(),
        };
        evidence.require_non_empty()?;
        Ok(evidence)
    }

    /// Test-run evidence; rejects an empty run ref.
    pub fn test_run(run_ref: &str) -> Result<Self, GradeError> {
        let evidence = Self::TestRun {
            run_ref: run_ref.to_owned(),
        };
        evidence.require_non_empty()?;
        Ok(evidence)
    }

    /// Bounded-analysis evidence; rejects empty bound or digest.
    pub fn bounded_analysis(bound: &str, digest: &str) -> Result<Self, GradeError> {
        let evidence = Self::BoundedAnalysis {
            bound: bound.to_owned(),
            digest: digest.to_owned(),
        };
        evidence.require_non_empty()?;
        Ok(evidence)
    }

    /// Explicit dependency on a prior upgrade record (always well-formed).
    #[must_use]
    pub const fn prior_upgrade(record: u64) -> Self {
        Self::PriorUpgrade { record }
    }

    /// Stable identifier used by revocation to find every record that relied
    /// on this evidence.
    #[must_use]
    pub fn evidence_id(&self) -> String {
        match self {
            Self::VerificationReceipt { digest } => format!("receipt:{digest}"),
            Self::TestRun { run_ref } => format!("test_run:{run_ref}"),
            Self::BoundedAnalysis { digest, .. } => format!("analysis:{digest}"),
            Self::PriorUpgrade { record } => format!("record:{record}"),
        }
    }

    /// Whether this evidence cites a prior upgrade record (explicit
    /// dependency used by the revocation cascade).
    #[must_use]
    pub const fn referenced_record(&self) -> Option<u64> {
        match self {
            Self::PriorUpgrade { record } => Some(*record),
            _ => None,
        }
    }

    fn require_non_empty(&self) -> Result<(), GradeError> {
        let empty = match self {
            Self::VerificationReceipt { digest } => digest.is_empty(),
            Self::TestRun { run_ref } => run_ref.is_empty(),
            Self::BoundedAnalysis { bound, digest } => bound.is_empty() || digest.is_empty(),
            Self::PriorUpgrade { .. } => false,
        };
        if empty {
            return Err(GradeError::EvidenceRequired);
        }
        Ok(())
    }

    /// Evidence-free upgrades are rejected even when the enum is constructed
    /// literally with an empty reference.
    #[must_use]
    pub(crate) fn has_empty_reference(&self) -> bool {
        match self {
            Self::VerificationReceipt { digest } => digest.is_empty(),
            Self::TestRun { run_ref } => run_ref.is_empty(),
            Self::BoundedAnalysis { bound, digest } => bound.is_empty() || digest.is_empty(),
            Self::PriorUpgrade { .. } => false,
        }
    }
}

/// One append-only upgrade record: what moved, what justified it, who did it,
/// and the ledger-ordinal (`when`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeUpgradeRecord {
    pub artifact: String,
    /// Grade before the upgrade (always the artifact's effective grade).
    pub from: GradeName,
    pub to: GradeName,
    pub evidence: GradeEvidence,
    pub actor: String,
    /// Monotonic ledger ordinal; strictly increasing across all events.
    pub ordinal: u64,
}

/// One append-only revocation note attached to an upgrade record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeRevocation {
    /// Ledger ordinal of the revocation event itself.
    pub ordinal: u64,
    /// The upgrade record being revoked.
    pub record: u64,
    pub reason: String,
    pub actor: String,
    /// The grade the artifact rests at after this revocation round
    /// (fail-closed: never the upgraded grade).
    pub restored_to: GradeName,
}

/// One queryable ledger entry for an artifact: the upgrade plus its current
/// revocation status and any revocation notes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeLedgerEntry {
    pub ordinal: u64,
    pub artifact: String,
    pub upgrade: GradeUpgradeRecord,
    pub revoked: bool,
    pub revocations: Vec<GradeRevocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GradeError {
    /// No baseline grade declared for the artifact (must be declared at
    /// construction).
    UnknownArtifact(String),
    /// Baseline grade already declared -- grades are fixed at construction.
    AlreadyDeclared(String),
    /// Upgrade carried no usable evidence reference.
    EvidenceRequired,
    /// The upgrade edge is not on the evidence lattice (downgrade, top, or
    /// unknown-terminal).
    UpgradeNotAllowed { from: GradeName, to: GradeName },
    /// A `PriorUpgrade` evidence cited a record that does not exist.
    NoSuchRecord(u64),
    /// A `PriorUpgrade` evidence cited an already-revoked record.
    AlreadyRevoked(u64),
}

/// Append-only evidence-grade ledger for artifacts/claims.
///
/// Baseline grades are fixed at construction; upgrades and revocations are
/// appended as ordered records and never deleted. History per artifact is
/// queryable in ordinal order.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeLedger {
    baseline: BTreeMap<String, GradeName>,
    records: BTreeMap<u64, GradeUpgradeRecord>,
    revoked: BTreeSet<u64>,
    revocations: BTreeMap<u64, GradeRevocation>,
    next_ordinal: u64,
}

impl GradeLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fix an artifact's baseline grade at construction time. One-shot per
    /// artifact: grades never change in place.
    pub fn declare_grade(&mut self, artifact: &str, grade: GradeName) -> Result<(), GradeError> {
        if self.baseline.contains_key(artifact) {
            return Err(GradeError::AlreadyDeclared(artifact.to_owned()));
        }
        self.baseline.insert(artifact.to_owned(), grade);
        Ok(())
    }

    /// The artifact's current grade: the `to` of its newest non-revoked
    /// upgrade record, else the baseline. `None` when undeclared.
    #[must_use]
    pub fn effective_grade(&self, artifact: &str) -> Option<GradeName> {
        let baseline = self.baseline.get(artifact).copied()?;
        let mut effective = baseline;
        // BTreeMap iteration is key (= ordinal) order, so later records win.
        for record in self.records.values() {
            if record.artifact == artifact && !self.revoked.contains(&record.ordinal) {
                effective = record.to;
            }
        }
        Some(effective)
    }

    /// Upgrade an artifact's grade with evidence. The `from` grade is always
    /// the artifact's current effective grade -- a caller can never claim a
    /// false starting point. Rejected when evidence is empty, the lattice
    /// edge is invalid, or a cited prior record is missing/revoked.
    pub fn upgrade(
        &mut self,
        artifact: &str,
        to: GradeName,
        evidence: GradeEvidence,
        actor: &str,
    ) -> Result<u64, GradeError> {
        let from = self
            .effective_grade(artifact)
            .ok_or_else(|| GradeError::UnknownArtifact(artifact.to_owned()))?;
        if evidence.has_empty_reference() {
            return Err(GradeError::EvidenceRequired);
        }
        if let Some(record) = evidence.referenced_record() {
            if !self.records.contains_key(&record) {
                return Err(GradeError::NoSuchRecord(record));
            }
            if self.revoked.contains(&record) {
                return Err(GradeError::AlreadyRevoked(record));
            }
        }
        if !from.may_upgrade_to(to) {
            return Err(GradeError::UpgradeNotAllowed { from, to });
        }
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;
        self.records.insert(
            ordinal,
            GradeUpgradeRecord {
                artifact: artifact.to_owned(),
                from,
                to,
                evidence,
                actor: actor.to_owned(),
                ordinal,
            },
        );
        Ok(ordinal)
    }

    /// Revoke one upgrade record and everything whose grade depended on it.
    ///
    /// The cascade covers explicit [`GradeEvidence::PriorUpgrade`]
    /// dependents (any artifact) and, fail-closed, any later same-artifact
    /// record whose `from` was the revoked record's `to` (sound
    /// over-approximation: revoke too much, never too little). Returns the
    /// number of records revoked (the target plus dependents).
    pub fn revoke_upgrade(
        &mut self,
        record: u64,
        reason: &str,
        actor: &str,
    ) -> Result<u64, GradeError> {
        if !self.records.contains_key(&record) {
            return Err(GradeError::NoSuchRecord(record));
        }
        let mut cascade = BTreeSet::new();
        let mut stack = vec![record];
        while let Some(current) = stack.pop() {
            if !cascade.insert(current) {
                continue;
            }
            let current_record = &self.records[&current];
            for (ordinal, candidate) in &self.records {
                if cascade.contains(ordinal) {
                    continue;
                }
                let explicit_dependent = candidate
                    .evidence
                    .referenced_record()
                    .is_some_and(|r| r == current);
                let implicit_dependent = candidate.artifact == current_record.artifact
                    && candidate.from == current_record.to;
                if (explicit_dependent || implicit_dependent) && *ordinal > current {
                    stack.push(*ordinal);
                }
            }
        }
        let to_revoke: Vec<u64> = cascade
            .into_iter()
            .filter(|ordinal| !self.revoked.contains(ordinal))
            .collect();
        if to_revoke.is_empty() {
            return Err(GradeError::AlreadyRevoked(record));
        }
        for ordinal in &to_revoke {
            self.revoked.insert(*ordinal);
        }
        // Fail-closed restore: the artifact rests at its pre-upgrade grade
        // (recomputed after the whole cascade), never at the upgraded grade.
        for ordinal in &to_revoke {
            let record_entry = &self.records[ordinal];
            let restored_to = self
                .effective_grade(&record_entry.artifact)
                .unwrap_or(record_entry.from);
            let revocation = GradeRevocation {
                ordinal: self.next_ordinal,
                record: *ordinal,
                reason: reason.to_owned(),
                actor: actor.to_owned(),
                restored_to,
            };
            self.next_ordinal += 1;
            self.revocations.insert(revocation.ordinal, revocation);
        }
        Ok(to_revoke.len() as u64)
    }

    /// Revoke every non-revoked record whose evidence carries
    /// `evidence_id` (e.g. the digest of a verification receipt whose cert
    /// was invalidated by the invalidation machinery), cascading to
    /// dependents. Returns the total number of records revoked.
    pub fn revoke_evidence(&mut self, evidence_id: &str, reason: &str, actor: &str) -> u64 {
        let targets: Vec<u64> = self
            .records
            .iter()
            .filter(|(ordinal, record)| {
                !self.revoked.contains(ordinal) && record.evidence.evidence_id() == evidence_id
            })
            .map(|(ordinal, _)| *ordinal)
            .collect();
        let mut total = 0;
        for target in targets {
            if let Ok(count) = self.revoke_upgrade(target, reason, actor) {
                total += count;
            }
        }
        total
    }

    /// Whether an upgrade record is currently revoked (records are never
    /// removed, only flagged).
    #[must_use]
    pub fn is_revoked(&self, record: u64) -> bool {
        self.revoked.contains(&record)
    }

    /// Full append-only history of one artifact in ordinal order, including
    /// revoked records and their revocation notes.
    #[must_use]
    pub fn history(&self, artifact: &str) -> Vec<GradeLedgerEntry> {
        let mut entries: Vec<GradeLedgerEntry> = self
            .records
            .values()
            .filter(|record| record.artifact == artifact)
            .map(|record| GradeLedgerEntry {
                ordinal: record.ordinal,
                artifact: artifact.to_owned(),
                upgrade: record.clone(),
                revoked: self.revoked.contains(&record.ordinal),
                revocations: self
                    .revocations
                    .values()
                    .filter(|note| note.record == record.ordinal)
                    .cloned()
                    .collect(),
            })
            .collect();
        entries.sort_by_key(|entry| entry.ordinal);
        entries
    }
}

// ---------------------------------------------------------------------------
// Cross-repo grade conformance wire (GRADE_NAME_MAPPING.md)
// ---------------------------------------------------------------------------

/// Hub-side V6 grade vocabulary (`CoverageGrade` in zero-abi identity.rs):
/// `proved`, `bounded_complete`, `observed`, `unknown`. Defined here as the
/// wire mirror so the shared conformance fixture can be validated from the
/// GraphZero side.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubGradeName {
    Proved,
    BoundedComplete,
    Observed,
    Unknown,
}

impl HubGradeName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proved => "proved",
            Self::BoundedComplete => "bounded_complete",
            Self::Observed => "observed",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether the claim being graded is a positive presence claim or an
/// absence claim. Absence certification is where the two vocabularies
/// diverge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Positive,
    Absence,
}

/// One row of the cross-repo grade conformance vector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeConformanceVector {
    pub id: String,
    /// GraphZero vocabulary.
    pub grade: GradeName,
    /// Hub (V6) vocabulary.
    pub hub_grade: HubGradeName,
    pub claim_kind: ClaimKind,
    /// Whether the decision record declares the two grades equivalent for
    /// this claim kind.
    pub equivalent: bool,
}

/// The shared conformance document (`grades-cross-repo/v1`): the same JSON
/// shape the hub fixture in ZeroStack `conformance/models/` will carry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GradeConformanceFixture {
    pub schema: String,
    pub vectors: Vec<GradeConformanceVector>,
}

pub const GRADES_CROSS_REPO_SCHEMA: &str = "grades-cross-repo/v1";

/// GraphZero -> V6 mapping at the composition boundary (positive claims).
///
/// Lossless rows: `Complete -> Proved`, `ObservedOnly -> Observed`,
/// `Unknown -> Unknown`. The documented divergence:
/// `SoundOverapproximation` maps to `BoundedComplete` for POSITIVE claims
/// only -- a sound over-approximation may over-report, so it never certifies
/// absence (`None`).
#[must_use]
pub fn hub_equivalent(grade: GradeName, claim_kind: ClaimKind) -> Option<HubGradeName> {
    match (grade, claim_kind) {
        (GradeName::Complete, _) => Some(HubGradeName::Proved),
        (GradeName::SoundOverapproximation, ClaimKind::Positive) => {
            Some(HubGradeName::BoundedComplete)
        }
        (GradeName::SoundOverapproximation, ClaimKind::Absence) => None,
        (GradeName::ObservedOnly, _) => Some(HubGradeName::Observed),
        (GradeName::Unknown, _) => Some(HubGradeName::Unknown),
    }
}

/// V6 -> GraphZero mapping at the composition boundary. Fail-closed in both
/// divergence directions: V6 `Observed` never promotes (only
/// `ObservedOnly`), V6 `Unknown` is terminal, and a `BoundedComplete`
/// absence certification is never fed back as `SoundOverapproximation`-
/// certified (`None`).
#[must_use]
pub fn grade_from_hub(grade: HubGradeName, claim_kind: ClaimKind) -> Option<GradeName> {
    match (grade, claim_kind) {
        (HubGradeName::Proved, _) => Some(GradeName::Complete),
        (HubGradeName::BoundedComplete, ClaimKind::Positive) => {
            Some(GradeName::SoundOverapproximation)
        }
        (HubGradeName::BoundedComplete, ClaimKind::Absence) => None,
        (HubGradeName::Observed, _) => Some(GradeName::ObservedOnly),
        (HubGradeName::Unknown, _) => Some(GradeName::Unknown),
    }
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-core/grades_tests.rs"]
mod tests;
