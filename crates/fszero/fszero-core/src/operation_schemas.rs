//! Canonical operation input/output JSON Schema ownership (fszero-ncib.1 review).
//!
//! The checked-in document is the only source of schema structure for MCP tools,
//! CodeMode tools, CodeMode methods, and domain operations. Surfaces materialize
//! catalogs from this document; parity checks use exact structural equality
//! (properties, types, requiredness, constraints, output shapes).

use super::filesystem_contract::FILESYSTEM_CONTRACT_VERSION;
use serde_json::{Map, Value, json};

/// Canonical `$schema` URL for materialized tool input schemas.
/// Owned here so core does not import `mcp_rpc` (fszero-ncib.2 dependency rule);
/// MCP surfaces re-export the same constant.
pub const JSON_SCHEMA_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Checked-in canonical schema catalog (digested with the operation ABI).
pub const OPERATION_ABI_SCHEMAS_JSON: &str =
    include_str!("../../../../contracts/fszero/operation-abi-schemas-v1.json");

pub const OPERATION_ABI_SCHEMAS_NAME: &str = "fszero-operation-abi-schemas";
pub const OPERATION_ABI_SCHEMAS_VERSION: &str = "1.0.0";

static SCHEMAS: OnceLock<Value> = OnceLock::new();

/// Parsed canonical schema document.
pub fn operation_abi_schemas_document() -> &'static Value {
    SCHEMAS.get_or_init(|| {
        serde_json::from_str(OPERATION_ABI_SCHEMAS_JSON)
            .expect("checked-in operation-abi-schemas-v1.json must be valid JSON")
    })
}

/// Deterministic SHA-256 of the stable-encoded schema document body.
pub fn operation_abi_schemas_digest() -> String {
    zero_abi::contract_digest_hex(operation_abi_schemas_document())
}

/// Deterministic JSON encoding with sorted object keys. Delegates to the
/// shared zero-abi canonical encoder so contract encoding cannot drift
/// across engines.
pub fn stable_json(value: &Value) -> String {
    zero_abi::canonical_json(value)
}

pub fn hex_encode_pub(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Normalize a JSON Schema object for exact parity comparison.
///
/// - Object keys sorted via `stable_json`
/// - `required` arrays compared as sorted sets (order-independent)
/// - Absent `required` treated as empty array
/// - `$schema` on nested fragments ignored only when comparing property subtrees?
///   No: full structural compare after normalizing required order only.
pub fn normalize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "required" {
                    let mut items: Vec<String> = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    items.sort();
                    out.insert(
                        k.clone(),
                        Value::Array(items.into_iter().map(Value::String).collect()),
                    );
                } else {
                    out.insert(k.clone(), normalize_schema(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_schema).collect()),
        other => other.clone(),
    }
}

/// Exact schema parity: missing/extra props, type changes, requiredness,
/// constraints, and nested shape all fail.
pub fn exact_schema_parity(expected: &Value, live: &Value, ctx: &str) -> Result<(), String> {
    let a = stable_json(&normalize_schema(expected));
    let b = stable_json(&normalize_schema(live));
    if a == b {
        return Ok(());
    }
    Err(format!(
        "schema parity failure at {ctx}: expected={a} live={b}"
    ))
}

fn tool_entry_input(entry: &Value) -> Result<&Value, String> {
    entry
        .get("input")
        .ok_or_else(|| "schema entry missing input".to_string())
}

fn tool_entry_output(entry: &Value) -> Option<&Value> {
    match entry.get("output") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

fn properties_object(schema: &Value) -> Result<&Map<String, Value>, String> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "schema missing properties object".to_string())
}

/// `properties_object` with a contextual error prefix (`codemode method X input: …`).
#[inline]
fn props_ctx(schema: &Value, ctx: impl std::fmt::Display) -> Result<&Map<String, Value>, String> {
    properties_object(schema).map_err(|e| format!("{ctx}: {e}"))
}

#[inline]
fn require_object_type(schema: &Value, ctx: impl std::fmt::Display) -> Result<(), String> {
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        Ok(())
    } else {
        Err(format!("{ctx} type must be object"))
    }
}

