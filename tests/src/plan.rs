//! Plan-level conformance gates G1-G10 (the *other* layer).
//!
//! These gates drive a planner host that serves `{ns}_execute_code` over the
//! MCP JSON-RPC framing: JS plan execution, `ctx.step`, aggregate op
//! coalescing, and the JS sandbox. They are the canonical plan-level checks
//! (`checks::CheckId` / `GATE_MAPPINGS`) and are DISTINCT from the raw-worker
//! RW1-RW10 gates (`raw_worker.rs`): a planner owns plan semantics a raw
//! worker cannot.
//!
//! This module re-homes the live plan-level driver that previously lived in
//! `lib.rs` so it is not silently dropped. It is reached via
//! `Surface::Planner`; a `*-codemode` raw-worker artifact is NEVER driven
//! here (that is the raw-worker path), and a `*-mcp` artifact only exercises
//! G1 exposure.

use crate::{CheckResult, McpClient, Ns, collect_refs, valid_execution_id, valid_ref};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use zero_abi::{canonical_json, sha256_hex};

/// Run the full plan-level G1-G10 conformance for one planner artifact.
///
/// G1 is MCP exposure (initialize + the three CodeMode tools present); G2-G10
/// drive `{ns}_execute_code` plans through [`run_live_checks`].
pub fn run_conformance(ns: Ns, bin: &Path, timeout: Duration) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    checks.push(match check_plan_exposure(ns, bin, timeout) {
        Ok(check) => check,
        Err(err) => CheckResult::fail("G1", "exposure", err.to_string()),
    });

    let client = match McpClient::spawn(bin, None, timeout) {
        Ok(mut client) => match client.initialize() {
            Ok(_) => Some(client),
            Err(err) => {
                checks.push(CheckResult::fail(
                    "G2",
                    "refs",
                    format!("planner initialize failed: {err}"),
                ));
                None
            }
        },
        Err(err) => {
            checks.push(CheckResult::fail(
                "G2",
                "refs",
                format!("could not spawn planner server: {err}"),
            ));
            None
        }
    };

    match client {
        Some(mut client) => checks.append(&mut run_live_checks(ns, &mut client)),
        None => {
            for (id, name) in [
                ("G3", "telemetry"),
                ("G4", "leak-proof"),
                ("G5", "errors"),
                ("G6", "ctx.step"),
                ("G7", "limits"),
                ("G8", "mutation"),
                ("G9", "coalescing"),
                ("G10", "sandbox-denial"),
            ] {
                checks.push(CheckResult::skip(
                    id,
                    name,
                    "planner server did not initialize",
                ));
            }
        }
    }
    checks
}

/// G1 exposure for a planner artifact: it initializes over JSON-RPC and lists
/// exactly the three CodeMode tools, and refuses to also serve the opposite
/// (MCP) surface.
fn check_plan_exposure(ns: Ns, bin: &Path, timeout: Duration) -> Result<CheckResult, String> {
    let codemode_tools: std::collections::BTreeSet<String> = ns.tool_names().into_iter().collect();
    let mut client = McpClient::spawn(bin, None, timeout).map_err(|e| e.to_string())?;
    client.initialize().map_err(|e| e.to_string())?;
    let served: std::collections::BTreeSet<String> = client
        .list_tools()
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    let mut details = Vec::new();
    if served != codemode_tools {
        details.push(format!(
            "planner artifact served {served:?}, expected exactly {codemode_tools:?}"
        ));
    }
    // Opposite (MCP) spawn failure is a pass: fail-closed refusal.
    if let Ok(mut wrong) = McpClient::spawn(bin, Some("mcp"), timeout)
        && wrong.initialize().is_ok()
        && wrong.list_tools().is_ok()
    {
        details.push(
            "planner artifact also served the mcp surface; surfaces must be mutually exclusive"
                .into(),
        );
    }
    Ok(CheckResult::with_details("G1", "exposure", details))
}

