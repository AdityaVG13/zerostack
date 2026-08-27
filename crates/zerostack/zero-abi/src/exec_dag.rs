//! Operation dependency DAG surface (V6-R15, ZS-EXEC-001).
//!
//! A plan's operations form an explicit dependency DAG: every node lists the
//! node ids it depends on, and the DAG is validated fail-closed (unique ids,
//! existing deps, no self-dep, acyclic). Deterministic traversals --
//! [`ExecDag::topo_order`], [`ExecDag::layers`] (independent/batchable
//! groups), [`ExecDag::critical_path`] -- are the dependency-aware
//! scheduling surface. Decision-boundary nodes mark where execution must
//! halt for a protected decision; the contingent-policy crossing rule
//! ([`ExecDag::crossing_rule`]) is the hub-side rule that crossing a
//! boundary requires an attached contingent policy, fail-closed otherwise.
//! Per-observation resolution stays with the DecisionGate (zero-codemode);
//! this module owns the structural rule.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::decision::ContingentPolicy;
use crate::digest::sha256_hex;
use crate::schema::canonical_json;

/// Maximum number of nodes in one plan DAG.
pub const MAX_EXEC_DAG_NODES: usize = 4096;
/// Maximum number of dependency edges per node.
pub const MAX_EXEC_DAG_DEPENDENCIES_PER_NODE: usize = 256;

/// Kind of a node in an execution plan DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ExecNodeKind {
    /// Plain operation node.
    Op,
    /// Decision-boundary node: execution must halt here unless a contingent
    /// policy covers the crossing (hub-side rule, fail-closed).
    DecisionBoundary,
}

/// One operation in a plan DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecNode {
    /// Unique opaque node id (e.g. `op:read_a`, `dec:choose_strategy`).
    pub id: String,
    /// Node kind (plain op or decision boundary).
    pub kind: ExecNodeKind,
    /// Positive scheduling weight (critical-path cost; 0 = unweighted).
    pub weight: u64,
    /// Explicit dependency edges: node ids that must complete first.
    pub deps: Vec<String>,
}

impl ExecNode {
    /// Build one node, rejecting empty ids and self-dependencies.
    pub fn new(
        id: impl Into<String>,
        kind: ExecNodeKind,
        weight: u64,
        deps: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ExecDagError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(ExecDagError::EmptyNodeId);
        }
        let deps: Vec<String> = deps.into_iter().map(Into::into).collect();
        for dep in &deps {
            if dep.trim().is_empty() {
                return Err(ExecDagError::EmptyDependency { node: id.clone() });
            }
            if dep == &id {
                return Err(ExecDagError::SelfDependency { node: id.clone() });
            }
        }
        Ok(ExecNode {
            id,
            kind,
            weight,
            deps,
        })
    }
}

/// A plan as an explicit dependency DAG (ZS-EXEC-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecDag {
    /// Nodes in arbitrary insertion order; order never affects semantics.
    pub nodes: Vec<ExecNode>,
}

impl ExecDag {
    /// New DAG from nodes (validated on first use by `validate`).
    pub fn new(nodes: Vec<ExecNode>) -> Self {
        ExecDag { nodes }
    }

