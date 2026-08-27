//! Higher-order conflict hypergraph (pairwise graphs are insufficient).

use std::collections::{BTreeMap, BTreeSet};

use graphzero_types::ContentHash;

/// Kind of multi-way conflict contributing to baseline dominance.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ConflictKind {
    MutualExclusion,
    BaselineDominance,
    CoverageGap,
    IdentityMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConflictHyperedge {
    pub id: ContentHash,
    pub kind: ConflictKind,
    /// Arity >= 2; higher-order conflicts use arity >= 3.
    pub members: BTreeSet<ContentHash>,
    pub premises: Vec<String>,
}

impl ConflictHyperedge {
    pub fn new(
        id: ContentHash,
        kind: ConflictKind,
        members: BTreeSet<ContentHash>,
        premises: Vec<String>,
    ) -> Result<Self, &'static str> {
        if members.len() < 2 {
            return Err("conflict hyperedge requires arity >= 2");
        }
        Ok(Self {
            id,
            kind,
            members,
            premises,
        })
    }

    #[must_use]
    pub fn arity(&self) -> usize {
        self.members.len()
    }

    #[must_use]
    pub fn is_higher_order(&self) -> bool {
        self.arity() >= 3
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConflictHypergraph {
    edges: BTreeMap<ContentHash, ConflictHyperedge>,
}

impl ConflictHypergraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, edge: ConflictHyperedge) {
        self.edges.insert(edge.id, edge);
    }

    #[must_use]
    pub fn higher_order_edges(&self) -> Vec<&ConflictHyperedge> {
        self.edges
            .values()
            .filter(|e| e.is_higher_order())
            .collect()
    }

    /// Pairwise projection loses higher-order structure -- used in adversarial tests.
    #[must_use]
    pub fn pairwise_projection(&self) -> BTreeSet<(ContentHash, ContentHash)> {
        let mut pairs = BTreeSet::new();
        for e in self.edges.values() {
            let v: Vec<_> = e.members.iter().copied().collect();
            for i in 0..v.len() {
                for j in (i + 1)..v.len() {
                    let (a, b) = if v[i] < v[j] {
                        (v[i], v[j])
                    } else {
                        (v[j], v[i])
                    };
                    pairs.insert((a, b));
                }
            }
        }
        pairs
    }

    /// True when some higher-order edge is not recoverable as a single pairwise fact.
    #[must_use]
    pub fn has_structure_beyond_pairwise(&self) -> bool {
        self.higher_order_edges().iter().any(|e| e.arity() >= 3)
    }
}
