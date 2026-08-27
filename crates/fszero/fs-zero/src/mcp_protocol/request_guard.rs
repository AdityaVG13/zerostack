//! Per-request deadline + cancel for CodeMode/MCP tools/call.
//!
//! Hub clients (ZeroStack) apply their own request timeout and historically
//! SIGKILL the child when it wedges. FSZero must bound work first, emit a
//! structured retryable JSON-RPC error, and stay ready for the next request.

use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Default tools/call wall bound. Shorter than the common hub 120s so FSZero
/// can return a retryable error before the host kills the child.
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 90_000;

/// How long to wait for an in-flight call to notice cancel before recycling
/// the session (abandoned worker may still be blocked in kernel I/O).
pub const REQUEST_CLEANUP_BOUND_MS: u64 = 250;

/// MCP cancellation SEP error code.
pub const RPC_REQUEST_CANCELLED: i64 = -32800;
/// Server-defined deadline exceeded (retryable).
pub const RPC_REQUEST_DEADLINE: i64 = -32001;

#[derive(Debug, Clone)]
pub struct RequestGuard {
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
    pub request_id: Value,
}

impl RequestGuard {
    pub fn new(request_id: Value, timeout: Duration) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + timeout,
            request_id,
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn is_expired(&self) -> bool {
        self.is_cancelled() || Instant::now() >= self.deadline
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// Resolve per-request timeout: `_meta.timeoutMs` > env > default.
pub fn resolve_request_timeout_ms(params: Option<&Value>) -> u64 {
    if let Some(ms) = params
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.get("timeoutMs").or_else(|| m.get("timeout_ms")))
        .and_then(Value::as_u64)
    {
        return ms.max(1);
    }
    if let Ok(raw) = std::env::var("FSZERO_REQUEST_TIMEOUT_MS") {
        if let Ok(ms) = raw.parse::<u64>() {
            return ms.max(1);
        }
    }
    DEFAULT_REQUEST_TIMEOUT_MS
}

pub fn deadline_error_data(kind: &'static str, message: &str) -> Value {
    serde_json::json!({ "kind": kind, "message": message, "retryable": true, })
}

pub fn matches_request_id(a: &Value, b: &Value) -> bool {
    a == b
}

#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/request_guard_tests.rs"]
mod tests;
