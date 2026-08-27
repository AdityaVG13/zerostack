//! Source-anchored project graph with coverage-gated negative knowledge.

use std::collections::{BTreeMap, BTreeSet};

use graphzero_types::ContentHash;

use crate::truth::TruthClass;

/// Content-addressed graph node identity.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NodeId(pub ContentHash);

/// Content-addressed graph edge identity.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct EdgeId(pub ContentHash);

/// Declared coverage of a graph region for absence / negative knowledge.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum CoverageClass {
    /// Complete scoped coverage -- absence may be certified inside the region.
    Complete,
    SoundOverapproximation,
    ObservedOnly,
    Partial,
    Unknown,
}

impl CoverageClass {
    #[must_use]
    pub const fn permits_absence_certificate(self) -> bool {
        matches!(self, Self::Complete)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::SoundOverapproximation => "sound_overapproximation",
            Self::ObservedOnly => "observed_only",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }
}

/// Structural relation kinds.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Relation {
    Defines,
    Calls,
    Imports,
    Implements,
    References,
    Tests,
    SchemaDepends,
    BuildDepends,
    EffectMayTouch,
}

/// Source anchor binding a fact to snapshot/producer/config identity.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceAnchor {
    /// Digest of the source root (snapshot identity).
    pub source_root: ContentHash,
    /// Producer / extractor binary+config identity.
    pub producer: ContentHash,
    /// Build/configuration digest.
    pub configuration: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphNode {
    pub id: NodeId,
    pub kind: String,
    pub name: String,
    pub anchor: SourceAnchor,
    pub truth: TruthClass,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphEdge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub relation: Relation,
    pub truth: TruthClass,
    pub provenance: Vec<SourceAnchor>,
}

/// Proof that a relation is absent inside a Completely covered region.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NegativeKnowledgeCertificate {
    pub relation: Relation,
    pub region: ContentHash,
    pub source_root: ContentHash,
    pub extractor_contract: ContentHash,
    pub checked_nodes: u64,
    pub coverage: CoverageClass,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectGraph {
    pub source_root: ContentHash,
    pub coverage: CoverageClass,
    pub coverage_region: ContentHash,
    pub extractor_contract: ContentHash,
    pub nodes: BTreeMap<NodeId, GraphNode>,
    pub edges: BTreeMap<EdgeId, GraphEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    DuplicateNode(NodeId),
    DuplicateEdge(EdgeId),
    MissingEndpoint(NodeId),
    /// Absence requested without complete scoped coverage.
    CoverageNotComplete {
        actual: CoverageClass,
    },
    RelationPresent(EdgeId),
    StaleRoot,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateNode(id) => write!(f, "duplicate node {}", id.0.to_hex()),
            Self::DuplicateEdge(id) => write!(f, "duplicate edge {}", id.0.to_hex()),
            Self::MissingEndpoint(id) => write!(f, "missing endpoint {}", id.0.to_hex()),
            Self::CoverageNotComplete { actual } => write!(
                f,
                "absence requires complete coverage; got {}",
                actual.as_str()
            ),
            Self::RelationPresent(id) => write!(f, "relation present as edge {}", id.0.to_hex()),
            Self::StaleRoot => write!(f, "source root mismatch (stale)"),
        }
    }
}

impl std::error::Error for GraphError {}

impl ProjectGraph {
    #[must_use]
    pub fn new(
        source_root: ContentHash,
        coverage: CoverageClass,
        coverage_region: ContentHash,
        extractor_contract: ContentHash,
    ) -> Self {
        Self {
            source_root,
            coverage,
            coverage_region,
            extractor_contract,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: GraphNode) -> Result<(), GraphError> {
        if node.anchor.source_root != self.source_root {
            return Err(GraphError::StaleRoot);
        }
        if self.nodes.contains_key(&node.id) {
            return Err(GraphError::DuplicateNode(node.id));
        }
        self.nodes.insert(node.id, node);
        Ok(())
    }

    pub fn add_edge(&mut self, edge: GraphEdge) -> Result<(), GraphError> {
        if edge
            .provenance
            .iter()
            .any(|a| a.source_root != self.source_root)
        {
            return Err(GraphError::StaleRoot);
        }
        if !self.nodes.contains_key(&edge.from) {
            return Err(GraphError::MissingEndpoint(edge.from));
        }
        if !self.nodes.contains_key(&edge.to) {
            return Err(GraphError::MissingEndpoint(edge.to));
        }
        if self.edges.contains_key(&edge.id) {
            return Err(GraphError::DuplicateEdge(edge.id));
        }
        self.edges.insert(edge.id, edge);
        Ok(())
    }

    #[must_use]
    pub fn neighbors(&self, node: NodeId, relation: Relation) -> BTreeSet<NodeId> {
        self.edges
            .values()
            .filter(|e| e.from == node && e.relation == relation)
            .map(|e| e.to)
            .collect()
    }

    /// Certify that `relation` is absent under **complete** scoped coverage.
    ///
    /// Incomplete coverage yields [`GraphError::CoverageNotComplete`] -- never
    /// a silent "proved absent" from a mere not-found scan.
    pub fn certify_absence(
        &self,
        relation: Relation,
    ) -> Result<NegativeKnowledgeCertificate, GraphError> {
        if !self.coverage.permits_absence_certificate() {
            return Err(GraphError::CoverageNotComplete {
                actual: self.coverage,
            });
        }
        if let Some(edge) = self.edges.values().find(|e| e.relation == relation) {
            return Err(GraphError::RelationPresent(edge.id));
        }
        Ok(NegativeKnowledgeCertificate {
            relation,
            region: self.coverage_region,
            source_root: self.source_root,
            extractor_contract: self.extractor_contract,
            checked_nodes: self.nodes.len() as u64,
            coverage: self.coverage,
        })
    }
}
