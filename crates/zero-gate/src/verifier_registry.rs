//! V6-R8: typed verifier registry, obligation checklists, successor-state
//! verification (ZS-VERIFY-001/002/003).
//!
//! A registered verifier is the only authority that may vouch for a
//! (domain, kind) pair. Lookup is deterministic and typed; an unknown pair is
//! a loud refusal -- never a silent skip. Every registered record binds the
//! verifier identity/version to the exact input roots it verified, its
//! result, its evidence digest, its runtime, and per-dimension coverage
//! grades. Successor-state verification recomputes the claimed successor root
//! from transition receipts and refuses any mismatch, attaching the receipts
//! to the fault.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};
use zero_abi::{
    canonical_json, sha256, CoverageGradeV1, DigestV1, ProtectedDimensionV1,
    ProtectedScopeObligationsV1,
};

pub const VERIFIER_REGISTRY_CONTRACT_VERSION_V1: u16 = 1;
pub const VERIFIER_REGISTRY_SCHEMA_VERSION_V1: &str = "zerostack.verifier_registry.v1";
pub const VERIFIER_REGISTRY_MAX_INPUT_ROOTS_V1: usize = 4096;
pub const VERIFIER_REGISTRY_MAX_GRADES_V1: usize = 64;
pub const OBLIGATION_CHECKLIST_MAX_ENTRIES_V1: usize = 64;
pub const OBLIGATION_CHECKLIST_MAX_EVIDENCE_REFS_V1: usize = 4096;
pub const SUCCESSOR_TRANSITION_MAX_RECEIPTS_V1: usize = 4096;

/// Domain of a registered verifier (ZS-VERIFY-001).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierDomainV1 {
    /// ZS-VERIFY-002: current-effect verification of an exact candidate delta.
    CurrentEffect,
    /// ZS-VERIFY-003: successor-state preservation of registered future actions.
    SuccessorState,
}

impl VerifierDomainV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            VerifierDomainV1::CurrentEffect => "current_effect",
            VerifierDomainV1::SuccessorState => "successor_state",
        }
    }
}

/// Outcome of one verification run. `Unknown` is terminal: it never grants
/// authority and nothing promotes it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierResultV1 {
    Pass,
    Fail,
    Unknown,
}

impl VerifierResultV1 {
    pub fn is_pass(self) -> bool {
        self == VerifierResultV1::Pass
    }

    pub fn as_str(self) -> &'static str {
        match self {
            VerifierResultV1::Pass => "pass",
            VerifierResultV1::Fail => "fail",
            VerifierResultV1::Unknown => "unknown",
        }
    }
}

/// One typed registry record: verifier identity/version bound to the exact
/// input roots it verified, its result, evidence digest, runtime, and
/// per-dimension coverage grades (ZS-VERIFY-001).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierRegistryRecordV1 {
    pub record_version: u16,
    pub verifier_id: String,
    pub verifier_version: String,
    pub domain: VerifierDomainV1,
    pub kind: ProtectedDimensionV1,
    /// Exact input roots (candidate delta roots, sandbox roots) verified.
    pub input_roots: Vec<DigestV1>,
    pub result: VerifierResultV1,
    pub evidence_digest: DigestV1,
    pub runtime_ms: u64,
    pub grades: BTreeMap<ProtectedDimensionV1, CoverageGradeV1>,
}