fn run_live_checks(ns: Ns, client: &mut McpClient) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    let describe_tool = format!("{}_codemode_describe", ns.as_str());
    let execute_tool = format!("{}_execute_code", ns.as_str());

    let capabilities = client.call_tool(&describe_tool, json!({ "name": "capabilities" }));
    let manifest = match capabilities {
        Ok(response) => match extract_json_payload(&response) {
            Some(manifest) => Some(manifest),
            None => {
                checks.push(CheckResult::fail(
                    "G7",
                    "limits",
                    "capabilities probe returned no JSON payload",
                ));
                None
            }
        },
        Err(err) => {
            checks.push(CheckResult::fail(
                "G7",
                "limits",
                format!("capabilities probe failed at MCP layer: {err}"),
            ));
            None
        }
    };
    let mut g7_limits = BTreeMap::new();
    if let Some(value) = manifest.as_ref() {
        let details = crate::validate_capability_manifest(ns, value);
        if let Some(limits) = value.get("limits").and_then(Value::as_object) {
            for (name, value) in limits {
                if let Some(value) = value.as_u64() {
                    g7_limits.insert(name.clone(), value);
                }
            }
        }
        if !details.is_empty() {
            checks.push(CheckResult::with_details("G7", "limits", details));
        }
    }

    let basic = client.call_tool(
        &execute_tool,
        json!({ "plan": "return { ok: true };", "form": "js" }),
    );
    let basic_value = basic.as_ref().ok().and_then(extract_json_payload);
    checks.push(check_refs(ns, basic_value.as_ref()));
    checks.push(check_telemetry(basic_value.as_ref()));
    checks.push(check_ctx_step(ns, client, &execute_tool));
    checks.push(check_leak_proof(ns, client, &execute_tool));
    checks.push(check_errors(ns, client, &execute_tool));
    if !checks.iter().any(|check| check.id == "G7") {
        checks.push(check_limits(ns, client, &execute_tool, &g7_limits));
    }
    checks.push(check_mutation(ns, client, &execute_tool, manifest.as_ref()));
    checks.push(check_coalescing(client, &execute_tool));
    checks.push(check_sandbox_denial(client, &execute_tool));
    checks
}

fn check_refs(ns: Ns, payload: Option<&Value>) -> CheckResult {
    let Some(payload) = payload else {
        return CheckResult::fail("G2", "refs", "execute_code did not return JSON payload");
    };
    let mut details = Vec::new();
    if let Some(execution_id) = payload.get("execution_id").and_then(Value::as_str) {
        if !valid_execution_id(execution_id) {
            details.push(format!("invalid execution_id {execution_id:?}"));
        }
    } else {
        details.push("missing execution_id".into());
    }
    for value in collect_refs(payload) {
        if !valid_ref(ns, &value) && !valid_execution_id(&value) {
            details.push(format!("invalid CodeMode ref {value:?}"));
        }
    }
    CheckResult::with_details("G2", "refs", details)
}

fn check_telemetry(payload: Option<&Value>) -> CheckResult {
    let Some(payload) = payload else {
        return CheckResult::fail(
            "G3",
            "telemetry",
            "execute_code did not return JSON payload",
        );
    };
    let Some(telemetry) = payload.get("telemetry") else {
        return CheckResult::fail("G3", "telemetry", "missing telemetry object");
    };
    CheckResult::with_details("G3", "telemetry", crate::validate_telemetry(telemetry))
}

fn check_leak_proof(ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return 'x'.repeat(70000);";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let visible = response.to_string().len();
            let payload = extract_json_payload(&response).unwrap_or(response);
            let refs = collect_refs(&payload);
            let mut details = Vec::new();
            if visible > 65_536 {
                details.push(format!(
                    "visible response is {visible} bytes, exceeds 64 KiB guard"
                ));
            }
            if !refs.iter().any(|value| valid_ref(ns, value)) {
                details.push("oversize result did not return a valid result/blob ref".into());
            }
            CheckResult::with_details("G4", "leak-proof", details)
        }
        Err(err) => CheckResult::fail("G4", "leak-proof", err.to_string()),
    }
}

fn check_errors(_ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let cases = [
        (
            "validation",
            json!({ "plan": "{ definitely invalid json", "form": "json" }),
        ),
        (
            "sandbox",
            json!({ "plan": "return fetch('https://example.com');", "form": "js" }),
        ),
        (
            "runtime",
            json!({ "plan": "throw new Error('boom');", "form": "js" }),
        ),
        (
            "substrate",
            json!({ "plan": "return zero.read('__zerostack_missing_target__');", "form": "js" }),
        ),
        (
            "policy",
            json!({ "plan": "return zero.edit('x', 'y');", "form": "js" }),
        ),
    ];
    let mut details = Vec::new();
    for (kind, args) in cases {
        match client.call_tool(execute_tool, args) {
            Ok(response) => {
                let payload = extract_json_payload(&response).unwrap_or(response);
                let error = payload
                    .get("error")
                    .or_else(|| payload.get("content").and_then(|v| v.get("error")));
                match error {
                    Some(error) => {
                        let error_details = crate::validate_error(error);
                        if !error_details.is_empty() {
                            details.push(format!("{kind} case invalid error: {error_details:?}"));
                        }
                        if error.get("kind").and_then(Value::as_str) != Some(kind) {
                            details.push(format!("{kind} case returned wrong kind: {error}"));
                        }
                    }
                    None => details.push(format!(
                        "{kind} case did not return structured error: {payload}"
                    )),
                }
            }
            Err(err) => details.push(format!("{kind} case MCP call failed: {err}")),
        }
    }
    CheckResult::with_details("G5", "errors", details)
}