    /// Fail-closed structural validation: unique non-empty ids, existing
    /// dependency targets, no self-dependency, acyclic, size caps.
    pub fn validate(&self) -> Result<(), ExecDagError> {
        if self.nodes.len() > MAX_EXEC_DAG_NODES {
            return Err(ExecDagError::TooManyNodes {
                count: self.nodes.len(),
            });
        }
        let mut seen: HashMap<&str, ()> = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(ExecDagError::EmptyNodeId);
            }
            if seen.insert(node.id.as_str(), ()).is_some() {
                return Err(ExecDagError::DuplicateNodeId {
                    id: node.id.clone(),
                });
            }
            if node.deps.len() > MAX_EXEC_DAG_DEPENDENCIES_PER_NODE {
                return Err(ExecDagError::TooManyDependencies {
                    node: node.id.clone(),
                });
            }
            for dep in &node.deps {
                if !seen.contains_key(dep.as_str()) && !self.nodes.iter().any(|n| n.id == *dep) {
                    return Err(ExecDagError::MissingDependency {
                        node: node.id.clone(),
                        dep: dep.clone(),
                    });
                }
                if dep == &node.id {
                    return Err(ExecDagError::SelfDependency {
                        node: node.id.clone(),
                    });
                }
            }
        }
        // Acyclic check via deterministic topological sort.
        self.topo_order().map(|_| ())
    }

    /// Deterministic topological order (Kahn with id-sorted ready set):
    /// a node always follows all of its dependencies, and equal-ready nodes
    /// come in lexicographic id order.
    pub fn topo_order(&self) -> Result<Vec<String>, ExecDagError> {
        let ids: Vec<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut remaining: HashMap<&str, usize> = HashMap::with_capacity(self.nodes.len());
        let mut children: HashMap<&str, Vec<&str>> = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            remaining.insert(node.id.as_str(), node.deps.len());
            children.entry(node.id.as_str()).or_default();
            for dep in &node.deps {
                children
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }
        let mut ready: BTreeSet<&str> = ids
            .iter()
            .copied()
            .filter(|id| remaining[*id] == 0)
            .collect();
        let mut order: Vec<String> = Vec::with_capacity(self.nodes.len());
        while let Some(&next) = ready.iter().next() {
            ready.remove(next);
            order.push(next.to_string());
            for child in &children[next] {
                let count = remaining.get_mut(child).expect("child in map");
                *count -= 1;
                if *count == 0 {
                    ready.insert(child);
                }
            }
        }
        if order.len() != self.nodes.len() {
            let stuck: Vec<String> = self
                .nodes
                .iter()
                .map(|n| n.id.clone())
                .filter(|id| !order.contains(id))
                .collect();
            return Err(ExecDagError::CycleDetected { remaining: stuck });
        }
        Ok(order)
    }

    /// Batchable independent groups (ZS-EXEC-001/005): layer `k` holds the
    /// nodes whose dependencies all sit in earlier layers. Nodes inside one
    /// layer are mutually independent and may run concurrently; nodes across
    /// layers are dependency-ordered and never reordered. Layers are sorted
    /// by node id, so the grouping is deterministic.
    pub fn layers(&self) -> Result<Vec<Vec<String>>, ExecDagError> {
        self.validate()?;
        let mut remaining: HashMap<&str, usize> = HashMap::with_capacity(self.nodes.len());
        let mut children: HashMap<&str, Vec<&str>> = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            remaining.insert(node.id.as_str(), node.deps.len());
            children.entry(node.id.as_str()).or_default();
            for dep in &node.deps {
                children
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.id.as_str());
            }
        }
        let mut frontier: BTreeSet<&str> = self
            .nodes
            .iter()
            .map(|n| n.id.as_str())
            .filter(|id| remaining[id] == 0)
            .collect();
        let mut layers: Vec<Vec<String>> = Vec::new();
        while !frontier.is_empty() {
            let layer: Vec<String> = frontier.iter().map(|id| id.to_string()).collect();
            let mut next: BTreeSet<&str> = BTreeSet::new();
            for id in &layer {
                for child in &children[id.as_str()] {
                    let count = remaining.get_mut(child).expect("child in map");
                    *count -= 1;
                    if *count == 0 {
                        next.insert(child);
                    }
                }
            }
            layers.push(layer);
            frontier = next;
        }
        Ok(layers)
    }

    /// Critical path: the highest total-weight dependency chain, computed in
    /// topological order with deterministic tie-breaking (lexicographically
    /// smallest node id wins ties). An empty DAG yields an empty path.
    pub fn critical_path(&self) -> Result<Vec<String>, ExecDagError> {
        let topo = self.topo_order()?;
        if topo.is_empty() {
            return Ok(Vec::new());
        }
        let index: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let weight_of = |id: &str| self.nodes[index[id]].weight;
        let mut dist: HashMap<&str, u64> = HashMap::with_capacity(self.nodes.len());
        let mut prev: HashMap<&str, String> = HashMap::new();
        for id in &topo {
            let mut best = 0u64;
            let mut best_dep: Option<&str> = None;
            for dep in &self.nodes[index[id.as_str()]].deps {
                let dep_dist = dist[dep.as_str()];
                if dep_dist > best
                    || (dep_dist == best && best_dep.is_none_or(|bd| dep.as_str() < bd))
                {
                    best = dep_dist;
                    best_dep = Some(dep.as_str());
                }
            }
            dist.insert(id.as_str(), best + weight_of(id));
            if let Some(dep) = best_dep {
                prev.insert(id.as_str(), dep.to_string());
            }
        }
        let mut end: &str = topo[0].as_str();
        for id in &topo {
            if dist[id.as_str()] > dist[end]
                || (dist[id.as_str()] == dist[end] && id.as_str() < end)
            {
                end = id.as_str();
            }
        }
        let mut path: Vec<String> = vec![end.to_string()];
        while let Some(p) = prev.get(path.last().expect("path non-empty").as_str()) {
            path.push(p.clone());
        }
        path.reverse();
        Ok(path)
    }

    /// Whether the plan contains any decision-boundary node.
    pub fn requires_policy(&self) -> bool {
        self.nodes
            .iter()
            .any(|n| n.kind == ExecNodeKind::DecisionBoundary)
    }

    /// Hub-side contingent-policy crossing rule (ZS-EXEC-001): a plan with
    /// decision-boundary nodes may be executed only with a contingent policy
    /// attached. No policy and a boundary present => fail closed naming the
    /// first boundary in deterministic topological order. A policy must
    /// itself validate; per-observation resolution stays with the
    /// DecisionGate at runtime.
    pub fn crossing_rule(&self, policy: Option<&ContingentPolicy>) -> Result<(), ExecDagError> {
        if !self.requires_policy() {
            return Ok(());
        }
        let Some(policy) = policy else {
            let order = self.topo_order()?;
            let first_boundary = self
                .nodes
                .iter()
                .filter(|n| n.kind == ExecNodeKind::DecisionBoundary)
                .map(|n| n.id.as_str())
                .min_by_key(|id| order.iter().position(|o| o == id))
                .expect("requires_policy guarantees a boundary node");
            return Err(ExecDagError::DecisionBoundaryUncovered {
                node_id: first_boundary.to_string(),
            });
        };
        policy
            .validate()
            .map_err(|error| ExecDagError::InvalidPolicy(error.to_string()))
    }

    /// Canonical plan digest: SHA-256 over canonical JSON of the nodes in
    /// id-sorted order. Same plan (any insertion order) => same digest.
    pub fn plan_digest(&self) -> Result<String, ExecDagError> {
        self.validate()?;
        let mut nodes = self.nodes.clone();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let value = serde_json::to_value(&nodes)
            .map_err(|error| ExecDagError::Serialize(error.to_string()))?;
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }
}

