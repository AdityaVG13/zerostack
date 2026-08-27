//! Budgets and truncation for batch ops (fszero-sz0t).
//!
//! Every batch accepts `budget: { deadline_us?, max_result_bytes?, max_matches_per_query? }`.
//! Enforcement is cooperative at operator loop boundaries; breach sets `truncated:true`
//! with completed prefix intact — never an opaque abort. Per-item and whole-batch caps
//! compose (min wins). Defaults are generous but finite.

use serde_json::Value;
use std::time::{Duration, Instant};

use super::*;

impl FSZeroSession {
    pub fn store_budget_evidence(&mut self, op: &str, dimension: &str, cap: usize, count: usize) {
        let payload = format!(
            "budget={dimension}\nop={op}\ncap={cap}\nscanned={count}\nattempted={count}\ncount={count}\n"
        );
        let _ = self
            .recovery
            .put_named_payload("budget_evidence", payload.as_bytes());
    }
}

pub fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.parse().ok()
}

/// Cached `FSZERO_BUDGET_MS` for the process lifetime (env mid-run is ignored).
/// Avoids a getenv+parse on every kernel op when the budget is unset (common).
pub fn budget_ms_cap() -> Option<usize> {
    use std::sync::OnceLock;
    static CAP: OnceLock<Option<usize>> = OnceLock::new();
    *CAP.get_or_init(|| env_usize("FSZERO_BUDGET_MS"))
}

/// Default whole-batch result byte cap (1 MiB) — finite so runaway globs cannot OOM.
pub const DEFAULT_MAX_RESULT_BYTES: usize = 1_048_576;
/// Default per-query match cap.
pub const DEFAULT_MAX_MATCHES_PER_QUERY: usize = 10_000;

/// Parsed batch budget envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchBudget {
    pub deadline: Option<Duration>,
    pub max_result_bytes: usize,
    pub max_matches_per_query: usize,
}

impl Default for BatchBudget {
    fn default() -> Self {
        Self {
            deadline: None,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_matches_per_query: DEFAULT_MAX_MATCHES_PER_QUERY,
        }
    }
}

impl BatchBudget {
    /// Parse `args.budget` object. Missing fields take generous defaults.
    pub fn from_args(args: &Value) -> Self {
        let mut b = Self::default();
        let Some(obj) = args.get("budget").and_then(Value::as_object) else {
            return b;
        };
        if let Some(us) = obj.get("deadline_us").and_then(Value::as_u64) {
            b.deadline = Some(Duration::from_micros(us));
        }
        if let Some(n) = obj.get("max_result_bytes").and_then(Value::as_u64) {
            b.max_result_bytes = n.min(usize::MAX as u64) as usize;
        }
        if let Some(n) = obj.get("max_matches_per_query").and_then(Value::as_u64) {
            b.max_matches_per_query = n.min(usize::MAX as u64) as usize;
        }
        b
    }

    /// Compose whole-batch match cap with a per-item limit (min wins).
    pub fn match_cap(&self, per_item: Option<usize>) -> usize {
        match per_item {
            Some(n) => n.min(self.max_matches_per_query),
            None => self.max_matches_per_query,
        }
    }

    /// Compose whole-batch byte cap with a per-item max_bytes (min wins).
    pub fn byte_cap(&self, per_item: Option<usize>) -> usize {
        match per_item {
            Some(n) => n.min(self.max_result_bytes),
            None => self.max_result_bytes,
        }
    }
}

/// Cooperative budget tracker for one batch execution.
#[derive(Debug)]
pub struct BatchBudgetTracker {
    pub budget: BatchBudget,
    started: Instant,
    /// Cumulative result payload bytes across completed rows.
    pub bytes_emitted: usize,
    /// True once any budget dimension was breached.
    pub hit: bool,
    pub hit_kind: Option<&'static str>,
}

impl BatchBudgetTracker {
    pub fn start(budget: BatchBudget) -> Self {
        Self {
            budget,
            started: Instant::now(),
            bytes_emitted: 0,
            hit: false,
            hit_kind: None,
        }
    }

    pub fn deadline_exceeded(&self) -> bool {
        self.budget
            .deadline
            .is_some_and(|d| self.started.elapsed() > d)
    }

    /// Check loop boundary: returns true when caller should stop producing more work.
    pub fn should_stop(&mut self) -> bool {
        if self.hit {
            return true;
        }
        if self.deadline_exceeded() {
            self.hit = true;
            self.hit_kind = Some("deadline");
            return true;
        }
        if self.bytes_emitted >= self.budget.max_result_bytes {
            self.hit = true;
            self.hit_kind = Some("max_result_bytes");
            return true;
        }
        false
    }

    pub fn record_bytes(&mut self, n: usize) {
        self.bytes_emitted = self.bytes_emitted.saturating_add(n);
        if self.bytes_emitted >= self.budget.max_result_bytes {
            self.hit = true;
            self.hit_kind = Some("max_result_bytes");
        }
    }

    /// Truncate content to remaining whole-batch + per-item cap. Preserves prefix.
    pub fn take_bytes(&mut self, data: &[u8], per_item: Option<usize>) -> (Vec<u8>, bool) {
        let cap = self.budget.byte_cap(per_item);
        let remaining = self
            .budget
            .max_result_bytes
            .saturating_sub(self.bytes_emitted);
        let take = cap.min(remaining).min(data.len());
        let truncated = take < data.len();
        if truncated {
            self.hit = true;
            self.hit_kind = Some(if remaining < cap {
                "max_result_bytes"
            } else {
                "per_item_bytes"
            });
        }
        let out = data[..take].to_vec();
        self.record_bytes(out.len());
        (out, truncated)
    }
}