fn canonical_inline_step_receipt(payload: &Value) -> bool {
    let Some(receipt) = payload.get("step_receipt") else {
        return false;
    };
    if receipt.get("schema").and_then(Value::as_str) != Some("zerostack.codemode.step_receipt.v1") {
        return false;
    }
    let Some(generation) = receipt.get("generation").and_then(Value::as_u64) else {
        return false;
    };
    let Some(request_id) = receipt.get("request_id").and_then(Value::as_u64) else {
        return false;
    };
    let Some(steps) = receipt.get("steps").and_then(Value::as_array) else {
        return false;
    };
    if steps.is_empty()
        || receipt.get("step_count").and_then(Value::as_u64) != Some(steps.len() as u64)
    {
        return false;
    }
    let mut previous = "0".repeat(64);
    for (index, step) in steps.iter().enumerate() {
        if step.get("index").and_then(Value::as_u64) != Some(index as u64)
            || step.get("generation").and_then(Value::as_u64) != Some(generation)
            || step.get("request_id").and_then(Value::as_u64) != Some(request_id)
            || step.get("previous_sha256").and_then(Value::as_str) != Some(previous.as_str())
        {
            return false;
        }
        let Some(claimed) = step.get("entry_sha256").and_then(Value::as_str) else {
            return false;
        };
        let mut body = step.clone();
        body.as_object_mut().unwrap().remove("entry_sha256");
        if sha256_hex(canonical_json(&body).as_bytes()) != claimed {
            return false;
        }
        previous = claimed.to_owned();
    }
    if receipt.get("head_sha256").and_then(Value::as_str) != Some(previous.as_str()) {
        return false;
    }
    let Some(claimed) = receipt.get("receipt_sha256").and_then(Value::as_str) else {
        return false;
    };
    let mut body = receipt.clone();
    body.as_object_mut().unwrap().remove("receipt_sha256");
    sha256_hex(canonical_json(&body).as_bytes()) == claimed
}

fn check_ctx_step(ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return ctx.step('x', () => ({value: 42}));";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let mut details = check_refs(ns, Some(&payload)).details;
            let refs = collect_refs(&payload);
            let inline_receipt = canonical_inline_step_receipt(&payload);
            if !inline_receipt && !refs.iter().any(|value| value.ends_with("/steps")) {
                details.push("no canonical inline or recoverable steps receipt returned".into());
            }
            CheckResult::with_details("G6", "ctx.step", details)
        }
        Err(err) => CheckResult::fail("G6", "ctx.step", err.to_string()),
    }
}

fn limit_probe_plan(name: &str, limit: u64) -> Option<String> {
    let above = limit.checked_add(1)?;
    match name {
        "max_code_bytes" => usize::try_from(above).ok().map(|size| "x".repeat(size)),
        "max_microtasks" => Some(format!(
            "let p = Promise.resolve(); for (let i=0;i<{above};i++) p = p.then(() => 1); return p;"
        )),
        "max_output_bytes" => Some(format!("return 'x'.repeat({above});")),
        "max_logical_ops" => Some(format!(
            "for (let i=0;i<{above};i++) {{ ctx.ref(i); }} return 1;"
        )),
        "max_parallel_width" => Some(format!(
            "return zero.queryMany ? zero.queryMany(Array.from({{length: {above}}}, (_, i) => String(i))) : 1;"
        )),
        "max_wall_ms"
        | "hard_max_wall_ms"
        | "max_memory_bytes"
        | "max_physical_ops"
        | "max_result_ref_bytes"
        | "max_refs_emitted" => None,
        _ => None,
    }
}

fn check_limits(
    _ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    limits: &BTreeMap<String, u64>,
) -> CheckResult {
    let mut details = Vec::new();
    for (name, limit) in limits {
        if let Some(plan) = limit_probe_plan(name, *limit) {
            match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
                Ok(response) => {
                    let payload = extract_json_payload(&response).unwrap_or(response);
                    let enforced = payload.get("ack").and_then(Value::as_str) == Some("X0")
                        || payload.get("error").is_some()
                        || (name == "max_output_bytes"
                            && payload.to_string().len() <= *limit as usize);
                    if !enforced {
                        details.push(format!("echoed limit {name} was not observably enforced"));
                    }
                }
                Err(err) => details.push(format!(
                    "echoed limit {name} probe failed at MCP layer: {err}"
                )),
            }
        } else {
            details.push(format!("echoed limit {name} has no generic violation probe; substrate must add one or omit the limit"));
        }
    }
    CheckResult::with_details("G7", "limits", details)
}

