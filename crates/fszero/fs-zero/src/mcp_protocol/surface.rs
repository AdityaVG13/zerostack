//! MCP surface dispatch — per-op MCP vs CodeMode tool catalogs.

use crate::codemode::{
    MAX_CODE_BYTES, MAX_LOGICAL_OPS, MAX_MEMORY_BYTES, MAX_MICROTASKS, MAX_OUTPUT_BYTES,
    MAX_PARALLEL_WIDTH, MAX_PHYSICAL_OPS, MAX_PLAN_STEPS, MAX_REFS_EMITTED, MAX_RESULT_REF_BYTES,
    MAX_WALL_MS, RESPONSE_REF, discovery_describe as codemode_discovery_describe,
    discovery_search as codemode_discovery_search, execute_plan as codemode_execute_plan,
    payload_tool_result, plan_tool_result,
};
use crate::core::{FSZeroSession, dispatch_mcp_tool};
use crate::mcp_rpc::{ack_tool_result, tool_names_sorted};
use crate::packaging::{
    PackageSurface, assert_surface_compiled, dual_surface_diagnostic, reject_dual_env_selection,
    surface_compiled_in,
};
use crate::surfaces::{codemode_tools, mcp_tools};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    PerOp,
    CodeMode,
}

impl SurfaceKind {
    pub fn as_package_surface(self) -> PackageSurface {
        match self {
            Self::PerOp => PackageSurface::Mcp,
            Self::CodeMode => PackageSurface::Codemode,
        }
    }

    /// Distinct MCP `serverInfo.name` per surface so agents can tell PerOp from CodeMode
    /// (R-PAR-REC-004 / fszero-2qdw.8). Names match package artifacts (`fszero-mcp`,
    /// `fszero-codemode`).
    pub fn server_name(self) -> &'static str {
        match self {
            Self::PerOp => "fszero-mcp",
            Self::CodeMode => "fszero-codemode",
        }
    }

    /// Additive surface label (same strings as package surface) for clients that key
    /// on a dedicated field rather than `serverInfo.name`.
    pub fn surface_field(self) -> &'static str {
        self.as_package_surface().as_str()
    }

    pub fn server_description(self) -> &'static str {
        match self {
            Self::PerOp => "FSZero MCP surface — per-op 1-token acks",
            Self::CodeMode => "FSZero CodeMode surface — 1-token plan execution",
        }
    }

    pub fn tools(self) -> Vec<Value> {
        tools_list_for_surface(self)
    }

    pub fn call_tool(
        self,
        sess: &mut FSZeroSession,
        name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        match self {
            Self::PerOp => call_per_op_tool(sess, name, args),
            Self::CodeMode => call_codemode_tool(sess, name, args),
        }
    }
}

/// `tools/list` materialization for one surface — peer catalog names never appear.
pub fn tools_list_for_surface(surface: SurfaceKind) -> Vec<Value> {
    let pkg = surface.as_package_surface();
    if !surface_compiled_in(pkg) {
        return Vec::new();
    }
    let tools = match surface {
        SurfaceKind::PerOp => mcp_tools(),
        SurfaceKind::CodeMode => codemode_tools(),
    };
    tool_names_sorted(tools)
}

