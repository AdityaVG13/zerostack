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
use crate::Sha256Digest;

pub const ROOTED_ABI_VERSION: &str = "zerostack.racc";
pub const ROOT_HASH_ALGORITHM: &str = "sha256";
pub const EVENT_LOG_GENESIS_DOMAIN: &[u8] = b"zerostack.eventlog.genesis\0";
pub const CONTRACT_VERSION: u16 = 1;

/// Fail-closed error for identity kernel construction and verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityKernelError {
    UnknownObjectClass(String),
    WrongAbiVersion { actual: String },
    NonCanonicalBytes(String),
    InvalidTaskContract(String),
    InvalidProtectedScope(String),
    InvalidFormationReceipt(String),
    InvalidEventRecord(String),
    TornEventLog { seq: u64, expected: Sha256Digest, actual: Sha256Digest },
    ReorderedEventLog { seq: u64, expected_parent: Sha256Digest, actual_parent: Sha256Digest },
    UncoveredObligation(String),
    EquivalentClaimForbidden(String),
    UnknownEventClass(String),
    InvalidMigrationReceipt(String),
    SourceRootMismatch,
    TargetRootMismatch,
    MigrationWithoutAbiChange,
    LegacyTargetAbi(String),
}

impl fmt::Display for IdentityKernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownObjectClass(class) => {
                write!(formatter, "unknown object class {class:?}")
            }
            Self::WrongAbiVersion { actual } => {
                write!(formatter, "abi version must be {ROOTED_ABI_VERSION}, got {actual}")
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
            Self::UnknownEventClass(class) => {
                write!(formatter, "unknown event class {class:?} (not one of the nine authoritative classes)")
            }
            Self::InvalidMigrationReceipt(detail) => {
                write!(formatter, "invalid rooted migration receipt: {detail}")
            }
            Self::SourceRootMismatch => write!(
                formatter,
                "migration receipt source root does not match its recorded legacy object"
            ),
            Self::TargetRootMismatch => write!(
                formatter,
                "migration receipt target root does not match the rooted object"
            ),
            Self::MigrationWithoutAbiChange => write!(
                formatter,
                "migration receipt requires a real ABI version change between source and target"
            ),
            Self::LegacyTargetAbi(actual) => write!(
                formatter,
                "migration target must use {ROOTED_ABI_VERSION}, got {actual}"
            ),
        }
    }
}

impl Error for IdentityKernelError {}

// ---------------------------------------------------------------------------
// Rooted ABI: versioned canonical bytes + algorithm-tagged object roots
// (ZS-KERNEL-001/002/007).
// ---------------------------------------------------------------------------

/// Every object class that can be rooted. `canonical_object_bytes` is the
/// single canonical encoding path for these classes; unknown classes are
/// rejected at the authority boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectClass {
    TaskContract,
    ProtectedScope,
    FormationReceipt,
    EventRecord,
    SuccessorRecord,
    ExecuteResult,
    ContinuationHandle,
    ContinuationCompactRecord,
    /// TokenZero/GraphZero decision-view objects (ZS-ADAPTER-008) rooted
    /// under the same canonical byte path as every other class.
    DecisionView,
    /// FSZero/GraphZero delta objects (exact-delta roots) rooted under the
    /// same canonical byte path.
    Delta,
    /// zero-gate authority objects (permits, assets, admission records)
    /// rooted under the same canonical byte path.
    AuthorityObject,
    /// W9-E trusted safe-expand handle (zerostack-qg2a): the self-rooted
    /// seal of every authority binding of one exact expansion.
    SafeExpandHandle,
    /// Rooted receipt for migrating a legacy rooted object into the current
    /// ABI (ZS-KERNEL-007): pins source and target roots under one receipt.
    MigrationReceipt,
}

impl ObjectClass {
    pub fn domain(self) -> &'static str {
        match self {
            ObjectClass::TaskContract => "zerostack.object.task_contract",
            ObjectClass::ProtectedScope => "zerostack.object.protected_scope",
            ObjectClass::FormationReceipt => "zerostack.object.formation_receipt",
            ObjectClass::EventRecord => "zerostack.object.event_record",
            ObjectClass::SuccessorRecord => "zerostack.object.successor_record",
            ObjectClass::ExecuteResult => "zerostack.object.execute_result",
            ObjectClass::ContinuationHandle => "zerostack.object.continuation_handle",
            ObjectClass::ContinuationCompactRecord => {
                "zerostack.object.continuation_compact_record"
            }
            ObjectClass::DecisionView => "zerostack.object.decision_view",
            ObjectClass::Delta => "zerostack.object.delta",
            ObjectClass::AuthorityObject => "zerostack.object.authority_object",
            ObjectClass::SafeExpandHandle => "zerostack.object.safe_expand_handle",
            ObjectClass::MigrationReceipt => "zerostack.object.migration_receipt",
        }
    }
}

