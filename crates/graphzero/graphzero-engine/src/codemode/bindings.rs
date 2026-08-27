//! Direct CodeMode bindings over the typed domain dispatcher (graphzero-o2uq.6).
//!
//! Binding inventory and discovery metadata are generated from `operation_abi`.
//! Recipe, JSON, and JavaScript paths all resolve to [`dispatch_binding`] for
//! domain ops — no MCP envelopes, no inter-op schema revalidation, and no
//! nested CodeMode planner.

use serde_json::{Value, json};

use crate::dispatcher::{AdapterKind, EngineContext, dispatch};
use crate::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, SEMANTIC_CONTRACT_VERSION, all_operations,
    codemode_binding_names, contract_digest_hex, resolve_operation,
};

use super::types::{
    CodeModeLimits, MAX_CODE_BYTES, MAX_LOGICAL_OPS, MAX_MEMORY_BYTES, MAX_MICROTASKS,
    MAX_OUTPUT_BYTES, default_max_wall_ms,
};

/// One CodeMode host binding derived from the registry.
#[derive(Clone, Debug)]
pub struct CodeModeBinding {
    /// Public binding path (`graph.blast`, `ctx.ref`, …).
    pub name: &'static str,
    /// Canonical operation name in the ABI.
    pub canonical: &'static str,
    pub read_only: bool,
    pub input_schema: Value,
    pub output_schema: Value,
}

/// Immutable binding table keyed by semantic contract digest.
#[derive(Clone, Debug)]
pub struct BindingTable {
    pub contract_digest: String,
    pub semantic_contract_version: String,
    pub bindings: Vec<CodeModeBinding>,
}

/// Build the binding table from the live registry (cached by digest in callers).
pub fn binding_table_from_registry() -> BindingTable {
    let mut bindings = Vec::new();
    for op in all_operations() {
        let Some(name) = op.exposure.codemode_binding else {
            continue;
        };
        // Skip pure meta ops that are not domain dispatch targets.
        if op.exposure.codemode_meta && op.exposure.codemode_binding.is_none() {
            continue;
        }
        bindings.push(CodeModeBinding {
            name,
            canonical: op.name,
            read_only: matches!(op.mutability, crate::operation_abi::Mutability::ReadOnly),
            input_schema: op.args.schema.clone(),
            output_schema: op.results.schema.clone(),
        });
    }
    // Stable order for golden diffs.
    bindings.sort_by(|a, b| a.name.cmp(b.name));
    BindingTable {
        contract_digest: contract_digest_hex(),
        semantic_contract_version: SEMANTIC_CONTRACT_VERSION.into(),
        bindings,
    }
}

/// Binding names match registry inventory (set equality).
pub fn binding_names_match_registry(table: &BindingTable) -> bool {
    let from_table: std::collections::BTreeSet<_> = table.bindings.iter().map(|b| b.name).collect();
    let from_reg: std::collections::BTreeSet<_> = codemode_binding_names().into_iter().collect();
    from_table == from_reg
}

/// Resolve a CodeMode binding or canonical/alias name to the registry op name.
pub fn resolve_binding_op(name: &str) -> Result<&'static str, DomainError> {
    // Nested planner / meta tools must never be invoked as domain bindings.
    if is_nested_planner_op(name) {
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!(
                "CodeMode binding refuses nested planner op '{name}'; domain bindings \
call the typed dispatcher only"
            ),
        )
        .with_op(name));
    }
    let op = resolve_operation(name).ok_or_else(|| {
        DomainError::new(
            DomainErrorKind::NotFound,
            format!("unknown binding or op '{name}'"),
        )
        .with_op(name)
    })?;
    if op.exposure.codemode_meta {
        return Err(DomainError::new(
            DomainErrorKind::Policy,
            format!("'{name}' is a CodeMode meta tool, not a domain binding"),
        )
        .with_op(name));
    }
    Ok(op.name)
}

fn is_nested_planner_op(name: &str) -> bool {
    matches!(
        name,
        "execute_code"
            | "gz_execute_code"
            | "codemode_search"
            | "gz_codemode_search"
            | "codemode_describe"
            | "gz_codemode_describe"
            | "tools/call"
            | "tools/list"
    )
}

/// Single-operation CodeMode path: one typed dispatch, no MCP, no plan nesting.
///
/// Resolves through the cached binding table (o2uq.9) when the name is a binding path.
pub fn dispatch_binding(
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    binding_or_op: &str,
    args: &Value,
) -> Result<DomainResult, DomainError> {
    // Touch cached table so discovery/bind resolve share one materialization.
    let _table = super::fuse::cached_binding_table();
    let op = resolve_binding_op(binding_or_op)?;
    let ctx = EngineContext::for_paths(repo_root, store_root, AdapterKind::CodeMode);
    dispatch(&ctx, op, args)
}

