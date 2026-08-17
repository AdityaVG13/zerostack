//! Canonical Causal Work IR (CWIR) v1 contracts.
//!
//! CWIR is an immutable, task-conditioned typed hypergraph. It records facts,
//! uncertainty, obligations, permitted effect identities, verification scope,
//! and bounded expansion requests. It does not render a prompt, authorize an
//! effect, or claim that opaque model continuation state is recoverable.
//!
//! The v1 wire identity is canonical sorted-key JSON. It is deliberately not
//! called ZCB1: a future binary codec requires its own reviewed version.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ArtifactOwner, Sha256Digest, canonical_json, sha256};

pub const CWIR_CONTRACT_VERSION: u16 = 1;
pub const CWIR_MODEL_VERSION: u16 = 1;
pub const CWIR_SEMANTIC_DOMAIN: &[u8] = b"zerostack.cwir.semantic.v1\0";
pub const CWIR_TASK_DOMAIN: &[u8] = b"zerostack.cwir.task.v1\0";
pub const CWIR_NODE_DOMAIN: &[u8] = b"zerostack.cwir.node.v1\0";
pub const CWIR_EDGE_DOMAIN: &[u8] = b"zerostack.cwir.edge.v1\0";
pub const CWIR_OBLIGATION_DOMAIN: &[u8] = b"zerostack.cwir.obligation.v1\0";
pub const CWIR_EXPANSION_DOMAIN: &[u8] = b"zerostack.cwir.expansion.v1\0";
pub const CWIR_MAX_CANONICAL_BYTES: usize = 1_048_576;
pub const CWIR_MAX_NODES: usize = 4_096;
pub const CWIR_MAX_EDGES: usize = 8_192;
pub const CWIR_MAX_OBLIGATIONS: usize = 1_024;
pub const CWIR_MAX_EXPANSIONS: usize = 1_024;
pub const CWIR_MAX_REFS_PER_ITEM: usize = 1_024;
pub const CWIR_MAX_EFFECTS: usize = 1_024;
pub const CWIR_MAX_CAPABILITIES: usize = 1_024;
pub const CWIR_MAX_IDENTITY_BYTES: usize = 256;
pub const CWIR_MAX_EXPANSION_INPUT_BYTES: u64 = 1_048_576;
pub const CWIR_MAX_EXPANSION_OUTPUT_BYTES: u64 = 16_777_216;
pub const CWIR_MAX_EXPANSION_WORK_UNITS: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirFailureCode {
    UnsupportedVersion,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    SerializationFailure,
    InvalidIdentity,
    ZeroDigest,
    DuplicateIdentity,
    NonCanonicalOrder,
    IdentityMismatch,
    SemanticDigestMismatch,
    TooManyNodes,
    TooManyEdges,
    TooManyObligations,
    TooManyExpansions,
    TooManyReferences,
    TooManyEffects,
    TooManyCapabilities,
    DanglingReference,
    InvalidHyperedge,
    SnapshotMismatch,
    StaleFact,
    MissingProvenance,
    InvalidEpistemicProduct,
    IllegalWaiver,
    InvalidObligationStatus,
    InvalidObligationTransition,
    InvalidResolutionEvidence,
    ExpansionIncomplete,
    ExpansionLimitExceeded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirError {
    pub code: CwirFailureCode,
    pub detail: String,
}

impl CwirError {
    pub fn new(code: CwirFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> CwirFailureCode {
        self.code
    }
}

impl fmt::Display for CwirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for CwirError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirNodeKind {
    Contract,
    State,
    Evidence,
    Claim,
    Hypothesis,
    Uncertainty,
    Obligation,
    Effect,
    Verification,
    Witness,
    Expansion,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirSoundness {
    Exact,
    SoundRestricted,
    EmpiricalIncomplete,
    Heuristic,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirCoverage {
    Complete,
    ScopedComplete,
    Partial,
    ObservedOnly,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirFreshness {
    Current,
    Stale,
    Conflict,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirDeterminism {
    Deterministic,
    Conditional,
    External,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirEpistemicProduct {
    pub authority: ArtifactOwner,
    pub soundness: CwirSoundness,
    pub coverage: CwirCoverage,
    pub freshness: CwirFreshness,
    pub determinism: CwirDeterminism,
}

impl CwirEpistemicProduct {
    fn validate(self) -> Result<(), CwirError> {
        if self.soundness == CwirSoundness::Exact
            && matches!(
                self.coverage,
                CwirCoverage::Partial | CwirCoverage::ObservedOnly | CwirCoverage::Unknown
            )
        {
            return Err(CwirError::new(
                CwirFailureCode::InvalidEpistemicProduct,
                "exact soundness requires complete or scoped-complete coverage",
            ));
        }
        if self.soundness == CwirSoundness::Exact
            && self.determinism == CwirDeterminism::Unknown
        {
            return Err(CwirError::new(
                CwirFailureCode::InvalidEpistemicProduct,
                "exact soundness cannot have unknown determinism",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirTaskContract {
    pub id: Sha256Digest,
    pub task_kind: String,
    pub specification_digest: Sha256Digest,
    pub required_snapshot: Sha256Digest,
}

#[derive(Serialize)]
struct TaskBody<'a> {
    contract_version: u16,
    model_version: u16,
    task_kind: &'a str,
    specification_digest: Sha256Digest,
    required_snapshot: Sha256Digest,
}

impl CwirTaskContract {
    pub fn new(
        task_kind: impl Into<String>,
        specification_digest: Sha256Digest,
        required_snapshot: Sha256Digest,
    ) -> Result<Self, CwirError> {
        let mut task = Self {
            id: Sha256Digest::ZERO,
            task_kind: task_kind.into(),
            specification_digest,
            required_snapshot,
        };
        task.validate_body()?;
        task.id = task.expected_id()?;
        Ok(task)
    }

    fn body(&self) -> TaskBody<'_> {
        TaskBody {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            task_kind: &self.task_kind,
            specification_digest: self.specification_digest,
            required_snapshot: self.required_snapshot,
        }
    }

    fn expected_id(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_TASK_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        validate_identity("task_kind", &self.task_kind)?;
        validate_digest("specification_digest", self.specification_digest)?;
        validate_digest("required_snapshot", self.required_snapshot)
    }

    fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        require_id("task", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirStateAnchor {
    pub project_root: Sha256Digest,
    pub fs_snapshot: Sha256Digest,
    pub graph_indexed_through: Sha256Digest,
    pub toolchain: Sha256Digest,
    pub runtime_manifest: Sha256Digest,
    pub capability_surface: Sha256Digest,
}

impl CwirStateAnchor {
    fn validate(self) -> Result<(), CwirError> {
        for (label, digest) in [
            ("project_root", self.project_root),
            ("fs_snapshot", self.fs_snapshot),
            ("graph_indexed_through", self.graph_indexed_through),
            ("toolchain", self.toolchain),
            ("runtime_manifest", self.runtime_manifest),
            ("capability_surface", self.capability_surface),
        ] {
            validate_digest(label, digest)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirNode {
    pub id: Sha256Digest,
    pub kind: CwirNodeKind,
    pub payload_digest: Sha256Digest,
    pub required_snapshot: Option<Sha256Digest>,
    pub active: bool,
    pub epistemic: CwirEpistemicProduct,
    pub provenance: Vec<Sha256Digest>,
}

#[derive(Serialize)]
struct NodeBody<'a> {
    contract_version: u16,
    model_version: u16,
    kind: CwirNodeKind,
    payload_digest: Sha256Digest,
    required_snapshot: Option<Sha256Digest>,
    active: bool,
    epistemic: CwirEpistemicProduct,
    provenance: &'a [Sha256Digest],
}

impl CwirNode {
    pub fn new(
        kind: CwirNodeKind,
        payload_digest: Sha256Digest,
        required_snapshot: Option<Sha256Digest>,
        active: bool,
        epistemic: CwirEpistemicProduct,
        mut provenance: Vec<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        normalize_unique(&mut provenance, "node provenance")?;
        let mut node = Self {
            id: Sha256Digest::ZERO,
            kind,
            payload_digest,
            required_snapshot,
            active,
            epistemic,
            provenance,
        };
        node.validate_body()?;
        node.id = node.expected_id()?;
        Ok(node)
    }

    fn body(&self) -> NodeBody<'_> {
        NodeBody {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            kind: self.kind,
            payload_digest: self.payload_digest,
            required_snapshot: self.required_snapshot,
            active: self.active,
            epistemic: self.epistemic,
            provenance: &self.provenance,
        }
    }

    fn expected_id(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_NODE_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        validate_digest("node payload_digest", self.payload_digest)?;
        if let Some(snapshot) = self.required_snapshot {
            validate_digest("node required_snapshot", snapshot)?;
        }
        self.epistemic.validate()?;
        validate_sorted_unique(
            &self.provenance,
            "node provenance",
            CWIR_MAX_REFS_PER_ITEM,
        )?;
        validate_digest_slice("node provenance", &self.provenance)
    }

    fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        require_id("node", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirEdgeKind {
    Supports,
    Derives,
    Contradicts,
    Discharges,
    Updates,
    Expands,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirHyperEdge {
    pub id: Sha256Digest,
    pub relation: CwirEdgeKind,
    pub sources: Vec<Sha256Digest>,
    pub target: Sha256Digest,
    pub proof_node: Option<Sha256Digest>,
}

#[derive(Serialize)]
struct EdgeBody<'a> {
    contract_version: u16,
    model_version: u16,
    relation: CwirEdgeKind,
    sources: &'a [Sha256Digest],
    target: Sha256Digest,
    proof_node: Option<Sha256Digest>,
}

impl CwirHyperEdge {
    pub fn new(
        relation: CwirEdgeKind,
        mut sources: Vec<Sha256Digest>,
        target: Sha256Digest,
        proof_node: Option<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        normalize_unique(&mut sources, "hyperedge sources")?;
        let mut edge = Self {
            id: Sha256Digest::ZERO,
            relation,
            sources,
            target,
            proof_node,
        };
        edge.validate_body()?;
        edge.id = edge.expected_id()?;
        Ok(edge)
    }

    fn body(&self) -> EdgeBody<'_> {
        EdgeBody {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            relation: self.relation,
            sources: &self.sources,
            target: self.target,
            proof_node: self.proof_node,
        }
    }

    fn expected_id(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_EDGE_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        if self.sources.is_empty() {
            return Err(CwirError::new(
                CwirFailureCode::InvalidHyperedge,
                "hyperedge sources must not be empty",
            ));
        }
        validate_sorted_unique(
            &self.sources,
            "hyperedge sources",
            CWIR_MAX_REFS_PER_ITEM,
        )?;
        validate_digest_slice("hyperedge sources", &self.sources)?;
        validate_digest("hyperedge target", self.target)?;
        if let Some(proof) = self.proof_node {
            validate_digest("hyperedge proof_node", proof)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        require_id("hyperedge", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirObligationKind {
    Decision,
    Execution,
    Verification,
    Restoration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirObligationStatus {
    Open,
    InProgress,
    Discharged,
    Failed,
    Blocked,
    Waived,
}

impl CwirObligationStatus {
    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Discharged | Self::Failed | Self::Blocked | Self::Waived
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirObligation {
    pub id: Sha256Digest,
    pub kind: CwirObligationKind,
    pub advisory: bool,
    pub status: CwirObligationStatus,
    pub required_snapshot: Sha256Digest,
    pub dependencies: Vec<Sha256Digest>,
    pub resolution_evidence: Option<Sha256Digest>,
}

#[derive(Serialize)]
struct ObligationBody<'a> {
    contract_version: u16,
    model_version: u16,
    kind: CwirObligationKind,
    advisory: bool,
    status: CwirObligationStatus,
    required_snapshot: Sha256Digest,
    dependencies: &'a [Sha256Digest],
    resolution_evidence: Option<Sha256Digest>,
}

impl CwirObligation {
    pub fn new_open(
        kind: CwirObligationKind,
        advisory: bool,
        required_snapshot: Sha256Digest,
        mut dependencies: Vec<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        normalize_unique(&mut dependencies, "obligation dependencies")?;
        Self::from_parts(
            kind,
            advisory,
            CwirObligationStatus::Open,
            required_snapshot,
            dependencies,
            None,
        )
    }

    fn from_parts(
        kind: CwirObligationKind,
        advisory: bool,
        status: CwirObligationStatus,
        required_snapshot: Sha256Digest,
        dependencies: Vec<Sha256Digest>,
        resolution_evidence: Option<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        let mut obligation = Self {
            id: Sha256Digest::ZERO,
            kind,
            advisory,
            status,
            required_snapshot,
            dependencies,
            resolution_evidence,
        };
        obligation.validate_body()?;
        obligation.id = obligation.expected_id()?;
        Ok(obligation)
    }

    pub fn transition(
        &self,
        next: CwirObligationStatus,
        resolution_evidence: Option<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        self.validate()?;
        let allowed = matches!(
            (self.status, next),
            (
                CwirObligationStatus::Open,
                CwirObligationStatus::InProgress
            ) | (
                CwirObligationStatus::Open,
                CwirObligationStatus::Discharged
            ) | (CwirObligationStatus::Open, CwirObligationStatus::Failed)
                | (
                    CwirObligationStatus::Open,
                    CwirObligationStatus::Blocked
                )
                | (CwirObligationStatus::Open, CwirObligationStatus::Waived)
                | (
                    CwirObligationStatus::InProgress,
                    CwirObligationStatus::Discharged
                )
                | (
                    CwirObligationStatus::InProgress,
                    CwirObligationStatus::Failed
                )
                | (
                    CwirObligationStatus::InProgress,
                    CwirObligationStatus::Blocked
                )
                | (
                    CwirObligationStatus::InProgress,
                    CwirObligationStatus::Waived
                )
        );
        if !allowed {
            return Err(CwirError::new(
                CwirFailureCode::InvalidObligationTransition,
                format!(
                    "obligation cannot transition from {:?} to {next:?}",
                    self.status
                ),
            ));
        }
        Self::from_parts(
            self.kind,
            self.advisory,
            next,
            self.required_snapshot,
            self.dependencies.clone(),
            resolution_evidence,
        )
    }

    fn body(&self) -> ObligationBody<'_> {
        ObligationBody {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            kind: self.kind,
            advisory: self.advisory,
            status: self.status,
            required_snapshot: self.required_snapshot,
            dependencies: &self.dependencies,
            resolution_evidence: self.resolution_evidence,
        }
    }

    fn expected_id(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_OBLIGATION_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        validate_digest("obligation required_snapshot", self.required_snapshot)?;
        validate_sorted_unique(
            &self.dependencies,
            "obligation dependencies",
            CWIR_MAX_REFS_PER_ITEM,
        )?;
        validate_digest_slice("obligation dependencies", &self.dependencies)?;
        if self.status == CwirObligationStatus::Waived && !self.advisory {
            return Err(CwirError::new(
                CwirFailureCode::IllegalWaiver,
                "a non-advisory obligation cannot be waived",
            ));
        }
        if self.status.is_terminal() != self.resolution_evidence.is_some() {
            return Err(CwirError::new(
                CwirFailureCode::InvalidObligationStatus,
                "terminal obligations require resolution evidence and non-terminal obligations forbid it",
            ));
        }
        if let Some(evidence) = self.resolution_evidence {
            validate_digest("obligation resolution_evidence", evidence)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        require_id("obligation", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirEffectSpace {
    pub effects: Vec<Sha256Digest>,
    pub capabilities: Vec<Sha256Digest>,
}

impl CwirEffectSpace {
    pub fn new(
        mut effects: Vec<Sha256Digest>,
        mut capabilities: Vec<Sha256Digest>,
    ) -> Result<Self, CwirError> {
        normalize_unique(&mut effects, "effect identities")?;
        normalize_unique(&mut capabilities, "capability identities")?;
        let result = Self {
            effects,
            capabilities,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<(), CwirError> {
        if self.effects.len() > CWIR_MAX_EFFECTS {
            return Err(CwirError::new(
                CwirFailureCode::TooManyEffects,
                format!("effect space contains {} effects", self.effects.len()),
            ));
        }
        if self.capabilities.len() > CWIR_MAX_CAPABILITIES {
            return Err(CwirError::new(
                CwirFailureCode::TooManyCapabilities,
                format!(
                    "effect space contains {} capabilities",
                    self.capabilities.len()
                ),
            ));
        }
        validate_sorted_unique(&self.effects, "effect identities", CWIR_MAX_EFFECTS)?;
        validate_sorted_unique(
            &self.capabilities,
            "capability identities",
            CWIR_MAX_CAPABILITIES,
        )?;
        validate_digest_slice("effect identities", &self.effects)?;
        validate_digest_slice("capability identities", &self.capabilities)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirVerifierClass {
    ExactChecker,
    SoundRestricted,
    EmpiricalIncomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirVerificationContract {
    pub verifier_digest: Sha256Digest,
    pub predicate_digest: Sha256Digest,
    pub scope_digest: Sha256Digest,
    pub class: CwirVerifierClass,
}

impl CwirVerificationContract {
    fn validate(self) -> Result<(), CwirError> {
        validate_digest("verification verifier_digest", self.verifier_digest)?;
        validate_digest("verification predicate_digest", self.predicate_digest)?;
        validate_digest("verification scope_digest", self.scope_digest)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirExpansionCost {
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_work_units: u64,
}

impl CwirExpansionCost {
    fn validate(self) -> Result<(), CwirError> {
        if self.max_input_bytes == 0 || self.max_output_bytes == 0 || self.max_work_units == 0 {
            return Err(CwirError::new(
                CwirFailureCode::ExpansionIncomplete,
                "expansion cost bounds must all be nonzero",
            ));
        }
        if self.max_input_bytes > CWIR_MAX_EXPANSION_INPUT_BYTES
            || self.max_output_bytes > CWIR_MAX_EXPANSION_OUTPUT_BYTES
            || self.max_work_units > CWIR_MAX_EXPANSION_WORK_UNITS
        {
            return Err(CwirError::new(
                CwirFailureCode::ExpansionLimitExceeded,
                "expansion cost exceeds the CWIR v1 contract bound",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirExpansion {
    pub id: Sha256Digest,
    pub owner: ArtifactOwner,
    pub capability: String,
    pub arguments_digest: Sha256Digest,
    pub required_snapshot: Sha256Digest,
    pub cost: CwirExpansionCost,
}

#[derive(Serialize)]
struct ExpansionBody<'a> {
    contract_version: u16,
    model_version: u16,
    owner: ArtifactOwner,
    capability: &'a str,
    arguments_digest: Sha256Digest,
    required_snapshot: Sha256Digest,
    cost: CwirExpansionCost,
}

impl CwirExpansion {
    pub fn new(
        owner: ArtifactOwner,
        capability: impl Into<String>,
        arguments_digest: Sha256Digest,
        required_snapshot: Sha256Digest,
        cost: CwirExpansionCost,
    ) -> Result<Self, CwirError> {
        let mut expansion = Self {
            id: Sha256Digest::ZERO,
            owner,
            capability: capability.into(),
            arguments_digest,
            required_snapshot,
            cost,
        };
        expansion.validate_body()?;
        expansion.id = expansion.expected_id()?;
        Ok(expansion)
    }

    fn body(&self) -> ExpansionBody<'_> {
        ExpansionBody {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            owner: self.owner,
            capability: &self.capability,
            arguments_digest: self.arguments_digest,
            required_snapshot: self.required_snapshot,
            cost: self.cost,
        }
    }

    fn expected_id(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_EXPANSION_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        validate_identity("expansion capability", &self.capability)?;
        validate_digest("expansion arguments_digest", self.arguments_digest)?;
        validate_digest("expansion required_snapshot", self.required_snapshot)?;
        self.cost.validate()
    }

    fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        require_id("expansion", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkIr {
    contract_version: u16,
    model_version: u16,
    task: CwirTaskContract,
    state: CwirStateAnchor,
    nodes: Vec<CwirNode>,
    edges: Vec<CwirHyperEdge>,
    obligations: Vec<CwirObligation>,
    effect_space: CwirEffectSpace,
    verification: CwirVerificationContract,
    expansions: Vec<CwirExpansion>,
    semantic_digest: Sha256Digest,
}

#[derive(Serialize)]
struct CwirSemanticBody<'a> {
    contract_version: u16,
    model_version: u16,
    task: &'a CwirTaskContract,
    state: CwirStateAnchor,
    nodes: &'a [CwirNode],
    edges: &'a [CwirHyperEdge],
    obligations: &'a [CwirObligation],
    effect_space: &'a CwirEffectSpace,
    verification: CwirVerificationContract,
    expansions: &'a [CwirExpansion],
}

impl CausalWorkIr {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task: CwirTaskContract,
        state: CwirStateAnchor,
        mut nodes: Vec<CwirNode>,
        mut edges: Vec<CwirHyperEdge>,
        mut obligations: Vec<CwirObligation>,
        effect_space: CwirEffectSpace,
        verification: CwirVerificationContract,
        mut expansions: Vec<CwirExpansion>,
    ) -> Result<Self, CwirError> {
        normalize_by_id(&mut nodes, |item| item.id, "nodes")?;
        normalize_by_id(&mut edges, |item| item.id, "hyperedges")?;
        normalize_by_id(&mut obligations, |item| item.id, "obligations")?;
        normalize_by_id(&mut expansions, |item| item.id, "expansions")?;
        let mut cwir = Self {
            contract_version: CWIR_CONTRACT_VERSION,
            model_version: CWIR_MODEL_VERSION,
            task,
            state,
            nodes,
            edges,
            obligations,
            effect_space,
            verification,
            expansions,
            semantic_digest: Sha256Digest::ZERO,
        };
        cwir.validate_body()?;
        cwir.semantic_digest = cwir.expected_semantic_digest()?;
        Ok(cwir)
    }

    pub const fn contract_version(&self) -> u16 {
        self.contract_version
    }

    pub const fn model_version(&self) -> u16 {
        self.model_version
    }

    pub const fn task(&self) -> &CwirTaskContract {
        &self.task
    }

    pub const fn state(&self) -> &CwirStateAnchor {
        &self.state
    }

    pub fn nodes(&self) -> &[CwirNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[CwirHyperEdge] {
        &self.edges
    }

    pub fn obligations(&self) -> &[CwirObligation] {
        &self.obligations
    }

    pub const fn effect_space(&self) -> &CwirEffectSpace {
        &self.effect_space
    }

    pub const fn verification(&self) -> &CwirVerificationContract {
        &self.verification
    }

    pub fn expansions(&self) -> &[CwirExpansion] {
        &self.expansions
    }

    pub const fn semantic_digest(&self) -> Sha256Digest {
        self.semantic_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CwirError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > CWIR_MAX_CANONICAL_BYTES {
            return Err(CwirError::new(
                CwirFailureCode::CanonicalPayloadTooLarge,
                format!("CWIR payload is {} bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CwirError> {
        if bytes.len() > CWIR_MAX_CANONICAL_BYTES {
            return Err(CwirError::new(
                CwirFailureCode::CanonicalPayloadTooLarge,
                format!("CWIR payload is {} bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(CwirError::new(
                CwirFailureCode::NonCanonicalEncoding,
                "CWIR bytes are not exact sorted-key JSON without whitespace",
            ));
        }
        let cwir: Self = serde_json::from_value(value).map_err(serialization_error)?;
        cwir.validate()?;
        Ok(cwir)
    }

    pub fn validate(&self) -> Result<(), CwirError> {
        self.validate_body()?;
        let expected = self.expected_semantic_digest()?;
        if self.semantic_digest != expected {
            return Err(CwirError::new(
                CwirFailureCode::SemanticDigestMismatch,
                format!(
                    "semantic digest {} does not match canonical body {}",
                    self.semantic_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    fn body(&self) -> CwirSemanticBody<'_> {
        CwirSemanticBody {
            contract_version: self.contract_version,
            model_version: self.model_version,
            task: &self.task,
            state: self.state,
            nodes: &self.nodes,
            edges: &self.edges,
            obligations: &self.obligations,
            effect_space: &self.effect_space,
            verification: self.verification,
            expansions: &self.expansions,
        }
    }

    fn expected_semantic_digest(&self) -> Result<Sha256Digest, CwirError> {
        digest_body(CWIR_SEMANTIC_DOMAIN, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirError> {
        if self.contract_version != CWIR_CONTRACT_VERSION
            || self.model_version != CWIR_MODEL_VERSION
        {
            return Err(CwirError::new(
                CwirFailureCode::UnsupportedVersion,
                format!(
                    "unsupported CWIR contract/model version {}/{}",
                    self.contract_version, self.model_version
                ),
            ));
        }
        if self.nodes.len() > CWIR_MAX_NODES {
            return Err(CwirError::new(
                CwirFailureCode::TooManyNodes,
                "node bound exceeded",
            ));
        }
        if self.edges.len() > CWIR_MAX_EDGES {
            return Err(CwirError::new(
                CwirFailureCode::TooManyEdges,
                "edge bound exceeded",
            ));
        }
        if self.obligations.len() > CWIR_MAX_OBLIGATIONS {
            return Err(CwirError::new(
                CwirFailureCode::TooManyObligations,
                "obligation bound exceeded",
            ));
        }
        if self.expansions.len() > CWIR_MAX_EXPANSIONS {
            return Err(CwirError::new(
                CwirFailureCode::TooManyExpansions,
                "expansion bound exceeded",
            ));
        }
        self.task.validate()?;
        self.state.validate()?;
        if self.task.required_snapshot != self.state.fs_snapshot {
            return Err(CwirError::new(
                CwirFailureCode::SnapshotMismatch,
                "task required_snapshot does not match the CWIR state anchor",
            ));
        }
        validate_id_order(&self.nodes, |item| item.id, "nodes")?;
        validate_id_order(&self.edges, |item| item.id, "hyperedges")?;
        validate_id_order(&self.obligations, |item| item.id, "obligations")?;
        validate_id_order(&self.expansions, |item| item.id, "expansions")?;
        self.effect_space.validate()?;
        self.verification.validate()?;

        let mut node_map = BTreeMap::new();
        for node in &self.nodes {
            node.validate()?;
            node_map.insert(node.id, node.kind);
        }
        for node in &self.nodes {
            for provenance in &node.provenance {
                require_reference(&node_map, *provenance, "node provenance")?;
            }
            if node.kind == CwirNodeKind::Evidence && node.active {
                if node.epistemic.freshness != CwirFreshness::Current {
                    return Err(CwirError::new(
                        CwirFailureCode::StaleFact,
                        format!("active evidence node {} is not current", node.id.to_hex()),
                    ));
                }
                if node.required_snapshot != Some(self.state.fs_snapshot) {
                    return Err(CwirError::new(
                        CwirFailureCode::SnapshotMismatch,
                        format!(
                            "active evidence node {} is not bound to the state snapshot",
                            node.id.to_hex()
                        ),
                    ));
                }
                if node.epistemic.soundness == CwirSoundness::Exact && node.provenance.is_empty()
                {
                    return Err(CwirError::new(
                        CwirFailureCode::MissingProvenance,
                        format!("exact evidence node {} has no provenance", node.id.to_hex()),
                    ));
                }
            }
        }

        for edge in &self.edges {
            edge.validate()?;
            for source in &edge.sources {
                require_reference(&node_map, *source, "hyperedge source")?;
            }
            require_reference(&node_map, edge.target, "hyperedge target")?;
            if let Some(proof) = edge.proof_node {
                require_proof_node(&node_map, proof, "hyperedge proof_node")?;
            }
        }

        let mut obligation_map = BTreeMap::new();
        for obligation in &self.obligations {
            obligation.validate()?;
            obligation_map.insert(obligation.id, ());
            if obligation.required_snapshot != self.state.fs_snapshot {
                return Err(CwirError::new(
                    CwirFailureCode::SnapshotMismatch,
                    format!(
                        "obligation {} is not bound to the state snapshot",
                        obligation.id.to_hex()
                    ),
                ));
            }
        }
        for obligation in &self.obligations {
            for dependency in &obligation.dependencies {
                if !obligation_map.contains_key(dependency) {
                    return Err(CwirError::new(
                        CwirFailureCode::DanglingReference,
                        format!(
                            "obligation dependency {} does not resolve",
                            dependency.to_hex()
                        ),
                    ));
                }
            }
            if let Some(evidence) = obligation.resolution_evidence {
                require_proof_node(&node_map, evidence, "obligation resolution_evidence")?;
            }
        }

        for expansion in &self.expansions {
            expansion.validate()?;
            if expansion.required_snapshot != self.state.fs_snapshot {
                return Err(CwirError::new(
                    CwirFailureCode::SnapshotMismatch,
                    format!(
                        "expansion {} is not bound to the state snapshot",
                        expansion.id.to_hex()
                    ),
                ));
            }
        }
        Ok(())
    }
}

pub fn cwir_contract_manifest() -> Value {
    json!({
        "contract": "zerostack.cwir",
        "contract_version": CWIR_CONTRACT_VERSION,
        "model_version": CWIR_MODEL_VERSION,
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "domains": {
            "semantic": "zerostack.cwir.semantic.v1\u{0}",
            "task": "zerostack.cwir.task.v1\u{0}",
            "node": "zerostack.cwir.node.v1\u{0}",
            "edge": "zerostack.cwir.edge.v1\u{0}",
            "obligation": "zerostack.cwir.obligation.v1\u{0}",
            "expansion": "zerostack.cwir.expansion.v1\u{0}"
        },
        "semantic_fields": [
            "contract_version", "model_version", "task", "state", "nodes", "edges",
            "obligations", "effect_space", "verification", "expansions"
        ],
        "task_fields": ["id", "task_kind", "specification_digest", "required_snapshot"],
        "state_anchor_fields": [
            "project_root", "fs_snapshot", "graph_indexed_through", "toolchain",
            "runtime_manifest", "capability_surface"
        ],
        "node_fields": [
            "id", "kind", "payload_digest", "required_snapshot", "active", "epistemic",
            "provenance"
        ],
        "node_kinds": [
            "contract", "state", "evidence", "claim", "hypothesis", "uncertainty",
            "obligation", "effect", "verification", "witness", "expansion"
        ],
        "epistemic_fields": ["authority", "soundness", "coverage", "freshness", "determinism"],
        "authority_values": ["zero_stack", "fs_zero", "graph_zero", "token_zero", "pi_zero_stack"],
        "soundness_values": ["exact", "sound_restricted", "empirical_incomplete", "heuristic", "unknown"],
        "coverage_values": ["complete", "scoped_complete", "partial", "observed_only", "unknown"],
        "freshness_values": ["current", "stale", "conflict", "unknown"],
        "determinism_values": ["deterministic", "conditional", "external", "unknown"],
        "edge_fields": ["id", "relation", "sources", "target", "proof_node"],
        "edge_kinds": ["supports", "derives", "contradicts", "discharges", "updates", "expands"],
        "obligation_fields": [
            "id", "kind", "advisory", "status", "required_snapshot", "dependencies",
            "resolution_evidence"
        ],
        "obligation_kinds": ["decision", "execution", "verification", "restoration"],
        "obligation_statuses": ["open", "in_progress", "discharged", "failed", "blocked", "waived"],
        "effect_space_fields": ["effects", "capabilities"],
        "verification_fields": ["verifier_digest", "predicate_digest", "scope_digest", "class"],
        "verifier_classes": ["exact_checker", "sound_restricted", "empirical_incomplete"],
        "expansion_fields": [
            "id", "owner", "capability", "arguments_digest", "required_snapshot", "cost"
        ],
        "expansion_cost_fields": ["max_input_bytes", "max_output_bytes", "max_work_units"],
        "failure_codes": [
            "unsupported_version", "canonical_payload_too_large", "non_canonical_encoding",
            "serialization_failure", "invalid_identity", "zero_digest", "duplicate_identity",
            "non_canonical_order", "identity_mismatch", "semantic_digest_mismatch",
            "too_many_nodes", "too_many_edges", "too_many_obligations", "too_many_expansions",
            "too_many_references", "too_many_effects", "too_many_capabilities",
            "dangling_reference", "invalid_hyperedge", "snapshot_mismatch", "stale_fact",
            "missing_provenance", "invalid_epistemic_product", "illegal_waiver",
            "invalid_obligation_status", "invalid_obligation_transition",
            "invalid_resolution_evidence", "expansion_incomplete", "expansion_limit_exceeded"
        ],
        "bounds": {
            "max_canonical_bytes": CWIR_MAX_CANONICAL_BYTES,
            "max_nodes": CWIR_MAX_NODES,
            "max_edges": CWIR_MAX_EDGES,
            "max_obligations": CWIR_MAX_OBLIGATIONS,
            "max_expansions": CWIR_MAX_EXPANSIONS,
            "max_references_per_item": CWIR_MAX_REFS_PER_ITEM,
            "max_effects": CWIR_MAX_EFFECTS,
            "max_capabilities": CWIR_MAX_CAPABILITIES,
            "max_identity_bytes": CWIR_MAX_IDENTITY_BYTES,
            "max_expansion_input_bytes": CWIR_MAX_EXPANSION_INPUT_BYTES,
            "max_expansion_output_bytes": CWIR_MAX_EXPANSION_OUTPUT_BYTES,
            "max_expansion_work_units": CWIR_MAX_EXPANSION_WORK_UNITS
        },
        "invariants": [
            "all semantic identities are content addressed",
            "duplicate identities and members are rejected",
            "provenance and hyperedge references resolve",
            "active evidence is current and bound to the exact state snapshot",
            "non_advisory_obligations_cannot_be_waived",
            "obligation_transitions_are_monotone_and_evidence_bound",
            "effect_capability_and_reference_sets_are_canonically_ordered",
            "graph_freshness_is_explicit_in_state_anchor",
            "expansions_bind_owner_capability_arguments_snapshot_and_cost",
            "cwir_does_not_authorize_effects_or_infer_model_continuation"
        ]
    })
}
pub fn cwir_contract_digest() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(
        canonical_json(&cwir_contract_manifest()).as_bytes(),
    ))
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<Sha256Digest, CwirError> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(Sha256Digest::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> CwirError {
    CwirError::new(CwirFailureCode::SerializationFailure, error.to_string())
}

fn validate_identity(label: &str, value: &str) -> Result<(), CwirError> {
    if value.is_empty()
        || value.len() > CWIR_MAX_IDENTITY_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        return Err(CwirError::new(
            CwirFailureCode::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ));
    }
    Ok(())
}

fn validate_digest(label: &str, digest: Sha256Digest) -> Result<(), CwirError> {
    if digest == Sha256Digest::ZERO {
        Err(CwirError::new(
            CwirFailureCode::ZeroDigest,
            format!("{label} must not be the zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn validate_digest_slice(label: &str, values: &[Sha256Digest]) -> Result<(), CwirError> {
    for digest in values {
        validate_digest(label, *digest)?;
    }
    Ok(())
}

fn require_id(label: &str, actual: Sha256Digest, expected: Sha256Digest) -> Result<(), CwirError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CwirError::new(
            CwirFailureCode::IdentityMismatch,
            format!(
                "{label} id {} does not match canonical body {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn normalize_unique<T: Ord>(values: &mut [T], label: &str) -> Result<(), CwirError> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CwirError::new(
            CwirFailureCode::DuplicateIdentity,
            format!("{label} contains a duplicate member"),
        ));
    }
    Ok(())
}

fn validate_sorted_unique<T: Ord>(
    values: &[T],
    label: &str,
    max: usize,
) -> Result<(), CwirError> {
    if values.len() > max {
        return Err(CwirError::new(
            CwirFailureCode::TooManyReferences,
            format!(
                "{label} contains {} members, maximum is {max}",
                values.len()
            ),
        ));
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(CwirError::new(
                CwirFailureCode::DuplicateIdentity,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(CwirError::new(
                CwirFailureCode::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn normalize_by_id<T, F>(values: &mut [T], id: F, label: &str) -> Result<(), CwirError>
where
    F: Fn(&T) -> Sha256Digest,
{
    values.sort_by_key(&id);
    if values.windows(2).any(|pair| id(&pair[0]) == id(&pair[1])) {
        return Err(CwirError::new(
            CwirFailureCode::DuplicateIdentity,
            format!("{label} contains a duplicate identity"),
        ));
    }
    Ok(())
}

fn validate_id_order<T, F>(values: &[T], id: F, label: &str) -> Result<(), CwirError>
where
    F: Fn(&T) -> Sha256Digest,
{
    for pair in values.windows(2) {
        let left = id(&pair[0]);
        let right = id(&pair[1]);
        if left == right {
            return Err(CwirError::new(
                CwirFailureCode::DuplicateIdentity,
                format!("{label} contains a duplicate identity"),
            ));
        }
        if left > right {
            return Err(CwirError::new(
                CwirFailureCode::NonCanonicalOrder,
                format!("{label} is not sorted by identity"),
            ));
        }
    }
    Ok(())
}

fn require_reference(
    nodes: &BTreeMap<Sha256Digest, CwirNodeKind>,
    id: Sha256Digest,
    label: &str,
) -> Result<(), CwirError> {
    if nodes.contains_key(&id) {
        Ok(())
    } else {
        Err(CwirError::new(
            CwirFailureCode::DanglingReference,
            format!("{label} {} does not resolve", id.to_hex()),
        ))
    }
}

fn require_proof_node(
    nodes: &BTreeMap<Sha256Digest, CwirNodeKind>,
    id: Sha256Digest,
    label: &str,
) -> Result<(), CwirError> {
    match nodes.get(&id) {
        Some(CwirNodeKind::Witness | CwirNodeKind::Verification) => Ok(()),
        Some(kind) => Err(CwirError::new(
            CwirFailureCode::InvalidResolutionEvidence,
            format!(
                "{label} {} resolves to {kind:?}, not witness or verification",
                id.to_hex()
            ),
        )),
        None => Err(CwirError::new(
            CwirFailureCode::DanglingReference,
            format!("{label} {} does not resolve", id.to_hex()),
        )),
    }
}

