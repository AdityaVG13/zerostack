//! Identity kernel completion (ZS-KERNEL-001/002/003/006/007/008,
//! ZS-CONTRACT-001/004).
//!
//! One rooted ABI version, one canonical byte path per object class, one
//! parent-rooted authoritative event log, one project-level successor CAS,
//! structured task contracts with protected-scope obligations, and payload
//! formation receipts.
//!
//! Fail-closed laws:
//! - [`canonical_object_bytes`] is the ONLY encoding path; unknown object
//!   classes are rejected, and every root binds class, ABI version, and the
//!   `sha256` algorithm tag in its preimage (KERNEL-001/002/007).
//! - A task-contract field change yields a different contract root, which
//!   invalidates every dependent formation receipt (CONTRACT-001).
//! - An uncovered required protected obligation is `Unknown` and can never be
//!   advertised as equivalent (CONTRACT-004).
//! - A formation receipt never verifies a relabeled payload (KERNEL-003).
//! - Event-log replay fails closed on missing or reordered events via root
//!   chaining (KERNEL-006).
//! - The successor CAS advances only on an exact declared parent with a
//!   verified new root; any crash before commit leaves the old root
//!   (KERNEL-008).

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digest::sha256;
use crate::DigestV1;

pub const ROOTED_ABI_VERSION_V6: &str = "zerostack.racc.v6";
pub const ROOT_HASH_ALGORITHM: &str = "sha256";
pub const EVENT_LOG_GENESIS_DOMAIN: &[u8] = b"zerostack.eventlog.genesis.v1\0";
pub const CONTRACT_VERSION_V1: u16 = 1;

/// Fail-closed error for identity kernel construction and verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityErrorV1 {
    UnknownObjectClass(String),
    WrongAbiVersion { actual: String },
    NonCanonicalBytes(String),
    InvalidTaskContract(String),
    InvalidProtectedScope(String),
    InvalidFormationReceipt(String),
    InvalidEventRecord(String),
    TornEventLog { seq: u64, expected: DigestV1, actual: DigestV1 },
    ReorderedEventLog { seq: u64, expected_parent: DigestV1, actual_parent: DigestV1 },
    UncoveredObligation(String),
    EquivalentClaimForbidden(String),
}

impl fmt::Display for IdentityErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownObjectClass(class) => {
                write!(formatter, "unknown object class {class:?}")
            }
            Self::WrongAbiVersion { actual } => {
                write!(formatter, "abi version must be {ROOTED_ABI_VERSION_V6}, got {actual}")
            }
            Self::NonCanonicalBytes(detail) => write!(formatter, "noncanonical bytes: {detail}"),
            Self::InvalidTaskContract(detail) => write!(formatter, "invalid task contract: {detail}"),
            Self::InvalidProtectedScope(detail) => write!(formatter, "invalid protected scope: {detail}"),
            Self::InvalidFormationReceipt(detail) => {
                write!(formatter, "invalid formation receipt: {detail}")
            }
            Self::InvalidEventRecord(detail) => write!(formatter, "invalid event record: {detail}"),
            Self::TornEventLog { seq, expected, actual } => write!(
                formatter,
                "torn event log at seq {seq}: expected head {expected}, record chained to {actual}"
            ),
            Self::ReorderedEventLog { seq, expected_parent, actual_parent } => write!(
                formatter,
                "reordered event log at seq {seq}: expected parent {expected_parent}, got {actual_parent}"
            ),
            Self::UncoveredObligation(dimension) => {
                write!(formatter, "protected obligation {dimension} is uncovered (Unknown)")
            }
            Self::EquivalentClaimForbidden(detail) => {
                write!(formatter, "equivalent claim forbidden: {detail}")
            }
        }
    }
}

impl Error for IdentityErrorV1 {}

// ---------------------------------------------------------------------------
// Rooted ABI: versioned canonical bytes + algorithm-tagged object roots
// (ZS-KERNEL-001/002/007).
// ---------------------------------------------------------------------------