/// Namespace default for G8 mutation capability (lookup table, not match arms).
fn expected_mutation(ns: Ns) -> &'static str {
    match ns {
        Ns::Fz => "allowed",
        Ns::Tz => "denied",
        Ns::Gz => "store_only",
    }
}

/// Pure interpret of a mutation probe response: `(declared x ack x error_kind)`.
fn interpret_mutation_probe(
    declared: &str,
    ack: Option<&str>,
    error_kind: Option<&str>,
    payload: &Value,
) -> Vec<String> {
    let mut details = Vec::new();
    match declared {
        "allowed" => {
            if ack == Some("X0") && error_kind == Some("policy") {
                details.push("allowed mutation capability rejected mutation with policy".into());
            }
        }
        "denied" | "readonly" | "store_only" => {
            if error_kind != Some("policy") {
                details.push(format!(
                    "{declared} mutation capability did not reject with policy: {payload}"
                ));
            }
        }
        _ => details.push(format!("unknown mutation capability {declared:?}")),
    }
    details
}

fn check_mutation(
    ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    manifest: Option<&Value>,
) -> CheckResult {
    let expected = expected_mutation(ns);
    let declared = manifest
        .and_then(|value| value.get("mutation"))
        .and_then(Value::as_str)
        .unwrap_or(expected);
    let mut details = Vec::new();
    if declared != expected {
        details.push(format!(
            "declared mutation {declared:?} does not match required namespace default"
        ));
    }
    match client.call_tool(
        execute_tool,
        json!({ "plan": "return zero.edit('x', 'y');", "form": "js" }),
    ) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let ack = payload.get("ack").and_then(Value::as_str);
            let error_kind = payload.pointer("/error/kind").and_then(Value::as_str);
            details.extend(interpret_mutation_probe(
                declared, ack, error_kind, &payload,
            ));
        }
        Err(err) => details.push(format!("mutation probe failed at MCP layer: {err}")),
    }
    CheckResult::with_details("G8", "mutation", details)
}

fn check_coalescing(client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return zero.queryMany ? zero.queryMany(Array.from({length: 100}, (_, i) => String(i))) : ctx.step('batch', () => Array.from({length: 100}, (_, i) => ctx.ref(i)));";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let telemetry = payload.get("telemetry");
            let mut details = Vec::new();
            match telemetry {
                Some(telemetry) => {
                    if telemetry
                        .get("physical_ops")
                        .and_then(Value::as_u64)
                        .unwrap_or(100)
                        >= 100
                    {
                        details.push(format!("physical_ops not coalesced below 100: {telemetry}"));
                    }
                    if telemetry
                        .get("batched_ops")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        < 1
                    {
                        details.push(format!("batched_ops < 1: {telemetry}"));
                    }
                }
                None => details.push("missing telemetry for coalescing probe".into()),
            }
            CheckResult::with_details("G9", "coalescing", details)
        }
        Err(err) => CheckResult::fail("G9", "coalescing", err.to_string()),
    }
}

fn check_sandbox_denial(client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let cases = [
        ("network/fetch", "return fetch('https://example.com');"),
        ("env", "return process.env.HOME;"),
        (
            "process/spawn",
            "return require('child_process').spawn('true');",
        ),
        (
            "raw host FS",
            "return require('fs').readFileSync('/etc/passwd', 'utf8');",
        ),
        (
            "direct DB/store",
            "return globalThis.db || globalThis.store || sqlite;",
        ),
        ("native modules", "return require('node:fs');"),
        ("timers", "return setTimeout(() => 1, 1);"),
    ];
    let mut details = Vec::new();
    for (name, plan) in cases {
        match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
            Ok(response) => {
                let payload = extract_json_payload(&response).unwrap_or(response);
                if payload.pointer("/error/kind").and_then(Value::as_str) != Some("sandbox") {
                    details.push(format!(
                        "{name} was not denied with sandbox error: {payload}"
                    ));
                }
            }
            Err(err) => details.push(format!("{name} probe failed at MCP layer: {err}")),
        }
    }
    CheckResult::with_details("G10", "sandbox-denial", details)
}

