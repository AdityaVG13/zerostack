#![forbid(unsafe_code)]

//! W9-E exact first and incremental expansion for one demand family
//! (`zerostack-rybb`).
//!
//! # The family: exact scenario closure (S3 task closure)
//!
//! One narrow, adjudicatable W9-E request family, end to end in ZeroStack:
//!
//! 1. **Demand compilation** (`compile_demand`): a request names exactly one
//!    declared demand scenario from the W8 project image plus a projection
//!    (atom roots inside that scenario's envelope). The demand plan is the
//!    scenario's full multi-file envelope; the projection is the exact view
//!    the first expansion returns. A primary file can never stand in for the
//!    multi-file closure.
//! 2. **Completeness check through published GraphZero inputs**
//!    (`check_completeness`): the hub consumes the published
//!    [`GraphZeroCompletenessInput`] envelope (coverage universe with
//!    trivalent per-atom coverage over one index) and emits a total
//!    `Safe`/`Unsafe`/`Unknown` verdict plus a V7 shadow certificate
//!    (`zerostack-4lfp`) whose root binds evidence, scope, contract,
//!    checker identity/version, and the resource ledger.
//! 3. **Issuance** ([`W9eRoute::compile_and_check`]): only a `Safe` fold of
//!    image validity and graph coverage issues a [`SafeExpandHandle`]
//!    (`zerostack-qg2a`). `Unsafe`/`Unknown` refuse with typed reasons;
//!    nothing is ever labeled complete on missing evidence.
//! 4. **Exactly one first expansion** ([`W9eRoute::expand_first`]): the
//!    handle is revalidated against live hub state, then the projection is
//!    returned root/projection exact. A second first expansion on the same
//!    handle is refused.
//! 5. **Continuation-bound incremental deltas**
//!    ([`W9eRoute::expand_delta`]): a sequence-bound continuation token
//!    appends only new (never-before-expanded) atoms from the certified
//!    envelope; every delta revalidates the live handle first.
//!
//! # Laws
//!
//! - **One grammar.** The only target reference is a scenario id plus atom
//!   roots that must resolve inside that scenario's declared envelope. There
//!   is no second target-ref grammar and no broad model-visible discovery:
//!   the model never sees an `ls`/`grep`/probe, and every lookup is ledged.
//! - **False-complete is a blocker.** The demand must *equal* the coverage
//!   universe and every demanded atom must be positively covered and
//!   L2-valid. A demand that under-declares the graph's coverage
//!   (`coverage_exceeds_demand`), a positively uncovered atom
//!   (`atom_not_covered`), an L2-invalid atom, or a protected atom being
//!   demanded are all `Unsafe`. Missing coverage, unknown coverage, an
//!   unknown envelope, or a demanded atom with no image record are
//!   `Unknown`. Nothing missing is ever labeled complete.
//! - **First attempt only.** The checker is total and never retries; a
//!   retried check (`attempt_count != 1`) refuses issuance, and a hidden
//!   retry observed after issue revokes the live handle.
//! - **Root/projection exact.** The first expansion returns exactly the
//!   projection atoms, and the returned set's root must equal the permit's
//!   projection root.
//! - **New atoms only.** A continuation delta may only append atoms that are
//!   in the certified envelope and not yet expanded; replaying a stale
//!   continuation is refused.
//! - **Bounded and ledged.** Every collection is bounded (the family
//!   certifies at most [`MAX_CERTIFIED_ATOMS`] atoms, matching the V7
//!   evidence-item bound), and every byte or lookup is recorded in a bounded
//!   [`ExpandLedger`] with `exact`/`estimate`/`unknown` measurement sources
//!   plus native-baseline comparison fields.
//!
//! GraphZero source is not edited from this module; the checker consumes the
//! published input envelope only.

use std::collections::{BTreeMap, BTreeSet};
use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zero_abi::{
    CheckerIdentity, CompletenessEvidence, EtnfError, EvidenceItem, ExpandOutcome,
    ExplicitFallback, FallbackKind, Falsifier, FiniteWitness, LiveCompleteness, LiveExpandState,
    ObjectClass, ProposedAuthorityTransition, ProposedTransitionKind, ROOTED_ABI_VERSION,
    ResourceLedger, RootedEvidence, SafeExpandHandle, SafeExpandIssueRequest, SafeExpandIssuer,
    SafetyVerdict, Sha256Digest, V7ShadowReport, canonical_json, canonical_object_bytes,
    object_root, sha256, sha256_hex,
};

use crate::project_image::{ProjectImageManifest, ValidityClass};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Wire schema of this module's route artifacts.
pub const DEMAND_EXPAND_SCHEMA_VERSION: &str = "zerostack.w9e.demand_expand.v1";
/// Published GraphZero completeness-input envelope schema (the checker
/// consumes this shape; GraphZero source is not edited from here).
pub const GRAPHZERO_COMPLETENESS_INPUT_SCHEMA_VERSION: &str =
    "zerostack.graphzero.completeness_input.v1";
/// Stable name of the implemented demand family.
pub const W9E_FAMILY_NAME: &str = "exact_scenario_closure";
/// Checker identity and version bound into every completeness certificate.
pub const CHECKER_ID: &str = "zerostack.w9e.completeness.total";
pub const CHECKER_VERSION: &str = "1.0.0";
/// Renderer contract identity bound into every handle (exact-atoms renderer).
pub const RENDERER_NAME: &str = "zerostack.w9e.renderer.exact_atoms";

/// Maximum certified atoms per demand. Bounded by the V7 evidence-item cap
/// ([`zero_abi::etnf::ETNF_MAX_EVIDENCE_ITEMS`]): the certificate binds one
/// evidence item per coverage record, so the family is narrow by
/// construction.
pub const MAX_CERTIFIED_ATOMS: usize = 128;
/// Maximum coverage-universe size the checker accepts (== certified bound).
pub const MAX_COVERAGE_UNIVERSE_ATOMS: usize = 128;
/// Maximum projection size (atoms returned by the first expansion).
pub const MAX_PROJECTION_ATOMS: usize = 128;
/// Maximum atoms in one continuation delta.
pub const MAX_DELTA_ATOMS: usize = 128;
/// Maximum protected atoms in one protected scope.
pub const MAX_PROTECTED_ATOMS: usize = 4096;
/// Maximum bytes of any bound string (scenario id, tenant, index version,
/// task id, scope id).
pub const MAX_DEMAND_STRING_BYTES: usize = 256;
/// Maximum rows of one bounded expand ledger.
pub const EXPAND_LEDGER_MAX_ROWS: usize = 32;

