//! Snap route trace helpers (FR-010, FR-011).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static SEMANTIC_DISABLED: OnceLock<AtomicBool> = OnceLock::new();

fn semantic_disabled_state() -> &'static AtomicBool {
    SEMANTIC_DISABLED.get_or_init(|| {
        let disabled = std::env::var("GRAPHZERO_DISABLE_SEMANTIC")
            .map(|value| value == "1")
            .unwrap_or(false);
        AtomicBool::new(disabled)
    })
}

#[cfg(test)]
fn set_semantic_disabled(disabled: bool) -> bool {
    semantic_disabled_state().swap(disabled, Ordering::Relaxed)
}

#[cfg(test)]
struct SemanticDisabledGuard(bool);

#[cfg(test)]
impl Drop for SemanticDisabledGuard {
    fn drop(&mut self) {
        semantic_disabled_state().store(self.0, Ordering::Relaxed);
    }
}

#[cfg(test)]
fn override_semantic_disabled(disabled: bool) -> SemanticDisabledGuard {
    SemanticDisabledGuard(set_semantic_disabled(disabled))
}

/// Ordered snap routes per moat layer 3.
pub const SNAP_ROUTE_ORDER: &[&str] = &["symbol", "trigram", "semantic"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapRouteTrace {
    pub attempted: Vec<&'static str>,
}

impl SnapRouteTrace {
    pub fn record_attempt(&mut self, route: &'static str) {
        if self.attempted.contains(&route) {
            return;
        }
        self.attempted.push(route);
    }

    pub fn as_order_list(&self) -> String {
        self.attempted.join(",")
    }
}

pub fn semantic_disabled() -> bool {
    semantic_disabled_state().load(Ordering::Relaxed)
}

fn semantic_route_enabled(semantic_enabled: bool) -> bool {
    semantic_enabled && !semantic_disabled()
}

pub fn resolve_route_order(
    symbol_hit: bool,
    trigram_hit: bool,
    semantic_enabled: bool,
) -> SnapRouteTrace {
    let mut trace = SnapRouteTrace::default();
    trace.record_attempt("symbol");
    if symbol_hit {
        return trace;
    }
    trace.record_attempt("trigram");
    if trigram_hit {
        return trace;
    }
    if semantic_route_enabled(semantic_enabled) {
        trace.record_attempt("semantic");
    }
    trace
}
