//! Why-graph schema v1 (FR-001, ADR-WHY-001).

use serde::{Deserialize, Serialize};

use crate::evidence::validate_confidence_score;

pub const SCHEMA_VERSION: u32 = 1;
pub const NODE_REF_SPLIT_KEY_FIELD: &str = "node_ref_split_key";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSourceKind {
    GitCommit,
    PrThread,
    Issue,
    AgentTrace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceSource {
    pub kind: ProvenanceSourceKind,
    pub stable_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhyRelation {
    Introduced,
    Modified,
    Discussed,
    Decided,
    Reviewed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionState {
    None,
    Redacted,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLinkState {
    Resolved,
    Pending,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WhyEdge {
    pub schema_version: u32,
    pub edge_id: String,
    pub source: ProvenanceSource,
    pub node_ref: Option<String>,
    pub relation: WhyRelation,
    /// Ordinal source-strength score (finite 0.0..=1.0, not a probability).
    /// See validate_confidence_score for validation.
    pub confidence: f32,
    pub source_freshness: Option<String>,
    pub evidence_refs: Vec<String>,
    pub redaction_state: RedactionState,
    pub node_link_state: NodeLinkState,
    /// Reserved for future query surface (FR-012).
    #[serde(default)]
    pub reserved: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAvailability {
    Available,
    Unknown,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCursor {
    pub source: ProvenanceSource,
    /// Connector-defined high-water mark. Positions must be bytewise monotonic
    /// for a given source; replaying the same position is idempotent only when
    /// the digest and last event match the stored cursor.
    pub position: String,
    pub digest: String,
    pub last_event_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhyQueryManifest {
    pub schema_version: u32,
    pub edge_count: usize,
    pub by_node: Vec<(String, usize)>,
}

impl WhyEdge {
    /// Return the explicit identity split key stored in `reserved`, if present.
    pub fn node_ref_split_key(&self) -> Result<Option<&str>, String> {
        let Some(value) = self.reserved.get(NODE_REF_SPLIT_KEY_FIELD) else {
            return Ok(None);
        };
        match value {
            serde_json::Value::String(key) if !key.trim().is_empty() => Ok(Some(key)),
            _ => Err(format!(
                "{NODE_REF_SPLIT_KEY_FIELD} must be a non-empty string"
            )),
        }
    }

    pub fn validate_for_persist(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if self.evidence_refs.is_empty() {
            return Err("evidence_refs must not be empty".into());
        }
        validate_confidence_score(self.confidence)?;
        if self.edge_id.is_empty() {
            return Err("edge_id required".into());
        }
        let split_key = self.node_ref_split_key()?;
        match (self.node_link_state, self.node_ref.as_ref()) {
            (NodeLinkState::Resolved, None) => {
                return Err("resolved edges require node_ref".into());
            }
            (NodeLinkState::Pending, Some(_)) => {
                return Err("pending edges must not have node_ref".into());
            }
            (NodeLinkState::Pending, None) if split_key.is_some() => {
                return Err("pending edges must not declare a node_ref split".into());
            }
            _ => {}
        }
        Ok(())
    }
}