/// Domain separation tags for derived roots.
const DEMAND_PLAN_DOMAIN: &[u8] = b"zerostack.w9e.demand_plan\0";
const PROJECTION_DOMAIN: &[u8] = b"zerostack.w9e.projection\0";
const SCOPE_DOMAIN: &[u8] = b"zerostack.w9e.protected_scope\0";
const REQUEST_DOMAIN: &[u8] = b"zerostack.w9e.demand_request\0";
const DELTA_DOMAIN: &[u8] = b"zerostack.w9e.incremental_delta\0";
const ISSUE_NONCE_DOMAIN: &[u8] = b"zerostack.w9e.issue_nonce\0";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Fail-closed error for the whole W9-E demand family.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DemandError {
    /// A bound string field was empty.
    EmptyString(&'static str),
    /// A bound collection exceeded its cap.
    BoundExceeded {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    /// A bound string field carried a control character.
    ControlCharacter(&'static str),
    /// A required root was the zero digest.
    ZeroRoot(&'static str),
    /// The request referenced a scenario the image does not declare.
    ScenarioNotFound { scenario_id: String },
    /// The projection is empty; a first expansion needs a nonempty view.
    EmptyProjection,
    /// A projection atom is not inside the scenario envelope.
    ProjectionExceedsDemand { atom_root: Sha256Digest },
    /// A delta atom is not in the certified envelope.
    DeltaAtomNotCertified { atom_root: Sha256Digest },
    /// A delta atom was already expanded (new atoms only).
    DeltaAtomAlreadyExpanded { atom_root: Sha256Digest },
    /// A delta atom is protected by the scope (defense in depth).
    DeltaAtomProtected { atom_root: Sha256Digest },
    /// A delta request carried no atoms.
    EmptyDelta,
    /// The continuation token is stale (sequence-bound).
    StaleContinuation {
        handle_id: Sha256Digest,
        expected: u64,
        actual: u64,
    },
    /// The session is exhausted: every certified atom is expanded.
    SessionExhausted { handle_id: Sha256Digest },
    /// No session exists for this handle id.
    UnknownHandle { handle_id: Sha256Digest },
    /// The handle was already first-expanded (exactly one first expansion).
    AlreadyFirstExpanded { handle_id: Sha256Digest },
    /// Internal root-consistency failure between the permit and session.
    SessionRootMismatch(&'static str),
    /// The returned atom set does not root to the permit's projection.
    ProjectionMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    /// SafeExpandHandle issuance refused (typed upstream error).
    HandleIssuance(String),
    /// Live revalidation returned `Unsafe`.
    RevalidationUnsafe { reasons: Vec<String> },
    /// Live revalidation returned `Unknown`.
    RevalidationUnknown { reasons: Vec<String> },
    /// The completeness certificate failed to build or bind.
    Certificate(String),
    /// The W8 project image rejected an input.
    Manifest(String),
    /// The published GraphZero input envelope was rejected.
    InvalidInput(String),
    /// Serialization failed.
    Serialization(String),
    /// Internal invariant violated.
    Internal(&'static str),
}

impl fmt::Display for DemandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyString(field) => write!(formatter, "w9e {field} must be nonempty"),
            Self::BoundExceeded { field, actual, maximum } => write!(
                formatter,
                "w9e {field} has {actual} items, maximum {maximum}"
            ),
            Self::ControlCharacter(field) => {
                write!(formatter, "w9e {field} must be free of control characters")
            }
            Self::ZeroRoot(field) => write!(formatter, "w9e requires a nonzero {field}"),
            Self::ScenarioNotFound { scenario_id } => {
                write!(formatter, "w9e scenario {scenario_id:?} not found in project image")
            }
            Self::EmptyProjection => write!(formatter, "w9e projection must be nonempty"),
            Self::ProjectionExceedsDemand { atom_root } => write!(
                formatter,
                "w9e projection atom {atom_root} is outside the scenario envelope"
            ),
            Self::DeltaAtomNotCertified { atom_root } => {
                write!(formatter, "w9e delta atom {atom_root} is not certified by the plan")
            }
            Self::DeltaAtomAlreadyExpanded { atom_root } => {
                write!(formatter, "w9e delta atom {atom_root} was already expanded")
            }
            Self::DeltaAtomProtected { atom_root } => {
                write!(formatter, "w9e delta atom {atom_root} is protected")
            }
            Self::EmptyDelta => write!(formatter, "w9e delta must be nonempty"),
            Self::StaleContinuation { handle_id, expected, actual } => write!(
                formatter,
                "w9e stale continuation for {handle_id}: expected seq {expected}, got {actual}"
            ),
            Self::SessionExhausted { handle_id } => {
                write!(formatter, "w9e session {handle_id} is exhausted; no new atoms remain")
            }
            Self::UnknownHandle { handle_id } => {
                write!(formatter, "w9e no session for handle {handle_id}")
            }
            Self::AlreadyFirstExpanded { handle_id } => write!(
                formatter,
                "w9e handle {handle_id} already performed its one first expansion"
            ),
            Self::SessionRootMismatch(field) => {
                write!(formatter, "w9e session {field} does not match the permit")
            }
            Self::ProjectionMismatch { expected, actual } => write!(
                formatter,
                "w9e returned atoms root to {actual}, projection requires {expected}"
            ),
            Self::HandleIssuance(detail) => write!(formatter, "w9e handle issuance refused: {detail}"),
            Self::RevalidationUnsafe { reasons } => {
                write!(formatter, "w9e live revalidation Unsafe: {reasons:?}")
            }
            Self::RevalidationUnknown { reasons } => {
                write!(formatter, "w9e live revalidation Unknown: {reasons:?}")
            }
            Self::Certificate(detail) => write!(formatter, "w9e certificate failure: {detail}"),
            Self::Manifest(detail) => write!(formatter, "w9e project image rejected input: {detail}"),
            Self::InvalidInput(detail) => {
                write!(formatter, "w9e invalid GraphZero completeness input: {detail}")
            }
            Self::Serialization(detail) => write!(formatter, "w9e serialization failure: {detail}"),
            Self::Internal(detail) => write!(formatter, "w9e internal invariant violated: {detail}"),
        }
    }
}

impl Error for DemandError {}

fn dem_err(msg: impl Into<String>) -> DemandError {
    DemandError::InvalidInput(msg.into())
}

fn validate_string(field: &'static str, value: &str) -> Result<(), DemandError> {
    if value.is_empty() {
        return Err(DemandError::EmptyString(field));
    }
    if value.len() > MAX_DEMAND_STRING_BYTES {
        return Err(DemandError::BoundExceeded {
            field,
            actual: value.len(),
            maximum: MAX_DEMAND_STRING_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(DemandError::ControlCharacter(field));
    }
    Ok(())
}

fn validate_atom_list(
    field: &'static str,
    atoms: &[Sha256Digest],
    maximum: usize,
) -> Result<(), DemandError> {
    if atoms.len() > maximum {
        return Err(DemandError::BoundExceeded {
            field,
            actual: atoms.len(),
            maximum,
        });
    }
    for atom in atoms {
        if *atom == Sha256Digest::ZERO {
            return Err(DemandError::ZeroRoot(field));
        }
    }
    Ok(())
}

fn sort_dedup_digests(atoms: &[Sha256Digest]) -> Vec<Sha256Digest> {
    let mut sorted: Vec<Sha256Digest> = atoms.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
}

/// `SHA-256(domain || canonical_json(value))` -- the only root derivation
/// path in this module.
fn domain_root(domain: &[u8], value: &Value) -> Sha256Digest {
    let canonical = canonical_json(value);
    let mut preimage = Vec::with_capacity(domain.len() + canonical.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(canonical.as_bytes());
    Sha256Digest::from_bytes(sha256(&preimage))
}

fn digest_hex_list(atoms: &[Sha256Digest]) -> Vec<String> {
    atoms.iter().map(|atom| atom.to_hex()).collect()
}

// ---------------------------------------------------------------------------
// The one target-ref grammar: scenario id + atom roots inside its envelope
// ---------------------------------------------------------------------------

/// A request in the single W9-E target-ref grammar: one scenario reference
/// plus a nonempty projection of atom roots that must resolve inside that
/// scenario's declared envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandRequest {
    pub scenario_id: String,
    /// Sorted, deduplicated atom roots; nonempty; every root inside the
    /// scenario envelope (checked at compile).
    pub projection_atoms: Vec<Sha256Digest>,
    /// Derived root over `scenario_id` and the projection.
    pub request_root: Sha256Digest,
}

impl DemandRequest {
    pub fn new(scenario_id: String, projection_atoms: Vec<Sha256Digest>) -> Result<Self, DemandError> {
        validate_string("scenario_id", &scenario_id)?;
        if projection_atoms.is_empty() {
            return Err(DemandError::EmptyProjection);
        }
        validate_atom_list("projection_atoms", &projection_atoms, MAX_PROJECTION_ATOMS)?;
        let projection_atoms = sort_dedup_digests(&projection_atoms);
        let request_root = domain_root(
            REQUEST_DOMAIN,
            &serde_json::json!({
                "scenario_id": scenario_id,
                "projection": digest_hex_list(&projection_atoms),
            }),
        );
        let request = Self {
            scenario_id,
            projection_atoms,
            request_root,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        validate_string("scenario_id", &self.scenario_id)?;
        if self.projection_atoms.is_empty() {
            return Err(DemandError::EmptyProjection);
        }
        validate_atom_list("projection_atoms", &self.projection_atoms, MAX_PROJECTION_ATOMS)?;
        let expected = domain_root(
            REQUEST_DOMAIN,
            &serde_json::json!({
                "scenario_id": self.scenario_id,
                "projection": digest_hex_list(&self.projection_atoms),
            }),
        );
        if self.request_root != expected {
            return Err(DemandError::Internal("request_root does not bind its fields"));
        }
        Ok(())
    }
}

/// The protected scope: a rooted set of atom roots that expansion must never
/// return. Demand compilation refuses any demanded protected atom.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedScope {
    pub scope_id: String,
    /// Sorted, deduplicated protected atom roots.
    pub protected_atoms: Vec<Sha256Digest>,
    /// Derived root over `scope_id` and the protected atoms.
    pub scope_root: Sha256Digest,
}

impl ProtectedScope {
    pub fn new(scope_id: String, protected_atoms: Vec<Sha256Digest>) -> Result<Self, DemandError> {
        validate_string("scope_id", &scope_id)?;
        validate_atom_list("protected_atoms", &protected_atoms, MAX_PROTECTED_ATOMS)?;
        let protected_atoms = sort_dedup_digests(&protected_atoms);
        let scope_root = domain_root(
            SCOPE_DOMAIN,
            &serde_json::json!({
                "scope_id": scope_id,
                "protected_atoms": digest_hex_list(&protected_atoms),
            }),
        );
        let scope = Self {
            scope_id,
            protected_atoms,
            scope_root,
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        validate_string("scope_id", &self.scope_id)?;
        validate_atom_list("protected_atoms", &self.protected_atoms, MAX_PROTECTED_ATOMS)?;
        let expected = domain_root(
            SCOPE_DOMAIN,
            &serde_json::json!({
                "scope_id": self.scope_id,
                "protected_atoms": digest_hex_list(&self.protected_atoms),
            }),
        );
        if self.scope_root != expected {
            return Err(DemandError::Internal("scope_root does not bind its fields"));
        }
        Ok(())
    }
}

/// A continuation delta request: atom roots to append. All atoms must be in
/// the certified envelope and not yet expanded (checked by the route).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalDeltaRequest {
    pub atom_refs: Vec<Sha256Digest>,
}

impl IncrementalDeltaRequest {
    pub fn new(atom_refs: Vec<Sha256Digest>) -> Result<Self, DemandError> {
        if atom_refs.is_empty() {
            return Err(DemandError::EmptyDelta);
        }
        validate_atom_list("atom_refs", &atom_refs, MAX_DELTA_ATOMS)?;
        let atom_refs = sort_dedup_digests(&atom_refs);
        let request = Self { atom_refs };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        if self.atom_refs.is_empty() {
            return Err(DemandError::EmptyDelta);
        }
        validate_atom_list("atom_refs", &self.atom_refs, MAX_DELTA_ATOMS)
    }
}

// ---------------------------------------------------------------------------
// Demand plan (the certified multi-file closure)
// ---------------------------------------------------------------------------

/// The compiled demand: the scenario's full certified envelope plus the
/// requested projection. The plan root binds the envelope; the projection
/// root binds the first-expansion view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandPlan {
    pub scenario_id: String,
    /// Sorted, deduplicated certified atom roots (the full multi-file
    /// envelope; never a primary-file subset).
    pub demanded_atoms: Vec<Sha256Digest>,
    pub demand_weight: u64,
    /// Sorted projection atoms; a subset of `demanded_atoms`.
    pub projection_atoms: Vec<Sha256Digest>,
    /// Derived root over the envelope.
    pub plan_root: Sha256Digest,
    /// Derived root over the projection.
    pub projection_root: Sha256Digest,
}

impl DemandPlan {
    pub fn new(
        scenario_id: String,
        demanded_atoms: Vec<Sha256Digest>,
        demand_weight: u64,
        projection_atoms: Vec<Sha256Digest>,
    ) -> Result<Self, DemandError> {
        validate_string("scenario_id", &scenario_id)?;
        if demanded_atoms.is_empty() {
            return Err(dem_err("demand envelope must be nonempty"));
        }
        validate_atom_list("demanded_atoms", &demanded_atoms, MAX_CERTIFIED_ATOMS)?;
        if projection_atoms.is_empty() {
            return Err(DemandError::EmptyProjection);
        }
        validate_atom_list("projection_atoms", &projection_atoms, MAX_PROJECTION_ATOMS)?;
        let demanded_atoms = sort_dedup_digests(&demanded_atoms);
        let projection_atoms = sort_dedup_digests(&projection_atoms);
        let demand_set: BTreeSet<Sha256Digest> = demanded_atoms.iter().copied().collect();
        for atom in &projection_atoms {
            if !demand_set.contains(atom) {
                return Err(DemandError::ProjectionExceedsDemand { atom_root: *atom });
            }
        }
        let plan_root = domain_root(
            DEMAND_PLAN_DOMAIN,
            &serde_json::json!({
                "demand_weight": demand_weight,
                "demanded_atoms": digest_hex_list(&demanded_atoms),
                "scenario_id": scenario_id,
            }),
        );
        let projection_root = projection_root_of(&projection_atoms);
        let plan = Self {
            scenario_id,
            demanded_atoms,
            demand_weight,
            projection_atoms,
            plan_root,
            projection_root,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        validate_string("scenario_id", &self.scenario_id)?;
        if self.demanded_atoms.is_empty() {
            return Err(dem_err("demand envelope must be nonempty"));
        }
        validate_atom_list("demanded_atoms", &self.demanded_atoms, MAX_CERTIFIED_ATOMS)?;
        validate_atom_list("projection_atoms", &self.projection_atoms, MAX_PROJECTION_ATOMS)?;
        let expected_plan = domain_root(
            DEMAND_PLAN_DOMAIN,
            &serde_json::json!({
                "demand_weight": self.demand_weight,
                "demanded_atoms": digest_hex_list(&self.demanded_atoms),
                "scenario_id": self.scenario_id,
            }),
        );
        let expected_projection = projection_root_of(&self.projection_atoms);
        if self.plan_root != expected_plan {
            return Err(DemandError::Internal("plan_root does not bind its fields"));
        }
        if self.projection_root != expected_projection {
            return Err(DemandError::Internal("projection_root does not bind its fields"));
        }
        let demand_set: BTreeSet<Sha256Digest> = self.demanded_atoms.iter().copied().collect();
        for atom in &self.projection_atoms {
            if !demand_set.contains(atom) {
                return Err(DemandError::ProjectionExceedsDemand { atom_root: *atom });
            }
        }
        Ok(())
    }

    pub fn contains(&self, atom_root: Sha256Digest) -> bool {
        self.demanded_atoms.binary_search(&atom_root).is_ok()
    }

    pub fn certified_atoms(&self) -> usize {
        self.demanded_atoms.len()
    }
}

/// The deterministic root of an atom set under the projection domain.
/// `expand_first` re-derives this over the returned atoms to prove
/// root/projection exactness.
pub fn projection_root_of(projection_atoms: &[Sha256Digest]) -> Sha256Digest {
    domain_root(
        PROJECTION_DOMAIN,
        &serde_json::json!({ "atoms": digest_hex_list(projection_atoms) }),
    )
}

// ---------------------------------------------------------------------------
// Published GraphZero completeness inputs
// ---------------------------------------------------------------------------

/// One atom's coverage record from the published coverage universe. `None`
/// means the checker evaluated the atom but its coverage is unknown --
/// `Unknown`, never a guessed subset.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageAtom {
    pub atom_root: Sha256Digest,
    pub covered: Option<bool>,
}

/// The published GraphZero completeness-input envelope the hub checker
/// consumes. GraphZero source is not edited from this module; production
/// GraphZero produces this shape over its coverage index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphZeroCompletenessInput {
    pub schema_version: String,
    /// The coverage index the universe was evaluated against.
    pub index_root: Sha256Digest,
    pub index_version: String,
    pub task_id: String,
    /// Every atom the checker evaluated for this task, sorted by root.
    /// Empty means no coverage evidence at all (Unknown).
    pub coverage_universe: Vec<CoverageAtom>,
    /// Must be exactly `1`: the completeness check is first-attempt only.
    pub attempt_count: u64,
}

impl GraphZeroCompletenessInput {
    pub fn new(
        index_root: Sha256Digest,
        index_version: String,
        task_id: String,
        coverage_universe: Vec<CoverageAtom>,
        attempt_count: u64,
    ) -> Result<Self, DemandError> {
        let input = Self {
            schema_version: GRAPHZERO_COMPLETENESS_INPUT_SCHEMA_VERSION.to_owned(),
            index_root,
            index_version,
            task_id,
            coverage_universe,
            attempt_count,
        };
        input.validate()?;
        Ok(input)
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        if self.schema_version != GRAPHZERO_COMPLETENESS_INPUT_SCHEMA_VERSION {
            return Err(DemandError::InvalidInput(format!(
                "unexpected schema_version {:?}",
                self.schema_version
            )));
        }
        if self.index_root == Sha256Digest::ZERO {
            return Err(DemandError::ZeroRoot("index_root"));
        }
        validate_string("index_version", &self.index_version)?;
        // `task_id` is bound tighter than the generic string bound because it
        // is embedded in a V7 witness fact ("task:<id>"), which allows at
        // most ETNF_MAX_ID_BYTES bytes.
        if self.task_id.is_empty() {
            return Err(DemandError::EmptyString("task_id"));
        }
        if self.task_id.len() > zero_abi::ETNF_MAX_ID_BYTES {
            return Err(DemandError::BoundExceeded {
                field: "task_id",
                actual: self.task_id.len(),
                maximum: zero_abi::ETNF_MAX_ID_BYTES,
            });
        }
        if self.task_id.chars().any(char::is_control) {
            return Err(DemandError::ControlCharacter("task_id"));
        }
        if self.coverage_universe.len() > MAX_COVERAGE_UNIVERSE_ATOMS {
            return Err(DemandError::BoundExceeded {
                field: "coverage_universe",
                actual: self.coverage_universe.len(),
                maximum: MAX_COVERAGE_UNIVERSE_ATOMS,
            });
        }
        let mut previous: Option<Sha256Digest> = None;
        for atom in &self.coverage_universe {
            if atom.atom_root == Sha256Digest::ZERO {
                return Err(DemandError::ZeroRoot("coverage_universe.atom_root"));
            }
            if let Some(previous) = previous {
                if atom.atom_root <= previous {
                    return Err(DemandError::InvalidInput(
                        "coverage_universe must be sorted by atom_root with no duplicates".into(),
                    ));
                }
            }
            previous = Some(atom.atom_root);
        }
        if self.attempt_count == 0 {
            return Err(DemandError::InvalidInput("attempt_count must be nonzero".into()));
        }
        Ok(())
    }
}

/// The digest of one coverage record; every certificate evidence item binds
/// exactly this digest, so the certificate transitively binds each record.
fn coverage_record_digest(atom: Sha256Digest, covered: Option<bool>) -> String {
    sha256_hex(
        canonical_json(&serde_json::json!({ "atom": atom.to_hex(), "covered": covered })).as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// Completeness check (total, first-attempt, certificate-emitting)
// ---------------------------------------------------------------------------

/// The total outcome of one completeness check through the published
/// GraphZero inputs. The [`V7ShadowReport`] is always produced (observable
/// evidence); its certificate exists exactly when the verdict is `Safe`.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletenessCheck {
    pub verdict: SafetyVerdict,
    pub report: V7ShadowReport,
    /// Parsed certificate root; `Some` exactly when `verdict` is `Safe`.
    pub certificate_root: Option<Sha256Digest>,
    /// Index lookups the check consumed (backend-private, ledged).
    pub backend_work: u64,
}

/// Deterministic coverage facts that are asserted only under `Safe`.
const SAFE_WITNESS_FACTS: [&str; 4] = [
    "first_attempt",
    "demand_equals_coverage_universe",
    "all_atoms_covered",
    "projection_within_demand",
];

/// Run the total completeness check over the published GraphZero inputs.
///
/// Fail-closed laws:
/// - Empty universe -> `Unknown` (`no_coverage_evidence`).
/// - `attempt_count != 1` -> `Unsafe` (`hidden_retry`).
/// - Coverage record `covered == None` -> `Unknown` (`coverage_unknown`).
/// - Coverage record `covered == Some(false)` -> `Unsafe` (`atom_not_covered`).
/// - A universe atom absent from the demand -> `Unsafe`
///   (`coverage_exceeds_demand`): the graph positively establishes the
///   demand under-declares the task closure (false-complete blocker).
/// - A demanded atom absent from the universe -> `Unknown`
///   (`demanded_atom_uncovered`): no evidence about it exists.
///
/// Verdicts fold under the ZS-KERNEL-004 meet (`Unsafe` dominates `Unknown`
/// dominates `Safe`).
pub fn check_completeness(
    input: &GraphZeroCompletenessInput,
    plan: &DemandPlan,
    scope: &ProtectedScope,
) -> Result<CompletenessCheck, DemandError> {
    input.validate()?;
    plan.validate()?;
    scope.validate()?;

    let mut verdicts: Vec<SafetyVerdict> = Vec::new();

    if input.attempt_count != 1 {
        verdicts.push(SafetyVerdict::Unsafe {
            reasons: vec!["hidden_retry".to_owned()],
        });
    }
    if input.coverage_universe.is_empty() {
        verdicts.push(SafetyVerdict::Unknown {
            reasons: vec!["no_coverage_evidence".to_owned()],
        });
    }

    let demand_set: BTreeSet<Sha256Digest> = plan.demanded_atoms.iter().copied().collect();
    let mut universe_set: BTreeSet<Sha256Digest> = BTreeSet::new();

    for atom in &input.coverage_universe {
        universe_set.insert(atom.atom_root);
        if !demand_set.contains(&atom.atom_root) {
            verdicts.push(SafetyVerdict::Unsafe {
                reasons: vec![format!("coverage_exceeds_demand:{}", atom.atom_root.to_hex())],
            });
            continue;
        }
        match atom.covered {
            None => verdicts.push(SafetyVerdict::Unknown {
                reasons: vec![format!("coverage_unknown:{}", atom.atom_root.to_hex())],
            }),
            Some(false) => verdicts.push(SafetyVerdict::Unsafe {
                reasons: vec![format!("atom_not_covered:{}", atom.atom_root.to_hex())],
            }),
            Some(true) => verdicts.push(SafetyVerdict::Safe),
        }
    }
    for atom in &plan.demanded_atoms {
        if !universe_set.contains(atom) {
            verdicts.push(SafetyVerdict::Unknown {
                reasons: vec![format!("demanded_atom_uncovered:{}", atom.to_hex())],
            });
        }
    }

    let verdict = SafetyVerdict::meet_all(verdicts);

    // Evidence: one coverage record per universe atom, anchored on the plan
    // root. The certificate root then binds the evidence root, which binds
    // every record.
    let evidence_items = input
        .coverage_universe
        .iter()
        .map(|atom| {
            EvidenceItem::new(
                format!("coverage:{}", atom.atom_root.to_hex()),
                coverage_record_digest(atom.atom_root, atom.covered),
            )
        })
        .collect::<Result<Vec<_>, EtnfError>>()
        .map_err(|error| DemandError::Certificate(error.to_string()))?;
    let evidence = RootedEvidence::new(plan.plan_root.to_hex(), evidence_items)
        .map_err(|error| DemandError::Certificate(error.to_string()))?;

    let checker = CheckerIdentity::new(CHECKER_ID, CHECKER_VERSION)
        .map_err(|error| DemandError::Certificate(error.to_string()))?;

    let mut witness_facts: Vec<String> = vec![
        format!("task:{}", input.task_id),
        "first_attempt".to_owned(),
    ];
    if verdict.grants_authority() {
        for fact in SAFE_WITNESS_FACTS {
            witness_facts.push(fact.to_owned());
        }
    }
    let witness = FiniteWitness::new(witness_facts)
        .map_err(|error| DemandError::Certificate(error.to_string()))?;

    let transition = if verdict.grants_authority() {
        Some(
            ProposedAuthorityTransition::new(
                ProposedTransitionKind::KeepProofLive,
                plan.plan_root.to_hex(),
            )
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        )
    } else {
        None
    };

    let fallback = ExplicitFallback::new(
        FallbackKind::FrozenRawBaseline,
        "Unknown coverage falls back to the frozen native baseline; a subset is never labeled complete",
    )
    .map_err(|error| DemandError::Certificate(error.to_string()))?;

    let falsifiers = vec![
        Falsifier::new("W9E-f1", "demanded atom missing from coverage universe")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f2", "coverage atom missing from demand plan")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f3", "coverage atom positively not covered")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f4", "coverage status unknown")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f5", "projection exceeds the certified envelope")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f6", "protected atom demanded")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f7", "delta re-expands a previously expanded atom")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
        Falsifier::new("W9E-f8", "continuation replay (stale session)")
            .map_err(|error| DemandError::Certificate(error.to_string()))?,
    ];

    let canonical_input = canonical_json(
        &serde_json::to_value(input)
            .map_err(|error| DemandError::Serialization(error.to_string()))?,
    );
    let ledger = ResourceLedger::new(
        canonical_input.len() as u64,
        input.coverage_universe.len() as u64,
        1,
        true,
    );

    let report = V7ShadowReport::new(
        verdict.clone(),
        checker,
        scope.scope_root.to_hex(),
        plan.plan_root.to_hex(),
        evidence,
        witness,
        transition,
        fallback,
        falsifiers,
        ledger,
    )
    .map_err(|error| DemandError::Certificate(error.to_string()))?;

    let certificate_root = match (&report.certificate, verdict.grants_authority()) {
        (Some(certificate), true) => Some(
            Sha256Digest::from_hex(&certificate.root)
                .map_err(|error| DemandError::Certificate(error.to_string()))?,
        ),
        (None, true) => {
            return Err(DemandError::Internal(
                "Safe completeness check without certificate",
            ))
        }
        (_, false) => None,
    };

    // Backend-private index work: one evaluation per universe record plus
    // one membership check per demanded atom.
    let backend_work = input.coverage_universe.len() as u64 + plan.demanded_atoms.len() as u64;

    Ok(CompletenessCheck {
        verdict,
        report,
        certificate_root,
        backend_work,
    })
}

// ---------------------------------------------------------------------------
// Demand compilation (project image + request + protected scope)
// ---------------------------------------------------------------------------

/// Result of demand compilation: either a ready plan or a typed refusal.
#[derive(Clone, Debug, PartialEq)]
pub enum CompileOutcome {
    Ready(CompiledDemand),
    Refused(SafetyVerdict),
}

/// A compiled demand ready for certification.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledDemand {
    pub plan: DemandPlan,
    /// Image lookups consumed at compile time (backend-private, ledged).
    pub compile_backend_work: u64,
}