impl VerifierRegistryRecordV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        verifier_id: impl Into<String>,
        verifier_version: impl Into<String>,
        domain: VerifierDomainV1,
        kind: ProtectedDimensionV1,
        input_roots: Vec<DigestV1>,
        result: VerifierResultV1,
        evidence_digest: DigestV1,
        runtime_ms: u64,
        grades: BTreeMap<ProtectedDimensionV1, CoverageGradeV1>,
    ) -> Result<Self, VerifierRegistryErrorV1> {
        let record = Self {
            record_version: VERIFIER_REGISTRY_CONTRACT_VERSION_V1,
            verifier_id: verifier_id.into(),
            verifier_version: verifier_version.into(),
            domain,
            kind,
            input_roots,
            result,
            evidence_digest,
            runtime_ms,
            grades,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), VerifierRegistryErrorV1> {
        if self.record_version != VERIFIER_REGISTRY_CONTRACT_VERSION_V1 {
            return Err(VerifierRegistryErrorV1::InvalidRecord(format!(
                "unsupported record version {}",
                self.record_version
            )));
        }
        if self.verifier_id.trim().is_empty() || self.verifier_version.trim().is_empty() {
            return Err(VerifierRegistryErrorV1::InvalidRecord(
                "verifier id and version must be nonblank".into(),
            ));
        }
        if self.input_roots.is_empty() || self.input_roots.len() > VERIFIER_REGISTRY_MAX_INPUT_ROOTS_V1 {
            return Err(VerifierRegistryErrorV1::InvalidRecord(format!(
                "input roots must be nonempty and at most {}",
                VERIFIER_REGISTRY_MAX_INPUT_ROOTS_V1
            )));
        }
        if self.input_roots.iter().any(|root| *root == DigestV1::ZERO) {
            return Err(VerifierRegistryErrorV1::InvalidRecord(
                "input roots must be nonzero".into(),
            ));
        }
        if self.evidence_digest == DigestV1::ZERO {
            return Err(VerifierRegistryErrorV1::InvalidRecord(
                "evidence digest must be nonzero".into(),
            ));
        }
        if self.runtime_ms == 0 {
            return Err(VerifierRegistryErrorV1::InvalidRecord(
                "runtime_ms must be nonzero (a run that never ran is not a run)".into(),
            ));
        }
        if self.grades.is_empty() || self.grades.len() > VERIFIER_REGISTRY_MAX_GRADES_V1 {
            return Err(VerifierRegistryErrorV1::InvalidRecord(format!(
                "grades must be nonempty and at most {} entries",
                VERIFIER_REGISTRY_MAX_GRADES_V1
            )));
        }
        if self.result.is_pass() {
            match self.grades.get(&self.kind) {
                Some(grade) if !grade.is_unknown() => {}
                _ => {
                    return Err(VerifierRegistryErrorV1::InvalidRecord(
                        "a passing record must grade its own kind as covered (never Unknown)".into(),
                    ))
                }
            }
        }
        Ok(())
    }
}

/// Deterministic lookup key: one registered verifier per (domain, kind).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierRegistryKeyV1 {
    pub domain: VerifierDomainV1,
    pub kind: ProtectedDimensionV1,
}

/// Typed, deterministic verifier registry. Lookup by (domain, kind) is
/// exact; an unknown pair is a loud `UnknownVerifier` refusal.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierRegistryV1 {
    /// Currently trusted version per verifier id (freshness authority).
    pub trusted_versions: BTreeMap<String, String>,
    /// Registered records, one per (domain, kind).
    pub records: BTreeMap<VerifierRegistryKeyV1, VerifierRegistryRecordV1>,
}