/// The versioned, algorithm-tagged preimage for one object root:
/// `sha256 || domain || abi_version || canonical_payload`. The algorithm tag
/// is structurally bound inside every root, so a root can never be replayed
/// under a different digest algorithm.
pub fn root_preimage(
    class: ObjectClass,
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
    _class: ObjectClass,
    abi_version: &str,
    payload: &Value,
) -> Result<Vec<u8>, IdentityKernelError> {
    if abi_version != ROOTED_ABI_VERSION {
        return Err(IdentityKernelError::WrongAbiVersion {
            actual: abi_version.to_owned(),
        });
    }
    let canonical = crate::canonical_json(payload);
    if canonical.is_empty() {
        return Err(IdentityKernelError::NonCanonicalBytes(
            "empty canonical payload".into(),
        ));
    }
    Ok(canonical.into_bytes())
}

/// The algorithm-tagged root for one object: sha256 over the versioned
/// preimage. This is the only way object roots are produced.
pub fn object_root(
    class: ObjectClass,
    abi_version: &str,
    canonical_payload: &[u8],
) -> Result<Sha256Digest, IdentityKernelError> {
    if abi_version != ROOTED_ABI_VERSION {
        return Err(IdentityKernelError::WrongAbiVersion {
            actual: abi_version.to_owned(),
        });
    }
    Ok(Sha256Digest::from_bytes(sha256(&root_preimage(
        class, abi_version, canonical_payload,
    ))))
}

/// Verify a root against a canonical payload: same class, same ABI version,
/// same bytes, and the recorded algorithm tag.
pub fn verify_object_root(
    class: ObjectClass,
    abi_version: &str,
    canonical_payload: &[u8],
    claimed: Sha256Digest,
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
pub enum ProtectedDimension {
    Tests,
    Api,
    Behavior,
    Security,
    Performance,
    FileEffects,
    UserVisibleOutput,
    SuccessorState,
}

impl ProtectedDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtectedDimension::Tests => "tests",
            ProtectedDimension::Api => "api",
            ProtectedDimension::Behavior => "behavior",
            ProtectedDimension::Security => "security",
            ProtectedDimension::Performance => "performance",
            ProtectedDimension::FileEffects => "file_effects",
            ProtectedDimension::UserVisibleOutput => "user_visible_output",
            ProtectedDimension::SuccessorState => "successor_state",
        }
    }
}

/// Coverage grade of one protected obligation. `Unknown` is terminal:
/// nothing promotes it, and it forbids equivalent claims.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGrade {
    Proved,
    BoundedComplete,
    Observed,
    Unknown,
}

impl CoverageGrade {
    pub fn is_unknown(self) -> bool {
        self == CoverageGrade::Unknown
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CoverageGrade::Proved => "proved",
            CoverageGrade::BoundedComplete => "bounded_complete",
            CoverageGrade::Observed => "observed",
            CoverageGrade::Unknown => "unknown",
        }
    }
}

/// One protected-scope obligation: a dimension, whether it is required, and
/// its current coverage grade.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeObligation {
    pub dimension: ProtectedDimension,
    pub required: bool,
    pub grade: CoverageGrade,
}