/// Compile demand from the W8 project image, one request, and the protected
/// scope.
///
/// Fail-closed laws:
/// - Scenario missing -> `Unknown` (`scenario_not_found:<id>`); scenario
///   with no declared envelope -> `Unknown` (`scenario_envelope_unknown`).
/// - Demanded atom with no image record -> `Unknown`
///   (`demanded_atom_missing_from_image`); no layer entry or unknown L2 ->
///   `Unknown` (`demanded_atom_layers_unknown`); L2-invalid -> `Unsafe`
///   (`demanded_atom_l2_invalid`).
/// - A demanded protected atom -> `Unsafe` (`protected_atom_demanded`).
/// - A projection atom outside the envelope -> `Unsafe`
///   (`projection_exceeds_demand`).
pub fn compile_demand(
    manifest: &ProjectImageManifest,
    request: &DemandRequest,
    scope: &ProtectedScope,
) -> Result<CompileOutcome, DemandError> {
    request.validate()?;
    scope.validate()?;
    manifest
        .validate()
        .map_err(|error| DemandError::Manifest(error.to_string()))?;

    let scenario = manifest
        .demand_scenarios
        .iter()
        .find(|scenario| scenario.scenario_id == request.scenario_id)
        .ok_or_else(|| DemandError::ScenarioNotFound {
            scenario_id: request.scenario_id.clone(),
        })?;

    if scenario.unknown_reason.is_some() || scenario.demanded_object_roots.is_empty() {
        return Ok(CompileOutcome::Refused(SafetyVerdict::Unknown {
            reasons: vec![format!(
                "scenario_envelope_unknown:{}",
                scenario.scenario_id
            )],
        }));
    }

    // Image lookups are counted and ledged; they are never model-visible
    // discovery.
    let mut backend_work: u64 = 1; // scenario resolution
    let mut verdicts: Vec<SafetyVerdict> = Vec::new();

    let image_set: BTreeSet<Sha256Digest> =
        manifest.exact_objects.iter().map(|object| object.digest).collect();
    let layer_map: BTreeMap<Sha256Digest, ValidityClass> = manifest
        .per_object_layers
        .iter()
        .map(|layer| (layer.object_root, layer.validity_class()))
        .collect();
    let scope_set: BTreeSet<Sha256Digest> = scope.protected_atoms.iter().copied().collect();

    for atom in &scenario.demanded_object_roots {
        backend_work += 2; // image membership + layer lookup
        if *atom == Sha256Digest::ZERO {
            return Err(DemandError::ZeroRoot("demanded_object_roots"));
        }
        if !image_set.contains(atom) {
            verdicts.push(SafetyVerdict::Unknown {
                reasons: vec![format!("demanded_atom_missing_from_image:{}", atom.to_hex())],
            });
            continue;
        }
        match layer_map.get(atom) {
            Some(ValidityClass::ValidResident) | Some(ValidityClass::ValidNotResident) => {
                verdicts.push(SafetyVerdict::Safe);
            }
            Some(ValidityClass::Invalid) => verdicts.push(SafetyVerdict::Unsafe {
                reasons: vec![format!("demanded_atom_l2_invalid:{}", atom.to_hex())],
            }),
            Some(ValidityClass::Unknown) | None => {
                verdicts.push(SafetyVerdict::Unknown {
                    reasons: vec![format!("demanded_atom_layers_unknown:{}", atom.to_hex())],
                });
            }
        }
        if scope_set.contains(atom) {
            verdicts.push(SafetyVerdict::Unsafe {
                reasons: vec![format!("protected_atom_demanded:{}", atom.to_hex())],
            });
        }
    }
    backend_work += scenario.demanded_object_roots.len() as u64; // scope membership

    // Membership via a set, not binary search: deserialized manifests are not
    // required to carry sorted scenario roots.
    let scenario_set: BTreeSet<Sha256Digest> =
        scenario.demanded_object_roots.iter().copied().collect();
    for atom in &request.projection_atoms {
        if !scenario_set.contains(atom) {
            verdicts.push(SafetyVerdict::Unsafe {
                reasons: vec![format!("projection_exceeds_demand:{}", atom.to_hex())],
            });
        }
    }

    let image_verdict = SafetyVerdict::meet_all(verdicts);
    if !image_verdict.grants_authority() {
        return Ok(CompileOutcome::Refused(image_verdict));
    }

    let plan = DemandPlan::new(
        scenario.scenario_id.clone(),
        scenario.demanded_object_roots.clone(),
        scenario.demand_weight,
        request.projection_atoms.clone(),
    )?;

    Ok(CompileOutcome::Ready(CompiledDemand {
        plan,
        compile_backend_work: backend_work,
    }))
}