/// Normalize a domain result for surface parity (drop transport-only noise).
///
/// Compares value + op + error shape; ignores wall timers and adapter labels.
pub fn normalize_for_parity(result: &DomainResult) -> Value {
    json!({
        "op": result.op,
        "value": strip_volatile(&result.value),
        "refs": normalize_refs_for_parity(&result.refs),
    })
}

/// Normalize a domain error for surface parity.
pub fn normalize_error_for_parity(err: &DomainError) -> Value {
    json!({
        "kind": err.kind.as_str(),
        "retryable": err.retryable,
        "op": err.op,
        // Message text may include paths; keep kind/retryable as the contract.
    })
}

fn strip_volatile(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                // Transport, timing, and store-mutation-order fields that are
                // allowed to differ across sequential surface runs on one store.
                if matches!(
                    k.as_str(),
                    "wall_ms"
                        | "wall_ns"
                        | "duration_ms"
                        | "timestamp"
                        | "execution_id"
                        | "adapter"
                        | "telemetry"
                        | "planner_owner"
                        | "compression_owner"
                        | "boundary_count"
                        | "contract_digest"
                        | "freshness_verified"
                        | "gap_blob_count"
                        | "tier_a_pct"
                        | "tier_b_pct"
                        | "tier_c_pct"
                        | "snapshot_id"
                        | "confidence"
                        | "generated_at" // wall-clock in absence/coverage certificates
                        | "store" // absolute store path differs per isolated clone
                        | "path" // absolute repo path is environment-local
                ) {
                    continue;
                }
                out.insert(k.clone(), strip_volatile(val));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(strip_volatile).collect()),
        Value::String(s) => {
            // Compact query/blob ids differ per store isolation; normalize for parity.
            if s.starts_with("q:") || s.starts_with("gz://") {
                Value::String("<ref>".into())
            } else {
                Value::String(s.clone())
            }
        }
        other => other.clone(),
    }
}

/// Normalize ref lists for cross-surface parity (ids differ per isolated store).
pub fn normalize_refs_for_parity(refs: &[String]) -> Vec<String> {
    refs.iter()
        .map(|r| {
            if r.starts_with("q:") || r.starts_with("gz://") {
                "<ref>".into()
            } else {
                r.clone()
            }
        })
        .collect()
}

/// Limits advertised for CodeMode sessions (deterministic, from constants).
pub fn enforced_limits() -> CodeModeLimits {
    CodeModeLimits::default()
}

/// Map domain limits violations to typed domain errors (graphzero-o2uq.6 AC).
pub fn limit_exceeded_error(limit: &str, detail: impl Into<String>) -> DomainError {
    DomainError::new(
        DomainErrorKind::Policy,
        format!("CodeMode limit exceeded ({limit}): {}", detail.into()),
    )
    .with_op("codemode_limits")
}

/// Check hard limits before plan execution; returns typed errors.
pub fn check_plan_limits(plan: &str, limits: &CodeModeLimits) -> Result<(), DomainError> {
    if plan.len() > limits.max_code_bytes {
        return Err(limit_exceeded_error(
            "max_code_bytes",
            format!("{} > {}", plan.len(), limits.max_code_bytes),
        ));
    }
    if plan.len() > MAX_CODE_BYTES {
        return Err(limit_exceeded_error(
            "max_code_bytes",
            format!("{} > {MAX_CODE_BYTES}", plan.len()),
        ));
    }
    Ok(())
}

/// Documented constant bounds used by golden/limit tests.
pub fn limit_constants() -> Value {
    json!({
        "max_logical_ops": MAX_LOGICAL_OPS,
        "max_microtasks": MAX_MICROTASKS,
        "max_output_bytes": MAX_OUTPUT_BYTES,
        "max_code_bytes": MAX_CODE_BYTES,
        "max_memory_bytes": MAX_MEMORY_BYTES,
        // Effective budget, not the compiled-in floor: hosts propagating their
        // own deadline must see the value plans actually run under.
        "max_wall_ms": default_max_wall_ms(),
    })
}

/// Static: binding module never nests MCP or starts a second planner.
pub fn bindings_source_is_direct() -> bool {
    let src = include_str!("bindings.rs");
    let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
    let scan = prod
        .split("/// Static: binding module never nests")
        .next()
        .unwrap_or(prod);
    let bad = [
        ["rqui", "ckjs"].concat(),
        ["mcp_", "dispatch"].concat(),
        ["Runtime", "::", "new"].concat(),
    ];
    bad.iter().all(|n| !scan.contains(n.as_str())) && scan.contains("dispatch(&ctx")
}
