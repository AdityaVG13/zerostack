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
use serde_json::{json, Value};

use crate::{canonical_json, sha256, ArtifactOwnerV1, DigestV1};

pub const CWIR_CONTRACT_VERSION_V1: u16 = 1;
pub const CWIR_MODEL_VERSION_V1: u16 = 1;
pub const CWIR_SEMANTIC_DOMAIN_V1: &[u8] = b"zerostack.cwir.semantic.v1\0";
pub const CWIR_TASK_DOMAIN_V1: &[u8] = b"zerostack.cwir.task.v1\0";
pub const CWIR_NODE_DOMAIN_V1: &[u8] = b"zerostack.cwir.node.v1\0";
pub const CWIR_EDGE_DOMAIN_V1: &[u8] = b"zerostack.cwir.edge.v1\0";
pub const CWIR_OBLIGATION_DOMAIN_V1: &[u8] = b"zerostack.cwir.obligation.v1\0";
pub const CWIR_EXPANSION_DOMAIN_V1: &[u8] = b"zerostack.cwir.expansion.v1\0";
pub const CWIR_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;
pub const CWIR_MAX_NODES_V1: usize = 4_096;
pub const CWIR_MAX_EDGES_V1: usize = 8_192;
pub const CWIR_MAX_OBLIGATIONS_V1: usize = 1_024;
pub const CWIR_MAX_EXPANSIONS_V1: usize = 1_024;
pub const CWIR_MAX_REFS_PER_ITEM_V1: usize = 1_024;
pub const CWIR_MAX_EFFECTS_V1: usize = 1_024;
pub const CWIR_MAX_CAPABILITIES_V1: usize = 1_024;
pub const CWIR_MAX_IDENTITY_BYTES_V1: usize = 256;
pub const CWIR_MAX_EXPANSION_INPUT_BYTES_V1: u64 = 1_048_576;
pub const CWIR_MAX_EXPANSION_OUTPUT_BYTES_V1: u64 = 16_777_216;
pub const CWIR_MAX_EXPANSION_WORK_UNITS_V1: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirFailureCodeV1 {
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
pub struct CwirErrorV1 {
    pub code: CwirFailureCodeV1,
    pub detail: String,
}

impl CwirErrorV1 {
    pub fn new(code: CwirFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> CwirFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for CwirErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for CwirErrorV1 {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirNodeKindV1 {
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
pub enum CwirSoundnessV1 {
    Exact,
    SoundRestricted,
    EmpiricalIncomplete,
    Heuristic,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirCoverageV1 {
    Complete,
    ScopedComplete,
    Partial,
    ObservedOnly,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirFreshnessV1 {
    Current,
    Stale,
    Conflict,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirDeterminismV1 {
    Deterministic,
    Conditional,
    External,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirEpistemicProductV1 {
    pub authority: ArtifactOwnerV1,
    pub soundness: CwirSoundnessV1,
    pub coverage: CwirCoverageV1,
    pub freshness: CwirFreshnessV1,
    pub determinism: CwirDeterminismV1,
}

impl CwirEpistemicProductV1 {
    fn validate(self) -> Result<(), CwirErrorV1> {
        if self.soundness == CwirSoundnessV1::Exact
            && matches!(
                self.coverage,
                CwirCoverageV1::Partial | CwirCoverageV1::ObservedOnly | CwirCoverageV1::Unknown
            )
        {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::InvalidEpistemicProduct,
                "exact soundness requires complete or scoped-complete coverage",
            ));
        }
        if self.soundness == CwirSoundnessV1::Exact
            && self.determinism == CwirDeterminismV1::Unknown
        {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::InvalidEpistemicProduct,
                "exact soundness cannot have unknown determinism",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirTaskContractV1 {
    pub id: DigestV1,
    pub task_kind: String,
    pub specification_digest: DigestV1,
    pub required_snapshot: DigestV1,
}

#[derive(Serialize)]
struct TaskBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    task_kind: &'a str,
    specification_digest: DigestV1,
    required_snapshot: DigestV1,
}

impl CwirTaskContractV1 {
    pub fn new(
        task_kind: impl Into<String>,
        specification_digest: DigestV1,
        required_snapshot: DigestV1,
    ) -> Result<Self, CwirErrorV1> {
        let mut task = Self {
            id: DigestV1::ZERO,
            task_kind: task_kind.into(),
            specification_digest,
            required_snapshot,
        };
        task.validate_body()?;
        task.id = task.expected_id()?;
        Ok(task)
    }

    fn body(&self) -> TaskBodyV1<'_> {
        TaskBodyV1 {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            task_kind: &self.task_kind,
            specification_digest: self.specification_digest,
            required_snapshot: self.required_snapshot,
        }
    }

    fn expected_id(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_TASK_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        validate_identity("task_kind", &self.task_kind)?;
        validate_digest("specification_digest", self.specification_digest)?;
        validate_digest("required_snapshot", self.required_snapshot)
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        require_id("task", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirStateAnchorV1 {
    pub project_root: DigestV1,
    pub fs_snapshot: DigestV1,
    pub graph_indexed_through: DigestV1,
    pub toolchain: DigestV1,
    pub runtime_manifest: DigestV1,
    pub capability_surface: DigestV1,
}

impl CwirStateAnchorV1 {
    fn validate(self) -> Result<(), CwirErrorV1> {
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
pub struct CwirNodeV1 {
    pub id: DigestV1,
    pub kind: CwirNodeKindV1,
    pub payload_digest: DigestV1,
    pub required_snapshot: Option<DigestV1>,
    pub active: bool,
    pub epistemic: CwirEpistemicProductV1,
    pub provenance: Vec<DigestV1>,
}

#[derive(Serialize)]
struct NodeBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    kind: CwirNodeKindV1,
    payload_digest: DigestV1,
    required_snapshot: Option<DigestV1>,
    active: bool,
    epistemic: CwirEpistemicProductV1,
    provenance: &'a [DigestV1],
}

impl CwirNodeV1 {
    pub fn new(
        kind: CwirNodeKindV1,
        payload_digest: DigestV1,
        required_snapshot: Option<DigestV1>,
        active: bool,
        epistemic: CwirEpistemicProductV1,
        mut provenance: Vec<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        normalize_unique(&mut provenance, "node provenance")?;
        let mut node = Self {
            id: DigestV1::ZERO,
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

    fn body(&self) -> NodeBodyV1<'_> {
        NodeBodyV1 {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            kind: self.kind,
            payload_digest: self.payload_digest,
            required_snapshot: self.required_snapshot,
            active: self.active,
            epistemic: self.epistemic,
            provenance: &self.provenance,
        }
    }

    fn expected_id(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_NODE_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        validate_digest("node payload_digest", self.payload_digest)?;
        if let Some(snapshot) = self.required_snapshot {
            validate_digest("node required_snapshot", snapshot)?;
        }
        self.epistemic.validate()?;
        validate_sorted_unique(
            &self.provenance,
            "node provenance",
            CWIR_MAX_REFS_PER_ITEM_V1,
        )?;
        validate_digest_slice("node provenance", &self.provenance)
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        require_id("node", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirEdgeKindV1 {
    Supports,
    Derives,
    Contradicts,
    Discharges,
    Updates,
    Expands,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirHyperEdgeV1 {
    pub id: DigestV1,
    pub relation: CwirEdgeKindV1,
    pub sources: Vec<DigestV1>,
    pub target: DigestV1,
    pub proof_node: Option<DigestV1>,
}

#[derive(Serialize)]
struct EdgeBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    relation: CwirEdgeKindV1,
    sources: &'a [DigestV1],
    target: DigestV1,
    proof_node: Option<DigestV1>,
}

impl CwirHyperEdgeV1 {
    pub fn new(
        relation: CwirEdgeKindV1,
        mut sources: Vec<DigestV1>,
        target: DigestV1,
        proof_node: Option<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        normalize_unique(&mut sources, "hyperedge sources")?;
        let mut edge = Self {
            id: DigestV1::ZERO,
            relation,
            sources,
            target,
            proof_node,
        };
        edge.validate_body()?;
        edge.id = edge.expected_id()?;
        Ok(edge)
    }

    fn body(&self) -> EdgeBodyV1<'_> {
        EdgeBodyV1 {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            relation: self.relation,
            sources: &self.sources,
            target: self.target,
            proof_node: self.proof_node,
        }
    }

    fn expected_id(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_EDGE_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        if self.sources.is_empty() {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::InvalidHyperedge,
                "hyperedge sources must not be empty",
            ));
        }
        validate_sorted_unique(
            &self.sources,
            "hyperedge sources",
            CWIR_MAX_REFS_PER_ITEM_V1,
        )?;
        validate_digest_slice("hyperedge sources", &self.sources)?;
        validate_digest("hyperedge target", self.target)?;
        if let Some(proof) = self.proof_node {
            validate_digest("hyperedge proof_node", proof)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        require_id("hyperedge", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirObligationKindV1 {
    Decision,
    Execution,
    Verification,
    Restoration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirObligationStatusV1 {
    Open,
    InProgress,
    Discharged,
    Failed,
    Blocked,
    Waived,
}

impl CwirObligationStatusV1 {
    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Discharged | Self::Failed | Self::Blocked | Self::Waived
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirObligationV1 {
    pub id: DigestV1,
    pub kind: CwirObligationKindV1,
    pub advisory: bool,
    pub status: CwirObligationStatusV1,
    pub required_snapshot: DigestV1,
    pub dependencies: Vec<DigestV1>,
    pub resolution_evidence: Option<DigestV1>,
}

#[derive(Serialize)]
struct ObligationBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    kind: CwirObligationKindV1,
    advisory: bool,
    status: CwirObligationStatusV1,
    required_snapshot: DigestV1,
    dependencies: &'a [DigestV1],
    resolution_evidence: Option<DigestV1>,
}

impl CwirObligationV1 {
    pub fn new_open(
        kind: CwirObligationKindV1,
        advisory: bool,
        required_snapshot: DigestV1,
        mut dependencies: Vec<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        normalize_unique(&mut dependencies, "obligation dependencies")?;
        Self::from_parts(
            kind,
            advisory,
            CwirObligationStatusV1::Open,
            required_snapshot,
            dependencies,
            None,
        )
    }

    fn from_parts(
        kind: CwirObligationKindV1,
        advisory: bool,
        status: CwirObligationStatusV1,
        required_snapshot: DigestV1,
        dependencies: Vec<DigestV1>,
        resolution_evidence: Option<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        let mut obligation = Self {
            id: DigestV1::ZERO,
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
        next: CwirObligationStatusV1,
        resolution_evidence: Option<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        self.validate()?;
        let allowed = matches!(
            (self.status, next),
            (
                CwirObligationStatusV1::Open,
                CwirObligationStatusV1::InProgress
            ) | (
                CwirObligationStatusV1::Open,
                CwirObligationStatusV1::Discharged
            ) | (CwirObligationStatusV1::Open, CwirObligationStatusV1::Failed)
                | (
                    CwirObligationStatusV1::Open,
                    CwirObligationStatusV1::Blocked
                )
                | (CwirObligationStatusV1::Open, CwirObligationStatusV1::Waived)
                | (
                    CwirObligationStatusV1::InProgress,
                    CwirObligationStatusV1::Discharged
                )
                | (
                    CwirObligationStatusV1::InProgress,
                    CwirObligationStatusV1::Failed
                )
                | (
                    CwirObligationStatusV1::InProgress,
                    CwirObligationStatusV1::Blocked
                )
                | (
                    CwirObligationStatusV1::InProgress,
                    CwirObligationStatusV1::Waived
                )
        );
        if !allowed {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::InvalidObligationTransition,
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

    fn body(&self) -> ObligationBodyV1<'_> {
        ObligationBodyV1 {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            kind: self.kind,
            advisory: self.advisory,
            status: self.status,
            required_snapshot: self.required_snapshot,
            dependencies: &self.dependencies,
            resolution_evidence: self.resolution_evidence,
        }
    }

    fn expected_id(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_OBLIGATION_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        validate_digest("obligation required_snapshot", self.required_snapshot)?;
        validate_sorted_unique(
            &self.dependencies,
            "obligation dependencies",
            CWIR_MAX_REFS_PER_ITEM_V1,
        )?;
        validate_digest_slice("obligation dependencies", &self.dependencies)?;
        if self.status == CwirObligationStatusV1::Waived && !self.advisory {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::IllegalWaiver,
                "a non-advisory obligation cannot be waived",
            ));
        }
        if self.status.is_terminal() != self.resolution_evidence.is_some() {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::InvalidObligationStatus,
                "terminal obligations require resolution evidence and non-terminal obligations forbid it",
            ));
        }
        if let Some(evidence) = self.resolution_evidence {
            validate_digest("obligation resolution_evidence", evidence)?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        require_id("obligation", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirEffectSpaceV1 {
    pub effects: Vec<DigestV1>,
    pub capabilities: Vec<DigestV1>,
}

impl CwirEffectSpaceV1 {
    pub fn new(
        mut effects: Vec<DigestV1>,
        mut capabilities: Vec<DigestV1>,
    ) -> Result<Self, CwirErrorV1> {
        normalize_unique(&mut effects, "effect identities")?;
        normalize_unique(&mut capabilities, "capability identities")?;
        let result = Self {
            effects,
            capabilities,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        if self.effects.len() > CWIR_MAX_EFFECTS_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyEffects,
                format!("effect space contains {} effects", self.effects.len()),
            ));
        }
        if self.capabilities.len() > CWIR_MAX_CAPABILITIES_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyCapabilities,
                format!(
                    "effect space contains {} capabilities",
                    self.capabilities.len()
                ),
            ));
        }
        validate_sorted_unique(&self.effects, "effect identities", CWIR_MAX_EFFECTS_V1)?;
        validate_sorted_unique(
            &self.capabilities,
            "capability identities",
            CWIR_MAX_CAPABILITIES_V1,
        )?;
        validate_digest_slice("effect identities", &self.effects)?;
        validate_digest_slice("capability identities", &self.capabilities)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CwirVerifierClassV1 {
    ExactChecker,
    SoundRestricted,
    EmpiricalIncomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirVerificationContractV1 {
    pub verifier_digest: DigestV1,
    pub predicate_digest: DigestV1,
    pub scope_digest: DigestV1,
    pub class: CwirVerifierClassV1,
}

impl CwirVerificationContractV1 {
    fn validate(self) -> Result<(), CwirErrorV1> {
        validate_digest("verification verifier_digest", self.verifier_digest)?;
        validate_digest("verification predicate_digest", self.predicate_digest)?;
        validate_digest("verification scope_digest", self.scope_digest)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirExpansionCostV1 {
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_work_units: u64,
}

impl CwirExpansionCostV1 {
    fn validate(self) -> Result<(), CwirErrorV1> {
        if self.max_input_bytes == 0 || self.max_output_bytes == 0 || self.max_work_units == 0 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::ExpansionIncomplete,
                "expansion cost bounds must all be nonzero",
            ));
        }
        if self.max_input_bytes > CWIR_MAX_EXPANSION_INPUT_BYTES_V1
            || self.max_output_bytes > CWIR_MAX_EXPANSION_OUTPUT_BYTES_V1
            || self.max_work_units > CWIR_MAX_EXPANSION_WORK_UNITS_V1
        {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::ExpansionLimitExceeded,
                "expansion cost exceeds the CWIR v1 contract bound",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CwirExpansionV1 {
    pub id: DigestV1,
    pub owner: ArtifactOwnerV1,
    pub capability: String,
    pub arguments_digest: DigestV1,
    pub required_snapshot: DigestV1,
    pub cost: CwirExpansionCostV1,
}

#[derive(Serialize)]
struct ExpansionBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    owner: ArtifactOwnerV1,
    capability: &'a str,
    arguments_digest: DigestV1,
    required_snapshot: DigestV1,
    cost: CwirExpansionCostV1,
}

impl CwirExpansionV1 {
    pub fn new(
        owner: ArtifactOwnerV1,
        capability: impl Into<String>,
        arguments_digest: DigestV1,
        required_snapshot: DigestV1,
        cost: CwirExpansionCostV1,
    ) -> Result<Self, CwirErrorV1> {
        let mut expansion = Self {
            id: DigestV1::ZERO,
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

    fn body(&self) -> ExpansionBodyV1<'_> {
        ExpansionBodyV1 {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            owner: self.owner,
            capability: &self.capability,
            arguments_digest: self.arguments_digest,
            required_snapshot: self.required_snapshot,
            cost: self.cost,
        }
    }

    fn expected_id(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_EXPANSION_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        validate_identity("expansion capability", &self.capability)?;
        validate_digest("expansion arguments_digest", self.arguments_digest)?;
        validate_digest("expansion required_snapshot", self.required_snapshot)?;
        self.cost.validate()
    }

    fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        require_id("expansion", self.id, self.expected_id()?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkIrV1 {
    contract_version: u16,
    model_version: u16,
    task: CwirTaskContractV1,
    state: CwirStateAnchorV1,
    nodes: Vec<CwirNodeV1>,
    edges: Vec<CwirHyperEdgeV1>,
    obligations: Vec<CwirObligationV1>,
    effect_space: CwirEffectSpaceV1,
    verification: CwirVerificationContractV1,
    expansions: Vec<CwirExpansionV1>,
    semantic_digest: DigestV1,
}

#[derive(Serialize)]
struct CwirSemanticBodyV1<'a> {
    contract_version: u16,
    model_version: u16,
    task: &'a CwirTaskContractV1,
    state: CwirStateAnchorV1,
    nodes: &'a [CwirNodeV1],
    edges: &'a [CwirHyperEdgeV1],
    obligations: &'a [CwirObligationV1],
    effect_space: &'a CwirEffectSpaceV1,
    verification: CwirVerificationContractV1,
    expansions: &'a [CwirExpansionV1],
}

impl CausalWorkIrV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task: CwirTaskContractV1,
        state: CwirStateAnchorV1,
        mut nodes: Vec<CwirNodeV1>,
        mut edges: Vec<CwirHyperEdgeV1>,
        mut obligations: Vec<CwirObligationV1>,
        effect_space: CwirEffectSpaceV1,
        verification: CwirVerificationContractV1,
        mut expansions: Vec<CwirExpansionV1>,
    ) -> Result<Self, CwirErrorV1> {
        normalize_by_id(&mut nodes, |item| item.id, "nodes")?;
        normalize_by_id(&mut edges, |item| item.id, "hyperedges")?;
        normalize_by_id(&mut obligations, |item| item.id, "obligations")?;
        normalize_by_id(&mut expansions, |item| item.id, "expansions")?;
        let mut cwir = Self {
            contract_version: CWIR_CONTRACT_VERSION_V1,
            model_version: CWIR_MODEL_VERSION_V1,
            task,
            state,
            nodes,
            edges,
            obligations,
            effect_space,
            verification,
            expansions,
            semantic_digest: DigestV1::ZERO,
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

    pub const fn task(&self) -> &CwirTaskContractV1 {
        &self.task
    }

    pub const fn state(&self) -> &CwirStateAnchorV1 {
        &self.state
    }

    pub fn nodes(&self) -> &[CwirNodeV1] {
        &self.nodes
    }

    pub fn edges(&self) -> &[CwirHyperEdgeV1] {
        &self.edges
    }

    pub fn obligations(&self) -> &[CwirObligationV1] {
        &self.obligations
    }

    pub const fn effect_space(&self) -> &CwirEffectSpaceV1 {
        &self.effect_space
    }

    pub const fn verification(&self) -> &CwirVerificationContractV1 {
        &self.verification
    }

    pub fn expansions(&self) -> &[CwirExpansionV1] {
        &self.expansions
    }

    pub const fn semantic_digest(&self) -> DigestV1 {
        self.semantic_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CwirErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        let bytes = canonical_json(&value).into_bytes();
        if bytes.len() > CWIR_MAX_CANONICAL_BYTES_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::CanonicalPayloadTooLarge,
                format!("CWIR payload is {} bytes", bytes.len()),
            ));
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, CwirErrorV1> {
        if bytes.len() > CWIR_MAX_CANONICAL_BYTES_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::CanonicalPayloadTooLarge,
                format!("CWIR payload is {} bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::NonCanonicalEncoding,
                "CWIR bytes are not exact sorted-key JSON without whitespace",
            ));
        }
        let cwir: Self = serde_json::from_value(value).map_err(serialization_error)?;
        cwir.validate()?;
        Ok(cwir)
    }

    pub fn validate(&self) -> Result<(), CwirErrorV1> {
        self.validate_body()?;
        let expected = self.expected_semantic_digest()?;
        if self.semantic_digest != expected {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::SemanticDigestMismatch,
                format!(
                    "semantic digest {} does not match canonical body {}",
                    self.semantic_digest.to_hex(),
                    expected.to_hex()
                ),
            ));
        }
        Ok(())
    }

    fn body(&self) -> CwirSemanticBodyV1<'_> {
        CwirSemanticBodyV1 {
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

    fn expected_semantic_digest(&self) -> Result<DigestV1, CwirErrorV1> {
        digest_body(CWIR_SEMANTIC_DOMAIN_V1, &self.body())
    }

    fn validate_body(&self) -> Result<(), CwirErrorV1> {
        if self.contract_version != CWIR_CONTRACT_VERSION_V1
            || self.model_version != CWIR_MODEL_VERSION_V1
        {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::UnsupportedVersion,
                format!(
                    "unsupported CWIR contract/model version {}/{}",
                    self.contract_version, self.model_version
                ),
            ));
        }
        if self.nodes.len() > CWIR_MAX_NODES_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyNodes,
                "node bound exceeded",
            ));
        }
        if self.edges.len() > CWIR_MAX_EDGES_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyEdges,
                "edge bound exceeded",
            ));
        }
        if self.obligations.len() > CWIR_MAX_OBLIGATIONS_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyObligations,
                "obligation bound exceeded",
            ));
        }
        if self.expansions.len() > CWIR_MAX_EXPANSIONS_V1 {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::TooManyExpansions,
                "expansion bound exceeded",
            ));
        }
        self.task.validate()?;
        self.state.validate()?;
        if self.task.required_snapshot != self.state.fs_snapshot {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::SnapshotMismatch,
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
            if node.kind == CwirNodeKindV1::Evidence && node.active {
                if node.epistemic.freshness != CwirFreshnessV1::Current {
                    return Err(CwirErrorV1::new(
                        CwirFailureCodeV1::StaleFact,
                        format!("active evidence node {} is not current", node.id.to_hex()),
                    ));
                }
                if node.required_snapshot != Some(self.state.fs_snapshot) {
                    return Err(CwirErrorV1::new(
                        CwirFailureCodeV1::SnapshotMismatch,
                        format!(
                            "active evidence node {} is not bound to the state snapshot",
                            node.id.to_hex()
                        ),
                    ));
                }
                if node.epistemic.soundness == CwirSoundnessV1::Exact && node.provenance.is_empty()
                {
                    return Err(CwirErrorV1::new(
                        CwirFailureCodeV1::MissingProvenance,
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
                return Err(CwirErrorV1::new(
                    CwirFailureCodeV1::SnapshotMismatch,
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
                    return Err(CwirErrorV1::new(
                        CwirFailureCodeV1::DanglingReference,
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
                return Err(CwirErrorV1::new(
                    CwirFailureCodeV1::SnapshotMismatch,
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

pub fn cwir_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.cwir",
        "contract_version": CWIR_CONTRACT_VERSION_V1,
        "model_version": CWIR_MODEL_VERSION_V1,
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
            "max_canonical_bytes": CWIR_MAX_CANONICAL_BYTES_V1,
            "max_nodes": CWIR_MAX_NODES_V1,
            "max_edges": CWIR_MAX_EDGES_V1,
            "max_obligations": CWIR_MAX_OBLIGATIONS_V1,
            "max_expansions": CWIR_MAX_EXPANSIONS_V1,
            "max_references_per_item": CWIR_MAX_REFS_PER_ITEM_V1,
            "max_effects": CWIR_MAX_EFFECTS_V1,
            "max_capabilities": CWIR_MAX_CAPABILITIES_V1,
            "max_identity_bytes": CWIR_MAX_IDENTITY_BYTES_V1,
            "max_expansion_input_bytes": CWIR_MAX_EXPANSION_INPUT_BYTES_V1,
            "max_expansion_output_bytes": CWIR_MAX_EXPANSION_OUTPUT_BYTES_V1,
            "max_expansion_work_units": CWIR_MAX_EXPANSION_WORK_UNITS_V1
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
pub fn cwir_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&cwir_contract_manifest_v1()).as_bytes(),
    ))
}

fn digest_body<T: Serialize>(domain: &[u8], value: &T) -> Result<DigestV1, CwirErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let canonical = canonical_json(&value);
    let mut bytes = Vec::with_capacity(domain.len() + canonical.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(canonical.as_bytes());
    Ok(DigestV1::from_bytes(sha256(&bytes)))
}

fn serialization_error(error: serde_json::Error) -> CwirErrorV1 {
    CwirErrorV1::new(CwirFailureCodeV1::SerializationFailure, error.to_string())
}

fn validate_identity(label: &str, value: &str) -> Result<(), CwirErrorV1> {
    if value.is_empty()
        || value.len() > CWIR_MAX_IDENTITY_BYTES_V1
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        return Err(CwirErrorV1::new(
            CwirFailureCodeV1::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ));
    }
    Ok(())
}

fn validate_digest(label: &str, digest: DigestV1) -> Result<(), CwirErrorV1> {
    if digest == DigestV1::ZERO {
        Err(CwirErrorV1::new(
            CwirFailureCodeV1::ZeroDigest,
            format!("{label} must not be the zero digest"),
        ))
    } else {
        Ok(())
    }
}

fn validate_digest_slice(label: &str, values: &[DigestV1]) -> Result<(), CwirErrorV1> {
    for digest in values {
        validate_digest(label, *digest)?;
    }
    Ok(())
}

fn require_id(label: &str, actual: DigestV1, expected: DigestV1) -> Result<(), CwirErrorV1> {
    if actual == expected {
        Ok(())
    } else {
        Err(CwirErrorV1::new(
            CwirFailureCodeV1::IdentityMismatch,
            format!(
                "{label} id {} does not match canonical body {}",
                actual.to_hex(),
                expected.to_hex()
            ),
        ))
    }
}

fn normalize_unique<T: Ord>(values: &mut Vec<T>, label: &str) -> Result<(), CwirErrorV1> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CwirErrorV1::new(
            CwirFailureCodeV1::DuplicateIdentity,
            format!("{label} contains a duplicate member"),
        ));
    }
    Ok(())
}

fn validate_sorted_unique<T: Ord>(
    values: &[T],
    label: &str,
    max: usize,
) -> Result<(), CwirErrorV1> {
    if values.len() > max {
        return Err(CwirErrorV1::new(
            CwirFailureCodeV1::TooManyReferences,
            format!(
                "{label} contains {} members, maximum is {max}",
                values.len()
            ),
        ));
    }
    for pair in values.windows(2) {
        if pair[0] == pair[1] {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::DuplicateIdentity,
                format!("{label} contains a duplicate member"),
            ));
        }
        if pair[0] > pair[1] {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::NonCanonicalOrder,
                format!("{label} is not strictly sorted"),
            ));
        }
    }
    Ok(())
}

fn normalize_by_id<T, F>(values: &mut Vec<T>, id: F, label: &str) -> Result<(), CwirErrorV1>
where
    F: Fn(&T) -> DigestV1,
{
    values.sort_by_key(&id);
    if values.windows(2).any(|pair| id(&pair[0]) == id(&pair[1])) {
        return Err(CwirErrorV1::new(
            CwirFailureCodeV1::DuplicateIdentity,
            format!("{label} contains a duplicate identity"),
        ));
    }
    Ok(())
}

fn validate_id_order<T, F>(values: &[T], id: F, label: &str) -> Result<(), CwirErrorV1>
where
    F: Fn(&T) -> DigestV1,
{
    for pair in values.windows(2) {
        let left = id(&pair[0]);
        let right = id(&pair[1]);
        if left == right {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::DuplicateIdentity,
                format!("{label} contains a duplicate identity"),
            ));
        }
        if left > right {
            return Err(CwirErrorV1::new(
                CwirFailureCodeV1::NonCanonicalOrder,
                format!("{label} is not sorted by identity"),
            ));
        }
    }
    Ok(())
}

fn require_reference(
    nodes: &BTreeMap<DigestV1, CwirNodeKindV1>,
    id: DigestV1,
    label: &str,
) -> Result<(), CwirErrorV1> {
    if nodes.contains_key(&id) {
        Ok(())
    } else {
        Err(CwirErrorV1::new(
            CwirFailureCodeV1::DanglingReference,
            format!("{label} {} does not resolve", id.to_hex()),
        ))
    }
}

fn require_proof_node(
    nodes: &BTreeMap<DigestV1, CwirNodeKindV1>,
    id: DigestV1,
    label: &str,
) -> Result<(), CwirErrorV1> {
    match nodes.get(&id) {
        Some(CwirNodeKindV1::Witness | CwirNodeKindV1::Verification) => Ok(()),
        Some(kind) => Err(CwirErrorV1::new(
            CwirFailureCodeV1::InvalidResolutionEvidence,
            format!(
                "{label} {} resolves to {kind:?}, not witness or verification",
                id.to_hex()
            ),
        )),
        None => Err(CwirErrorV1::new(
            CwirFailureCodeV1::DanglingReference,
            format!("{label} {} does not resolve", id.to_hex()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn epistemic(soundness: CwirSoundnessV1) -> CwirEpistemicProductV1 {
        CwirEpistemicProductV1 {
            authority: ArtifactOwnerV1::FsZero,
            soundness,
            coverage: CwirCoverageV1::Complete,
            freshness: CwirFreshnessV1::Current,
            determinism: CwirDeterminismV1::Deterministic,
        }
    }

    fn sample(reverse: bool, soundness: CwirSoundnessV1, state_byte: u8) -> CausalWorkIrV1 {
        let snapshot = digest(state_byte);
        let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
        let state = CwirStateAnchorV1 {
            project_root: digest(3),
            fs_snapshot: snapshot,
            graph_indexed_through: digest(4),
            toolchain: digest(5),
            runtime_manifest: digest(6),
            capability_surface: digest(7),
        };
        let state_node = CwirNodeV1::new(
            CwirNodeKindV1::State,
            digest(8),
            Some(snapshot),
            true,
            epistemic(CwirSoundnessV1::Exact),
            vec![],
        )
        .unwrap();
        let evidence = CwirNodeV1::new(
            CwirNodeKindV1::Evidence,
            digest(9),
            Some(snapshot),
            true,
            epistemic(soundness),
            vec![state_node.id],
        )
        .unwrap();
        let witness = CwirNodeV1::new(
            CwirNodeKindV1::Witness,
            digest(10),
            Some(snapshot),
            true,
            epistemic(CwirSoundnessV1::Exact),
            vec![evidence.id],
        )
        .unwrap();
        let edge = CwirHyperEdgeV1::new(
            CwirEdgeKindV1::Supports,
            vec![state_node.id, evidence.id],
            witness.id,
            Some(witness.id),
        )
        .unwrap();
        let obligation =
            CwirObligationV1::new_open(CwirObligationKindV1::Verification, false, snapshot, vec![])
                .unwrap()
                .transition(CwirObligationStatusV1::Discharged, Some(witness.id))
                .unwrap();
        let expansion = CwirExpansionV1::new(
            ArtifactOwnerV1::GraphZero,
            "graph.expand",
            digest(11),
            snapshot,
            CwirExpansionCostV1 {
                max_input_bytes: 512,
                max_output_bytes: 1024,
                max_work_units: 100,
            },
        )
        .unwrap();
        let mut nodes = vec![state_node, evidence, witness];
        if reverse {
            nodes.reverse();
        }
        CausalWorkIrV1::new(
            task,
            state,
            nodes,
            vec![edge],
            vec![obligation],
            CwirEffectSpaceV1::new(vec![digest(12)], vec![digest(14), digest(13)]).unwrap(),
            CwirVerificationContractV1 {
                verifier_digest: digest(15),
                predicate_digest: digest(16),
                scope_digest: digest(17),
                class: CwirVerifierClassV1::ExactChecker,
            },
            vec![expansion],
        )
        .unwrap()
    }

    #[test]
    fn canonical_round_trip_and_contract_digest_are_stable() {
        let cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let bytes = cwir.canonical_bytes().unwrap();
        assert_eq!(CausalWorkIrV1::from_canonical_bytes(&bytes).unwrap(), cwir);
        assert_eq!(
            cwir_contract_digest_v1().to_hex(),
            "f64a0d73c075bb7330943379d52a1d2da6bb9272f02d6b15254baf829b32b30c"
        );
    }

    #[test]
    fn insertion_order_is_semantically_invariant() {
        let left = sample(false, CwirSoundnessV1::Exact, 1);
        let right = sample(true, CwirSoundnessV1::Exact, 1);
        assert_eq!(left.semantic_digest(), right.semantic_digest());
        assert_eq!(
            left.canonical_bytes().unwrap(),
            right.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn every_epistemic_and_state_field_changes_semantic_identity() {
        let base = sample(false, CwirSoundnessV1::Exact, 1);
        let base_digest = base.semantic_digest();
        let semantic_with_state = |state: CwirStateAnchorV1| {
            CausalWorkIrV1::new(
                base.task.clone(),
                state,
                base.nodes.clone(),
                base.edges.clone(),
                base.obligations.clone(),
                base.effect_space.clone(),
                base.verification,
                base.expansions.clone(),
            )
            .unwrap()
            .semantic_digest()
        };
        let mut state = base.state;
        state.project_root = digest(18);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.graph_indexed_through = digest(19);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.toolchain = digest(20);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.runtime_manifest = digest(21);
        assert_ne!(base_digest, semantic_with_state(state));
        let mut state = base.state;
        state.capability_surface = digest(22);
        assert_ne!(base_digest, semantic_with_state(state));
        assert_ne!(
            base_digest,
            sample(false, CwirSoundnessV1::Exact, 23).semantic_digest()
        );

        let semantic_with_epistemic = |epistemic: CwirEpistemicProductV1| {
            let snapshot = digest(1);
            let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
            let state = CwirStateAnchorV1 {
                project_root: digest(3),
                fs_snapshot: snapshot,
                graph_indexed_through: digest(4),
                toolchain: digest(5),
                runtime_manifest: digest(6),
                capability_surface: digest(7),
            };
            let claim = CwirNodeV1::new(
                CwirNodeKindV1::Claim,
                digest(8),
                Some(snapshot),
                true,
                epistemic,
                vec![],
            )
            .unwrap();
            CausalWorkIrV1::new(
                task,
                state,
                vec![claim],
                vec![],
                vec![],
                CwirEffectSpaceV1::new(vec![], vec![]).unwrap(),
                CwirVerificationContractV1 {
                    verifier_digest: digest(15),
                    predicate_digest: digest(16),
                    scope_digest: digest(17),
                    class: CwirVerifierClassV1::ExactChecker,
                },
                vec![],
            )
            .unwrap()
            .semantic_digest()
        };
        let base_epistemic = epistemic(CwirSoundnessV1::Exact);
        let base_epistemic_digest = semantic_with_epistemic(base_epistemic);
        let mut changed = base_epistemic;
        changed.authority = ArtifactOwnerV1::GraphZero;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.soundness = CwirSoundnessV1::SoundRestricted;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.coverage = CwirCoverageV1::ScopedComplete;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.freshness = CwirFreshnessV1::Unknown;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
        let mut changed = base_epistemic;
        changed.determinism = CwirDeterminismV1::Conditional;
        assert_ne!(base_epistemic_digest, semantic_with_epistemic(changed));
    }

    #[test]
    fn stale_or_unbound_active_evidence_is_rejected() {
        let invalid_evidence = |required_snapshot: Option<DigestV1>, freshness: CwirFreshnessV1| {
            let snapshot = digest(1);
            let task = CwirTaskContractV1::new("edit", digest(2), snapshot).unwrap();
            let state = CwirStateAnchorV1 {
                project_root: digest(3),
                fs_snapshot: snapshot,
                graph_indexed_through: digest(4),
                toolchain: digest(5),
                runtime_manifest: digest(6),
                capability_surface: digest(7),
            };
            let state_node = CwirNodeV1::new(
                CwirNodeKindV1::State,
                digest(8),
                Some(snapshot),
                true,
                epistemic(CwirSoundnessV1::Exact),
                vec![],
            )
            .unwrap();
            let mut evidence_epistemic = epistemic(CwirSoundnessV1::Exact);
            evidence_epistemic.freshness = freshness;
            let evidence = CwirNodeV1::new(
                CwirNodeKindV1::Evidence,
                digest(9),
                required_snapshot,
                true,
                evidence_epistemic,
                vec![state_node.id],
            )
            .unwrap();
            CausalWorkIrV1::new(
                task,
                state,
                vec![state_node, evidence],
                vec![],
                vec![],
                CwirEffectSpaceV1::new(vec![], vec![]).unwrap(),
                CwirVerificationContractV1 {
                    verifier_digest: digest(15),
                    predicate_digest: digest(16),
                    scope_digest: digest(17),
                    class: CwirVerifierClassV1::ExactChecker,
                },
                vec![],
            )
            .unwrap_err()
            .failure_code()
        };

        assert_eq!(
            invalid_evidence(Some(digest(99)), CwirFreshnessV1::Current),
            CwirFailureCodeV1::SnapshotMismatch
        );
        assert_eq!(
            invalid_evidence(None, CwirFreshnessV1::Current),
            CwirFailureCodeV1::SnapshotMismatch
        );
        assert_eq!(
            invalid_evidence(Some(digest(1)), CwirFreshnessV1::Stale),
            CwirFailureCodeV1::StaleFact
        );
    }

    #[test]
    fn dangling_and_duplicate_references_are_rejected() {
        let error = CwirNodeV1::new(
            CwirNodeKindV1::Claim,
            digest(20),
            None,
            false,
            epistemic(CwirSoundnessV1::SoundRestricted),
            vec![digest(21), digest(21)],
        )
        .unwrap_err();
        assert_eq!(error.failure_code(), CwirFailureCodeV1::DuplicateIdentity);

        let mut cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let node = &mut cwir.nodes[0];
        node.provenance.push(digest(250));
        node.provenance.sort();
        node.id = node.expected_id().unwrap();
        cwir.nodes.sort_by_key(|item| item.id);
        assert_eq!(
            cwir.validate_body().unwrap_err().failure_code(),
            CwirFailureCodeV1::DanglingReference
        );
    }

    #[test]
    fn obligation_lifecycle_is_monotone_and_non_advisory_cannot_be_waived() {
        let obligation =
            CwirObligationV1::new_open(CwirObligationKindV1::Decision, false, digest(1), vec![])
                .unwrap();
        let error = obligation
            .transition(CwirObligationStatusV1::Waived, Some(digest(2)))
            .unwrap_err();
        assert_eq!(error.failure_code(), CwirFailureCodeV1::IllegalWaiver);
        let discharged = obligation
            .transition(CwirObligationStatusV1::Discharged, Some(digest(2)))
            .unwrap();
        assert_eq!(
            discharged
                .transition(CwirObligationStatusV1::InProgress, None)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::InvalidObligationTransition
        );
    }

    #[test]
    fn noncanonical_and_tampered_wire_bytes_are_rejected() {
        let cwir = sample(false, CwirSoundnessV1::Exact, 1);
        let mut bytes = cwir.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::NonCanonicalEncoding
        );

        let mut value = serde_json::to_value(&cwir).unwrap();
        value["semantic_digest"] = Value::String(digest(99).to_hex());
        let bytes = canonical_json(&value).into_bytes();
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::SemanticDigestMismatch
        );

        let mut value = serde_json::to_value(&cwir).unwrap();
        value["nodes"].as_array_mut().unwrap().swap(0, 1);
        let bytes = canonical_json(&value).into_bytes();
        assert_eq!(
            CausalWorkIrV1::from_canonical_bytes(&bytes)
                .unwrap_err()
                .failure_code(),
            CwirFailureCodeV1::NonCanonicalOrder
        );
    }

    #[test]
    fn exact_epistemic_status_fails_closed() {
        let mut invalid = epistemic(CwirSoundnessV1::Exact);
        invalid.coverage = CwirCoverageV1::Partial;
        assert_eq!(
            CwirNodeV1::new(
                CwirNodeKindV1::Evidence,
                digest(1),
                Some(digest(2)),
                true,
                invalid,
                vec![digest(3)],
            )
            .unwrap_err()
            .failure_code(),
            CwirFailureCodeV1::InvalidEpistemicProduct
        );
    }

    #[test]
    fn expansion_bounds_fail_loud() {
        let error = CwirExpansionV1::new(
            ArtifactOwnerV1::FsZero,
            "fs.expand",
            digest(1),
            digest(2),
            CwirExpansionCostV1 {
                max_input_bytes: CWIR_MAX_EXPANSION_INPUT_BYTES_V1 + 1,
                max_output_bytes: 1,
                max_work_units: 1,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.failure_code(),
            CwirFailureCodeV1::ExpansionLimitExceeded
        );
    }
}