impl ScopeObligation {
    pub fn new(
        dimension: ProtectedDimension,
        required: bool,
        grade: CoverageGrade,
    ) -> Result<Self, IdentityKernelError> {
        let obligation = Self {
            dimension,
            required,
            grade,
        };
        obligation.validate()?;
        Ok(obligation)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
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
pub struct ProtectedScopeObligations {
    pub obligations: Vec<ScopeObligation>,
}

impl ProtectedScopeObligations {
    pub fn new(obligations: Vec<ScopeObligation>) -> Result<Self, IdentityKernelError> {
        let scope = Self { obligations };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        let mut seen = std::collections::BTreeSet::new();
        for obligation in &self.obligations {
            obligation.validate()?;
            if !seen.insert(obligation.dimension) {
                return Err(IdentityKernelError::InvalidProtectedScope(format!(
                    "duplicate dimension {}",
                    obligation.dimension.as_str()
                )));
            }
        }
        Ok(())
    }

    /// Dimensions with grade `Unknown` (uncovered). Required or not, they are
    /// listed so callers can route to the baseline fallback.
    pub fn uncovered(&self) -> Vec<ProtectedDimension> {
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
                    CoverageGrade::Proved | CoverageGrade::BoundedComplete
                )
        })
    }

    /// Fail-closed equivalent-claim gate: returns the uncovered required
    /// dimension, or `Ok(())` when the claim is permitted. This is the
    /// CONTRACT-004 acceptance surface -- an uncovered property is
    /// `Unknown` and can never be advertised as equivalent.
    pub fn check_equivalent_claim(&self) -> Result<(), IdentityKernelError> {
        if let Some(obligation) = self
            .obligations
            .iter()
            .find(|obligation| obligation.required && obligation.grade.is_unknown())
        {
            return Err(IdentityKernelError::UncoveredObligation(
                obligation.dimension.as_str().to_owned(),
            ));
        }
        if !self.equivalent_claim_permitted() {
            return Err(IdentityKernelError::EquivalentClaimForbidden(
                "a required obligation is only Observed, not Proved or BoundedComplete".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_json(&self) -> Result<Value, IdentityKernelError> {
        serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Structured task contract (ZS-CONTRACT-001).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectPolicy {
    ReadOnly,
    ReversibleMutations,
    ApprovalRequiredMutations,
    IrreversibleForbidden,
}

impl SideEffectPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            SideEffectPolicy::ReadOnly => "read_only",
            SideEffectPolicy::ReversibleMutations => "reversible_mutations",
            SideEffectPolicy::ApprovalRequiredMutations => "approval_required_mutations",
            SideEffectPolicy::IrreversibleForbidden => "irreversible_forbidden",
        }
    }
}

/// What happens when the run reaches `Unknown`: the execute surface routes
/// to the frozen raw baseline; a policy can also reject with no mutation, or
/// treat Unknown as a hard error.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    FrozenRawBaseline,
    RejectedNoMutation,
    UnknownIsError,
}

impl FallbackPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            FallbackPolicy::FrozenRawBaseline => "frozen_raw_baseline",
            FallbackPolicy::RejectedNoMutation => "rejected_no_mutation",
            FallbackPolicy::UnknownIsError => "unknown_is_error",
        }
    }
}

/// Bounded resource budget for one task execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBudget {
    pub max_fuel: u64,
    pub max_elapsed_ms: u64,
    pub max_io_bytes: u64,
    pub max_risk_units: u64,
}