/// Every object class that can be rooted. `canonical_object_bytes` is the
/// single canonical encoding path for these classes; unknown classes are
/// rejected at the authority boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectClassV1 {
    TaskContract,
    ProtectedScope,
    FormationReceipt,
    EventRecord,
    SuccessorRecord,
    ExecuteResult,
}

impl ObjectClassV1 {
    pub fn domain(self) -> &'static str {
        match self {
            ObjectClassV1::TaskContract => "zerostack.object.task_contract.v1",
            ObjectClassV1::ProtectedScope => "zerostack.object.protected_scope.v1",
            ObjectClassV1::FormationReceipt => "zerostack.object.formation_receipt.v1",
            ObjectClassV1::EventRecord => "zerostack.object.event_record.v1",
            ObjectClassV1::SuccessorRecord => "zerostack.object.successor_record.v1",
            ObjectClassV1::ExecuteResult => "zerostack.object.execute_result.v1",
        }
    }
}

/// The versioned, algorithm-tagged preimage for one object root:
/// `sha256 || domain || abi_version || canonical_payload`. The algorithm tag
/// is structurally bound inside every root, so a root can never be replayed
/// under a different digest algorithm.
pub fn root_preimage(
    class: ObjectClassV1,
    abi_version: &str,
    canonical_payload: &[u8],
) -> Vec<u8> {
    let mut preimage = Vec::with_capacity(64 + canonical_payload.len());
    preimage.extend_from_slice(ROOT_HASH_ALGORITHM.as_bytes());
    preimage.push(0);
    preimage.extend_from_slice(class.domain().as_bytes());
    preimage.push(0);
    preimage.extend_from_slice(abi_version.as_bytes());
    preimage.push(0);
    preimage.extend_from_slice(canonical_payload);
    preimage
}

/// Canonical bytes for one object class. The class is bound into the root
/// preimage by the caller of [`object_root`]; unknown classes are rejected by
/// construction (closed enum), and wrong ABI versions or structurally
/// noncanonical payloads fail closed here.
pub fn canonical_object_bytes(
    _class: ObjectClassV1,
    abi_version: &str,
    payload: &Value,
) -> Result<Vec<u8>, IdentityErrorV1> {
    if abi_version != ROOTED_ABI_VERSION_V6 {
        return Err(IdentityErrorV1::WrongAbiVersion {
            actual: abi_version.to_owned(),
        });
    }
    let canonical = crate::canonical_json(payload);
    if canonical.is_empty() {
        return Err(IdentityErrorV1::NonCanonicalBytes(
            "empty canonical payload".into(),
        ));
    }
    Ok(canonical.into_bytes())
}

/// The algorithm-tagged root for one object: sha256 over the versioned
/// preimage. This is the only way object roots are produced.
pub fn object_root(
    class: ObjectClassV1,
    abi_version: &str,
    canonical_payload: &[u8],
) -> Result<DigestV1, IdentityErrorV1> {
    if abi_version != ROOTED_ABI_VERSION_V6 {
        return Err(IdentityErrorV1::WrongAbiVersion {
            actual: abi_version.to_owned(),
        });
    }
    Ok(DigestV1::from_bytes(sha256(&root_preimage(
        class, abi_version, canonical_payload,
    ))))
}

/// Verify a root against a canonical payload: same class, same ABI version,
/// same bytes, and the recorded algorithm tag.
pub fn verify_object_root(
    class: ObjectClassV1,
    abi_version: &str,
    canonical_payload: &[u8],
    claimed: DigestV1,
) -> bool {
    object_root(class, abi_version, canonical_payload)
        .map(|actual| actual == claimed)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Protected scope obligations (ZS-CONTRACT-004).
// ---------------------------------------------------------------------------

/// The protected dimensions of a task. A dimension is a property of the
/// successor state that must hold; an uncovered required dimension is
/// `Unknown` and forbids any equivalent claim.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedDimensionV1 {
    Tests,
    Api,
    Behavior,
    Security,
    Performance,
    FileEffects,
    UserVisibleOutput,
    SuccessorState,
}