// ---------------------------------------------------------------------------
// Bounded expand ledger + native baseline + adjudication metrics
// ---------------------------------------------------------------------------

/// One bounded ledger row. `measurement_source` is `exact`, `estimate`, or
/// `unknown`; nothing estimated is ever reported as exact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandLedgerRow {
    pub class: String,
    pub amount: u64,
    pub unit: String,
    pub measurement_source: String,
}

/// Bounded resource ledger of one expansion/delta (backend-private work,
/// visible bytes, retry count, native baseline comparison fields).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandLedger {
    pub rows: Vec<ExpandLedgerRow>,
}

impl ExpandLedger {
    pub fn empty() -> Self {
        Self { rows: Vec::new() }
    }

    /// Append one row; refuses beyond [`EXPAND_LEDGER_MAX_ROWS`].
    pub fn push(
        &mut self,
        class: impl Into<String>,
        amount: u64,
        unit: impl Into<String>,
        measurement_source: impl Into<String>,
    ) -> Result<(), DemandError> {
        if self.rows.len() >= EXPAND_LEDGER_MAX_ROWS {
            return Err(DemandError::BoundExceeded {
                field: "ledger.rows",
                actual: self.rows.len(),
                maximum: EXPAND_LEDGER_MAX_ROWS,
            });
        }
        self.rows.push(ExpandLedgerRow {
            class: class.into(),
            amount,
            unit: unit.into(),
            measurement_source: measurement_source.into(),
        });
        Ok(())
    }