/// Full live `inputSchema` object (`$schema` / type / properties / required).
fn tool_input_schema_json(input: &Value) -> Value {
    json!({
        "$schema": JSON_SCHEMA_2020_12,
        "type": input.get("type").cloned().unwrap_or(json!("object")),
        "properties": input.get("properties").cloned().unwrap_or(json!({})),
        "required": input.get("required").cloned().unwrap_or(json!([])),
    })
}

fn materialize_tool_entry(entry: &Value) -> Value {
    let name = entry["name"].as_str().expect("tool name");
    let description = entry["description"].as_str().unwrap_or("");
    let input = tool_entry_input(entry).expect("tool input");
    let _ = properties_object(input).expect("tool properties");
    let mut schema = json!({ "name": name, "description": description, "inputSchema": tool_input_schema_json(input), });
    if let Some(out) = tool_entry_output(entry) {
        schema["outputSchema"] = out.clone();
    }
    schema
}

static MCP_TOOLS: OnceLock<Vec<Value>> = OnceLock::new();
static CODEMODE_TOOLS: OnceLock<Vec<Value>> = OnceLock::new();

/// Materialize MCP tool catalog from the canonical schema document (cached).
fn materialize_tools_from_doc_key(key: &str) -> Vec<Value> {
    let doc = operation_abi_schemas_document();
    let tools = doc[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} array in schema document"));
    tools.iter().map(materialize_tool_entry).collect()
}

pub fn materialize_mcp_tools() -> Vec<Value> {
    MCP_TOOLS
        .get_or_init(|| materialize_tools_from_doc_key("mcp_tools"))
        .clone()
}

/// Materialize CodeMode MCP-facing tool catalog from the schema document (cached).
pub fn materialize_codemode_tools() -> Vec<Value> {
    CODEMODE_TOOLS
        .get_or_init(|| materialize_tools_from_doc_key("codemode_tools"))
        .clone()
}

fn schema_array_entry(doc_key: &str, field: &str, want: &str) -> Option<&'static Value> {
    operation_abi_schemas_document()[doc_key]
        .as_array()?
        .iter()
        .find(|e| e.get(field).and_then(Value::as_str) == Some(want))
}

/// Look up a canonical MCP tool schema entry by name.
pub fn mcp_tool_schema_entry(name: &str) -> Option<&'static Value> {
    schema_array_entry("mcp_tools", "name", name)
}

pub fn codemode_tool_schema_entry(name: &str) -> Option<&'static Value> {
    schema_array_entry("codemode_tools", "name", name)
}

pub fn codemode_method_schema_entry(path: &str) -> Option<&'static Value> {
    schema_array_entry("codemode_methods", "path", path)
}

pub fn domain_operation_schemas(op_id: &str) -> Option<&'static Value> {
    operation_abi_schemas_document()["domain_operations"].get(op_id)
}

/// Expected live tool inputSchema (full object including $schema/type).
fn expected_tool_input_schema(
    label: &str,
    entry: Option<&'static Value>,
    name: &str,
) -> Result<Value, String> {
    let entry = entry.ok_or_else(|| format!("unknown {label} tool {name}"))?;
    Ok(tool_input_schema_json(tool_entry_input(entry)?))
}

fn expected_tool_output_schema(
    label: &str,
    entry: Option<&'static Value>,
    name: &str,
) -> Result<Option<Value>, String> {
    let entry = entry.ok_or_else(|| format!("unknown {label} tool {name}"))?;
    Ok(tool_entry_output(entry).cloned())
}

/// Expected live MCP tool inputSchema (full object including $schema/type).
pub fn expected_mcp_input_schema(name: &str) -> Result<Value, String> {
    expected_tool_input_schema("mcp", mcp_tool_schema_entry(name), name)
}

pub fn expected_mcp_output_schema(name: &str) -> Result<Option<Value>, String> {
    expected_tool_output_schema("mcp", mcp_tool_schema_entry(name), name)
}

pub fn expected_codemode_tool_input_schema(name: &str) -> Result<Value, String> {
    expected_tool_input_schema("codemode", codemode_tool_schema_entry(name), name)
}

pub fn expected_codemode_tool_output_schema(name: &str) -> Result<Option<Value>, String> {
    expected_tool_output_schema("codemode", codemode_tool_schema_entry(name), name)
}

