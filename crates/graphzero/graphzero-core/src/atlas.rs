//! Address Atlas: task fingerprint -> calibrated locus / evidence / effect / verifier route.

use std::collections::BTreeMap;

use graphzero_types::ContentHash;

use crate::graph::NodeId;
use crate::truth::TruthClass;

/// Snap / address confidence level. Top rank alone is never a certificate.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum SnapLevel {
    /// Exact unique address.
    S0,
    /// Small calibrated candidate set.
    S1,
    /// Broader calibrated set with premises.
    S2,
    /// No sound calibrated locus.
    Unknown,
}

impl SnapLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S0 => "S0",
            Self::S1 => "S1",
            Self::S2 => "S2",
            Self::Unknown => "unknown",
        }
    }

    /// Top-ranked locus alone never certifies without premises for S1/S2/Unknown.
    #[must_use]
    pub const fn top_rank_is_certificate(self) -> bool {
        matches!(self, Self::S0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskFingerprint {
    pub digest: ContentHash,
    pub tokens: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LocusRank {
    pub node: NodeId,
    pub score: u32,
    pub truth: TruthClass,
    pub premises: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AddressAtlas {
    /// Name / symbol token -> candidate nodes (ordered by insertion rank).
    index: BTreeMap<String, Vec<NodeId>>,
    node_truth: BTreeMap<NodeId, TruthClass>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtlasError {
    EmptyFingerprint,
}

impl std::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFingerprint => write!(f, "empty task fingerprint"),
        }
    }
}

impl std::error::Error for AtlasError {}

impl AddressAtlas {
    #[must_use]
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
            node_truth: BTreeMap::new(),
        }
    }

    pub fn insert_symbol(&mut self, symbol: impl Into<String>, node: NodeId, truth: TruthClass) {
        let symbol = symbol.into();
        self.index.entry(symbol).or_default().push(node);
        self.node_truth.insert(node, truth);
    }

    /// Resolve a fingerprint into calibrated snap level + ranked loci. Returns
    /// Unknown when no token hits; S0 only for a single exact unique hit with exact
    /// truth class; otherwise S1/S2 with premises (never certificate from rank alone).
    pub fn resolve(&self, fp: &TaskFingerprint) -> Result<(SnapLevel, Vec<LocusRank>), AtlasError> {
        if fp.tokens.is_empty() {
            return Err(AtlasError::EmptyFingerprint);
        }
        let mut hits: BTreeMap<NodeId, (u32, Vec<String>)> = BTreeMap::new();
        for tok in &fp.tokens {
            if let Some(nodes) = self.index.get(tok) {
                for n in nodes {
                    let e = hits.entry(*n).or_insert((0, Vec::new()));
                    e.0 += 1;
                    e.1.push(format!("token:{tok}"));
                }
            }
        }
        if hits.is_empty() {
            return Ok((SnapLevel::Unknown, Vec::new()));
        }
        let mut ranks: Vec<LocusRank> = hits
            .into_iter()
            .map(|(node, (score, premises))| LocusRank {
                node,
                score,
                truth: self
                    .node_truth
                    .get(&node)
                    .copied()
                    .unwrap_or(TruthClass::Unknown),
                premises,
            })
            .collect();
        ranks.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.node.0.cmp(&b.node.0)));
        let level = if ranks.len() == 1 && ranks[0].truth.is_exact() {
            SnapLevel::S0
        } else if ranks.len() <= 3 {
            SnapLevel::S1
        } else {
            SnapLevel::S2
        };
        Ok((level, ranks))
    }
}

impl Default for AddressAtlas {
    fn default() -> Self {
        Self::new()
    }
}