/// Fail-closed DAG errors (ZS-EXEC-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecDagError {
    /// A node id was empty.
    EmptyNodeId,
    /// Two nodes share an id.
    DuplicateNodeId { id: String },
    /// A dependency target does not exist in the plan.
    MissingDependency { node: String, dep: String },
    /// A node lists itself as a dependency.
    SelfDependency { node: String },
    /// The plan contains a cycle; `remaining` lists the nodes that never
    /// became ready.
    CycleDetected { remaining: Vec<String> },
    /// The plan exceeds the node cap.
    TooManyNodes { count: usize },
    /// A node exceeds the per-node dependency cap.
    TooManyDependencies { node: String },
    /// A decision-boundary node has no attached contingent policy: crossing
    /// is refused fail-closed, naming the first boundary in topo order.
    DecisionBoundaryUncovered { node_id: String },
    /// The attached contingent policy is defective.
    InvalidPolicy(String),
    /// A dependency listed an empty id.
    EmptyDependency { node: String },
    /// Canonical serialization failed.
    Serialize(String),
}

impl std::fmt::Display for ExecDagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecDagError::EmptyNodeId => write!(f, "empty node id"),
            ExecDagError::DuplicateNodeId { id } => write!(f, "duplicate node id {id}"),
            ExecDagError::MissingDependency { node, dep } => {
                write!(f, "node {node} depends on missing node {dep}")
            }
            ExecDagError::SelfDependency { node } => write!(f, "node {node} depends on itself"),
            ExecDagError::CycleDetected { remaining } => {
                write!(f, "dependency cycle among nodes {remaining:?}")
            }
            ExecDagError::TooManyNodes { count } => {
                write!(f, "plan has {count} nodes, cap is {MAX_EXEC_DAG_NODES}")
            }
            ExecDagError::TooManyDependencies { node } => write!(
                f,
                "node {node} exceeds dependency cap {MAX_EXEC_DAG_DEPENDENCIES_PER_NODE}"
            ),
            ExecDagError::DecisionBoundaryUncovered { node_id } => {
                write!(
                    f,
                    "decision boundary {node_id} uncovered: crossing requires a contingent policy"
                )
            }
            ExecDagError::InvalidPolicy(detail) => {
                write!(f, "invalid contingent policy: {detail}")
            }
            ExecDagError::EmptyDependency { node } => {
                write!(f, "node {node} lists an empty dependency id")
            }
            ExecDagError::Serialize(detail) => write!(f, "serialization failed: {detail}"),
        }
    }
}

impl std::error::Error for ExecDagError {}