/// Exact parity of one live tool value against the registry.
fn validate_live_tool(tool: &Value, surface: &str) -> Result<(), String> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("live {surface} tool missing name"))?;
    let (expected_in, expected_out, prefix) = match surface {
        "mcp" => (
            expected_mcp_input_schema(name)?,
            expected_mcp_output_schema(name)?,
            format!("mcp.{name}"),
        ),
        "codemode" => (
            expected_codemode_tool_input_schema(name)?,
            expected_codemode_tool_output_schema(name)?,
            format!("codemode_tool.{name}"),
        ),
        other => return Err(format!("unknown live tool surface {other}")),
    };
    let live_in = tool
        .get("inputSchema")
        .ok_or_else(|| format!("live {surface} tool {name} missing inputSchema"))?;
    exact_schema_parity(&expected_in, live_in, &format!("{prefix}.inputSchema"))?;
    match (expected_out, tool.get("outputSchema")) {
        (None, None) | (None, Some(Value::Null)) => Ok(()),
        (Some(exp), Some(live)) => {
            exact_schema_parity(&exp, live, &format!("{prefix}.outputSchema"))
        }
        (Some(_), None) => Err(format!("{prefix}: missing outputSchema")),
        (None, Some(_)) => Err(format!("{prefix}: unexpected outputSchema")),
    }
}

/// Shared set-diff error: `{label} missing=[…] extra=[…]`.
#[inline]
fn set_mismatch<T: Ord + std::fmt::Debug>(
    label: &str,
    expected: &BTreeSet<T>,
    live: &BTreeSet<T>,
) -> String {
    format!(
        "{label} missing={:?} extra={:?}",
        expected.difference(live).collect::<Vec<_>>(),
        live.difference(expected).collect::<Vec<_>>()
    )
}

