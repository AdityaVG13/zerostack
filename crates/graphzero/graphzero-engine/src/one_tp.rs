//! GraphZero's 1TP adoption posture.
//!
//! TokenZero owns token measurement and provider certification. GraphZero only
//! publishes a finite registered bound per canonical operation class.

use serde_json::{Value, json};

use crate::operation_abi::all_operations;

pub const ONE_TP_SCHEMA: &str = "graphzero.one_tp_ack.v1";
pub const ONE_TP_ACK: &str = "C";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaRegistration {
    pub operation: &'static str,
    pub operation_class: &'static str,
    pub floor_token_denominator: &'static str,
    pub max_ta: u64,
}

const REGISTRY: &[TaRegistration] = &[
    reg("blast", "analysis", "analysis_floor_tokens", 32),
    reg("callers", "query", "query_floor_tokens", 16),
    reg("codemode_describe", "metadata", "metadata_floor_tokens", 8),
    reg("codemode_search", "metadata", "metadata_floor_tokens", 8),
    reg("ctx_ref", "context", "context_floor_tokens", 16),
    reg("ctx_step", "context", "context_floor_tokens", 16),
    reg("defs", "query", "query_floor_tokens", 16),
    reg("execute_code", "execution", "execution_floor_tokens", 32),
    reg("expand", "materialize", "materialize_floor_tokens", 32),
    reg("index", "mutation", "mutation_floor_tokens", 32),
    reg("orient", "analysis", "analysis_floor_tokens", 32),
    reg("query", "query", "query_floor_tokens", 16),
    reg("multi_query", "query_batch", "query_floor_tokens", 16),
    reg("recall", "query", "query_floor_tokens", 16),
    reg("remember", "mutation", "mutation_floor_tokens", 32),
    reg("reserve", "mutation", "mutation_floor_tokens", 32),
    reg("search", "query", "query_floor_tokens", 16),
    reg("snap", "materialize", "materialize_floor_tokens", 32),
    reg("verify", "analysis", "analysis_floor_tokens", 32),
    reg("orient.callers", "analysis", "analysis_floor_tokens", 32),
    reg("orient.changes", "analysis", "analysis_floor_tokens", 32),
    reg("orient.callpath", "analysis", "analysis_floor_tokens", 32),
    reg("orient.context", "analysis", "analysis_floor_tokens", 32),
    reg("orient.deps", "analysis", "analysis_floor_tokens", 32),
    reg("orient.delta", "analysis", "analysis_floor_tokens", 32),
    reg("orient.hot", "analysis", "analysis_floor_tokens", 32),
    reg("orient.locate", "analysis", "analysis_floor_tokens", 32),
    reg("orient.outline", "analysis", "analysis_floor_tokens", 32),
    reg("orient.recall", "analysis", "analysis_floor_tokens", 32),
    reg(
        "orient.reading_set",
        "analysis",
        "analysis_floor_tokens",
        32,
    ),
    reg("orient.rg_l1", "analysis", "analysis_floor_tokens", 32),
    reg("orient.rg_l2", "analysis", "analysis_floor_tokens", 32),
    reg("orient.rg_l3", "analysis", "analysis_floor_tokens", 32),
    reg("orient.rg_l4", "analysis", "analysis_floor_tokens", 32),
    reg("orient.search", "analysis", "analysis_floor_tokens", 32),
    reg("orient.symbol", "analysis", "analysis_floor_tokens", 32),
    reg("orient.word", "analysis", "analysis_floor_tokens", 32),
];

const fn reg(
    operation: &'static str,
    operation_class: &'static str,
    floor_token_denominator: &'static str,
    max_ta: u64,
) -> TaRegistration {
    TaRegistration {
        operation,
        operation_class,
        floor_token_denominator,
        max_ta,
    }
}

pub fn registration(operation: &str) -> Option<&'static TaRegistration> {
    REGISTRY.iter().find(|entry| entry.operation == operation)
}

pub fn validate_registry() -> Result<(), String> {
    let operations = all_operations();
    let canonical: Vec<&str> = operations.iter().map(|operation| operation.name).collect();
    let mut canonical_seen = std::collections::BTreeSet::new();
    for operation in operations {
        if !canonical_seen.insert(operation.name) {
            return Err(format!("duplicate canonical operation {}", operation.name));
        }
        if registration(operation.name).is_none() {
            return Err(format!("missing TA registration for {}", operation.name));
        }
    }
    if REGISTRY.len() != canonical.len() {
        return Err(format!(
            "TA registry count mismatch: {} != {}",
            REGISTRY.len(),
            canonical.len()
        ));
    }
    let mut registry_seen = std::collections::BTreeSet::new();
    for entry in REGISTRY {
        if !registry_seen.insert(entry.operation) {
            return Err(format!("duplicate TA registration for {}", entry.operation));
        }
        if !canonical.contains(&entry.operation) {
            return Err(format!("stale TA registration for {}", entry.operation));
        }
        if entry.max_ta == 0 || entry.floor_token_denominator.is_empty() {
            return Err(format!("invalid TA registration for {}", entry.operation));
        }
    }
    Ok(())
}

pub fn registry() -> &'static [TaRegistration] {
    REGISTRY
}

pub fn status_value() -> Value {
    validate_registry().expect("canonical operation TA registry must be complete");
    json!({
        "schema": ONE_TP_SCHEMA,
        "ordinal_grammar": "gz://o/<generation>/<one-based-ordinal>",
        "ordinal_namespace": "gz",
        "nodes_are_symbols": true,
        "ta": {
            "posture": "registered_bounds_only",
            "token_measurement_owner": "TokenZero",
            "token_certification": false,
            "registry": REGISTRY.iter().map(|entry| json!({
                "operation": entry.operation,
                "operation_class": entry.operation_class,
                "floor_token_denominator": entry.floor_token_denominator,
                "max_ta": entry.max_ta,
            })).collect::<Vec<_>>(),
        },
    })
}

pub fn status_json() -> Value {
    status_value()
}

pub fn ack(
    snapshot_generation: u64,
    counts: graphzero_store::OrdinalCounts,
    operation: &str,
) -> Result<Value, String> {
    let ta =
        registration(operation).ok_or_else(|| format!("unregistered operation {operation}"))?;
    Ok(json!({
        "schema": ONE_TP_SCHEMA,
        "ack": ONE_TP_ACK,
        "snapshot_generation": snapshot_generation,
        "ordinal_counts": {
            "nodes": counts.symbols,
            "symbols": counts.symbols,
            "edges": counts.edges,
            "total": counts.total,
        },
        "ta": {
            "operation": ta.operation,
            "operation_class": ta.operation_class,
            "floor_token_denominator": ta.floor_token_denominator,
            "max_ta": ta.max_ta,
            "token_certified": false,
        }
    }))
}
