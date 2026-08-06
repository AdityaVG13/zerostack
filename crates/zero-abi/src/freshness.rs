//! Engine-neutral certified invalidation and freshness contract.
//!
//! Engines own dependency discovery. This module freezes only shared identities,
//! canonical digests, closure comparison, and fail-closed outcomes. Wall clock is
//! deliberately absent: repository, assembly, index, and closure identities are
//! the only freshness authority.

use crate::{DigestV1, canonical_json, sha256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const FRESHNESS_CONTRACT_VERSION: u16 = 1;
pub const FRESHNESS_MODEL_VERSION: &str = "zerostack.invalidation-freshness.v1";
pub const FRESHNESS_MAX_REPOSITORIES: usize = 64;
pub const FRESHNESS_MAX_NODES: usize = 4_096;
pub const FRESHNESS_MAX_EDGES: usize = 16_384;
pub const FRESHNESS_MAX_WITNESSES: usize = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerDomainV1 {
    Source,
    FilesystemIndex,
    GraphIndex,
    TokenCache,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessHeadV1 {
    pub repository: String,
    pub head: String,
}

impl FreshnessHeadV1 {
    pub fn new(
        repository: impl Into<String>,
        head: impl Into<String>,
    ) -> Result<Self, FreshnessErrorV1> {
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
pub enum DependencyEdgeKindV1 {
    Reads,
    Derives,
    Invalidates,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyEdgeV1 {
    pub producer: String,
    pub consumer: String,
    pub kind: DependencyEdgeKindV1,
}

impl DependencyEdgeV1 {
    pub fn new(
        producer: impl Into<String>,
        consumer: impl Into<String>,
        kind: DependencyEdgeKindV1,
    ) -> Result<Self, FreshnessErrorV1> {
        let value = Self {
            producer: producer.into(),
            consumer: consumer.into(),
            kind,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), FreshnessErrorV1> {
        validate_identity("edge producer", &self.producer)?;
        validate_identity("edge consumer", &self.consumer)?;
        if self.producer == self.consumer {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::InvalidIdentity,
                "dependency edge cannot be a self edge",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EssentialDependencyWitnessV1 {
    pub path: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EssentialDependencyCertificate {
    pub schema_version: u16,
    pub dependency: DependencyEdgeV1,
    pub witness: EssentialDependencyWitnessV1,
    pub certificate_digest: DigestV1,
}

impl EssentialDependencyCertificate {
    pub fn new(dependency: DependencyEdgeV1, path: Vec<String>) -> Result<Self, FreshnessErrorV1> {
        let mut value = Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            dependency,
            witness: EssentialDependencyWitnessV1 { path },
            certificate_digest: DigestV1::ZERO,
        };
        value.validate_payload()?;
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_digest(&self) -> DigestV1 {
        digest_json(
            &json!({"schema_version": self.schema_version, "dependency": self.dependency, "witness": self.witness}),
        )
    }

    fn validate_payload(&self) -> Result<(), FreshnessErrorV1> {
        validate_version(self.schema_version)?;
        self.dependency.validate()?;
        if !(2..=FRESHNESS_MAX_NODES).contains(&self.witness.path.len()) {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::MissingProofScope,
                "essential dependency witness path must contain 2..=4096 nodes",
            ));
        }
        for node in &self.witness.path {
            validate_identity("witness node", node)?;
        }
        if self.witness.path.first() != Some(&self.dependency.producer)
            || self.witness.path.last() != Some(&self.dependency.consumer)
        {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::MissingProofScope,
                "essential dependency witness endpoints do not match its edge",
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), FreshnessErrorV1> {
        self.validate_payload()?;
        require_digest(self.certificate_digest, self.expected_digest())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertifiedInfluenceClosure {
    pub schema_version: u16,
    pub model_version: String,
    pub assembly_manifest_digest: DigestV1,
    pub source_repository_heads: Vec<FreshnessHeadV1>,
    pub producer_domains: Vec<ProducerDomainV1>,
    pub influence_scope: Vec<String>,
    pub edges: Vec<DependencyEdgeV1>,
    pub essential_dependencies: Vec<EssentialDependencyCertificate>,
    pub certificate_digest: DigestV1,
}

impl CertifiedInfluenceClosure {
    pub fn new(
        assembly_manifest_digest: DigestV1,
        mut source_repository_heads: Vec<FreshnessHeadV1>,
        mut producer_domains: Vec<ProducerDomainV1>,
        mut influence_scope: Vec<String>,
        mut edges: Vec<DependencyEdgeV1>,
        mut essential_dependencies: Vec<EssentialDependencyCertificate>,
    ) -> Result<Self, FreshnessErrorV1> {
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
            certificate_digest: DigestV1::ZERO,
        };
        value.validate_payload()?;
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_digest(&self) -> DigestV1 {
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

    fn validate_payload(&self) -> Result<(), FreshnessErrorV1> {
        validate_version(self.schema_version)?;
        if self.model_version != FRESHNESS_MODEL_VERSION {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::ModelVersionMismatch,
                "unsupported freshness model version",
            ));
        }
        if self.assembly_manifest_digest == DigestV1::ZERO {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::AssemblyMismatch,
                "assembly manifest digest cannot be zero",
            ));
        }
        if self.source_repository_heads.is_empty()
            || self.producer_domains.is_empty()
            || self.influence_scope.is_empty()
        {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::MissingProofScope,
                "source heads, producer domains, and influence scope cannot be empty",
            ));
        }
        if self.source_repository_heads.len() > FRESHNESS_MAX_REPOSITORIES
            || self.influence_scope.len() > FRESHNESS_MAX_NODES
            || self.edges.len() > FRESHNESS_MAX_EDGES
            || self.essential_dependencies.len() > FRESHNESS_MAX_WITNESSES
        {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::ContractLimitExceeded,
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
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::DuplicateIdentity,
                "source repository identity appears with more than one head",
            ));
        }
        if self
            .essential_dependencies
            .windows(2)
            .any(|pair| pair[0].dependency == pair[1].dependency)
        {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::DuplicateIdentity,
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
                return Err(FreshnessErrorV1::new(
                    FreshnessFailureCodeV1::MissingProofScope,
                    "dependency edge endpoint is outside influence scope",
                ));
            }
        }
        let edges: BTreeSet<&DependencyEdgeV1> = self.edges.iter().collect();
        for certificate in &self.essential_dependencies {
            certificate.validate()?;
            if !edges.contains(&certificate.dependency) {
                return Err(FreshnessErrorV1::new(
                    FreshnessFailureCodeV1::MissingEdge,
                    "essential dependency is absent from the certified edge set",
                ));
            }
            for pair in certificate.witness.path.windows(2) {
                if !self
                    .edges
                    .iter()
                    .any(|edge| edge.producer == pair[0] && edge.consumer == pair[1])
                {
                    return Err(FreshnessErrorV1::new(
                        FreshnessFailureCodeV1::MissingEdge,
                        "essential dependency witness traverses an uncertified edge",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), FreshnessErrorV1> {
        self.validate_payload()?;
        require_digest(self.certificate_digest, self.expected_digest())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FreshnessErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        Ok(canonical_json(&value).into_bytes())
    }
}

pub fn influence_closure_v1(
    assembly_manifest_digest: DigestV1,
    source_repository_heads: Vec<FreshnessHeadV1>,
    producer_domains: Vec<ProducerDomainV1>,
    influence_scope: Vec<String>,
    edges: Vec<DependencyEdgeV1>,
    essential_dependencies: Vec<EssentialDependencyCertificate>,
) -> Result<CertifiedInfluenceClosure, FreshnessErrorV1> {
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
    pub replay_identity: DigestV1,
    pub certificate_digest: DigestV1,
}

impl IndexedThroughCertificate {
    pub fn new(
        index_id: impl Into<String>,
        index_generation: u64,
        influence: CertifiedInfluenceClosure,
    ) -> Result<Self, FreshnessErrorV1> {
        let mut value = Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            model_version: FRESHNESS_MODEL_VERSION.into(),
            index_id: index_id.into(),
            index_generation,
            influence,
            replay_identity: DigestV1::ZERO,
            certificate_digest: DigestV1::ZERO,
        };
        value.validate_payload()?;
        value.replay_identity = value.expected_replay_identity();
        value.certificate_digest = value.expected_digest();
        Ok(value)
    }

    fn expected_replay_identity(&self) -> DigestV1 {
        digest_json(
            &json!({"domain": "zerostack.freshness.replay.v1", "index_id": self.index_id, "index_generation": self.index_generation, "influence_digest": self.influence.certificate_digest}),
        )
    }

    fn expected_digest(&self) -> DigestV1 {
        digest_json(
            &json!({"schema_version": self.schema_version, "model_version": self.model_version, "index_id": self.index_id, "index_generation": self.index_generation, "influence": self.influence, "replay_identity": self.replay_identity}),
        )
    }

    fn validate_payload(&self) -> Result<(), FreshnessErrorV1> {
        validate_version(self.schema_version)?;
        if self.model_version != FRESHNESS_MODEL_VERSION {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::ModelVersionMismatch,
                "unsupported indexed-through model version",
            ));
        }
        validate_identity("index id", &self.index_id)?;
        self.influence.validate()
    }

    pub fn validate(&self) -> Result<(), FreshnessErrorV1> {
        self.validate_payload()?;
        if self.replay_identity != self.expected_replay_identity() {
            return Err(FreshnessErrorV1::new(
                FreshnessFailureCodeV1::ReplayIdentityMismatch,
                "indexed-through replay identity mismatch",
            ));
        }
        require_digest(self.certificate_digest, self.expected_digest())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FreshnessErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(serialization_error)?;
        Ok(canonical_json(&value).into_bytes())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessStatusV1 {
    Fresh,
    IndexBehind,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FreshnessFailureCodeV1 {
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
pub struct FreshnessDecisionV1 {
    pub schema_version: u16,
    pub status: FreshnessStatusV1,
    pub trusted: bool,
    pub failure_code: Option<FreshnessFailureCodeV1>,
    pub detail: String,
    pub indexed_certificate_digest: Option<DigestV1>,
}

impl FreshnessDecisionV1 {
    fn fresh(indexed: &IndexedThroughCertificate) -> Self {
        Self {
            schema_version: FRESHNESS_CONTRACT_VERSION,
            status: FreshnessStatusV1::Fresh,
            trusted: true,
            failure_code: None,
            detail: "exact assembly, source heads, influence scope, and frontier certified".into(),
            indexed_certificate_digest: Some(indexed.certificate_digest),
        }
    }
    fn rejected(
        status: FreshnessStatusV1,
        failure_code: FreshnessFailureCodeV1,
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

pub fn decide_freshness_v1(
    required: &CertifiedInfluenceClosure,
    indexed: &IndexedThroughCertificate,
    minimum_generation: u64,
) -> FreshnessDecisionV1 {
    if let Err(error) = required.validate() {
        return FreshnessDecisionV1::rejected(
            FreshnessStatusV1::Unknown,
            error.code,
            format!("required closure is invalid: {}", error.detail),
        );
    }
    if let Err(error) = indexed.validate() {
        return FreshnessDecisionV1::rejected(
            FreshnessStatusV1::Unknown,
            error.code,
            format!("indexed certificate is invalid: {}", error.detail),
        );
    }
    let actual = &indexed.influence;
    if actual.assembly_manifest_digest != required.assembly_manifest_digest {
        return FreshnessDecisionV1::rejected(
            FreshnessStatusV1::Unknown,
            FreshnessFailureCodeV1::AssemblyMismatch,
            "assembly manifest identity differs",
        );
    }
    match compare_heads(
        &required.source_repository_heads,
        &actual.source_repository_heads,
    ) {
        HeadComparison::Exact => {}
        HeadComparison::Missing => {
            return FreshnessDecisionV1::rejected(
                FreshnessStatusV1::IndexBehind,
                FreshnessFailureCodeV1::MissingProofScope,
                "indexed proof omits a required repository head",
            );
        }
        HeadComparison::Changed => {
            return FreshnessDecisionV1::rejected(
                FreshnessStatusV1::IndexBehind,
                FreshnessFailureCodeV1::SourceHeadMismatch,
                "indexed proof was collected for a different source head",
            );
        }
        HeadComparison::Extra => {
            return FreshnessDecisionV1::rejected(
                FreshnessStatusV1::Unknown,
                FreshnessFailureCodeV1::IncomparableScope,
                "indexed proof has incomparable repository scope",
            );
        }
    }
    if indexed.index_generation < minimum_generation {
        return FreshnessDecisionV1::rejected(
            FreshnessStatusV1::IndexBehind,
            FreshnessFailureCodeV1::GenerationRollback,
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
    FreshnessDecisionV1::fresh(indexed)
}

pub fn freshness_contract_manifest_v1() -> Value {
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

pub fn freshness_contract_digest_v1() -> DigestV1 {
    digest_json(&freshness_contract_manifest_v1())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreshnessErrorV1 {
    pub code: FreshnessFailureCodeV1,
    pub detail: String,
}
impl FreshnessErrorV1 {
    pub fn new(code: FreshnessFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for FreshnessErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}
impl Error for FreshnessErrorV1 {}

fn validate_version(version: u16) -> Result<(), FreshnessErrorV1> {
    if version == FRESHNESS_CONTRACT_VERSION {
        Ok(())
    } else {
        Err(FreshnessErrorV1::new(
            FreshnessFailureCodeV1::UnsupportedSchemaVersion,
            format!("unsupported schema version {version}"),
        ))
    }
}
fn validate_identity(label: &str, value: &str) -> Result<(), FreshnessErrorV1> {
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b':' | b'_' | b'-')
        })
    {
        Err(FreshnessErrorV1::new(
            FreshnessFailureCodeV1::InvalidIdentity,
            format!("{label} is empty, too long, or contains a non-canonical byte"),
        ))
    } else {
        Ok(())
    }
}
fn digest_json(value: &Value) -> DigestV1 {
    DigestV1::from_bytes(sha256(canonical_json(value).as_bytes()))
}
fn require_digest(actual: DigestV1, expected: DigestV1) -> Result<(), FreshnessErrorV1> {
    if actual == expected {
        Ok(())
    } else {
        Err(FreshnessErrorV1::new(
            FreshnessFailureCodeV1::CertificateDigestMismatch,
            "certificate digest does not match canonical payload bytes",
        ))
    }
}
fn serialization_error(error: serde_json::Error) -> FreshnessErrorV1 {
    FreshnessErrorV1::new(
        FreshnessFailureCodeV1::SerializationFailure,
        error.to_string(),
    )
}
fn sort_unique<T: Ord>(values: &mut [T], label: &str) -> Result<(), FreshnessErrorV1> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(FreshnessErrorV1::new(
            FreshnessFailureCodeV1::DuplicateIdentity,
            format!("duplicate {label}"),
        ))
    } else {
        Ok(())
    }
}
fn require_strict_order<T: Ord>(values: &[T], label: &str) -> Result<(), FreshnessErrorV1> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        Err(FreshnessErrorV1::new(
            FreshnessFailureCodeV1::NonCanonicalOrder,
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
fn compare_heads(required: &[FreshnessHeadV1], actual: &[FreshnessHeadV1]) -> HeadComparison {
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
fn compare_sets<T: Ord>(required: &[T], actual: &[T], label: &str) -> FreshnessDecisionV1 {
    let required: BTreeSet<&T> = required.iter().collect();
    let actual: BTreeSet<&T> = actual.iter().collect();
    if required.is_subset(&actual) {
        FreshnessDecisionV1::rejected(
            FreshnessStatusV1::Unknown,
            FreshnessFailureCodeV1::ScopeInflation,
            format!("indexed proof claims unrequested {label} scope"),
        )
    } else if actual.is_subset(&required) {
        FreshnessDecisionV1::rejected(
            FreshnessStatusV1::IndexBehind,
            if label == "dependency edge" {
                FreshnessFailureCodeV1::MissingEdge
            } else {
                FreshnessFailureCodeV1::MissingProofScope
            },
            format!("indexed proof omits required {label} scope"),
        )
    } else {
        FreshnessDecisionV1::rejected(
            FreshnessStatusV1::Unknown,
            FreshnessFailureCodeV1::IncomparableScope,
            format!("indexed and required {label} scopes are incomparable"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }
    fn closure(head: u8, extra_scope: bool) -> CertifiedInfluenceClosure {
        let mut scope = vec!["fs:file".into(), "graph:symbol".into()];
        let mut edges = vec![
            DependencyEdgeV1::new("fs:file", "graph:symbol", DependencyEdgeKindV1::Derives)
                .unwrap(),
        ];
        if extra_scope {
            scope.push("token:cache".into());
            edges.push(
                DependencyEdgeV1::new("graph:symbol", "token:cache", DependencyEdgeKindV1::Derives)
                    .unwrap(),
            );
        }
        let essential = EssentialDependencyCertificate::new(
            edges[0].clone(),
            vec!["fs:file".into(), "graph:symbol".into()],
        )
        .unwrap();
        influence_closure_v1(
            digest(9),
            vec![FreshnessHeadV1::new("ZeroStack", format!("head-{head}")).unwrap()],
            vec![
                ProducerDomainV1::FilesystemIndex,
                ProducerDomainV1::GraphIndex,
            ],
            scope,
            edges,
            vec![essential],
        )
        .unwrap()
    }
    #[test]
    fn freshness_exact_closure_passes() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, required.clone()).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::Fresh);
        assert!(decision.trusted);
        assert_eq!(decision.failure_code, None);
    }
    #[test]
    fn freshness_stale_head_is_never_fresh() {
        let required = closure(2, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 8, closure(1, false)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::IndexBehind);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::SourceHeadMismatch)
        );
        assert!(!decision.trusted);
        assert_eq!(decision.indexed_certificate_digest, None);
    }
    #[test]
    fn freshness_generation_rollback_is_typed() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 6, required.clone()).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::GenerationRollback)
        );
    }
    #[test]
    fn freshness_scope_inflation_is_unknown() {
        let required = closure(1, false);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, closure(1, true)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(decision.status, FreshnessStatusV1::Unknown);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::ScopeInflation)
        );
    }
    #[test]
    fn freshness_missing_edge_is_not_fresh() {
        let required = closure(1, true);
        let indexed = IndexedThroughCertificate::new("graph-index", 7, closure(1, false)).unwrap();
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_ne!(decision.status, FreshnessStatusV1::Fresh);
        assert!(!decision.trusted);
    }
    #[test]
    fn freshness_replay_identity_mutation_is_rejected() {
        let required = closure(1, false);
        let mut indexed =
            IndexedThroughCertificate::new("graph-index", 7, required.clone()).unwrap();
        indexed.replay_identity = digest(55);
        let decision = decide_freshness_v1(&required, &indexed, 7);
        assert_eq!(
            decision.failure_code,
            Some(FreshnessFailureCodeV1::ReplayIdentityMismatch)
        );
        assert!(!decision.trusted);
    }
    #[test]
    fn freshness_canonical_bytes_and_contract_digest_are_stable() {
        let required = closure(1, false);
        assert_eq!(
            required.canonical_bytes().unwrap(),
            required.canonical_bytes().unwrap()
        );
        assert_ne!(freshness_contract_digest_v1(), DigestV1::ZERO);
        assert_eq!(
            freshness_contract_manifest_v1()["wall_clock_is_authority"],
            false
        );
    }
    #[test]
    fn freshness_duplicate_repository_identity_is_rejected() {
        let mut value = closure(1, false);
        value
            .source_repository_heads
            .push(FreshnessHeadV1::new("ZeroStack", "head-2").unwrap());
        value.source_repository_heads.sort();
        assert_eq!(
            value.validate().unwrap_err().code,
            FreshnessFailureCodeV1::DuplicateIdentity
        );
    }

    #[test]
    fn freshness_old_schema_version_has_typed_outcome() {
        let mut indexed =
            IndexedThroughCertificate::new("graph-index", 7, closure(1, false)).unwrap();
        indexed.schema_version = 0;
        assert_eq!(
            indexed.validate().unwrap_err().code,
            FreshnessFailureCodeV1::UnsupportedSchemaVersion
        );
    }

    #[test]
    fn freshness_wire_shape_rejects_unknown_fields() {
        let required = closure(1, false);
        let mut value = serde_json::to_value(required).unwrap();
        value["timestamp"] = json!("2099-01-01T00:00:00Z");
        assert!(serde_json::from_value::<CertifiedInfluenceClosure>(value).is_err());
    }
}