impl VerifierRegistryV1 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the currently trusted version of a verifier identity.
    pub fn set_trusted_version(
        &mut self,
        verifier_id: impl Into<String>,
        version: impl Into<String>,
    ) {
        self.trusted_versions.insert(verifier_id.into(), version.into());
    }

    /// Typed registration. Refuses verifiers that are not trusted at all and
    /// duplicate (domain, kind) keys -- loud, never a silent overwrite.
    pub fn register(
        &mut self,
        record: VerifierRegistryRecordV1,
    ) -> Result<(), VerifierRegistryErrorV1> {
        record.validate()?;
        if !self.trusted_versions.contains_key(&record.verifier_id) {
            return Err(VerifierRegistryErrorV1::MissingTrustedVerifier {
                verifier_id: record.verifier_id.clone(),
            });
        }
        let key = VerifierRegistryKeyV1 {
            domain: record.domain,
            kind: record.kind,
        };
        if self.records.contains_key(&key) {
            return Err(VerifierRegistryErrorV1::DuplicateRegistration {
                domain: key.domain,
                kind: key.kind,
            });
        }
        self.records.insert(key, record);
        Ok(())
    }

    /// Deterministic lookup. Unknown (domain, kind) is a loud refusal --
    /// never a silent skip.
    pub fn lookup(
        &self,
        domain: VerifierDomainV1,
        kind: ProtectedDimensionV1,
    ) -> Result<&VerifierRegistryRecordV1, VerifierRegistryErrorV1> {
        self.records
            .get(&VerifierRegistryKeyV1 { domain, kind })
            .ok_or(VerifierRegistryErrorV1::UnknownVerifier { domain, kind })
    }

    /// Version freshness of a record against the currently trusted version
    /// of its verifier identity.
    pub fn freshness(
        &self,
        record: &VerifierRegistryRecordV1,
    ) -> Result<(), VerifierRegistryErrorV1> {
        let expected = self
            .trusted_versions
            .get(&record.verifier_id)
            .ok_or(VerifierRegistryErrorV1::MissingTrustedVerifier {
                verifier_id: record.verifier_id.clone(),
            })?;
        if expected != &record.verifier_version {
            return Err(VerifierRegistryErrorV1::StaleVerifier {
                verifier_id: record.verifier_id.clone(),
                expected: expected.clone(),
                observed: record.verifier_version.clone(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Obligation checklist (ZS-VERIFY-002).
// ---------------------------------------------------------------------------

/// One dimension's obligation with the evidence refs substantiating it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObligationChecklistEntryV1 {
    pub dimension: ProtectedDimensionV1,
    pub required: bool,
    pub evidence_refs: Vec<DigestV1>,
}

/// Obligation checklist mapping dimensions (Security, Tests, ...) to evidence
/// refs, bound to the exact subject root (candidate delta / successor state)
/// it was verified against. Substituting a different delta after verification
/// is a loud `DeltaSubstitutedAfterVerification` refusal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObligationChecklistV1 {
    pub checklist_version: u16,
    pub subject_root: DigestV1,
    pub entries: Vec<ObligationChecklistEntryV1>,
}

impl ObligationChecklistV1 {
    pub fn new(
        subject_root: DigestV1,
        entries: Vec<ObligationChecklistEntryV1>,
    ) -> Result<Self, VerifierRegistryErrorV1> {
        let checklist = Self {
            checklist_version: VERIFIER_REGISTRY_CONTRACT_VERSION_V1,
            subject_root,
            entries,
        };
        checklist.validate()?;
        Ok(checklist)
    }

    pub fn validate(&self) -> Result<(), VerifierRegistryErrorV1> {
        if self.checklist_version != VERIFIER_REGISTRY_CONTRACT_VERSION_V1 {
            return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                "unsupported checklist version {}",
                self.checklist_version
            )));
        }
        if self.subject_root == DigestV1::ZERO {
            return Err(VerifierRegistryErrorV1::InvalidChecklist(
                "subject root must be nonzero".into(),
            ));
        }
        if self.entries.is_empty() || self.entries.len() > OBLIGATION_CHECKLIST_MAX_ENTRIES_V1 {
            return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                "entries must be nonempty and at most {}",
                OBLIGATION_CHECKLIST_MAX_ENTRIES_V1
            )));
        }
        let mut seen = BTreeMap::new();
        for entry in &self.entries {
            if seen.insert(entry.dimension, ()).is_some() {
                return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                    "duplicate dimension {}",
                    entry.dimension.as_str()
                )));
            }
            if entry.required && entry.evidence_refs.is_empty() {
                return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                    "required dimension {} has no evidence refs",
                    entry.dimension.as_str()
                )));
            }
            if entry.evidence_refs.len() > OBLIGATION_CHECKLIST_MAX_EVIDENCE_REFS_V1 {
                return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                    "dimension {} exceeds {} evidence refs",
                    entry.dimension.as_str(),
                    OBLIGATION_CHECKLIST_MAX_EVIDENCE_REFS_V1
                )));
            }
            if entry.evidence_refs.iter().any(|r| *r == DigestV1::ZERO) {
                return Err(VerifierRegistryErrorV1::InvalidChecklist(format!(
                    "dimension {} has a zero evidence ref",
                    entry.dimension.as_str()
                )));
            }
        }
        Ok(())
    }

    pub fn evidence_for(&self, dimension: ProtectedDimensionV1) -> Option<&[DigestV1]> {
        self.entries
            .iter()
            .find(|entry| entry.dimension == dimension)
            .map(|entry| entry.evidence_refs.as_slice())
    }

    /// The subject in hand must be exactly the one this checklist was
    /// verified against. A substituted delta is a loud refusal.
    pub fn assert_delta_unsubstituted(
        &self,
        actual_subject_root: DigestV1,
    ) -> Result<(), VerifierRegistryErrorV1> {
        if actual_subject_root != self.subject_root {
            return Err(VerifierRegistryErrorV1::DeltaSubstitutedAfterVerification {
                expected: self.subject_root,
                observed: actual_subject_root,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Successor-state verification (ZS-VERIFY-003).
// ---------------------------------------------------------------------------

/// A registered future action: a declared contract the successor state must
/// preserve. A locally-passing edit that breaks it is rejected by
/// successor-state verification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFutureActionV1 {
    pub action_id: String,
    /// Declared interface/invariant the future action needs.
    pub contract_digest: DigestV1,
    pub required_dimensions: Vec<ProtectedDimensionV1>,
}

impl RegisteredFutureActionV1 {
    pub fn new(
        action_id: impl Into<String>,
        contract_digest: DigestV1,
        required_dimensions: Vec<ProtectedDimensionV1>,
    ) -> Result<Self, VerifierRegistryErrorV1> {
        let action = Self {
            action_id: action_id.into(),
            contract_digest,
            required_dimensions,
        };
        action.validate()?;
        Ok(action)
    }

    pub fn validate(&self) -> Result<(), VerifierRegistryErrorV1> {
        if self.action_id.trim().is_empty() {
            return Err(VerifierRegistryErrorV1::InvalidFutureAction(
                "action id must be nonblank".into(),
            ));
        }
        if self.contract_digest == DigestV1::ZERO {
            return Err(VerifierRegistryErrorV1::InvalidFutureAction(
                "contract digest must be nonzero".into(),
            ));
        }
        if self.required_dimensions.is_empty() {
            return Err(VerifierRegistryErrorV1::InvalidFutureAction(
                "required dimensions must be nonempty".into(),
            ));
        }
        let mut seen = BTreeMap::new();
        for dimension in &self.required_dimensions {
            if seen.insert(*dimension, ()).is_some() {
                return Err(VerifierRegistryErrorV1::InvalidFutureAction(format!(
                    "duplicate required dimension {}",
                    dimension.as_str()
                )));
            }
        }
        Ok(())
    }
}

/// A state transition carrying a claimed successor root and the receipts
/// substantiating it. Successor-state verification recomputes the successor
/// from the receipts and refuses any mismatch (loud fault with receipts).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorStateTransitionV1 {
    pub transition_version: u16,
    pub predecessor_root: DigestV1,
    pub claimed_successor_root: DigestV1,
    pub receipts: Vec<DigestV1>,
}

impl SuccessorStateTransitionV1 {
    pub fn new(
        predecessor_root: DigestV1,
        claimed_successor_root: DigestV1,
        receipts: Vec<DigestV1>,
    ) -> Result<Self, VerifierRegistryErrorV1> {
        let transition = Self {
            transition_version: VERIFIER_REGISTRY_CONTRACT_VERSION_V1,
            predecessor_root,
            claimed_successor_root,
            receipts,
        };
        transition.validate()?;
        Ok(transition)
    }

    pub fn validate(&self) -> Result<(), VerifierRegistryErrorV1> {
        if self.transition_version != VERIFIER_REGISTRY_CONTRACT_VERSION_V1 {
            return Err(VerifierRegistryErrorV1::InvalidTransition(format!(
                "unsupported transition version {}",
                self.transition_version
            )));
        }
        if self.predecessor_root == DigestV1::ZERO || self.claimed_successor_root == DigestV1::ZERO
        {
            return Err(VerifierRegistryErrorV1::InvalidTransition(
                "predecessor and claimed successor roots must be nonzero".into(),
            ));
        }
        if self.predecessor_root == self.claimed_successor_root {
            return Err(VerifierRegistryErrorV1::InvalidTransition(
                "a transition must change the state root".into(),
            ));
        }
        if self.receipts.is_empty() || self.receipts.len() > SUCCESSOR_TRANSITION_MAX_RECEIPTS_V1 {
            return Err(VerifierRegistryErrorV1::InvalidTransition(format!(
                "receipts must be nonempty and at most {}",
                SUCCESSOR_TRANSITION_MAX_RECEIPTS_V1
            )));
        }
        if self.receipts.iter().any(|receipt| *receipt == DigestV1::ZERO) {
            return Err(VerifierRegistryErrorV1::InvalidTransition(
                "receipts must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

/// The successor state: root, protected-scope obligations, and the obligation
/// checklist whose evidence refs substantiate each dimension.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorStateV1 {
    pub root: DigestV1,
    pub obligations: ProtectedScopeObligationsV1,
    pub checklist: ObligationChecklistV1,
}

impl SuccessorStateV1 {
    pub fn new(
        root: DigestV1,
        obligations: ProtectedScopeObligationsV1,
        checklist: ObligationChecklistV1,
    ) -> Result<Self, VerifierRegistryErrorV1> {
        let successor = Self {
            root,
            obligations,
            checklist,
        };
        successor.validate()?;
        Ok(successor)
    }

    pub fn validate(&self) -> Result<(), VerifierRegistryErrorV1> {
        if self.root == DigestV1::ZERO {
            return Err(VerifierRegistryErrorV1::InvalidSuccessorState(
                "successor root must be nonzero".into(),
            ));
        }
        self.obligations
            .validate()
            .map_err(|error| VerifierRegistryErrorV1::InvalidSuccessorState(error.to_string()))?;
        self.checklist.validate()?;
        if self.checklist.subject_root != self.root {
            return Err(VerifierRegistryErrorV1::InvalidSuccessorState(
                "obligation checklist subject root must equal the successor root".into(),
            ));
        }
        Ok(())
    }
}

/// Sealed receipt of one successor-state verification, minted only on
/// acceptance. Binds verifier identity/version, domain, roots, action, and
/// the evidence digest of the transition receipts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorVerificationReceiptV1 {
    pub receipt_version: u16,
    pub verifier_id: String,
    pub verifier_version: String,
    pub domain: VerifierDomainV1,
    pub predecessor_root: DigestV1,
    pub successor_root: DigestV1,
    pub action_id: String,
    pub evidence_digest: DigestV1,
    pub runtime_ms: u64,
}

impl SuccessorVerificationReceiptV1 {
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_value(self).map(|value| canonical_json(&value))
    }

    pub fn receipt_root(&self) -> Result<DigestV1, serde_json::Error> {
        self.canonical_json()
            .map(|bytes| DigestV1::from_bytes(sha256(bytes.as_bytes())))
    }
}

/// Fail-loud fault vocabulary of the verifier registry. Every variant is
/// Display-able and none is silently skippable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifierRegistryErrorV1 {
    UnknownVerifier {
        domain: VerifierDomainV1,
        kind: ProtectedDimensionV1,
    },
    MissingTrustedVerifier {
        verifier_id: String,
    },
    StaleVerifier {
        verifier_id: String,
        expected: String,
        observed: String,
    },
    DuplicateRegistration {
        domain: VerifierDomainV1,
        kind: ProtectedDimensionV1,
    },
    InvalidRecord(String),
    InvalidChecklist(String),
    InvalidFutureAction(String),
    InvalidTransition(String),
    InvalidSuccessorState(String),
    DeltaSubstitutedAfterVerification {
        expected: DigestV1,
        observed: DigestV1,
    },
    UnverifiedDelta {
        delta_root: DigestV1,
    },
    NonPassingVerifierResult {
        result: VerifierResultV1,
    },
    SuccessorMismatch {
        verifier_id: String,
        claimed: DigestV1,
        recomputed: DigestV1,
        receipts: Vec<DigestV1>,
    },
    RegisteredFutureActionNotPreserved {
        action_id: String,
        dimension: ProtectedDimensionV1,
        grade: CoverageGradeV1,
    },
}

impl fmt::Display for VerifierRegistryErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifierRegistryErrorV1::UnknownVerifier { domain, kind } => write!(
                formatter,
                "unknown registered verifier for ({}, {}) -- refusing, no silent skip",
                domain.as_str(),
                kind.as_str()
            ),
            VerifierRegistryErrorV1::MissingTrustedVerifier { verifier_id } => {
                write!(formatter, "verifier {verifier_id} has no trusted version")
            }
            VerifierRegistryErrorV1::StaleVerifier {
                verifier_id,
                expected,
                observed,
            } => write!(
                formatter,
                "verifier {verifier_id} is stale: expected version {expected}, observed {observed}"
            ),
            VerifierRegistryErrorV1::DuplicateRegistration { domain, kind } => write!(
                formatter,
                "duplicate registration for ({}, {})",
                domain.as_str(),
                kind.as_str()
            ),
            VerifierRegistryErrorV1::InvalidRecord(reason) => {
                write!(formatter, "invalid verifier registry record: {reason}")
            }
            VerifierRegistryErrorV1::InvalidChecklist(reason) => {
                write!(formatter, "invalid obligation checklist: {reason}")
            }
            VerifierRegistryErrorV1::InvalidFutureAction(reason) => {
                write!(formatter, "invalid registered future action: {reason}")
            }
            VerifierRegistryErrorV1::InvalidTransition(reason) => {
                write!(formatter, "invalid successor transition: {reason}")
            }
            VerifierRegistryErrorV1::InvalidSuccessorState(reason) => {
                write!(formatter, "invalid successor state: {reason}")
            }
            VerifierRegistryErrorV1::DeltaSubstitutedAfterVerification { expected, observed } => {
                write!(
                    formatter,
                    "delta substituted after verification: expected root {expected}, observed {observed}"
                )
            }
            VerifierRegistryErrorV1::UnverifiedDelta { delta_root } => write!(
                formatter,
                "delta {delta_root} was never among the registered verifier's verified input roots"
            ),
            VerifierRegistryErrorV1::NonPassingVerifierResult { result } => write!(
                formatter,
                "registered verifier result is {}, which never grants authority",
                result.as_str()
            ),
            VerifierRegistryErrorV1::SuccessorMismatch {
                verifier_id,
                claimed,
                recomputed,
                receipts,
            } => write!(
                formatter,
                "successor mismatch under verifier {verifier_id}: claimed {claimed}, recomputed {recomputed}, receipts {}",
                receipts
                    .iter()
                    .map(|receipt| receipt.to_hex())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            VerifierRegistryErrorV1::RegisteredFutureActionNotPreserved {
                action_id,
                dimension,
                grade,
            } => write!(
                formatter,
                "registered future action {action_id} not preserved: dimension {} has grade {}",
                dimension.as_str(),
                grade.as_str()
            ),
        }
    }
}

impl Error for VerifierRegistryErrorV1 {}

// ---------------------------------------------------------------------------
// Verification entry points.
// ---------------------------------------------------------------------------

/// Current-effect authority validation (ZS-VERIFY-002): the delta in hand
/// must be exactly the delta the registered verifier verified, the verifier
/// must be fresh and passing, and the obligation checklist must bind the same
/// subject root. Substituting a different delta after verification is a loud
/// refusal.
pub fn assert_current_effect_authority_v1(
    registry: &VerifierRegistryV1,
    kind: ProtectedDimensionV1,
    delta_root: DigestV1,
    checklist: &ObligationChecklistV1,
) -> Result<(), VerifierRegistryErrorV1> {
    let record = registry.lookup(VerifierDomainV1::CurrentEffect, kind)?;
    registry.freshness(record)?;
    if !record.result.is_pass() {
        return Err(VerifierRegistryErrorV1::NonPassingVerifierResult {
            result: record.result,
        });
    }
    checklist.validate()?;
    checklist.assert_delta_unsubstituted(delta_root)?;
    if !record.input_roots.contains(&delta_root) {
        return Err(VerifierRegistryErrorV1::UnverifiedDelta { delta_root });
    }
    Ok(())
}

/// Successor-state verification (ZS-VERIFY-003). Uses the typed registry
/// (unknown verifier = loud refusal), checks verifier freshness and passing
/// result, recomputes the successor root from the transition receipts, checks
/// that the successor state preserves every required dimension of the
/// registered future action with evidence-backed coverage, and mints a sealed
/// receipt on acceptance. Any mismatch is a loud fault carrying the receipts.
pub fn verify_successor_state_v1<R>(
    registry: &VerifierRegistryV1,
    transition: &SuccessorStateTransitionV1,
    successor: &SuccessorStateV1,
    action: &RegisteredFutureActionV1,
    recompute_successor_root: R,
) -> Result<SuccessorVerificationReceiptV1, VerifierRegistryErrorV1>
where
    R: Fn(DigestV1, &[DigestV1]) -> DigestV1,
{
    let record = registry.lookup(
        VerifierDomainV1::SuccessorState,
        ProtectedDimensionV1::SuccessorState,
    )?;
    registry.freshness(record)?;
    if !record.result.is_pass() {
        return Err(VerifierRegistryErrorV1::NonPassingVerifierResult {
            result: record.result,
        });
    }
    transition.validate()?;
    successor.validate()?;
    action.validate()?;

    // Recompute the successor from the receipts; refuse any mismatch with the
    // receipts attached to the fault.
    let recomputed = recompute_successor_root(transition.predecessor_root, &transition.receipts);
    if recomputed != transition.claimed_successor_root {
        return Err(VerifierRegistryErrorV1::SuccessorMismatch {
            verifier_id: record.verifier_id.clone(),
            claimed: transition.claimed_successor_root,
            recomputed,
            receipts: transition.receipts.clone(),
        });
    }

    // Preservation: every required dimension of the registered future action
    // must still be covered (grade != Unknown) with evidence refs.
    for dimension in &action.required_dimensions {
        let grade = successor
            .obligations
            .obligations
            .iter()
            .find(|obligation| obligation.dimension == *dimension)
            .map(|obligation| obligation.grade)
            .unwrap_or(CoverageGradeV1::Unknown);
        if grade.is_unknown() {
            return Err(VerifierRegistryErrorV1::RegisteredFutureActionNotPreserved {
                action_id: action.action_id.clone(),
                dimension: *dimension,
                grade,
            });
        }
        let evidence = successor.checklist.evidence_for(*dimension);
        if evidence.is_none_or(|refs| refs.is_empty()) {
            return Err(VerifierRegistryErrorV1::RegisteredFutureActionNotPreserved {
                action_id: action.action_id.clone(),
                dimension: *dimension,
                grade,
            });
        }
    }

    let evidence_digest = serde_json::to_value(&transition.receipts)
        .map(|value| DigestV1::from_bytes(sha256(canonical_json(&value).as_bytes())))
        .map_err(|error| VerifierRegistryErrorV1::InvalidTransition(error.to_string()))?;

    Ok(SuccessorVerificationReceiptV1 {
        receipt_version: VERIFIER_REGISTRY_CONTRACT_VERSION_V1,
        verifier_id: record.verifier_id.clone(),
        verifier_version: record.verifier_version.clone(),
        domain: record.domain,
        predecessor_root: transition.predecessor_root,
        successor_root: recomputed,
        action_id: action.action_id.clone(),
        evidence_digest,
        runtime_ms: record.runtime_ms,
    })
}

