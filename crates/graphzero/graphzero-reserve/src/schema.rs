//! IntentReservation v1 (P5.2).

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    Declared,
    Active,
    Released,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentOperation {
    pub kind: String,
    pub target_symbol: Option<String>,
    pub intent_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentReservation {
    pub schema_version: u32,
    pub reservation_id: String,
    pub repo_id: String,
    pub agent_id: String,
    pub intent_ops: Vec<IntentOperation>,
    pub footprint_ref: String,
    pub evidence_refs: Vec<String>,
    pub ttl_seconds: u64,
    pub status: ReservationStatus,
    pub created_at: u64,
    pub expires_at: u64,
    /// Contract nodes (gz://node/...) included in the footprint.
    pub contract_nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareResponse {
    pub reservation_id: String,
    pub footprint_ref: String,
    pub status: ReservationStatus,
    pub ttl_seconds: u64,
    pub expires_at: u64,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictGraphEdge {
    pub from_reservation_id: String,
    pub to_agent_id: String,
    pub node: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReservationCheckResponse {
    pub verdict: String,
    pub overlap_nodes: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub conflict_edges: Vec<ConflictGraphEdge>,
    pub coverage: Option<f64>,
    pub certificate: Option<serde_json::Value>,
    pub blocking_reservation_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationQueryResponse {
    pub active_count: usize,
    pub reservations: Vec<IntentReservation>,
}