/// Server-boundary fail-closed gate (not just env parsing).
///
/// Called before accepting stdio/FastMCP traffic so a dual-selection or
/// wrong-feature artifact never advertises a mixed catalog.
pub fn assert_server_surface_boundary(surface: SurfaceKind) -> Result<(), String> {
    reject_dual_env_selection()?;
    let pkg = surface.as_package_surface();
    assert_surface_compiled(pkg)?;
    // Catalog exclusivity: tools/list must contain only the intended family.
    let tools = tools_list_for_surface(surface);
    if tools.is_empty() {
        return Err(format!(
            "fszero: empty tools/list for surface '{}' (catalog materialization failed)",
            pkg.as_str()
        ));
    }
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    match surface {
        SurfaceKind::PerOp => {
            if names
                .iter()
                .any(|n| n.starts_with("fz_") || n.contains("codemode"))
            {
                return Err(dual_surface_diagnostic(
                    "PerOp tools/list leaked CodeMode catalog entries",
                ));
            }

            if !names.iter().any(|n| n.starts_with("fszero.")) {
                return Err("fszero: PerOp tools/list missing fszero.* tools".into());
            }
        }
        SurfaceKind::CodeMode => {
            if names.iter().any(|n| n.starts_with("fszero.")) {
                return Err(dual_surface_diagnostic(
                    "CodeMode tools/list leaked FastMCP per-op catalog entries",
                ));
            }
            if !names.iter().any(|n| *n == "fz_execute_code") {
                return Err("fszero: CodeMode tools/list missing fz_execute_code".into());
            }
        }
    }
    Ok(())
}
pub fn call_per_op_tool(
    sess: &mut FSZeroSession,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let outcome = dispatch_mcp_tool(sess, name, args).map_err(|e| e.to_string())?;
    let ack = outcome
        .result
        .ack
        .as_deref()
        .unwrap_or(if outcome.result.ok { "ok" } else { "X0" });
    let mut result = ack_tool_result(sess, ack, outcome.result.ok, outcome.detail.as_deref());
    let mut payloads = Vec::<Vec<u8>>::new();
    let mut misses = 0u64;
    if outcome.opcode == Some('X') && outcome.result.ok {
        if let Some(bytes) = sess.expand("expand") {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            payloads.push(bytes);
            if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
                content.push(serde_json::json!({"type":"text","text":text}));
            }
            if let Some(structured) = result.get_mut("structuredContent") {
                structured["payload"] = serde_json::json!(text);
            }
        } else {
            misses += 1;
        }
    } else if let Some(key) = outcome.recovery_key.as_deref() {
        if let Some(bytes) = sess.expand(key) {
            payloads.push(bytes);
        } else {
            misses += 1;
        }
    }
    if payloads.is_empty() {
        if let Some(detail) = outcome.detail.as_ref() {
            payloads.push(detail.as_bytes().to_vec());
        }
    }
    if let Some(evidence) = outcome.inline_evidence.as_ref() {
        if let Some(structured) = result.get_mut("structuredContent") {
            structured["evidence"] = serde_json::json!(evidence);
        }
    }
    crate::mcp_rpc::attach_observed_tool_measurement(&mut result, &payloads, 1, misses);
    Ok(result)
}

fn measured_result(mut result: Value, payloads: Vec<Vec<u8>>, misses: u64) -> Value {
    crate::mcp_rpc::attach_observed_tool_measurement(&mut result, &payloads, 1, misses);
    result
}