/// Pull the substrate's JSON payload out of an MCP tool response. Payload
/// priority (load-bearing, do not reorder): top-level `structuredContent`;
/// `result.structuredContent`; whole-body markers (`ack`|`contract_version`|
/// `telemetry`); `result.content[]` (structured/json before text); bare
/// `result`; top-level `content[]`.
fn extract_json_payload(response: &Value) -> Option<Value> {
    for extractor in JSON_PAYLOAD_EXTRACTORS {
        if let Some(payload) = extractor(response) {
            return Some(payload);
        }
    }
    None
}

const JSON_PAYLOAD_EXTRACTORS: &[fn(&Value) -> Option<Value>] = &[
    payload_top_level_structured,
    payload_result_structured,
    payload_whole_body_markers,
    payload_result_content,
    payload_bare_result,
    payload_top_level_content,
];

fn payload_top_level_structured(response: &Value) -> Option<Value> {
    response
        .get("structuredContent")
        .filter(|structured| structured.is_object())
        .cloned()
}

fn payload_result_structured(response: &Value) -> Option<Value> {
    response
        .get("result")
        .and_then(|r| r.get("structuredContent"))
        .filter(|structured| structured.is_object())
        .cloned()
}

fn payload_whole_body_markers(response: &Value) -> Option<Value> {
    if response.get("ack").is_some()
        || response.get("contract_version").is_some()
        || response.get("telemetry").is_some()
    {
        Some(response.clone())
    } else {
        None
    }
}

fn payload_result_content(response: &Value) -> Option<Value> {
    response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|c| payload_from_content(c))
}

fn payload_bare_result(response: &Value) -> Option<Value> {
    response
        .get("result")
        .filter(|result| result.is_object())
        .cloned()
}

fn payload_top_level_content(response: &Value) -> Option<Value> {
    response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| payload_from_content(c))
}

fn explicit_content_payload(item: &Value) -> Option<Value> {
    match item.get("structuredContent") {
        Some(structured) if structured.is_object() => Some(structured.clone()),
        _ => item.get("json").cloned(),
    }
}

fn payload_from_content(content: &[Value]) -> Option<Value> {
    for item in content {
        if let Some(payload) = explicit_content_payload(item) {
            return Some(payload);
        }
    }
    for item in content {
        if let Some(text) = item.get("text").and_then(Value::as_str)
            && let Ok(parsed) = serde_json::from_str::<Value>(text)
            && matches!(parsed, Value::Object(_) | Value::Array(_))
        {
            return Some(parsed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_gates_emit_only_the_canonical_g_vocabulary() {
        // The plan layer emits G1-G10 and is distinct from the raw RW layer.
        let g: std::collections::HashSet<&str> = crate::checks::GATE_MAPPINGS
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        let rw: std::collections::HashSet<&str> = crate::checks::RAW_GATE_MAPPINGS
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert!(g.is_disjoint(&rw));
        // limit_probe_plan is the planner's per-limit violation generator.
        assert!(limit_probe_plan("max_output_bytes", 64).is_some());
        assert!(limit_probe_plan("max_wall_ms", 1).is_none());
    }

    #[test]
    fn inline_step_receipt_requires_complete_digest_chain() {
        let mut step = json!({
            "index": 0,
            "name": "gate",
            "generation": 7,
            "request_id": 11,
            "previous_sha256": "0".repeat(64),
            "value": {"value": 42},
        });
        step["entry_sha256"] = json!(sha256_hex(canonical_json(&step).as_bytes()));
        let mut receipt = json!({
            "schema": "zerostack.codemode.step_receipt.v1",
            "generation": 7,
            "request_id": 11,
            "step_count": 1,
            "head_sha256": step["entry_sha256"],
            "steps": [step],
        });
        receipt["receipt_sha256"] = json!(sha256_hex(canonical_json(&receipt).as_bytes()));
        let mut payload = json!({"step_receipt": receipt});
        assert!(canonical_inline_step_receipt(&payload));
        payload["step_receipt"]["steps"][0]["value"]["value"] = json!(43);
        assert!(!canonical_inline_step_receipt(&payload));
    }

    #[test]
    fn mutation_interpret_matrix_pins_user_authorization_policy() {
        // Plan G8 owns user-surface authorization; raw RW8 does not.
        let denied_ok = interpret_mutation_probe("denied", None, Some("policy"), &json!({}));
        assert!(denied_ok.is_empty());
        let denied_wrong =
            interpret_mutation_probe("denied", None, Some("runtime"), &json!({"k": "v"}));
        assert!(!denied_wrong.is_empty());
        let allowed_ok = interpret_mutation_probe("allowed", Some("X0"), None, &json!({}));
        assert!(allowed_ok.is_empty());
    }
}