impl ProtectedDimensionV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtectedDimensionV1::Tests => "tests",
            ProtectedDimensionV1::Api => "api",
            ProtectedDimensionV1::Behavior => "behavior",
            ProtectedDimensionV1::Security => "security",
            ProtectedDimensionV1::Performance => "performance",
            ProtectedDimensionV1::FileEffects => "file_effects",
            ProtectedDimensionV1::UserVisibleOutput => "user_visible_output",
            ProtectedDimensionV1::SuccessorState => "successor_state",
        }
    }
}

/// Coverage grade of one protected obligation. `Unknown` is terminal:
/// nothing promotes it, and it forbids equivalent claims.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGradeV1 {
    Proved,
    BoundedComplete,
    Observed,
    Unknown,
}

impl CoverageGradeV1 {
    pub fn is_unknown(self) -> bool {
        self == CoverageGradeV1::Unknown
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CoverageGradeV1::Proved => "proved",
            CoverageGradeV1::BoundedComplete => "bounded_complete",
            CoverageGradeV1::Observed => "observed",
            CoverageGradeV1::Unknown => "unknown",
        }
    }
}

/// One protected-scope obligation: a dimension, whether it is required, and
/// its current coverage grade.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeObligationV1 {
    pub dimension: ProtectedDimensionV1,
    pub required: bool,
    pub grade: CoverageGradeV1,
}

impl ScopeObligationV1 {
    pub fn new(
        dimension: ProtectedDimensionV1,
        required: bool,
        grade: CoverageGradeV1,
    ) -> Result<Self, IdentityErrorV1> {
        let obligation = Self {
            dimension,
            required,
            grade,
        };
        obligation.validate()?;
        Ok(obligation)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.grade.is_unknown() && self.required {
            // An uncovered REQUIRED obligation is representable (it is the
            // fail-closed state), but only as Unknown -- callers must never
            // claim equivalence over it. Validation allows it so the
            // forbidden claim is what fails, not the representation.
        }
        Ok(())
    }
}

/// The protected scope of a task: the full set of obligations over the
/// successor state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedScopeObligationsV1 {
    pub obligations: Vec<ScopeObligationV1>,
}

impl ProtectedScopeObligationsV1 {
    pub fn new(obligations: Vec<ScopeObligationV1>) -> Result<Self, IdentityErrorV1> {
        let scope = Self { obligations };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        let mut seen = std::collections::BTreeSet::new();
        for obligation in &self.obligations {
            obligation.validate()?;
            if !seen.insert(obligation.dimension) {
                return Err(IdentityErrorV1::InvalidProtectedScope(format!(
                    "duplicate dimension {}",
                    obligation.dimension.as_str()
                )));
            }
        }
        Ok(())
    }

    /// Dimensions with grade `Unknown` (uncovered). Required or not, they are
    /// listed so callers can route to the baseline fallback.
    pub fn uncovered(&self) -> Vec<ProtectedDimensionV1> {
        self.obligations
            .iter()
            .filter(|obligation| obligation.grade.is_unknown())
            .map(|obligation| obligation.dimension)
            .collect()
    }

    /// Whether an equivalent claim is permitted: every REQUIRED obligation
    /// must be `Proved` or `BoundedComplete` -- never `Unknown` (and never
    /// merely `Observed` for a required dimension, which is a stronger
    /// fail-closed rule than the corpus minimum).
    pub fn equivalent_claim_permitted(&self) -> bool {
        self.obligations.iter().all(|obligation| {
            !obligation.required
                || matches!(
                    obligation.grade,
                    CoverageGradeV1::Proved | CoverageGradeV1::BoundedComplete
                )
        })
    }

