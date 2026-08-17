//! Typed execution stream events (V6-R15, ZS-ADAPTER-007 open piece:
//! progress/streaming channel with step receipts).
//!
//! Long executions deliver incremental results as a stream of typed events
//! ending in exactly one terminal event (`Completed` or `Failed`). Each
//! completed node carries a step receipt (deterministic result digest +
//! output bytes). The events are harness-independent ABI values; the bounded
//! sink that delivers them lives in zsx-core (`StreamSink`).

use serde::{Deserialize, Serialize};

/// Step receipt for one completed node (bounded-result audit trail).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReceipt {
    /// Node id this receipt covers.
    pub node_id: String,
    /// Deterministic result digest of the node's output.
    pub result_digest: String,
    /// Output bytes accounted for by this receipt.
    pub output_bytes: u64,
}

/// One typed stream event of an incremental execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "PascalCase")]
pub enum ExecStreamEvent {
    /// Execution of a plan DAG started.
    PlanStarted {
        /// Monotonic per-stream sequence (terminal event has the highest).
        seq: u64,
        /// Canonical plan digest.
        plan_digest: String,
        /// Total nodes in the plan.
        total_nodes: usize,
    },
    /// A node started running.
    StepStarted { seq: u64, node_id: String },
    /// A node completed, with its step receipt.
    StepCompleted {
        seq: u64,
        node_id: String,
        step_receipt: StepReceipt,
    },
    /// Optional progress note from long-running execution (reserved for
    /// host wiring; the DAG executor itself emits step events only).
    Progress { seq: u64, node_id: String, detail: String },
    /// Terminal success: exactly one per stream, never followed by events.
    Completed {
        seq: u64,
        /// Content root of the settled execution trace.
        trace_root: String,
        /// Deterministic merged result digest (topological order).
        result_digest: String,
    },
    /// Terminal failure: exactly one per stream, never followed by events.
    Failed {
        seq: u64,
        /// Typed failure code (e.g. `OpFailed`, `DecisionBoundaryUncovered`).
        failure_code: String,
        detail: String,
    },
}

impl ExecStreamEvent {
    /// Whether this event is terminal (Completed or Failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, ExecStreamEvent::Completed { .. } | ExecStreamEvent::Failed { .. })
    }

    /// Monotonic stream sequence.
    pub fn seq(&self) -> u64 {
        match self {
            ExecStreamEvent::PlanStarted { seq, .. }
            | ExecStreamEvent::StepStarted { seq, .. }
            | ExecStreamEvent::StepCompleted { seq, .. }
            | ExecStreamEvent::Progress { seq, .. }
            | ExecStreamEvent::Completed { seq, .. }
            | ExecStreamEvent::Failed { seq, .. } => *seq,
        }
    }

    /// Node id this event concerns, when applicable.
    pub fn node_id(&self) -> Option<&str> {
        match self {
            ExecStreamEvent::StepStarted { node_id, .. }
            | ExecStreamEvent::StepCompleted { node_id, .. }
            | ExecStreamEvent::Progress { node_id, .. } => Some(node_id),
            _ => None,
        }
    }
}