    /// Sum of every row with the given class.
    pub fn total(&self, class: &str) -> u64 {
        self.rows
            .iter()
            .filter(|row| row.class == class)
            .map(|row| row.amount)
            .sum()
    }

    pub fn validate(&self) -> Result<(), DemandError> {
        if self.rows.len() > EXPAND_LEDGER_MAX_ROWS {
            return Err(DemandError::BoundExceeded {
                field: "ledger.rows",
                actual: self.rows.len(),
                maximum: EXPAND_LEDGER_MAX_ROWS,
            });
        }
        for row in &self.rows {
            match row.measurement_source.as_str() {
                "exact" | "estimate" | "unknown" => {}
                other => {
                    return Err(DemandError::InvalidInput(format!(
                        "ledger row measurement_source must be exact|estimate|unknown, got {other:?}"
                    )))
                }
            }
        }
        Ok(())
    }
}

/// The declared native-discovery counterfactual for the same atom set
/// (what `ls`/grep/probe would cost natively). Estimate by declaration;
/// never presented as exact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBaseline {
    pub discovery_bytes: u64,
    pub probe_count: u64,
}

impl NativeBaseline {
    pub fn new(discovery_bytes: u64, probe_count: u64) -> Self {
        Self {
            discovery_bytes,
            probe_count,
        }
    }
}