    /// Fail-closed equivalent-claim gate: returns the uncovered required
    /// dimension, or `Ok(())` when the claim is permitted. This is the
    /// CONTRACT-004 acceptance surface -- an uncovered property is
    /// `Unknown` and can never be advertised as equivalent.
    pub fn check_equivalent_claim(&self) -> Result<(), IdentityErrorV1> {
        if let Some(obligation) = self
            .obligations
            .iter()
            .find(|obligation| obligation.required && obligation.grade.is_unknown())
        {
            return Err(IdentityErrorV1::UncoveredObligation(
                obligation.dimension.as_str().to_owned(),
            ));
        }
        if !self.equivalent_claim_permitted() {
            return Err(IdentityErrorV1::EquivalentClaimForbidden(
                "a required obligation is only Observed, not Proved or BoundedComplete".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_json(&self) -> Result<Value, IdentityErrorV1> {
        serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Structured task contract (ZS-CONTRACT-001).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectPolicyV1 {
    ReadOnly,
    ReversibleMutations,
    ApprovalRequiredMutations,
    IrreversibleForbidden,
}

impl SideEffectPolicyV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            SideEffectPolicyV1::ReadOnly => "read_only",
            SideEffectPolicyV1::ReversibleMutations => "reversible_mutations",
            SideEffectPolicyV1::ApprovalRequiredMutations => "approval_required_mutations",
            SideEffectPolicyV1::IrreversibleForbidden => "irreversible_forbidden",
        }
    }
}

/// What happens when the run reaches `Unknown`: V6 routes to the frozen raw
/// baseline; a policy can also reject with no mutation, or treat Unknown as a
/// hard error.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicyV1 {
    FrozenRawBaseline,
    RejectedNoMutation,
    UnknownIsError,
}

impl FallbackPolicyV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            FallbackPolicyV1::FrozenRawBaseline => "frozen_raw_baseline",
            FallbackPolicyV1::RejectedNoMutation => "rejected_no_mutation",
            FallbackPolicyV1::UnknownIsError => "unknown_is_error",
        }
    }
}

/// Bounded resource budget for one task execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBudgetV1 {
    pub max_fuel: u64,
    pub max_elapsed_ms: u64,
    pub max_io_bytes: u64,
    pub max_risk_units: u64,
}

impl TaskBudgetV1 {
    pub fn new(
        max_fuel: u64,
        max_elapsed_ms: u64,
        max_io_bytes: u64,
        max_risk_units: u64,
    ) -> Result<Self, IdentityErrorV1> {
        let budget = Self {
            max_fuel,
            max_elapsed_ms,
            max_io_bytes,
            max_risk_units,
        };
        budget.validate()?;
        Ok(budget)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.max_fuel == 0
            || self.max_elapsed_ms == 0
            || self.max_io_bytes == 0
            || self.max_risk_units == 0
        {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "all budget bounds must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

/// The structured task contract: criteria, protected scope, effect policy,
/// environment, budget, fallback, and the bound model/harness/tool contract
/// digests. Every field participates in the contract root.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredTaskContractV1 {
    pub contract_version: u16,
    pub task_kind: String,
    pub acceptance_criteria: Vec<String>,
    pub protected_scope: ProtectedScopeObligationsV1,
    pub side_effect_policy: SideEffectPolicyV1,
    pub environment_fixture_refs: Vec<String>,
    pub initial_project_root: String,
    pub budget: TaskBudgetV1,
    pub deadline_unix_ms: Option<u64>,
    pub fallback_policy: FallbackPolicyV1,
    pub subjective_dimensions: Vec<String>,
    pub harness_contract_digest: Option<DigestV1>,
    pub model_contract_digest: Option<DigestV1>,
    pub tool_contract_digest: Option<DigestV1>,
}

impl StructuredTaskContractV1 {
    pub fn new(
        task_kind: impl Into<String>,
        acceptance_criteria: Vec<String>,
        protected_scope: ProtectedScopeObligationsV1,
        side_effect_policy: SideEffectPolicyV1,
        environment_fixture_refs: Vec<String>,
        initial_project_root: impl Into<String>,
        budget: TaskBudgetV1,
        deadline_unix_ms: Option<u64>,
        fallback_policy: FallbackPolicyV1,
        subjective_dimensions: Vec<String>,
        harness_contract_digest: Option<DigestV1>,
        model_contract_digest: Option<DigestV1>,
        tool_contract_digest: Option<DigestV1>,
    ) -> Result<Self, IdentityErrorV1> {
        let contract = Self {
            contract_version: CONTRACT_VERSION_V1,
            task_kind: task_kind.into(),
            acceptance_criteria,
            protected_scope,
            side_effect_policy,
            environment_fixture_refs,
            initial_project_root: initial_project_root.into(),
            budget,
            deadline_unix_ms,
            fallback_policy,
            subjective_dimensions,
            harness_contract_digest,
            model_contract_digest,
            tool_contract_digest,
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.contract_version != CONTRACT_VERSION_V1 {
            return Err(IdentityErrorV1::InvalidTaskContract(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.task_kind.is_empty() {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "task_kind must be nonempty".into(),
            ));
        }
        if self.acceptance_criteria.is_empty() {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "acceptance_criteria must be nonempty".into(),
            ));
        }
        if self.acceptance_criteria.iter().any(|criterion| criterion.is_empty()) {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "acceptance criteria must not be empty strings".into(),
            ));
        }
        if self.initial_project_root.is_empty() {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "initial_project_root must be nonempty".into(),
            ));
        }
        self.budget.validate()?;
        self.protected_scope.validate()?;
        if self
            .environment_fixture_refs
            .iter()
            .any(|reference| reference.is_empty())
        {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "environment_fixture_refs must not be empty strings".into(),
            ));
        }
        if self.subjective_dimensions.iter().any(|name| name.is_empty()) {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "subjective_dimensions must not be empty strings".into(),
            ));
        }
        Ok(())
    }

    /// Canonical bytes for this contract under the rooted ABI.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClassV1::TaskContract, ROOTED_ABI_VERSION_V6, &value)
    }

    /// The contract root: any field change produces a different root.
    pub fn contract_root(&self) -> Result<DigestV1, IdentityErrorV1> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClassV1::TaskContract, ROOTED_ABI_VERSION_V6, &bytes)
    }
}

