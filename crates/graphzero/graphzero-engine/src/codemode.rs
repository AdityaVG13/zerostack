//! GraphZero recipe and JSON-DAG compatibility executor.
//!
//! GraphZero keeps its domain registry, refs, graph dispatch, and telemetry.
//! JavaScript plans are rejected here and must run through the aggregate
//! `zerostack-codemode-host` or `zsx` against the raw worker.

// ── sub-modules ──

pub mod bindings;
pub mod discovery;
pub(crate) mod errors;
pub(crate) mod executor;
pub mod fuse;
pub(crate) mod plan;
pub(crate) mod response;
pub(crate) mod state;
pub(crate) mod steps;
pub mod types;
pub(crate) mod utils;

/// GraphZero never links or creates a JavaScript runtime.
pub fn js_runtime_compiled() -> bool {
    false
}

/// Aggregate-host topology has no local runtime creation counter.
pub fn sandbox_runtime_creation_count() -> u64 {
    0
}

/// Compatibility no-op retained for callers that assert runtime absence.
pub fn reset_sandbox_runtime_creation_count_for_tests() {}

// ── public re-exports ──

pub use types::{CodeModeError, CodeModeLimits, CodeModeResponse, CodeModeTelemetry, StepRecord};

pub use discovery::{describe, search};

pub use bindings::{
    BindingTable, CodeModeBinding, binding_table_from_registry, check_plan_limits,
    dispatch_binding, normalize_error_for_parity, normalize_for_parity, resolve_binding_op,
};
pub use fuse::{
    FusedOutcome, FusedStep, FusionProfile, binding_table_build_count, cached_binding_table,
    fused_dispatch, fused_unfused_semantic_parity, profile_fusion, unfused_dispatch,
};
pub use state::{codemode_context_build_count, reset_codemode_context_build_count_for_tests};

pub use executor::{
    execute, execute_plan, execute_with_host, execute_with_host_and_limits,
    execute_with_host_options, execute_with_host_options_controlled, materialize_failure,
    parallel_group_peak, reset_parallel_group_peak_for_tests,
};

// ── host trait ──

use serde_json::Value;

pub trait CodeModeHostOps {
    fn reserve(&self, action: &str, args: &Value) -> Result<Value, String>;
}

// ── envelope feature flag ──

pub fn envelope_v1_enabled() -> bool {
    matches!(std::env::var("ZERO_ENVELOPE"), Ok(v) if v.eq_ignore_ascii_case("v1"))
}