/// Adjudication metrics for one first expansion against the adjudicated
/// ground truth. `false_complete` is `true` exactly when the route claimed a
/// complete expansion whose returned atoms do not cover the ground truth --
/// the release blocker this corpus measures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjudicatedMetrics {
    pub false_complete: bool,
    pub first_try_sufficiency: bool,
    pub visible_bytes: u64,
    pub backend_work: u64,
    pub retry_count: u64,
    pub native_baseline_bytes: u64,
    pub certified_atoms: usize,
    pub expanded_atoms: usize,
    /// `native_baseline_bytes - visible_bytes` (saturating).
    pub native_savings_bytes: u64,
}

/// Derive adjudication metrics from one first expansion and the adjudicated
/// ground-truth closure.
pub fn adjudicate(
    expansion: &FirstExpansion,
    ground_truth: &BTreeSet<Sha256Digest>,
) -> AdjudicatedMetrics {
    let returned: BTreeSet<Sha256Digest> =
        expansion.atoms.iter().map(|atom| atom.atom_root).collect();
    AdjudicatedMetrics {
        false_complete: !ground_truth.is_subset(&returned),
        first_try_sufficiency: expansion.first_try_sufficiency,
        visible_bytes: expansion.visible_bytes,
        backend_work: expansion.ledger.total("backend_work"),
        retry_count: expansion.ledger.total("retry_count"),
        native_baseline_bytes: expansion.native_baseline.discovery_bytes,
        certified_atoms: expansion.certified_atoms,
        expanded_atoms: expansion.atoms.len(),
        native_savings_bytes: expansion
            .native_baseline
            .discovery_bytes
            .saturating_sub(expansion.visible_bytes),
    }
}

// ---------------------------------------------------------------------------
// Expansion artifacts
// ---------------------------------------------------------------------------

/// One exact atom returned by an expansion.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandedAtom {
    pub atom_root: Sha256Digest,
    pub byte_len: u64,
}

/// The sequence-bound continuation token. Created only by the one first
/// expansion; replaying a stale token is refused by the route.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncrementalSession {
    handle_id: Sha256Digest,
    delta_seq: u64,
    terminal: bool,
}

impl IncrementalSession {
    pub fn handle_id(&self) -> Sha256Digest {
        self.handle_id
    }

    pub fn delta_seq(&self) -> u64 {
        self.delta_seq
    }

    pub fn terminal(&self) -> bool {
        self.terminal
    }
}

/// The result of the exactly-one first expansion: the projection returned
/// root/projection exact, with the bounded ledger and the continuation.
#[derive(Clone, Debug, PartialEq)]
pub struct FirstExpansion {
    pub handle_id: Sha256Digest,
    /// The read-only authority revalidated at expansion time.
    pub permit: zero_abi::ExpandPermit,
    pub plan: DemandPlan,
    /// Exactly the projection atoms, sorted.
    pub atoms: Vec<ExpandedAtom>,
    /// Re-derived over `atoms`; equals `permit.projection_root()`.
    pub projection_root: Sha256Digest,
    pub visible_bytes: u64,
    pub certified_atoms: usize,
    /// Always `true` by construction: the family is first-attempt only.
    pub first_try_sufficiency: bool,
    pub ledger: ExpandLedger,
    pub native_baseline: NativeBaseline,
    pub session: IncrementalSession,
}

impl FirstExpansion {
    /// Internal exactness check: the returned atom set must root to the
    /// permit's projection and match the plan's projection.
    pub fn validate(&self) -> Result<(), DemandError> {
        let returned: Vec<Sha256Digest> = self.atoms.iter().map(|atom| atom.atom_root).collect();
        let returned_root = projection_root_of(&returned);
        if returned_root != self.projection_root {
            return Err(DemandError::ProjectionMismatch {
                expected: self.projection_root,
                actual: returned_root,
            });
        }
        if returned_root != self.permit.projection_root() {
            return Err(DemandError::ProjectionMismatch {
                expected: self.permit.projection_root(),
                actual: returned_root,
            });
        }
        if returned != self.plan.projection_atoms {
            return Err(DemandError::Internal(
                "first expansion atoms differ from the plan projection",
            ));
        }
        let mut visible: u64 = 0;
        for atom in &self.atoms {
            visible = visible.saturating_add(atom.byte_len);
        }
        if visible != self.visible_bytes {
            return Err(DemandError::Internal(
                "visible_bytes does not sum the returned atoms",
            ));
        }
        self.ledger.validate()?;
        Ok(())
    }
}

/// One continuation-bound incremental delta: only new atoms, appended after
/// live revalidation of the handle.
#[derive(Clone, Debug, PartialEq)]
pub struct IncrementalDelta {
    pub handle_id: Sha256Digest,
    pub delta_seq: u64,
    /// The new atoms appended by this delta (never previously expanded).
    pub atoms: Vec<ExpandedAtom>,
    pub new_atoms: usize,
    pub visible_bytes_delta: u64,
    pub visible_bytes_total: u64,
    pub certified_atoms: usize,
    pub expanded_atoms: usize,
    /// `true` when every certified atom is now expanded.
    pub terminal: bool,
    pub delta_root: Sha256Digest,
    pub ledger: ExpandLedger,
    /// The continuation for the next delta.
    pub session: IncrementalSession,
}

impl IncrementalDelta {
    pub fn validate(&self) -> Result<(), DemandError> {
        if self.session.delta_seq != self.delta_seq {
            return Err(DemandError::Internal(
                "delta session sequence does not match the delta",
            ));
        }
        if self.session.handle_id != self.handle_id {
            return Err(DemandError::Internal(
                "delta session handle does not match the delta",
            ));
        }
        let mut visible: u64 = 0;
        for atom in &self.atoms {
            visible = visible.saturating_add(atom.byte_len);
        }
        if visible != self.visible_bytes_delta {
            return Err(DemandError::Internal(
                "visible_bytes_delta does not sum the delta atoms",
            ));
        }
        if self.new_atoms != self.atoms.len() {
            return Err(DemandError::Internal(
                "new_atoms does not match the delta atom count",
            ));
        }
        self.ledger.validate()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The trusted W9-E route
// ---------------------------------------------------------------------------

/// Outcome of one `compile_and_check` call.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteOutcome {
    Issued {
        handle: SafeExpandHandle,
        plan: DemandPlan,
        certificate_root: Sha256Digest,
        checker_identity: String,
        checker_version: String,
    },
    Refused {
        verdict: SafetyVerdict,
    },
}

#[derive(Clone, Debug)]
struct SessionState {
    handle: SafeExpandHandle,
    plan: DemandPlan,
    project_root: Sha256Digest,
    request_root: Sha256Digest,
    scope_root: Sha256Digest,
    protected_atoms: BTreeSet<Sha256Digest>,
    atom_lens: BTreeMap<Sha256Digest, u64>,
    expanded: BTreeSet<Sha256Digest>,
    delta_seq: u64,
    visible_bytes: u64,
    backend_work: u64,
    terminal: bool,
}

/// The trusted hub-owned W9-E route. Owns the issuance secret (guests never
/// see it), the live index/tenant/epoch bindings, the per-handle session
/// registry (exactly-one-first-expansion law), and every revalidation.
#[derive(Clone, Debug)]
pub struct W9eRoute {
    issuer: SafeExpandIssuer,
    tenant: String,
    epoch: u64,
    index_root: Sha256Digest,
    index_version: String,
    issue_serial: u64,
    sessions: BTreeMap<Sha256Digest, SessionState>,
}

impl W9eRoute {
    pub fn new(
        secret: [u8; 32],
        tenant: String,
        epoch: u64,
        index_root: Sha256Digest,
        index_version: String,
    ) -> Result<Self, DemandError> {
        validate_string("tenant", &tenant)?;
        validate_string("index_version", &index_version)?;
        if index_root == Sha256Digest::ZERO {
            return Err(DemandError::ZeroRoot("index_root"));
        }
        Ok(Self {
            issuer: SafeExpandIssuer::new(secret),
            tenant,
            epoch,
            index_root,
            index_version,
            issue_serial: 0,
            sessions: BTreeMap::new(),
        })
    }