// ---------------------------------------------------------------------------
// Payload formation receipt (ZS-KERNEL-003).
// ---------------------------------------------------------------------------

/// Binds a payload root to its constructor, contract, dependency roots,
/// execution record, and epoch. Verifying a payload against a receipt
/// requires the exact payload root AND the exact contract root; a relabeled
/// payload with a valid key fails.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadFormationReceiptV1 {
    pub receipt_version: u16,
    pub constructor_identity: String,
    pub contract_root: DigestV1,
    pub dependency_roots: Vec<String>,
    pub execution_record_root: String,
    pub payload_root: String,
    pub epoch: u64,
    pub abi_version: String,
}

impl PayloadFormationReceiptV1 {
    pub fn new(
        constructor_identity: impl Into<String>,
        contract_root: DigestV1,
        dependency_roots: Vec<String>,
        execution_record_root: impl Into<String>,
        payload_root: impl Into<String>,
        epoch: u64,
    ) -> Result<Self, IdentityErrorV1> {
        let receipt = Self {
            receipt_version: CONTRACT_VERSION_V1,
            constructor_identity: constructor_identity.into(),
            contract_root,
            dependency_roots,
            execution_record_root: execution_record_root.into(),
            payload_root: payload_root.into(),
            epoch,
            abi_version: ROOTED_ABI_VERSION_V6.to_owned(),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.receipt_version != CONTRACT_VERSION_V1 {
            return Err(IdentityErrorV1::InvalidFormationReceipt(format!(
                "unsupported receipt version {}",
                self.receipt_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION_V6 {
            return Err(IdentityErrorV1::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.constructor_identity.is_empty() {
            return Err(IdentityErrorV1::InvalidFormationReceipt(
                "constructor_identity must be nonempty".into(),
            ));
        }
        if self.execution_record_root.is_empty() || self.payload_root.is_empty() {
            return Err(IdentityErrorV1::InvalidFormationReceipt(
                "execution_record_root and payload_root must be nonempty".into(),
            ));
        }
        if self.dependency_roots.iter().any(|root| root.is_empty()) {
            return Err(IdentityErrorV1::InvalidFormationReceipt(
                "dependency_roots must not be empty strings".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClassV1::FormationReceipt, ROOTED_ABI_VERSION_V6, &value)
    }

    pub fn receipt_root(&self) -> Result<DigestV1, IdentityErrorV1> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClassV1::FormationReceipt, ROOTED_ABI_VERSION_V6, &bytes)
    }

    /// Verify a payload against this receipt: the payload root must match the
    /// receipt's payload root AND the contract root must match the receipt's
    /// contract root. Relabeling an unrelated payload with this receipt's
    /// roots fails one of the two bindings.
    pub fn verify_payload(&self, contract_root: DigestV1, payload_root: &str) -> bool {
        self.contract_root == contract_root && self.payload_root == payload_root
    }
}

// ---------------------------------------------------------------------------
// Parent-rooted authoritative event log (ZS-KERNEL-006).
// ---------------------------------------------------------------------------

/// One chained event record. `parent_root` is the head of the log before
/// this record; replay detects missing or reordered events by chaining.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventRecordV1 {
    pub seq: u64,
    pub parent_root: DigestV1,
    pub event_type: String,
    pub payload_root: String,
    pub authority: String,
}

impl EventRecordV1 {
    pub fn new(
        seq: u64,
        parent_root: DigestV1,
        event_type: impl Into<String>,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
    ) -> Result<Self, IdentityErrorV1> {
        let record = Self {
            seq,
            parent_root,
            event_type: event_type.into(),
            payload_root: payload_root.into(),
            authority: authority.into(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.event_type.is_empty() {
            return Err(IdentityErrorV1::InvalidEventRecord(
                "event_type must be nonempty".into(),
            ));
        }
        if self.payload_root.is_empty() {
            return Err(IdentityErrorV1::InvalidEventRecord(
                "payload_root must be nonempty".into(),
            ));
        }
        if self.authority.is_empty() {
            return Err(IdentityErrorV1::InvalidEventRecord(
                "authority must be nonempty".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClassV1::EventRecord, ROOTED_ABI_VERSION_V6, &value)
    }

    pub fn record_root(&self) -> Result<DigestV1, IdentityErrorV1> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClassV1::EventRecord, ROOTED_ABI_VERSION_V6, &bytes)
    }
}

/// The genesis head of every event log.
pub fn event_log_genesis() -> DigestV1 {
    DigestV1::from_bytes(sha256(EVENT_LOG_GENESIS_DOMAIN))
}

/// In-memory append-only authoritative event log with parent-root chaining.
/// Compaction is a sealed-snapshot rewrite owned by the caller; this type
/// never rewrites history.
#[derive(Clone, Debug)]
pub struct EventLogV1 {
    records: Vec<EventRecordV1>,
}

impl EventLogV1 {
    pub fn new() -> Self {
        Self { records: Vec::new() }
    }

    pub fn from_records(records: Vec<EventRecordV1>) -> Self {
        Self { records }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[EventRecordV1] {
        &self.records
    }

    /// The current head root. An empty log is the genesis root.
    pub fn head(&self) -> Result<DigestV1, IdentityErrorV1> {
        match self.records.last() {
            Some(record) => record.record_root(),
            None => Ok(event_log_genesis()),
        }
    }

    /// Append one event, chaining it to the current head.
    pub fn append(
        &mut self,
        event_type: impl Into<String>,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
    ) -> Result<EventRecordV1, IdentityErrorV1> {
        let parent_root = self.head()?;
        let seq = self.records.len() as u64;
        let record = EventRecordV1::new(
            seq,
            parent_root,
            event_type,
            payload_root,
            authority,
        )?;
        self.records.push(record.clone());
        Ok(record)
    }

    /// Replay a record sequence from genesis, fail-closed. Returns the
    /// replayed head root. A torn tail (missing last record), a missing
    /// middle record, or a reordered record all fail via root chaining.
    pub fn replay(records: &[EventRecordV1]) -> Result<DigestV1, IdentityErrorV1> {
        let mut running = event_log_genesis();
        for record in records {
            if record.seq != 0 && record.parent_root != running {
                return Err(IdentityErrorV1::ReorderedEventLog {
                    seq: record.seq,
                    expected_parent: running,
                    actual_parent: record.parent_root,
                });
            }
            if record.seq == 0 && record.parent_root != event_log_genesis() {
                return Err(IdentityErrorV1::ReorderedEventLog {
                    seq: 0,
                    expected_parent: event_log_genesis(),
                    actual_parent: record.parent_root,
                });
            }
            running = record.record_root()?;
        }
        Ok(running)
    }

    /// Verify the full chain of this log; returns the sealed head root.
    pub fn verify_chain(&self) -> Result<DigestV1, IdentityErrorV1> {
        Self::replay(&self.records)
    }

    /// Verify the log and additionally check that the caller's expected head
    /// equals the replayed head -- this is how a torn tail is detected after
    /// a process kill: the persisted prefix replays to a head that does not
    /// match the sealed head.
    pub fn verify_chain_against(&self, sealed_head: DigestV1) -> Result<(), IdentityErrorV1> {
        let replayed = self.verify_chain()?;
        if replayed != sealed_head {
            return Err(IdentityErrorV1::TornEventLog {
                seq: self.records.len() as u64,
                expected: sealed_head,
                actual: replayed,
            });
        }
        Ok(())
    }
}

impl Default for EventLogV1 {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Project-level successor CAS (ZS-KERNEL-008).
// ---------------------------------------------------------------------------

/// Why the successor CAS did not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorUnchangedReasonV1 {
    /// The declared parent root does not match the current root (concurrent
    /// advance or stale handle).
    DeclaredParentMismatch,
    /// The verified successor root equals the current root: nothing changed.
    NoVerifiedChange,
}

/// Outcome of one successor CAS attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorOutcomeV1 {
    Advanced { new_current_root: DigestV1 },
    Unchanged { reason: SuccessorUnchangedReasonV1 },
}

/// Project-level "verified successor root becomes current XOR unchanged" CAS.
/// The ONLY mutation is [`ProjectSuccessorCasV1::try_advance`], which requires
/// an exact declared parent and a verified successor root. Verification and
/// authorization are pure observations that never mutate the CAS, so a crash
/// before commit leaves the old root and a crash after commit leaves the
/// complete new root -- never a partial state.
#[derive(Clone, Copy, Debug)]
pub struct ProjectSuccessorCasV1 {
    current_root: DigestV1,
}

impl ProjectSuccessorCasV1 {
    pub fn new(genesis: DigestV1) -> Self {
        Self {
            current_root: genesis,
        }
    }

    pub fn current(&self) -> DigestV1 {
        self.current_root
    }

    /// Advance the project root. Fails closed:
    /// - declared parent != current  -> `Unchanged(DeclaredParentMismatch)`
    /// - verified successor == current -> `Unchanged(NoVerifiedChange)`
    /// - otherwise advances and returns the new root.
    pub fn try_advance(
        &mut self,
        declared_parent_root: DigestV1,
        verified_successor_root: DigestV1,
    ) -> SuccessorOutcomeV1 {
        if declared_parent_root != self.current_root {
            return SuccessorOutcomeV1::Unchanged {
                reason: SuccessorUnchangedReasonV1::DeclaredParentMismatch,
            };
        }
        if verified_successor_root == self.current_root {
            return SuccessorOutcomeV1::Unchanged {
                reason: SuccessorUnchangedReasonV1::NoVerifiedChange,
            };
        }
        self.current_root = verified_successor_root;
        SuccessorOutcomeV1::Advanced {
            new_current_root: self.current_root,
        }
    }
}

// ---------------------------------------------------------------------------
// Successor record (bound for the event log and audit).
// ---------------------------------------------------------------------------

/// A sealed record of one successor advance (or unchanged decision).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorRecordV1 {
    pub record_version: u16,
    pub declared_parent_root: DigestV1,
    pub verified_successor_root: DigestV1,
    pub advanced: bool,
    pub authority: String,
    pub abi_version: String,
}

impl SuccessorRecordV1 {
    pub fn new(
        declared_parent_root: DigestV1,
        verified_successor_root: DigestV1,
        advanced: bool,
        authority: impl Into<String>,
    ) -> Result<Self, IdentityErrorV1> {
        let record = Self {
            record_version: CONTRACT_VERSION_V1,
            declared_parent_root,
            verified_successor_root,
            advanced,
            authority: authority.into(),
            abi_version: ROOTED_ABI_VERSION_V6.to_owned(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.record_version != CONTRACT_VERSION_V1 {
            return Err(IdentityErrorV1::InvalidEventRecord(format!(
                "unsupported record version {}",
                self.record_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION_V6 {
            return Err(IdentityErrorV1::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.authority.is_empty() {
            return Err(IdentityErrorV1::InvalidEventRecord(
                "authority must be nonempty".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClassV1::SuccessorRecord, ROOTED_ABI_VERSION_V6, &value)
    }

    pub fn record_root(&self) -> Result<DigestV1, IdentityErrorV1> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClassV1::SuccessorRecord, ROOTED_ABI_VERSION_V6, &bytes)
    }
}

// ---------------------------------------------------------------------------
// Harness contract (ZS-CONTRACT-003).
// ---------------------------------------------------------------------------

/// Serialization scheme for tool arguments/results across the harness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializationSchemeV1 {
    CanonicalJson,
    CompactRefs,
}

/// Message ordering guarantee the harness promises for tool calls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrderingV1 {
    StrictCallOrder,
    CompletionPermitsReordering,
}

/// Transcript policy: what the harness records about a session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptPolicyV1 {
    FullRecording,
    DecisionsAndResultsOnly,
    None,
}

/// Cancellation semantics the harness guarantees for in-flight calls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationSemanticsV1 {
    CooperativeAtCallBoundaries,
    HardDeadlineOnly,
}

/// The harness contract: tool serialization, message ordering, transcript
/// policy, cancellation semantics, the native tool set digest, and the
/// adapter renderer version. Any change alters the contract root and
/// invalidates dependent certificates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessContractV1 {
    pub contract_version: u16,
    pub harness_name: String,
    pub serialization: SerializationSchemeV1,
    pub message_ordering: MessageOrderingV1,
    pub transcript_policy: TranscriptPolicyV1,
    pub cancellation_semantics: CancellationSemanticsV1,
    pub native_tool_set_digest: DigestV1,
    pub adapter_renderer_version: u16,
    pub abi_version: String,
}

impl HarnessContractV1 {
    pub fn new(
        harness_name: impl Into<String>,
        serialization: SerializationSchemeV1,
        message_ordering: MessageOrderingV1,
        transcript_policy: TranscriptPolicyV1,
        cancellation_semantics: CancellationSemanticsV1,
        native_tool_set_digest: DigestV1,
        adapter_renderer_version: u16,
    ) -> Result<Self, IdentityErrorV1> {
        let contract = Self {
            contract_version: CONTRACT_VERSION_V1,
            harness_name: harness_name.into(),
            serialization,
            message_ordering,
            transcript_policy,
            cancellation_semantics,
            native_tool_set_digest,
            adapter_renderer_version,
            abi_version: ROOTED_ABI_VERSION_V6.to_owned(),
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), IdentityErrorV1> {
        if self.contract_version != CONTRACT_VERSION_V1 {
            return Err(IdentityErrorV1::InvalidTaskContract(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION_V6 {
            return Err(IdentityErrorV1::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.harness_name.is_empty() {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "harness_name must be nonempty".into(),
            ));
        }
        if self.adapter_renderer_version == 0 {
            return Err(IdentityErrorV1::InvalidTaskContract(
                "adapter_renderer_version must be nonzero".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityErrorV1::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClassV1::TaskContract, ROOTED_ABI_VERSION_V6, &value)
    }

    pub fn contract_root(&self) -> Result<DigestV1, IdentityErrorV1> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClassV1::TaskContract, ROOTED_ABI_VERSION_V6, &bytes)
    }
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/identity.rs"]
mod tests;