impl TaskBudget {
    pub fn new(
        max_fuel: u64,
        max_elapsed_ms: u64,
        max_io_bytes: u64,
        max_risk_units: u64,
    ) -> Result<Self, IdentityKernelError> {
        let budget = Self {
            max_fuel,
            max_elapsed_ms,
            max_io_bytes,
            max_risk_units,
        };
        budget.validate()?;
        Ok(budget)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.max_fuel == 0
            || self.max_elapsed_ms == 0
            || self.max_io_bytes == 0
            || self.max_risk_units == 0
        {
            return Err(IdentityKernelError::InvalidTaskContract(
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
pub struct StructuredTaskContract {
    pub contract_version: u16,
    pub task_kind: String,
    pub acceptance_criteria: Vec<String>,
    pub protected_scope: ProtectedScopeObligations,
    pub side_effect_policy: SideEffectPolicy,
    pub environment_fixture_refs: Vec<String>,
    pub initial_project_root: String,
    pub budget: TaskBudget,
    pub deadline_unix_ms: Option<u64>,
    pub fallback_policy: FallbackPolicy,
    pub subjective_dimensions: Vec<String>,
    pub harness_contract_digest: Option<Sha256Digest>,
    pub model_contract_digest: Option<Sha256Digest>,
    pub tool_contract_digest: Option<Sha256Digest>,
}

impl StructuredTaskContract {
    pub fn new(
        task_kind: impl Into<String>,
        acceptance_criteria: Vec<String>,
        protected_scope: ProtectedScopeObligations,
        side_effect_policy: SideEffectPolicy,
        environment_fixture_refs: Vec<String>,
        initial_project_root: impl Into<String>,
        budget: TaskBudget,
        deadline_unix_ms: Option<u64>,
        fallback_policy: FallbackPolicy,
        subjective_dimensions: Vec<String>,
        harness_contract_digest: Option<Sha256Digest>,
        model_contract_digest: Option<Sha256Digest>,
        tool_contract_digest: Option<Sha256Digest>,
    ) -> Result<Self, IdentityKernelError> {
        let contract = Self {
            contract_version: CONTRACT_VERSION,
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

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.contract_version != CONTRACT_VERSION {
            return Err(IdentityKernelError::InvalidTaskContract(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.task_kind.is_empty() {
            return Err(IdentityKernelError::InvalidTaskContract(
                "task_kind must be nonempty".into(),
            ));
        }
        if self.acceptance_criteria.is_empty() {
            return Err(IdentityKernelError::InvalidTaskContract(
                "acceptance_criteria must be nonempty".into(),
            ));
        }
        if self.acceptance_criteria.iter().any(|criterion| criterion.is_empty()) {
            return Err(IdentityKernelError::InvalidTaskContract(
                "acceptance criteria must not be empty strings".into(),
            ));
        }
        if self.initial_project_root.is_empty() {
            return Err(IdentityKernelError::InvalidTaskContract(
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
            return Err(IdentityKernelError::InvalidTaskContract(
                "environment_fixture_refs must not be empty strings".into(),
            ));
        }
        if self.subjective_dimensions.iter().any(|name| name.is_empty()) {
            return Err(IdentityKernelError::InvalidTaskContract(
                "subjective_dimensions must not be empty strings".into(),
            ));
        }
        Ok(())
    }

    /// Canonical bytes for this contract under the rooted ABI.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::TaskContract, ROOTED_ABI_VERSION, &value)
    }

    /// The contract root: any field change produces a different root.
    pub fn contract_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::TaskContract, ROOTED_ABI_VERSION, &bytes)
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
pub struct PayloadFormationReceipt {
    pub receipt_version: u16,
    pub constructor_identity: String,
    pub contract_root: Sha256Digest,
    pub dependency_roots: Vec<String>,
    pub execution_record_root: String,
    pub payload_root: String,
    pub epoch: u64,
    pub abi_version: String,
}

impl PayloadFormationReceipt {
    pub fn new(
        constructor_identity: impl Into<String>,
        contract_root: Sha256Digest,
        dependency_roots: Vec<String>,
        execution_record_root: impl Into<String>,
        payload_root: impl Into<String>,
        epoch: u64,
    ) -> Result<Self, IdentityKernelError> {
        let receipt = Self {
            receipt_version: CONTRACT_VERSION,
            constructor_identity: constructor_identity.into(),
            contract_root,
            dependency_roots,
            execution_record_root: execution_record_root.into(),
            payload_root: payload_root.into(),
            epoch,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.receipt_version != CONTRACT_VERSION {
            return Err(IdentityKernelError::InvalidFormationReceipt(format!(
                "unsupported receipt version {}",
                self.receipt_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.constructor_identity.is_empty() {
            return Err(IdentityKernelError::InvalidFormationReceipt(
                "constructor_identity must be nonempty".into(),
            ));
        }
        if self.execution_record_root.is_empty() || self.payload_root.is_empty() {
            return Err(IdentityKernelError::InvalidFormationReceipt(
                "execution_record_root and payload_root must be nonempty".into(),
            ));
        }
        if self.dependency_roots.iter().any(|root| root.is_empty()) {
            return Err(IdentityKernelError::InvalidFormationReceipt(
                "dependency_roots must not be empty strings".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::FormationReceipt, ROOTED_ABI_VERSION, &value)
    }

    pub fn receipt_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::FormationReceipt, ROOTED_ABI_VERSION, &bytes)
    }

    /// Verify a payload against this receipt: the payload root must match the
    /// receipt's payload root AND the contract root must match the receipt's
    /// contract root. Relabeling an unrelated payload with this receipt's
    /// roots fails one of the two bindings.
    pub fn verify_payload(&self, contract_root: Sha256Digest, payload_root: &str) -> bool {
        self.contract_root == contract_root && self.payload_root == payload_root
    }

    /// Fail-closed dependency re-check for cache reuse (ZS-KERNEL-003).
    /// `verify_payload` alone does not revoke reuse when a dependency mutates
    /// after formation; this re-checks that the CURRENT dependency roots are
    /// exactly the set this receipt was formed against. Any added, removed,
    /// or changed dependency root revokes reuse. Order-insensitive: the
    /// dependency set is compared as a normalized set.
    pub fn verify_against(&self, current_dependency_roots: &[String]) -> bool {
        let mut recorded: Vec<&str> = self.dependency_roots.iter().map(String::as_str).collect();
        let mut current: Vec<&str> = current_dependency_roots.iter().map(String::as_str).collect();
        recorded.sort_unstable();
        current.sort_unstable();
        recorded.dedup();
        current.dedup();
        recorded == current
    }
}

// ---------------------------------------------------------------------------
// Parent-rooted authoritative event log (ZS-KERNEL-006).
// ---------------------------------------------------------------------------

/// Typed authoritative event classes (ZS-KERNEL-006). The wire record keeps a
/// `String` `event_type`; this enum is the typed kernel boundary that
/// enumerates every authoritative event class -- state transitions, evidence
/// observations, cache decisions, executions, verification, authority
/// issuance, commits, rollbacks, and resource charges. Unknown class names
/// fail closed at the typed boundary (`UnknownEventClass`), so a runtime
/// journal cannot append an unclassified event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventClass {
    /// Project state transition (root change, phase change, branch).
    StateTransition,
    /// Observation of raw evidence (span read, trace, diff).
    EvidenceObservation,
    /// Cache admission/refusal decision.
    CacheDecision,
    /// Task execution (started/finished/interrupted).
    Execution,
    /// Verification decision (certificate checked, verdict issued).
    Verification,
    /// Authority issuance (permit, asset, capability grant).
    AuthorityIssuance,
    /// Committed mutation (successor CAS advance).
    Commit,
    /// Rolled-back mutation.
    Rollback,
    /// Resource charge (fuel, elapsed, io, risk).
    ResourceCharge,
}

impl EventClass {
    /// All nine authoritative event classes.
    pub const ALL: [EventClass; 9] = [
        EventClass::StateTransition,
        EventClass::EvidenceObservation,
        EventClass::CacheDecision,
        EventClass::Execution,
        EventClass::Verification,
        EventClass::AuthorityIssuance,
        EventClass::Commit,
        EventClass::Rollback,
        EventClass::ResourceCharge,
    ];

    /// The wire `event_type` string for this class.
    pub fn as_str(self) -> &'static str {
        match self {
            EventClass::StateTransition => "state_transition",
            EventClass::EvidenceObservation => "evidence_observation",
            EventClass::CacheDecision => "cache_decision",
            EventClass::Execution => "execution",
            EventClass::Verification => "verification",
            EventClass::AuthorityIssuance => "authority_issuance",
            EventClass::Commit => "commit",
            EventClass::Rollback => "rollback",
            EventClass::ResourceCharge => "resource_charge",
        }
    }

    /// Parse a wire `event_type` string, fail-closed on anything outside the
    /// nine authoritative classes.
    pub fn from_str(class: &str) -> Result<Self, IdentityKernelError> {
        match class {
            "state_transition" => Ok(EventClass::StateTransition),
            "evidence_observation" => Ok(EventClass::EvidenceObservation),
            "cache_decision" => Ok(EventClass::CacheDecision),
            "execution" => Ok(EventClass::Execution),
            "verification" => Ok(EventClass::Verification),
            "authority_issuance" => Ok(EventClass::AuthorityIssuance),
            "commit" => Ok(EventClass::Commit),
            "rollback" => Ok(EventClass::Rollback),
            "resource_charge" => Ok(EventClass::ResourceCharge),
            other => Err(IdentityKernelError::UnknownEventClass(other.to_owned())),
        }
    }
}

/// One chained event record. `parent_root` is the head of the log before
/// this record; replay detects missing or reordered events by chaining.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventRecord {
    pub seq: u64,
    pub parent_root: Sha256Digest,
    pub event_type: String,
    pub payload_root: String,
    pub authority: String,
}

impl EventRecord {
    pub fn new(
        seq: u64,
        parent_root: Sha256Digest,
        event_type: impl Into<String>,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
    ) -> Result<Self, IdentityKernelError> {
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

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.event_type.is_empty() {
            return Err(IdentityKernelError::InvalidEventRecord(
                "event_type must be nonempty".into(),
            ));
        }
        if self.payload_root.is_empty() {
            return Err(IdentityKernelError::InvalidEventRecord(
                "payload_root must be nonempty".into(),
            ));
        }
        if self.authority.is_empty() {
            return Err(IdentityKernelError::InvalidEventRecord(
                "authority must be nonempty".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::EventRecord, ROOTED_ABI_VERSION, &value)
    }

    pub fn record_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::EventRecord, ROOTED_ABI_VERSION, &bytes)
    }
}

/// The genesis head of every event log.
pub fn event_log_genesis() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(EVENT_LOG_GENESIS_DOMAIN))
}

/// In-memory append-only authoritative event log with parent-root chaining.
/// Compaction is a sealed-snapshot rewrite owned by the caller; this type
/// never rewrites history.
#[derive(Clone, Debug)]
pub struct EventLog {
    records: Vec<EventRecord>,
}

impl EventLog {
    pub fn new() -> Self {
        Self { records: Vec::new() }
    }

    pub fn from_records(records: Vec<EventRecord>) -> Self {
        Self { records }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[EventRecord] {
        &self.records
    }

    /// The current head root. An empty log is the genesis root.
    pub fn head(&self) -> Result<Sha256Digest, IdentityKernelError> {
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
    ) -> Result<EventRecord, IdentityKernelError> {
        let parent_root = self.head()?;
        let seq = self.records.len() as u64;
        let record = EventRecord::new(
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
    pub fn replay(records: &[EventRecord]) -> Result<Sha256Digest, IdentityKernelError> {
        let mut running = event_log_genesis();
        for record in records {
            if record.seq != 0 && record.parent_root != running {
                return Err(IdentityKernelError::ReorderedEventLog {
                    seq: record.seq,
                    expected_parent: running,
                    actual_parent: record.parent_root,
                });
            }
            if record.seq == 0 && record.parent_root != event_log_genesis() {
                return Err(IdentityKernelError::ReorderedEventLog {
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
    pub fn verify_chain(&self) -> Result<Sha256Digest, IdentityKernelError> {
        Self::replay(&self.records)
    }

    /// Verify the log and additionally check that the caller's expected head
    /// equals the replayed head -- this is how a torn tail is detected after
    /// a process kill: the persisted prefix replays to a head that does not
    /// match the sealed head.
    pub fn verify_chain_against(&self, sealed_head: Sha256Digest) -> Result<(), IdentityKernelError> {
        let replayed = self.verify_chain()?;
        if replayed != sealed_head {
            return Err(IdentityKernelError::TornEventLog {
                seq: self.records.len() as u64,
                expected: sealed_head,
                actual: replayed,
            });
        }
        Ok(())
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Project-level successor CAS (ZS-KERNEL-008).
// ---------------------------------------------------------------------------

/// Why the successor CAS did not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorUnchangedReason {
    /// The declared parent root does not match the current root (concurrent
    /// advance or stale handle).
    DeclaredParentMismatch,
    /// The verified successor root equals the current root: nothing changed.
    NoVerifiedChange,
}

/// Outcome of one successor CAS attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorOutcome {
    Advanced { new_current_root: Sha256Digest },
    Unchanged { reason: SuccessorUnchangedReason },
}

/// Project-level "verified successor root becomes current XOR unchanged" CAS.
/// The ONLY mutation is [`ProjectSuccessorCas::try_advance`], which requires
/// an exact declared parent and a verified successor root. Verification and
/// authorization are pure observations that never mutate the CAS, so a crash
/// before commit leaves the old root and a crash after commit leaves the
/// complete new root -- never a partial state.
#[derive(Clone, Copy, Debug)]
pub struct ProjectSuccessorCas {
    current_root: Sha256Digest,
}

impl ProjectSuccessorCas {
    pub fn new(genesis: Sha256Digest) -> Self {
        Self {
            current_root: genesis,
        }
    }

    pub fn current(&self) -> Sha256Digest {
        self.current_root
    }

    /// Advance the project root. Fails closed:
    /// - declared parent != current  -> `Unchanged(DeclaredParentMismatch)`
    /// - verified successor == current -> `Unchanged(NoVerifiedChange)`
    /// - otherwise advances and returns the new root.
    pub fn try_advance(
        &mut self,
        declared_parent_root: Sha256Digest,
        verified_successor_root: Sha256Digest,
    ) -> SuccessorOutcome {
        if declared_parent_root != self.current_root {
            return SuccessorOutcome::Unchanged {
                reason: SuccessorUnchangedReason::DeclaredParentMismatch,
            };
        }
        if verified_successor_root == self.current_root {
            return SuccessorOutcome::Unchanged {
                reason: SuccessorUnchangedReason::NoVerifiedChange,
            };
        }
        self.current_root = verified_successor_root;
        SuccessorOutcome::Advanced {
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
pub struct SuccessorRecord {
    pub record_version: u16,
    pub declared_parent_root: Sha256Digest,
    pub verified_successor_root: Sha256Digest,
    pub advanced: bool,
    pub authority: String,
    pub abi_version: String,
}

impl SuccessorRecord {
    pub fn new(
        declared_parent_root: Sha256Digest,
        verified_successor_root: Sha256Digest,
        advanced: bool,
        authority: impl Into<String>,
    ) -> Result<Self, IdentityKernelError> {
        let record = Self {
            record_version: CONTRACT_VERSION,
            declared_parent_root,
            verified_successor_root,
            advanced,
            authority: authority.into(),
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.record_version != CONTRACT_VERSION {
            return Err(IdentityKernelError::InvalidEventRecord(format!(
                "unsupported record version {}",
                self.record_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.authority.is_empty() {
            return Err(IdentityKernelError::InvalidEventRecord(
                "authority must be nonempty".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::SuccessorRecord, ROOTED_ABI_VERSION, &value)
    }

    pub fn record_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::SuccessorRecord, ROOTED_ABI_VERSION, &bytes)
    }
}

// ---------------------------------------------------------------------------
// Harness contract (ZS-CONTRACT-003).
// ---------------------------------------------------------------------------

/// Serialization scheme for tool arguments/results across the harness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializationScheme {
    CanonicalJson,
    CompactRefs,
}

/// Message ordering guarantee the harness promises for tool calls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrdering {
    StrictCallOrder,
    CompletionPermitsReordering,
}

/// Transcript policy: what the harness records about a session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptPolicy {
    FullRecording,
    DecisionsAndResultsOnly,
    None,
}

/// Cancellation semantics the harness guarantees for in-flight calls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationSemantics {
    CooperativeAtCallBoundaries,
    HardDeadlineOnly,
}

/// The harness contract: tool serialization, message ordering, transcript
/// policy, cancellation semantics, the native tool set digest, and the
/// adapter renderer version. Any change alters the contract root and
/// invalidates dependent certificates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessContract {
    pub contract_version: u16,
    pub harness_name: String,
    pub serialization: SerializationScheme,
    pub message_ordering: MessageOrdering,
    pub transcript_policy: TranscriptPolicy,
    pub cancellation_semantics: CancellationSemantics,
    pub native_tool_set_digest: Sha256Digest,
    pub adapter_renderer_version: u16,
    pub abi_version: String,
}

impl HarnessContract {
    pub fn new(
        harness_name: impl Into<String>,
        serialization: SerializationScheme,
        message_ordering: MessageOrdering,
        transcript_policy: TranscriptPolicy,
        cancellation_semantics: CancellationSemantics,
        native_tool_set_digest: Sha256Digest,
        adapter_renderer_version: u16,
    ) -> Result<Self, IdentityKernelError> {
        let contract = Self {
            contract_version: CONTRACT_VERSION,
            harness_name: harness_name.into(),
            serialization,
            message_ordering,
            transcript_policy,
            cancellation_semantics,
            native_tool_set_digest,
            adapter_renderer_version,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.contract_version != CONTRACT_VERSION {
            return Err(IdentityKernelError::InvalidTaskContract(format!(
                "unsupported contract version {}",
                self.contract_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.harness_name.is_empty() {
            return Err(IdentityKernelError::InvalidTaskContract(
                "harness_name must be nonempty".into(),
            ));
        }
        if self.adapter_renderer_version == 0 {
            return Err(IdentityKernelError::InvalidTaskContract(
                "adapter_renderer_version must be nonzero".into(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::TaskContract, ROOTED_ABI_VERSION, &value)
    }

    pub fn contract_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::TaskContract, ROOTED_ABI_VERSION, &bytes)
    }
}

// ---------------------------------------------------------------------------
// Rooted ABI migration receipt (ZS-KERNEL-007).
// ---------------------------------------------------------------------------

/// Max canonical bytes for one migration receipt payload.
pub const MIGRATION_RECEIPT_MAX_CANONICAL_BYTES: usize = 64 * 1024;
pub const MIGRATION_RECEIPT_MAX_REASON_BYTES: usize = 512;

/// A rooted receipt for migrating one legacy rooted object into the current
/// ABI (ZS-KERNEL-007). The receipt pins four facts under one root:
///
/// - the legacy object's class, declared ABI version, canonical bytes, and
///   recorded root (verified against the versioned `root_preimage` with the
///   legacy ABI tag -- legacy payloads are not re-canonicalized);
/// - the replacement object's class and rooted bytes (verified through
///   [`canonical_object_bytes`] + [`object_root`], the only current path);
/// - a real ABI version change (source != target);
/// - a nonempty migration reason.
///
/// Incompatible-version mismatches still fail closed -- this receipt is the
/// machinery for *recording* a deliberate, audited migration, not a backdoor
/// around version checks. The receipt itself is rootable under
/// [`ObjectClass::MigrationReceipt`] like every other object class.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootedAbiMigrationReceipt {
    pub receipt_version: u16,
    pub source_class: ObjectClass,
    pub source_abi_version: String,
    pub source_canonical_bytes_hex: String,
    pub source_root: Sha256Digest,
    pub target_class: ObjectClass,
    pub target_abi_version: String,
    pub target_canonical_bytes_hex: String,
    pub target_root: Sha256Digest,
    pub migration_reason: String,
    pub abi_version: String,
}

impl RootedAbiMigrationReceipt {
    /// Migrate a legacy rooted object into the current ABI.
    ///
    /// `source_canonical_bytes` are the legacy object's own canonical bytes
    /// (already canonical under its legacy ABI); the source root must match
    /// `sha256(root_preimage(source_class, source_abi_version, source_bytes))`.
    /// `target_value` is re-canonicalized and rooted through the current path.
    pub fn new(
        source_class: ObjectClass,
        source_abi_version: impl Into<String>,
        source_canonical_bytes: &[u8],
        source_root: Sha256Digest,
        target_class: ObjectClass,
        target_value: &Value,
        migration_reason: impl Into<String>,
    ) -> Result<Self, IdentityKernelError> {
        let source_abi_version = source_abi_version.into();
        let migration_reason = migration_reason.into();
        if source_abi_version == ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::MigrationWithoutAbiChange);
        }
        if migration_reason.trim().is_empty()
            || migration_reason.len() > MIGRATION_RECEIPT_MAX_REASON_BYTES
        {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "migration_reason must be nonempty and bounded".into(),
            ));
        }
        if source_canonical_bytes.is_empty()
            || source_canonical_bytes.len() > MIGRATION_RECEIPT_MAX_CANONICAL_BYTES
        {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "source canonical bytes must be nonempty and bounded".into(),
            ));
        }
        let actual_source = Sha256Digest::from_bytes(sha256(&root_preimage(
            source_class,
            &source_abi_version,
            source_canonical_bytes,
        )));
        if actual_source != source_root {
            return Err(IdentityKernelError::SourceRootMismatch);
        }
        let target_canonical =
            canonical_object_bytes(target_class, ROOTED_ABI_VERSION, target_value)?;
        let target_root =
            object_root(target_class, ROOTED_ABI_VERSION, &target_canonical)?;
        let receipt = Self {
            receipt_version: CONTRACT_VERSION,
            source_class,
            source_abi_version,
            source_canonical_bytes_hex: hex_encode(source_canonical_bytes),
            source_root,
            target_class,
            target_abi_version: ROOTED_ABI_VERSION.to_owned(),
            target_canonical_bytes_hex: hex_encode(&target_canonical),
            target_root,
            migration_reason,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), IdentityKernelError> {
        if self.receipt_version != CONTRACT_VERSION {
            return Err(IdentityKernelError::InvalidMigrationReceipt(format!(
                "unsupported receipt version {}",
                self.receipt_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.target_abi_version != ROOTED_ABI_VERSION {
            return Err(IdentityKernelError::LegacyTargetAbi(
                self.target_abi_version.clone(),
            ));
        }
        if self.source_abi_version == self.target_abi_version {
            return Err(IdentityKernelError::MigrationWithoutAbiChange);
        }
        if self.migration_reason.trim().is_empty()
            || self.migration_reason.len() > MIGRATION_RECEIPT_MAX_REASON_BYTES
        {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "migration_reason must be nonempty and bounded".into(),
            ));
        }
        let source_bytes = hex_decode(&self.source_canonical_bytes_hex).ok_or_else(|| {
            IdentityKernelError::InvalidMigrationReceipt(
                "source_canonical_bytes_hex is not valid hex".into(),
            )
        })?;
        if source_bytes.is_empty()
            || source_bytes.len() > MIGRATION_RECEIPT_MAX_CANONICAL_BYTES
        {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "source canonical bytes must be nonempty and bounded".into(),
            ));
        }
        if self.source_root == Sha256Digest::ZERO || self.target_root == Sha256Digest::ZERO {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "source and target roots must be nonzero".into(),
            ));
        }
        // Re-verify the legacy root against the recorded legacy preimage.
        let actual_source = Sha256Digest::from_bytes(sha256(&root_preimage(
            self.source_class,
            &self.source_abi_version,
            &source_bytes,
        )));
        if actual_source != self.source_root {
            return Err(IdentityKernelError::SourceRootMismatch);
        }
        let target_bytes = hex_decode(&self.target_canonical_bytes_hex).ok_or_else(|| {
            IdentityKernelError::InvalidMigrationReceipt(
                "target_canonical_bytes_hex is not valid hex".into(),
            )
        })?;
        if target_bytes.is_empty() || target_bytes.len() > MIGRATION_RECEIPT_MAX_CANONICAL_BYTES {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "target canonical bytes must be nonempty and bounded".into(),
            ));
        }
        if object_root(self.target_class, ROOTED_ABI_VERSION, &target_bytes)?
            != self.target_root
        {
            return Err(IdentityKernelError::TargetRootMismatch);
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdentityKernelError> {
        let value = serde_json::to_value(self)
            .map_err(|error| IdentityKernelError::NonCanonicalBytes(error.to_string()))?;
        canonical_object_bytes(ObjectClass::MigrationReceipt, ROOTED_ABI_VERSION, &value)
    }

    pub fn receipt_root(&self) -> Result<Sha256Digest, IdentityKernelError> {
        let bytes = self.canonical_bytes()?;
        object_root(ObjectClass::MigrationReceipt, ROOTED_ABI_VERSION, &bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, IdentityKernelError> {
        if bytes.is_empty() || bytes.len() > MIGRATION_RECEIPT_MAX_CANONICAL_BYTES {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "receipt bytes must be nonempty and bounded".into(),
            ));
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| IdentityKernelError::InvalidMigrationReceipt(error.to_string()))?;
        let receipt: Self = serde_json::from_value(value).map_err(|error| {
            IdentityKernelError::InvalidMigrationReceipt(error.to_string())
        })?;
        receipt.validate()?;
        let canonical = receipt.canonical_bytes()?;
        if canonical != bytes {
            return Err(IdentityKernelError::InvalidMigrationReceipt(
                "receipt bytes are not canonical sorted-key JSON".into(),
            ));
        }
        Ok(receipt)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect()
}

