//! CodeMode / substrate runtime health (fszero-iod).
//!
//! When the durable response slot cannot be recovered after an execution,
//! orchestrators previously saw a synthetic `cm://exec/response-unavailable`
//! while `backend_status` still reported green health — so conservative
//! native fallback never engaged. This module is the single counter that
//! substrate misses must bump, and the fail-open latch they must set.

use serde_json::{Value, json};

/// Consecutive substrate failures required before fail-open engages.
/// Missing response payload is a hard substrate fault: one strike trips
/// the latch so the hub can fall back without waiting for a streak.
pub const FAIL_OPEN_AFTER: u64 = 1;

#[derive(Debug, Clone, Default)]
pub struct RuntimeHealth {
    pub consecutive_failures: u64,
    pub fail_open: bool,
    pub last_error: Option<String>,
    pub substrate_failures: u64,
}

impl RuntimeHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful CodeMode response retrieval. Clears the
    /// consecutive streak and releases fail-open so a healed store can
    /// resume the CodeMode path.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.fail_open = false;
        self.last_error = None;
    }

    /// Record a substrate-class failure (missing child payload, store
    /// degraded mid-execution, unreadable response slot). Trips fail-open
    /// once [`FAIL_OPEN_AFTER`] consecutive faults accumulate.
    pub fn record_substrate_failure(&mut self, detail: impl Into<String>) -> &Self {
        let detail = detail.into();
        self.substrate_failures = self.substrate_failures.saturating_add(1);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_error = Some(detail);
        if self.consecutive_failures >= FAIL_OPEN_AFTER {
            self.fail_open = true;
        }
        self
    }

    /// Reports that the standalone substrate cannot serve authoritative work.
    /// ZeroKernel owns any cross-engine retry or rerouting decision; FSZero
    /// itself remains fail-closed.
    pub fn native_fallback(&self) -> bool {
        self.fail_open
    }

    pub fn to_json(&self) -> Value {
        json!({
            "consecutive_failures": self.consecutive_failures, "fail_open": self.fail_open,
            "native_fallback": self.native_fallback(), "substrate_failures": self.substrate_failures,
            "last_error": self.last_error,
        })
    }
}