/// Collect string field `key` from each object entry into a set.
#[inline]
fn json_str_set<'a, I: IntoIterator<Item = &'a Value>>(entries: I, key: &str) -> BTreeSet<String> {
    entries
        .into_iter()
        .filter_map(|e| e.get(key).and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn validate_live_catalog(tools: &[Value], doc_key: &str, surface: &str) -> Result<(), String> {
    let expected_names = json_str_set(
        operation_abi_schemas_document()[doc_key]
            .as_array()
            .ok_or_else(|| format!("{doc_key} missing"))?,
        "name",
    );
    let live_names = json_str_set(tools, "name");
    if expected_names != live_names {
        let label = if surface == "mcp" {
            "mcp catalog name mismatch"
        } else {
            "codemode tool catalog mismatch"
        };
        return Err(set_mismatch(label, &expected_names, &live_names));
    }
    for tool in tools {
        validate_live_tool(tool, surface)?;
    }
    Ok(())
}

/// Exact parity of one live MCP tool value against the registry.
pub fn validate_live_mcp_tool(tool: &Value) -> Result<(), String> {
    validate_live_tool(tool, "mcp")
}
/// Exact parity for the full live MCP catalog vs registry.
pub fn validate_live_mcp_catalog(tools: &[Value]) -> Result<(), String> {
    validate_live_catalog(tools, "mcp_tools", "mcp")
}
pub fn validate_live_codemode_tool(tool: &Value) -> Result<(), String> {
    validate_live_tool(tool, "codemode")
}
pub fn validate_live_codemode_tool_catalog(tools: &[Value]) -> Result<(), String> {
    validate_live_catalog(tools, "codemode_tools", "codemode")
}

/// CodeMode METHODS path set must equal codemode_methods schema entries, and
/// each method must have complete input+output schema structure.

fn codemode_method_path(entry: &Value) -> Result<&str, String> {
    entry
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "codemode method missing path".to_string())
}

pub fn validate_codemode_method_schemas(live_paths: &BTreeSet<String>) -> Result<(), String> {
    let methods = operation_abi_schemas_document()["codemode_methods"]
        .as_array()
        .ok_or_else(|| "codemode_methods missing".to_string())?;
    let expected = json_str_set(methods, "path");
    if &expected != live_paths {
        return Err(set_mismatch(
            "codemode method path mismatch",
            &expected,
            live_paths,
        ));
    }
    for entry in methods {
        let path = codemode_method_path(entry)?;
        let input = tool_entry_input(entry)?;
        let in_ctx = format!("codemode method {path} input");
        props_ctx(input, in_ctx.clone())?;
        require_object_type(input, format!("{in_ctx}."))?;
        let output = tool_entry_output(entry)
            .ok_or_else(|| format!("codemode method {path} missing output schema"))?;
        let out_ctx = format!("codemode method {path} output");
        require_object_type(output, format!("{out_ctx}."))?;
        props_ctx(output, out_ctx)?;
        // signature string must be present for discovery parity.
        if entry.get("signature").and_then(Value::as_str).is_none() {
            return Err(format!("codemode method {path} missing signature"));
        }
    }
    Ok(())
}

/// Validate the schema document itself + binding to registry op ids.
pub fn validate_operation_abi_schemas(registry_ops: &BTreeSet<&str>) -> Result<(), String> {
    let doc = operation_abi_schemas_document();
    let name = doc
        .pointer("/document/name")
        .and_then(Value::as_str)
        .ok_or_else(|| "schemas document.name missing".to_string())?;
    if name != OPERATION_ABI_SCHEMAS_NAME {
        return Err(format!("schemas document.name mismatch: {name}"));
    }
    let version = doc
        .pointer("/document/version")
        .and_then(Value::as_str)
        .ok_or_else(|| "schemas document.version missing".to_string())?;
    if version != OPERATION_ABI_SCHEMAS_VERSION {
        return Err(format!(
            "schemas document.version {version} != {OPERATION_ABI_SCHEMAS_VERSION}"
        ));
    }

    let domain = doc
        .get("domain_operations")
        .and_then(Value::as_object)
        .ok_or_else(|| "domain_operations missing".to_string())?;
    let domain_ids: BTreeSet<&str> = domain.keys().map(String::as_str).collect();
    if domain_ids != *registry_ops {
        return Err(set_mismatch(
            "domain_operations ids mismatch",
            registry_ops,
            &domain_ids,
        ));
    }
    for (op_id, schemas) in domain {
        let input = schemas
            .get("input")
            .ok_or_else(|| format!("domain op {op_id} missing input"))?;
        props_ctx(input, format!("domain {op_id} input"))?;
        let output = schemas
            .get("output")
            .ok_or_else(|| format!("domain op {op_id} missing output"))?;
        // doctor output is still an object schema
        require_object_type(output, format!("domain {op_id} output."))?;
    }

    for surface in ["mcp_tools", "codemode_tools"] {
        let arr = doc
            .get(surface)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{surface} missing"))?;
        let mut names = BTreeSet::new();
        for entry in arr {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{surface} entry missing name"))?;
            if !names.insert(name) {
                return Err(format!("duplicate {surface} name {name}"));
            }
            let input = tool_entry_input(entry)?;
            props_ctx(input, format!("{surface}.{name} input"))?;
            if let Some(op) = entry.get("canonical_op") {
                if !op.is_null() {
                    let op = op
                        .as_str()
                        .ok_or_else(|| format!("{surface}.{name} canonical_op not string"))?;
                    if !registry_ops.contains(op) {
                        return Err(format!(
                            "{surface}.{name} canonical_op {op} not in registry"
                        ));
                    }
                }
            }
            // output optional
            if let Some(out) = tool_entry_output(entry) {
                require_object_type(out, format!("{surface}.{name} output."))?;
            }
        }
    }

    let methods = doc
        .get("codemode_methods")
        .and_then(Value::as_array)
        .ok_or_else(|| "codemode_methods missing".to_string())?;
    let mut paths = BTreeSet::new();
    for entry in methods {
        let path = codemode_method_path(entry)?;
        if !paths.insert(path) {
            return Err(format!("duplicate codemode method {path}"));
        }
        let op = entry
            .get("canonical_op")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("codemode method {path} missing canonical_op"))?;
        if !registry_ops.contains(op) {
            return Err(format!(
                "codemode method {path} canonical_op {op} not in registry"
            ));
        }
        let input = tool_entry_input(entry)?;
        props_ctx(input, format!("method {path} input"))?;
        let output =
            tool_entry_output(entry).ok_or_else(|| format!("method {path} missing output"))?;
        props_ctx(output, format!("method {path} output"))?;
    }

    let d1 = operation_abi_schemas_digest();
    let d2 = operation_abi_schemas_digest();
    if d1 != d2 || d1.len() != 64 {
        return Err("schemas digest unstable".into());
    }
    let _ = FILESYSTEM_CONTRACT_VERSION; // keep coupling visible for future negotiation
    Ok(())
}