    /// The renderer contract root every handle binds (exact-atoms renderer
    /// for this family). A pure constant function.
    pub fn renderer_contract_root() -> Sha256Digest {
        let value = serde_json::json!({
            "renderer": RENDERER_NAME,
            "family": W9E_FAMILY_NAME,
            "version": 1,
        });
        // Constant canonical input: both fallible calls cannot fail.
        let canonical = canonical_object_bytes(ObjectClass::AuthorityObject, ROOTED_ABI_VERSION, &value)
            .expect("renderer contract is canonical");
        object_root(ObjectClass::AuthorityObject, ROOTED_ABI_VERSION, &canonical)
            .expect("renderer contract roots")
    }

    fn issue_nonce(&mut self, plan_root: Sha256Digest, projection_root: Sha256Digest) -> Sha256Digest {
        self.issue_serial = self.issue_serial.saturating_add(1);
        domain_root(
            ISSUE_NONCE_DOMAIN,
            &serde_json::json!({
                "epoch": self.epoch,
                "index_version": self.index_version,
                "issue_serial": self.issue_serial,
                "plan_root": plan_root.to_hex(),
                "projection_root": projection_root.to_hex(),
            }),
        )
    }

    /// Compile demand, run the total completeness check through the
    /// published GraphZero inputs, fold image validity with graph coverage,
    /// and issue a [`SafeExpandHandle`] only on a `Safe` fold.
    pub fn compile_and_check(
        &mut self,
        manifest: &ProjectImageManifest,
        request: &DemandRequest,
        scope: &ProtectedScope,
        input: &GraphZeroCompletenessInput,
    ) -> Result<RouteOutcome, DemandError> {
        if manifest.root == Sha256Digest::ZERO {
            return Err(DemandError::ZeroRoot("project_root"));
        }
        if input.index_root != self.index_root {
            return Ok(RouteOutcome::Refused {
                verdict: SafetyVerdict::Unsafe {
                    reasons: vec!["index_root_mismatch".to_owned()],
                },
            });
        }
        if input.index_version != self.index_version {
            return Ok(RouteOutcome::Refused {
                verdict: SafetyVerdict::Unsafe {
                    reasons: vec!["index_version_mismatch".to_owned()],
                },
            });
        }

        let compiled = match compile_demand(manifest, request, scope)? {
            CompileOutcome::Ready(compiled) => compiled,
            CompileOutcome::Refused(verdict) => {
                return Ok(RouteOutcome::Refused { verdict });
            }
        };

        let check = check_completeness(input, &compiled.plan, scope)?;
        if !check.verdict.grants_authority() {
            return Ok(RouteOutcome::Refused {
                verdict: check.verdict,
            });
        }
        let certificate_root = check.certificate_root.ok_or_else(|| {
            DemandError::Internal("Safe completeness check without certificate root")
        })?;

        let evidence = CompletenessEvidence {
            certificate_root,
            verdict: SafetyVerdict::Safe,
            checker_identity: CHECKER_ID.to_owned(),
            checker_version: CHECKER_VERSION.to_owned(),
            first_attempt: true,
        };
        evidence
            .validate()
            .map_err(|error| DemandError::HandleIssuance(error.to_string()))?;

        let nonce = self.issue_nonce(compiled.plan.plan_root, compiled.plan.projection_root);
        let issue_request = SafeExpandIssueRequest {
            project_root: manifest.root,
            request_root: request.request_root,
            protected_scope_root: scope.scope_root,
            demand_plan_root: compiled.plan.plan_root,
            index_root: self.index_root,
            index_version: self.index_version.clone(),
            renderer_contract: Self::renderer_contract_root(),
            tenant: self.tenant.clone(),
            epoch: self.epoch,
            projection_root: compiled.plan.projection_root,
            completeness: evidence,
            issue_nonce: nonce,
        };
        let handle = self
            .issuer
            .issue(&issue_request)
            .map_err(|error| DemandError::HandleIssuance(error.to_string()))?;

        let atom_lens: BTreeMap<Sha256Digest, u64> = manifest
            .exact_objects
            .iter()
            .map(|object| (object.digest, object.byte_len))
            .collect();
        let protected_atoms: BTreeSet<Sha256Digest> =
            scope.protected_atoms.iter().copied().collect();

        self.sessions.insert(
            handle.handle_id(),
            SessionState {
                handle: handle.clone(),
                plan: compiled.plan.clone(),
                project_root: manifest.root,
                request_root: request.request_root,
                scope_root: scope.scope_root,
                protected_atoms,
                atom_lens,
                expanded: BTreeSet::new(),
                delta_seq: 0,
                visible_bytes: 0,
                backend_work: compiled
                    .compile_backend_work
                    .saturating_add(check.backend_work),
                terminal: false,
            },
        );

        Ok(RouteOutcome::Issued {
            handle,
            plan: compiled.plan,
            certificate_root,
            checker_identity: CHECKER_ID.to_owned(),
            checker_version: CHECKER_VERSION.to_owned(),
        })
    }

    /// Live revalidation of one handle (passthrough; typed outcome).
    pub fn revalidate(
        &self,
        handle: &SafeExpandHandle,
        live: &LiveExpandState,
    ) -> ExpandOutcome {
        self.issuer.revalidate(handle, live)
    }

    /// Build the live hub state that mirrors this route's bindings for one
    /// issued handle. Tests mutate fields to exercise stale/mismatch paths.
    pub fn current_live_state(
        &self,
        handle: &SafeExpandHandle,
        verdict: SafetyVerdict,
        hidden_retry_after_issue: bool,
    ) -> Result<LiveExpandState, DemandError> {
        let state = self
            .sessions
            .get(&handle.handle_id())
            .ok_or(DemandError::UnknownHandle {
                handle_id: handle.handle_id(),
            })?;
        Ok(LiveExpandState {
            project_root: state.project_root,
            request_root: state.request_root,
            protected_scope_root: state.scope_root,
            demand_plan_root: state.plan.plan_root,
            index_root: self.index_root,
            index_version: self.index_version.clone(),
            renderer_contract: Self::renderer_contract_root(),
            tenant: self.tenant.clone(),
            epoch: self.epoch,
            projection_root: state.plan.projection_root,
            completeness: LiveCompleteness {
                certificate_root: Some(handle.completeness().certificate_root()),
                verdict,
                checker_identity: Some(handle.completeness().checker_identity().to_owned()),
                checker_version: Some(handle.completeness().checker_version().to_owned()),
                first_attempt: handle.completeness().first_attempt(),
            },
            hidden_retry_after_issue,
        })
    }

    fn revalidate_or_refuse(
        &self,
        handle: &SafeExpandHandle,
        live: &LiveExpandState,
    ) -> Result<zero_abi::ExpandPermit, DemandError> {
        match self.issuer.revalidate(handle, live) {
            ExpandOutcome::Safe(permit) => Ok(permit),
            ExpandOutcome::Unsafe { reasons } => {
                Err(DemandError::RevalidationUnsafe { reasons })
            }
            ExpandOutcome::Unknown { reasons } => {
                Err(DemandError::RevalidationUnknown { reasons })
            }
        }
    }

    fn check_session_consistency(
        state: &SessionState,
        permit: &zero_abi::ExpandPermit,
    ) -> Result<(), DemandError> {
        if state.project_root != permit.project_root() {
            return Err(DemandError::SessionRootMismatch("project_root"));
        }
        if state.request_root != permit.request_root() {
            return Err(DemandError::SessionRootMismatch("request_root"));
        }
        if state.scope_root != permit.protected_scope_root() {
            return Err(DemandError::SessionRootMismatch("protected_scope_root"));
        }
        if state.plan.plan_root != permit.demand_plan_root() {
            return Err(DemandError::SessionRootMismatch("demand_plan_root"));
        }
        if state.plan.projection_root != permit.projection_root() {
            return Err(DemandError::SessionRootMismatch("projection_root"));
        }
        Ok(())
    }