fn call_codemode_tool(sess: &mut FSZeroSession, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "fz_execute_code" => {
            sess.recovery.reset_metrics();
            let plan = args.get("plan").and_then(Value::as_str).unwrap_or("");
            sess.codemode_defer_wire_receipt = true;
            let ack = codemode_execute_plan(sess, plan);
            let (payload, ok) = resolve_codemode_response(sess, &ack);
            let envelope = args.get("envelope").and_then(Value::as_str);
            let result = plan_tool_result(sess, payload, ok, envelope);
            sess.codemode_defer_wire_receipt = false;
            Ok(result)
        }
        "fz_codemode_search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let ack = codemode_discovery_search(sess, query);
            // fszero-cr3v: discovery is metadata — return ranked methods inline.
            // S2-only primary text forced a second expand of an opaque store key.
            let body = sess
                .expand(crate::codemode::SEARCH_REF)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let methods: Vec<String> = body
                .lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect();
            let payload = serde_json::json!({
                "ack": ack,
                "query": query,
                "methods": methods,
                "ranking": body,
            });
            let bytes = serde_json::to_vec(&payload).unwrap_or_default();
            Ok(measured_result(
                payload_tool_result(payload, true),
                vec![bytes],
                0,
            ))
        }
        "fz_codemode_describe" => {
            let target = args
                .get("name")
                .or_else(|| args.get("target"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if target == "capabilities" {
                let manifest = serde_json::json!({
                    "contract_version": "1.0", "ns": "fz", "mutation": "allowed",
                    "plan_forms": ["recipe", "json", "js"],
                    "limits": {
                        "max_plan_steps": MAX_PLAN_STEPS, "max_parallel_width": MAX_PARALLEL_WIDTH,
                        "max_logical_ops": MAX_LOGICAL_OPS, "max_physical_ops": MAX_PHYSICAL_OPS,
                        "max_wall_ms": MAX_WALL_MS, "max_microtasks": MAX_MICROTASKS,
                        "max_memory_bytes": MAX_MEMORY_BYTES, "max_output_bytes": MAX_OUTPUT_BYTES,
                        "max_result_ref_bytes": MAX_RESULT_REF_BYTES, "max_refs_emitted": MAX_REFS_EMITTED,
                        "max_code_bytes": MAX_CODE_BYTES
                    }
                });
                let bytes = serde_json::to_vec(&manifest).unwrap_or_default();
                return Ok(measured_result(
                    payload_tool_result(manifest, true),
                    vec![bytes],
                    0,
                ));
            }
            let ack = codemode_discovery_describe(sess, target);
            let ok = ack != "X0";
            if ok {
                let doc = sess
                    .expand(crate::codemode::DESCRIBE_REF)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                Ok(measured_result(
                    payload_tool_result(serde_json::json!({"name":target,"description":doc}), true),
                    vec![doc.as_bytes().to_vec()],
                    0,
                ))
            } else {
                let payload = serde_json::json!({"ack":"X0","error":{"kind":"validation","message":"unknown description target","retryable":false}});
                let bytes = serde_json::to_vec(&payload).unwrap_or_default();
                Ok(measured_result(
                    payload_tool_result(payload, false),
                    vec![bytes],
                    0,
                ))
            }
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// Resolve the CodeMode response for `fz_execute_code` (fszero-iod).
///
/// Prefer the in-memory stash written by `finish()`, then durable expand of
/// `RESPONSE_REF`. A miss is a substrate fault: bump runtime health, trip
/// fail-open / native-fallback, and return a schema-valid error payload.
/// Even when the plan ack was `C`, a missing payload is reported as failure.
fn codemode_payload_ok(ack: &str, payload: &Value) -> bool {
    ack == "C" && payload.get("error").map(|e| e.is_null()).unwrap_or(true)
}

pub fn resolve_codemode_response(sess: &mut FSZeroSession, ack: &str) -> (Value, bool) {
    if let Some(payload) = sess.take_codemode_response().or_else(|| {
        sess.expand(RESPONSE_REF)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    }) {
        sess.runtime_health.record_success();
        let ok = codemode_payload_ok(ack, &payload);
        return (payload, ok);
    }

    let detail = sess
        .recovery
        .take_store_error()
        .unwrap_or_else(|| "missing response payload (store degraded?)".to_string());
    sess.runtime_health.record_substrate_failure(&detail);
    let health = sess.runtime_health.to_json();
    let native_fallback = sess.runtime_health.native_fallback();
    let root = sess
        .workspace_root()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    // The fallback MUST satisfy the declared output schema
    // (ack + execution_id required): a degraded store previously
    // produced a bare {ack, error} object, and the MCP layer's
    // -32602 schema violation MASKED the real failure (observed
    // live from a droid session).
    let payload = serde_json::json!({
        "ack": "X0", "execution_id": "cm://exec/response-unavailable",
        "refs": {},
        "error": { "kind": "substrate", "message": detail, "retryable": true, "root": root, "telemetry_ref": "codemode/telemetry" },
        "fz_runtime_health": health, "native_fallback": native_fallback,
    });
    (payload, false)
}

#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/surface_catalog_tests.rs"]
mod catalog_tests;
#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/surface_accounting_tests.rs"]
mod accounting_tests;
