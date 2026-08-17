//! Engine-neutral certified invalidation and freshness contract.
//!
//! Engines own dependency discovery. This module freezes only shared identities,
//! canonical digests, closure comparison, and fail-closed outcomes. Wall clock is
//! deliberately absent: repository, assembly, index, and closure identities are
//! the only freshness authority.

use crate::{Sha256Digest, canonical_json, sha256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const FRESHNESS_CONTRACT_VERSION: u16 = 1;
pub const FRESHNESS_MODEL_VERSION: &str = "zerostack.invalidation-freshness";
pub const FRESHNESS_MAX_REPOSITORIES: usize = 64;
pub const FRESHNESS_MAX_NODES: usize = 4_096;
pub const FRESHNESS_MAX_EDGES: usize = 16_384;
pub const FRESHNESS_MAX_WITNESSES: usize = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerDomain {
    Source,
    FilesystemIndex,
    GraphIndex,
    TokenCache,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessHead {
    pub repository: String,
    pub head: String,
}

impl FreshnessHead {
    pub fn new(
        repository: impl Into<String>,
        head: impl Into<String>,
    ) -> Result<Self, FreshnessError> {
        let value = Self {
            repository: repository.into(),
            head: head.into(),
        };
        validate_identity("repository", &value.repository)?;
        validate_identity("source head", &value.head)?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyEdgeKind {
    Reads,
    Derives,
    Invalidates,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyEdge {
    pub producer: String,
    pub consumer: String,
    pub kind: DependencyEdgeKind,
}

impl DependencyEdge {
    pub fn new(
        producer: impl Into<String>,
        consumer: impl Into<String>,
        kind: DependencyEdgeKind,
    ) -> Result<Self, FreshnessError> {
        let value = Self {
            producer: producer.into(),
            consumer: consumer.into(),
            kind,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), FreshnessError> {
        validate_identity("edge producer", &self.producer)?;
        validate_identity("edge consumer", &self.consumer)?;
        if self.producer == self.consumer {
            return Err(FreshnessError::new(
                FreshnessFailureCode::InvalidIdentity,
                "dependency edge cannot be a self edge",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EssentialDependencyWitness {
    pub path: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EssentialDependencyCertificate {
    pub schema_version: u16,
    pub dependency: DependencyEdge,
    pub witness: EssentialDependencyWitness,
    pub certificate_digest: Sha256Digest,
}

impl EssentialDependencyCertificate {
    pub fn new(dependency: DependencyEdge, path: Vec<String>) -> Result<Self, FreshnessError> {
        let mut value = Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            dependency,
            witness: EssentialDependencyWitness { path },
            certificate_digest: Sha256Digest::ZERO,
        };
        value.validate_payload()?;
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_digest(&self) -> Sha256Digest {
        digest_json(
            &json!({"schema_version": self.schema_version, "dependency": self.dependency, "witness": self.witness}),
        )
    }

    fn validate_payload(&self) -> Result<(), FreshnessError> {
        validate_version(self.schema_version)?;
        self.dependency.validate()?;
        if !(2..=FRESHNESS_MAX_NODES).contains(&self.witness.path.len()) {
            return Err(FreshnessError::new(
                FreshnessFailureCode::MissingProofScope,
                "essential dependency witness path must contain 2..=4096 nodes",
            ));
        }
        for node in &self.witness.path {
            validate_identity("witness node", node)?;
        }
        if self.witness.path.first() != Some(&self.dependency.producer)
            || self.witness.path.last() != Some(&self.dependency.consumer)
        {
            return Err(FreshnessError::new(
                FreshnessFailureCode::MissingProofScope,
                "essential dependency witness endpoints do not match its edge",
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), FreshnessError> {
        self.validate_payload()?;
        require_digest(self.certificate_digest, self.expected_digest())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertifiedInfluenceClosure {
    pub schema_version: u16,
    pub model_version: String,
    pub assembly_manifest_digest: Sha256Digest,
    pub source_repository_heads: Vec<FreshnessHead>,
    pub producer_domains: Vec<ProducerDomain>,
    pub influence_scope: Vec<String>,
    pub edges: Vec<DependencyEdge>,
    pub essential_dependencies: Vec<EssentialDependencyCertificate>,
    pub certificate_digest: Sha256Digest,
}

impl CertifiedInfluenceClosure {
    pub fn new(
        assembly_manifest_digest: Sha256Digest,
        mut source_repository_heads: Vec<FreshnessHead>,
        mut producer_domains: Vec<ProducerDomain>,
        mut influence_scope: Vec<String>,
        mut edges: Vec<DependencyEdge>,
        mut essential_dependencies: Vec<EssentialDependencyCertificate>,
    ) -> Result<Self, FreshnessError> {
        sort_unique(&mut source_repository_heads, "source repository head")?;
        sort_unique(&mut producer_domains, "producer domain")?;
        sort_unique(&mut influence_scope, "influence scope node")?;
        sort_unique(&mut edges, "dependency edge")?;
        sort_unique(
            &mut essential_dependencies,
            "essential dependency certificate",
        )?;
        let mut value = Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            model_version: FRESHNESS_MODEL_VERSION.into(),
            assembly_manifest_digest,
            source_repository_heads,
            producer_domains,
            influence_scope,
            edges,
            essential_dependencies,
            certificate_digest: Sha256Digest::ZERO,
        };
        value.validate_payload()?;
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_digest(&self) -> Sha256Digest {
        digest_json(&json!({
            "schema_version": self.schema_version,
            "model_version": self.model_version,
            "assembly_manifest_digest": self.assembly_manifest_digest,
            "source_repository_heads": self.source_repository_heads,
            "producer_domains": self.producer_domains,
            "influence_scope": self.influence_scope,
            "edges": self.edges,
            "essential_dependencies": self.essential_dependencies,
        }))
    }

    fn validate_payload(&self) -> Result<(), FreshnessError> {
        validate_version(self.schema_version)?;
        if self.model_version != FRESHNESS_MODEL_VERSION {
            return Err(FreshnessError::new(
                FreshnessFailureCode::ModelVersionMismatch,
                "unsupported freshness model version",
            ));
        }
        if self.assembly_manifest_digest == Sha256Digest::ZERO {
            return Err(FreshnessError::new(
                FreshnessFailureCode::AssemblyMismatch,
                "assembly manifest digest cannot be zero",
            ));
        }
        if self.source_repository_heads.is_empty()
            || self.producer_domains.is_empty()
            || self.influence_scope.is_empty()
        {
            return Err(FreshnessError::new(
                FreshnessFailureCode::MissingProofScope,
                "source heads, producer domains, and influence scope cannot be empty",
            ));
        }
        if self.source_repository_heads.len() > FRESHNESS_MAX_REPOSITORIES
            || self.influence_scope.len() > FRESHNESS_MAX_NODES
            || self.edges.len() > FRESHNESS_MAX_EDGES
            || self.essential_dependencies.len() > FRESHNESS_MAX_WITNESSES
        {
            return Err(FreshnessError::new(
                FreshnessFailureCode::ContractLimitExceeded,
                "freshness closure exceeds a frozen limit",
            ));
        }
        require_strict_order(&self.source_repository_heads, "source repository heads")?;
        require_strict_order(&self.producer_domains, "producer domains")?;
        require_strict_order(&self.influence_scope, "influence scope")?;
        require_strict_order(&self.edges, "dependency edges")?;
        require_strict_order(
            &self.essential_dependencies,
            "essential dependency certificates",
        )?;
        if self
            .source_repository_heads
            .windows(2)
            .any(|pair| pair[0].repository == pair[1].repository)
        {
            return Err(FreshnessError::new(
                FreshnessFailureCode::DuplicateIdentity,
                "source repository identity appears with more than one head",
            ));
        }
        if self
            .essential_dependencies
            .windows(2)
            .any(|pair| pair[0].dependency == pair[1].dependency)
        {
            return Err(FreshnessError::new(
                FreshnessFailureCode::DuplicateIdentity,
                "essential dependency has more than one certificate",
            ));
        }
        for head in &self.source_repository_heads {
            validate_identity("repository", &head.repository)?;
            validate_identity("source head", &head.head)?;
        }
        for node in &self.influence_scope {
            validate_identity("influence scope node", node)?;
        }
        let scope: BTreeSet<&str> = self.influence_scope.iter().map(String::as_str).collect();
        for edge in &self.edges {
            edge.validate()?;
            if !scope.contains(edge.producer.as_str()) || !scope.contains(edge.consumer.as_str()) {
                return Err(FreshnessError::new(
                    FreshnessFailureCode::MissingProofScope,
                    "dependency edge endpoint is outside influence scope",
                ));
            }
        }
        let edges: BTreeSet<&DependencyEdge> = self.edges.iter().collect();
        for certificate in &self.essential_dependencies {
            certificate.validate()?;
            if !edges.contains(&certificate.dependency) {
                return Err(FreshnessError::new(
                    FreshnessFailureCode::MissingEdge,
                    "essential dependency is absent from the certified edge set",
                ));
            }
            for pair in certificate.witness.path.windows(2) {
                if !self
                    .edges
                    .iter()
                    .any(|edge| edge.producer == pair[0] && edge.consumer == pair[1])
                {
                    return Err(FreshnessError::new(
                        FreshnessFailureCode::MissingEdge,
                        "essential dependency witness traverses an uncertified edge",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), FreshnessError> {
        self.validate_payload()?;
        require_digest(self.certificate_digest, self.expected_digest())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FreshnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        Ok(canonical_json(&value).into_bytes())
    }
}

pub fn influence_closure(
    assembly_manifest_digest: Sha256Digest,
    source_repository_heads: Vec<FreshnessHead>,
    producer_domains: Vec<ProducerDomain>,
    influence_scope: Vec<String>,
    edges: Vec<DependencyEdge>,
    essential_dependencies: Vec<EssentialDependencyCertificate>,
) -> Result<CertifiedInfluenceClosure, FreshnessError> {
    CertifiedInfluenceClosure::new(
        assembly_manifest_digest,
        source_repository_heads,
        producer_domains,
        influence_scope,
        edges,
        essential_dependencies,
    )
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedThroughCertificate {
    pub schema_version: u16,
    pub model_version: String,
    pub index_id: String,
    pub index_generation: u64,
    pub influence: CertifiedInfluenceClosure,
    pub replay_identity: Sha256Digest,
    pub certificate_digest: Sha256Digest,
}

impl IndexedThroughCertificate {
    pub fn new(
        index_id: impl Into<String>,
        index_generation: u64,
        influence: CertifiedInfluenceClosure,
    ) -> Result<Self, FreshnessError> {
        let mut value = Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            model_version: FRESHNESS_MODEL_VERSION.into(),
            index_id: index_id.into(),
            index_generation,
            influence,
            replay_identity: Sha256Digest::ZERO,
            certificate_digest: Sha256Digest::ZERO,
        };
        value.validate_payload()?;
        value.replay_identity = value.expected_replay_identity();
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_replay_identity(&self) -> Sha256Digest {
        digest_json(
            &json!({"domain": "zerostack.freshness.replay", "index_id": self.index_id, "index_generation": self.index_generation, "influence_digest": self.influence.certificate_digest}),
        )
    }

    fn expected_digest(&self) -> Sha256Digest {
        digest_json(
            &json!({"schema_version": self.schema_version, "model_version": self.model_version, "index_id": self.index_id, "index_generation": self.index_generation, "influence": self.influence, "replay_identity": self.replay_identity}),
        )
    }

    fn validate_payload(&self) -> Result<(), FreshnessError> {
        validate_version(self.schema_version)?;
        if self.model_version != FRESHNESS_MODEL_VERSION {
            return Err(FreshnessError::new(
                FreshnessFailureCode::ModelVersionMismatch,
                "unsupported indexed-through model version",
            ));
        }
        validate_identity("index id", &self.index_id)?;
        self.influence.validate()
    }

    pub fn validate(&self) -> Result<(), FreshnessError> {
        self.validate_payload()?;
        if self.replay_identity != self.expected_replay_identity() {
            return Err(FreshnessError::new(
                FreshnessFailureCode::ReplayIdentityMismatch,
                "indexed-through replay identity mismatch",
            ));
        }
        require_digest(self.certificate_digest, self.expected_digest())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FreshnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        Ok(canonical_json(&value).into_bytes())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessStatus {
    Fresh,
    IndexBehind,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FreshnessFailureCode {
    UnsupportedSchemaVersion,
    ModelVersionMismatch,
    InvalidIdentity,
    DuplicateIdentity,
    NonCanonicalOrder,
    ContractLimitExceeded,
    AssemblyMismatch,
    SourceHeadMismatch,
    MissingProofScope,
    MissingEdge,
    ScopeInflation,
    IncomparableScope,
    GenerationRollback,
    ReplayIdentityMismatch,
    CertificateDigestMismatch,
    SerializationFailure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessDecision {
    pub schema_version: u16,
    pub status: FreshnessStatus,
    pub trusted: bool,
    pub failure_code: Option<FreshnessFailureCode>,
    pub detail: String,
    pub indexed_certificate_digest: Option<Sha256Digest>,
}

impl FreshnessDecision {
    fn fresh(indexed: &IndexedThroughCertificate) -> Self {
        Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            status: FreshnessStatus::Fresh,
            trusted: true,
            failure_code: None,
            detail: "exact assembly, source heads, influence scope, and frontier certified".into(),
            indexed_certificate_digest: Some(indexed.certificate_digest),
        }
    }
    fn rejected(
        status: FreshnessStatus,
        failure_code: FreshnessFailureCode,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            status,
            trusted: false,
            failure_code: Some(failure_code),
            detail: detail.into(),
            indexed_certificate_digest: None,
        }
    }
}

pub fn decide_freshness(
    required: &CertifiedInfluenceClosure,
    indexed: &IndexedThroughCertificate,
    minimum_generation: u64,
) -> FreshnessDecision {
    if let Err(error) = required.validate() {
        return FreshnessDecision::rejected(
            FreshnessStatus::Unknown,
            error.code,
            format!("required closure is invalid: {}", error.detail),
        );
    }
    if let Err(error) = indexed.validate() {
        return FreshnessDecision::rejected(
            FreshnessStatus::Unknown,
            error.code,
            format!("indexed certificate is invalid: {}", error.detail),
        );
    }
    let actual = &indexed.influence;
    if actual.assembly_manifest_digest != required.assembly_manifest_digest {
        return FreshnessDecision::rejected(
            FreshnessStatus::Unknown,
            FreshnessFailureCode::AssemblyMismatch,
            "assembly manifest identity differs",
        );
    }
    match compare_heads(
        &required.source_repository_heads,
        &actual.source_repository_heads,
    ) {
        HeadComparison::Exact => {}
        HeadComparison::Missing => {
            return FreshnessDecision::rejected(
                FreshnessStatus::IndexBehind,
                FreshnessFailureCode::MissingProofScope,
                "indexed proof omits a required repository head",
            );
        }
        HeadComparison::Changed => {
            return FreshnessDecision::rejected(
                FreshnessStatus::IndexBehind,
                FreshnessFailureCode::SourceHeadMismatch,
                "indexed proof was collected for a different source head",
            );
        }
        HeadComparison::Extra => {
            return FreshnessDecision::rejected(
                FreshnessStatus::Unknown,
                FreshnessFailureCode::IncomparableScope,
                "indexed proof has incomparable repository scope",
            );
        }
    }
    if indexed.index_generation < minimum_generation {
        return FreshnessDecision::rejected(
            FreshnessStatus::IndexBehind,
            FreshnessFailureCode::GenerationRollback,
            "index generation is below the required generation",
        );
    }
    if actual.producer_domains != required.producer_domains {
        return compare_sets(
            &required.producer_domains,
            &actual.producer_domains,
            "producer domain",
        );
    }
    if actual.influence_scope != required.influence_scope {
        return compare_sets(
            &required.influence_scope,
            &actual.influence_scope,
            "influence scope",
        );
    }
    if actual.edges != required.edges {
        return compare_sets(&required.edges, &actual.edges, "dependency edge");
    }
    if actual.essential_dependencies != required.essential_dependencies {
        return compare_sets(
            &required.essential_dependencies,
            &actual.essential_dependencies,
            "essential dependency witness",
        );
    }
    FreshnessDecision::fresh(indexed)
}

pub fn freshness_contract_manifest() -> Value {
    json!({
        "contract_version": FRESHNESS_CONTRACT_VERSION,
        "model_version": FRESHNESS_MODEL_VERSION,
        "authority": "ZeroStack",
        "engine_semantics": ["E-FS", "E-GRAPH", "E-TOKEN"],
        "identity_authority": ["assembly_manifest_digest", "source_repository_heads", "index_id", "index_generation", "influence_scope", "dependency_edges"],
        "outcomes": ["fresh", "index_behind", "unknown"],
        "wall_clock_is_authority": false,
        "limits": {"repositories": FRESHNESS_MAX_REPOSITORIES, "nodes": FRESHNESS_MAX_NODES, "edges": FRESHNESS_MAX_EDGES, "witnesses": FRESHNESS_MAX_WITNESSES}
    })
}

pub fn freshness_contract_digest() -> Sha256Digest {
    digest_json(&freshness_contract_manifest())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreshnessError {
    pub code: FreshnessFailureCode,
    pub detail: String,
}
impl FreshnessError {
    pub fn new(code: FreshnessFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for FreshnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}
impl Error for FreshnessError {}

fn validate_version(version: u16) -> Result<(), FreshnessError> {
    if version == FRESHNESS_CONTRACT_VERSION {
        Ok(())
    } else {
        Err(FreshnessError::new(
            FreshnessFailureCode::UnsupportedSchemaVersion,
            format!("unsupported schema version {version}"),
        ))
    }
}
fn validate_identity(label: &str, value: &str) -> Result<(), FreshnessError> {
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        Err(FreshnessError::new(
            FreshnessFailureCode::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ))
    } else {
        Ok(())
    }
}
fn digest_json(value: &Value) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(canonical_json(value).as_bytes()))
}
fn require_digest(actual: Sha256Digest, expected: Sha256Digest) -> Result<(), FreshnessError> {
    if actual == expected {
        Ok(())
    } else {
        Err(FreshnessError::new(
            FreshnessFailureCode::CertificateDigestMismatch,
            "certificate digest does not match canonical payload bytes",
        ))
    }
}
fn serialization_error(error: serde_json::Error) -> FreshnessError {
    FreshnessError::new(
        FreshnessFailureCode::SerializationFailure,
        error.to_string(),
    )
}
fn sort_unique<T: Ord>(values: &mut [T], label: &str) -> Result<(), FreshnessError> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(FreshnessError::new(
            FreshnessFailureCode::DuplicateIdentity,
            format!("duplicate {label}"),
        ))
    } else {
        Ok(())
    }
}
fn require_strict_order<T: Ord>(values: &[T], label: &str) -> Result<(), FreshnessError> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(FreshnessError::new(
            FreshnessFailureCode::NonCanonicalOrder,
            format!("{label} must be strictly sorted and unique"),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum HeadComparison {
    Exact,
    Missing,
    Changed,
    Extra,
}
fn compare_heads(required: &[FreshnessHead], actual: &[FreshnessHead]) -> HeadComparison {
    let required: BTreeMap<&str, &str> = required
        .iter()
        .map(|head| (head.repository.as_str(), head.head.as_str()))
        .collect();
    let actual: BTreeMap<&str, &str> = actual
        .iter()
        .map(|head| (head.repository.as_str(), head.head.as_str()))
        .collect();
    if required == actual {
        HeadComparison::Exact
    } else if required
        .keys()
        .any(|repository| !actual.contains_key(repository))
    {
        HeadComparison::Missing
    } else if required
        .iter()
        .any(|(repository, head)| actual.get(repository) != Some(head))
    {
        HeadComparison::Changed
    } else {
        HeadComparison::Extra
    }
}
fn compare_sets<T: Ord>(required: &[T], actual: &[T], label: &str) -> FreshnessDecision {
    let required: BTreeSet<&T> = required.iter().collect();
    let actual: BTreeSet<&T> = actual.iter().collect();
    if required.is_subset(&actual) {
        FreshnessDecision::rejected(
            FreshnessStatus::Unknown,
            FreshnessFailureCode::ScopeInflation,
            format!("indexed proof claims unrequested {label} scope"),
        )
    } else if actual.is_subset(&required) {
        FreshnessDecision::rejected(
            FreshnessStatus::IndexBehind,
            if label == "dependency edge" {
                FreshnessFailureCode::MissingEdge
            } else {
                FreshnessFailureCode::MissingProofScope
            },
            format!("indexed proof omits required {label} scope"),
        )
    } else {
        FreshnessDecision::rejected(
            FreshnessStatus::Unknown,
            FreshnessFailureCode::IncomparableScope,
            format!("indexed and required {label} scopes are incomparable"),
        )
    }
}

