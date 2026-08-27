//! Capability discovery (search / describe) — the public API surface.
//!
//! Binding inventory is derived from `operation_abi` so CodeMode discovery
//! cannot drift from the canonical registry (graphzero-o2uq.1).

use serde_json::{Value, json};

use super::fuse::cached_binding_table;
use crate::operation_abi::{
    SEMANTIC_CONTRACT_VERSION, all_operations, codemode_discovery_hits, contract_digest_hex,
    resolve_operation,
};

use super::types::{MAX_CODE_BYTES, MAX_LOGICAL_OPS, MAX_MICROTASKS, MAX_OUTPUT_BYTES};

fn enforced_limits_manifest() -> Value {
    json!({
        "max_logical_ops": MAX_LOGICAL_OPS,
        "max_microtasks": MAX_MICROTASKS,
        "max_output_bytes": MAX_OUTPUT_BYTES,
        "max_code_bytes": MAX_CODE_BYTES,
    })
}

fn capability_manifest() -> Value {
    json!({
        "contract_version": "1.0",
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "contract_digest": contract_digest_hex(),
        "ns": "gz",
        // store_only: remember/reserve/index mutate GraphZero's own store
        // (memory, reservations, shards) but NEVER repository files.
        "mutation": "store_only",
        "plan_forms": ["recipe", "json", "js"],
        "limits": enforced_limits_manifest(),
        "zeroref": graphzero_store::ZeroRefDescriptor::from_env().to_json(),
        "one_tp": crate::one_tp::status_value(),
    })
}

pub fn search(query: &str) -> Result<String, serde_json::Error> {
    let q = query.to_ascii_lowercase();
    let mut hits = codemode_discovery_hits();
    // Preserve batch_variant annotation for graph.query when present.
    for hit in &mut hits {
        if hit.get("name").and_then(Value::as_str) == Some("graph.query") {
            hit.as_object_mut()
                .map(|m| m.insert("batch_variant".into(), json!("graph.multiQuery")));
        }
    }
    if !q.is_empty() {
        hits.retain(|h| h.to_string().to_ascii_lowercase().contains(&q));
    }
    serde_json::to_string(&json!({
        "surface":"gz_codemode_search",
        "hits": hits,
        "limits": enforced_limits_manifest(),
        "safety": safety_metadata(),
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "contract_digest": contract_digest_hex(),
    }))
}

pub fn describe(name: &str) -> Result<String, serde_json::Error> {
    if name == "capabilities" {
        return serde_json::to_string(&capability_manifest());
    }
    if name == "limits" {
        return serde_json::to_string(&enforced_limits_manifest());
    }
    if name == "one_tp" {
        return serde_json::to_string(&crate::one_tp::status_value());
    }
    if name == "contract" {
        return serde_json::to_string(&json!({
            "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
            "contract_digest": contract_digest_hex(),
            "operation_count": all_operations().len(),
        }));
    }
    let methods = discovery_registry();
    let hit = methods
        .into_iter()
        .find(|m| m.get("name").and_then(Value::as_str) == Some(name));
    let resolved = resolve_operation(name);
    serde_json::to_string(&json!({
        "name": name,
        "description": hit.unwrap_or_else(|| json!({"error":"unknown method or recipe"})),
        "canonical": resolved.map(|op| op.name),
        "limits": enforced_limits_manifest(),
        "safety": safety_metadata(),
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "contract_digest": contract_digest_hex(),
        "examples": {
            "recipe": "callers:beta",
            "json": {"steps":[{"id":"a","op":"query","surface":"callers","target":"beta"}]},
            "code": "const c = await graph.callers('beta'); return await ctx.ref({c});"
        }
    }))
}

fn discovery_registry() -> Vec<Value> {
    // Production path uses the process-wide cached binding table (o2uq.9).
    let table = cached_binding_table();
    table
        .bindings
        .iter()
        .map(|b| {
            let required: Vec<&str> = b
                .input_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            json!({
                "name": b.name,
                "canonical": b.canonical,
                "signature": format!("{}(...)", b.name),
                "required_args": required,
                "read_only": b.read_only,
                "mutating": !b.read_only,
                "input_schema": b.input_schema,
                "output_schema": b.output_schema,
                "contract_digest": table.contract_digest,
            })
        })
        .collect()
}

fn safety_metadata() -> Value {
    json!({
        "read_only": false,
        "mutation_surface": "store_only",
        "denied": ["fetch/network", "process/spawn", "env", "raw_host_fs", "direct_db_store", "native_module_loading", "unbounded_timer"],
        "limits_enforced": ["logical_ops", "code_bytes", "output_bytes", "microtasks"],
    })
}
