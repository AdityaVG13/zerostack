//! Cross-engine telemetry counters used by RecoveryStore to distinguish
//! ref-only transfers from payload materialization.
//!
//! This module intentionally contains no payload-bearing data structures so
//! that telemetry records stay small and safe to log/serialize.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossEngineTelemetry {
    /// Refs presented to this store for expansion or piping.
    pub refs_received: u64,
    /// Refs produced by this store for the next hop/engine.
    pub refs_sent: u64,
    /// Ref-only transfers completed without materializing payload bytes.
    pub ref_transfers: u64,
    /// Payload bytes returned to callers (explicit materialization).
    pub payload_bytes_materialized: u64,
    /// Payload bytes moved between sibling stores and the shared CAS without
    /// leaving the store boundary (rehydration/piping).
    pub payload_bytes_piped: u64,
    /// Payload bytes read from durable stores for verification or fallback.
    pub store_bytes_read: u64,
    /// Payload bytes written to durable stores for verification or piping.
    pub store_bytes_written: u64,
}

impl CrossEngineTelemetry {
    pub fn new() -> Self {
        Self::default()
    }
}