    fn build_ledger_rows(
        visible: u64,
        backend: u64,
        certified: usize,
        expanded: usize,
        native: &NativeBaseline,
    ) -> Result<ExpandLedger, DemandError> {
        let mut ledger = ExpandLedger::empty();
        ledger.push("visible_bytes", visible, "bytes", "exact")?;
        ledger.push("backend_work", backend, "lookups", "exact")?;
        ledger.push("retry_count", 0, "attempts", "exact")?;
        ledger.push("first_try_sufficiency", 1, "bool", "exact")?;
        ledger.push("false_complete", 0, "bool", "exact")?;
        ledger.push("certified_atoms", certified as u64, "count", "exact")?;
        ledger.push("expanded_atoms", expanded as u64, "count", "exact")?;
        ledger.push(
            "native_baseline_bytes",
            native.discovery_bytes,
            "bytes",
            "estimate",
        )?;
        ledger.push("native_baseline_probes", native.probe_count, "probes", "estimate")?;
        Ok(ledger)
    }

    /// Perform the exactly-one first expansion. Revalidates every handle
    /// binding against live hub state, then returns exactly the projection
    /// atoms (root/projection exact) with the bounded ledger and the
    /// continuation token. A second first expansion on the same handle is
    /// refused.
    pub fn expand_first(
        &mut self,
        handle: &SafeExpandHandle,
        live: &LiveExpandState,
        native: &NativeBaseline,
    ) -> Result<FirstExpansion, DemandError> {
        let handle_id = handle.handle_id();
        let state = self
            .sessions
            .get(&handle_id)
            .ok_or(DemandError::UnknownHandle { handle_id })?
            .clone();
        if !state.expanded.is_empty() {
            return Err(DemandError::AlreadyFirstExpanded { handle_id });
        }

        let permit = self.revalidate_or_refuse(handle, live)?;
        Self::check_session_consistency(&state, &permit)?;

        let atoms: Vec<ExpandedAtom> = state
            .plan
            .projection_atoms
            .iter()
            .map(|atom| ExpandedAtom {
                atom_root: *atom,
                byte_len: state.atom_lens.get(atom).copied().unwrap_or(0),
            })
            .collect();
        let projection_root = projection_root_of(
            &atoms.iter().map(|atom| atom.atom_root).collect::<Vec<_>>(),
        );
        if projection_root != permit.projection_root() {
            return Err(DemandError::ProjectionMismatch {
                expected: permit.projection_root(),
                actual: projection_root,
            });
        }

        let visible_bytes: u64 = atoms.iter().map(|atom| atom.byte_len).sum();
        let backend_work = state
            .backend_work
            .saturating_add(1)
            .saturating_add(atoms.len() as u64);
        let expanded_set: BTreeSet<Sha256Digest> =
            atoms.iter().map(|atom| atom.atom_root).collect();
        let terminal = expanded_set.len() == state.plan.certified_atoms();

        let ledger = Self::build_ledger_rows(
            visible_bytes,
            backend_work,
            state.plan.certified_atoms(),
            atoms.len(),
            native,
        )?;

        let session = IncrementalSession {
            handle_id,
            delta_seq: 0,
            terminal,
        };

        let expansion = FirstExpansion {
            handle_id,
            permit,
            plan: state.plan.clone(),
            atoms,
            projection_root,
            visible_bytes,
            certified_atoms: state.plan.certified_atoms(),
            first_try_sufficiency: true,
            ledger,
            native_baseline: *native,
            session,
        };
        expansion.validate()?;

        // Commit the exactly-once transition (all fallible work already ran).
        let mut state = state;
        state.expanded = expanded_set;
        state.visible_bytes = visible_bytes;
        state.backend_work = backend_work;
        state.terminal = terminal;
        self.sessions.insert(handle_id, state);
        Ok(expansion)
    }

    /// Append one continuation-bound incremental delta. The handle is
    /// revalidated against live hub state first; the delta may only contain
    /// new atoms from the certified envelope (never previously expanded,
    /// never protected). Returns the delta plus the next continuation.
    pub fn expand_delta(
        &mut self,
        session: &IncrementalSession,
        request: &IncrementalDeltaRequest,
        live: &LiveExpandState,
    ) -> Result<IncrementalDelta, DemandError> {
        request.validate()?;
        let handle_id = session.handle_id;
        let state = self
            .sessions
            .get(&handle_id)
            .ok_or(DemandError::UnknownHandle { handle_id })?
            .clone();
        if session.delta_seq != state.delta_seq {
            return Err(DemandError::StaleContinuation {
                handle_id,
                expected: state.delta_seq,
                actual: session.delta_seq,
            });
        }

        let permit = self.revalidate_or_refuse(&state.handle, live)?;
        Self::check_session_consistency(&state, &permit)?;

        if state.terminal {
            return Err(DemandError::SessionExhausted { handle_id });
        }
        let mut new_atoms: Vec<Sha256Digest> = Vec::with_capacity(request.atom_refs.len());
        for atom in &request.atom_refs {
            if !state.plan.contains(*atom) {
                return Err(DemandError::DeltaAtomNotCertified { atom_root: *atom });
            }
            if state.expanded.contains(atom) {
                return Err(DemandError::DeltaAtomAlreadyExpanded { atom_root: *atom });
            }
            if state.protected_atoms.contains(atom) {
                return Err(DemandError::DeltaAtomProtected { atom_root: *atom });
            }
            new_atoms.push(*atom);
        }

        let delta_seq = state.delta_seq.saturating_add(1);
        let delta_root = domain_root(
            DELTA_DOMAIN,
            &serde_json::json!({
                "atoms": digest_hex_list(&new_atoms),
                "delta_seq": delta_seq,
                "handle_id": handle_id.to_hex(),
            }),
        );

        let atoms: Vec<ExpandedAtom> = new_atoms
            .iter()
            .map(|atom| ExpandedAtom {
                atom_root: *atom,
                byte_len: state.atom_lens.get(atom).copied().unwrap_or(0),
            })
            .collect();
        let new_atom_count = atoms.len();
        let visible_bytes_delta: u64 = atoms.iter().map(|atom| atom.byte_len).sum();

        let mut state = state;
        for atom in &new_atoms {
            state.expanded.insert(*atom);
        }
        state.visible_bytes = state.visible_bytes.saturating_add(visible_bytes_delta);
        state.backend_work = state
            .backend_work
            .saturating_add(1)
            .saturating_add(atoms.len() as u64);
        state.delta_seq = delta_seq;
        state.terminal = state.expanded.len() == state.plan.certified_atoms();

        let mut ledger = ExpandLedger::empty();
        ledger.push("visible_bytes", visible_bytes_delta, "bytes", "exact")?;
        ledger.push("visible_bytes_total", state.visible_bytes, "bytes", "exact")?;
        ledger.push("backend_work", state.backend_work, "lookups", "exact")?;
        ledger.push("retry_count", 0, "attempts", "exact")?;
        ledger.push("false_complete", 0, "bool", "exact")?;
        ledger.push("new_atoms", new_atom_count as u64, "count", "exact")?;
        ledger.push("expanded_atoms", state.expanded.len() as u64, "count", "exact")?;
        ledger.push(
            "terminal",
            if state.terminal { 1 } else { 0 },
            "bool",
            "exact",
        )?;

        let next_session = IncrementalSession {
            handle_id,
            delta_seq,
            terminal: state.terminal,
        };

        let delta = IncrementalDelta {
            handle_id,
            delta_seq,
            atoms,
            new_atoms: new_atom_count,
            visible_bytes_delta,
            visible_bytes_total: state.visible_bytes,
            certified_atoms: state.plan.certified_atoms(),
            expanded_atoms: state.expanded.len(),
            terminal: state.terminal,
            delta_root,
            ledger,
            session: next_session,
        };
        delta.validate()?;

        // Commit the appended delta (all fallible work already ran).
        self.sessions.insert(handle_id, state);
        Ok(delta)
    }
}
