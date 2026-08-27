//! Blast-radius types and errors.

use crate::accounting::PreventedReadAccounting;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const BLAST_SCHEMA_VERSION: u32 = 1;

/// Intent parse result (shared with reserve footprints via graphzero-store).
pub type BlastIntentParse = graphzero_store::IntentParse;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeProvenance {
    pub kind: String,
    pub edge_kind: String,
    pub from_symbol: String,
    pub to_symbol: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BreakSite {
    pub symbol: String,
    pub evidence_ref: String,
    /// Path-min score: `min(provenance-path edge confidences) × (tier_a_pct / 100)`.
    /// Not best-incoming-edge confidence. Evidence ref may still pick the best edge.
    pub confidence: f64,
    pub tier: String,
    pub hop: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<EdgeProvenance>,
    /// Canonical snap-to-file target `<path>#L<start>-#L<end>` (bead 5htnw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Intent metadata: hit kind, always `blast` here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Enclosing symbol at the target span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sym: Option<String>,
    /// Inlined content window for top hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoveringTest {
    pub path_hint: String,
    pub evidence_ref: String,
}

/// Heuristic probe tags (string keys, env, config paths). Not a ranked risk
/// score and not proof that an edit is unsafe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SilentRisk {
    pub kind: String,
    pub evidence_ref: String,
    pub detail: String,
    /// Always `"heuristic"` on current product surfaces.
    #[serde(default = "silent_risk_class_heuristic")]
    pub class: String,
}

fn silent_risk_class_heuristic() -> String {
    "heuristic".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedEdit {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeculativeBlastRequest {
    pub world_ref: String,
    pub focus_symbols: Vec<String>,
    pub planned_edits: Vec<PlannedEdit>,
    /// Optional FSZero world-ref v1 enumeration envelope (JSON text). When
    /// present it is strictly validated before any graph work; its
    /// `world_ref` must match `world_ref` (or supplies it when empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_envelope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedImpact {
    pub kind: String,
    pub symbol: Option<String>,
    pub path: String,
    pub provenance: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeculativeBlastReport {
    pub schema_version: u32,
    pub world_ref: String,
    pub focus_symbols: Vec<String>,
    pub base: Vec<BlastRadiusCapsule>,
    pub planned_impacts: Vec<PlannedImpact>,
    pub impacted_symbols: Vec<String>,
    pub impacted_files: Vec<String>,
    pub impacted_tests: Vec<CoveringTest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetrievalNode {
    pub symbol: String,
    pub seed: bool,
    pub hop: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetrievalEdge {
    pub from_symbol: String,
    pub to_symbol: String,
    pub edge_kind: String,
    pub provenance_kind: String,
    pub evidence_ref: String,
    pub hop: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetrievalNeighborhood {
    pub schema_version: u32,
    pub seeds: Vec<String>,
    pub max_hops: u32,
    pub nodes: Vec<RetrievalNode>,
    pub edges: Vec<RetrievalEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlastCoverageFooter {
    pub tier_a_percent: f64,
    pub tier_b_percent: f64,
    pub tier_c_percent: f64,
    pub freshness_verified: bool,
    pub snapshot_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlastRadiusCapsule {
    pub schema_version: u32,
    pub intent: String,
    pub target_ref: String,
    pub target_symbol: String,
    pub break_sites: Vec<BreakSite>,
    pub covering_tests: Vec<CoveringTest>,
    pub silent_risk: Vec<SilentRisk>,
    pub coverage: BlastCoverageFooter,
    pub certificate: Value,
    pub accounting: PreventedReadAccounting,
    /// Durable next page (`gz://query/<id>`) when break_sites were capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug)]
pub enum BlastError {
    Parse(String),
    SymbolNotFound(String),
    Store(String),
    MalformedIndex {
        blob_idx: u32,
        blob_hash_count: usize,
    },
    Serialization(String),
    /// FSZero world-ref v1 enumeration envelope validation failure.
    WorldEnvelope(String),
}

impl std::fmt::Display for BlastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlastError::Parse(s) => write!(f, "parse: {s}"),
            BlastError::SymbolNotFound(s) => write!(f, "symbol not found: {s}"),
            BlastError::Store(s) => write!(f, "store: {s}"),
            BlastError::MalformedIndex {
                blob_idx,
                blob_hash_count,
            } => write!(
                f,
                "malformed index: blob_idx {blob_idx} out of range for {blob_hash_count} blob hashes"
            ),
            BlastError::Serialization(s) => write!(f, "serialization: {s}"),
            BlastError::WorldEnvelope(s) => write!(f, "world envelope: {s}"),
        }
    }
}

impl std::error::Error for BlastError {}
