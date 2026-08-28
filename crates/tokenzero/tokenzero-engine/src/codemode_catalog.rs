//! Progressive discovery catalog for CodeMode methods.

use serde::Serialize;
use serde_json::{Value, json};

use crate::codemode_wire::{CodeModeLimits, OperationClass, classify_method};

#[derive(Debug, Clone, Serialize)]
struct MethodDef {
    path: &'static str,
    connector: &'static str,
    description: &'static str,
    signature: &'static str,
}

macro_rules! methods {
    ($( $path:literal, $connector:literal, $description:literal => $signature:literal; )*) => {
        &[ $(MethodDef { path: $path, connector: $connector, description: $description, signature: $signature }),* ]
    };
}

const METHOD_CATALOG: &[MethodDef] = methods! {
    "codemode.search", "codemode", "Search available methods by keyword" =>
        "codemode.search(query: string): Promise<{ results: Array<{ path, description, score }> }>";
    "codemode.describe", "codemode", "Get full TypeScript signature for a method" =>
        "codemode.describe(path: string): Promise<{ path, description, types: string }>";
    "codemode.journalDoctor", "codemode", "List unresolved plan journals and safe recovery advice without deleting evidence" =>
        "codemode.journalDoctor(): Promise<{ schema_version, unresolved, resolved_count, corrupt }>";
    "codemode.journalInspect", "codemode", "Inspect a redacted durable plan journal by execution id" =>
        "codemode.journalInspect(execution_id: string): Promise<PlanJournal>";
    "codemode.journalResume", "codemode", "Validate that an unresolved journal can be safely resumed with the original plan" =>
        "codemode.journalResume(execution_id: string): Promise<{ state, resume }>";
    "codemode.journalRollback", "codemode", "CAS-verified reverse-order rollback of an unresolved plan journal" =>
        "codemode.journalRollback(execution_id: string): Promise<{ state, rolled_back }>";
    "codemode.limits", "codemode", "Return active CodeMode sandbox, output, ref, and operation limits" =>
        "codemode.limits(): Promise<CodeModeLimits>";
};
/// All CodeMode method paths (primary catalog entries, including aliases).
pub fn method_paths() -> Vec<&'static str> {
    METHOD_CATALOG.iter().map(|m| m.path).collect()
}

pub fn search_catalog(query: &str) -> Value {
    let query_lower = query.to_lowercase();
    let mut results: Vec<(f64, &MethodDef)> = METHOD_CATALOG
        .iter()
        .filter_map(|m| {
            let haystack = format!("{} {} {}", m.path, m.description, m.signature).to_lowercase();
            let score = if m.path.to_lowercase().contains(&query_lower) {
                1.0
            } else if m.description.to_lowercase().contains(&query_lower) {
                0.7
            } else if haystack.contains(&query_lower) {
                0.4
            } else {
                return None;
            };
            Some((score, m))
        })
        .collect();
    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    json!({
        "results": results.iter().map(|(score, m)| json!({
            "path": m.path,
            "connector": m.connector,
            "description": m.description,
            "signature": m.signature,
            "example": make_example(m.path),
            "score": score,
        })).collect::<Vec<_>>(),
        "total": results.len(),
        "truncated": false,
        "hint": "Use describe:<path> for full details, or call the method directly in your plan."
    })
}

fn make_example(path: &str) -> &'static str {
    match path {
        "codemode.search" => r#"codemode.search("journal")"#,
        "codemode.describe" => r#"codemode.describe("codemode.limits")"#,
        "codemode.journalDoctor" => r#"codemode.journalDoctor()"#,
        "codemode.journalInspect" => r#"codemode.journalInspect(execution_id)"#,
        "codemode.journalResume" => r#"codemode.journalResume(execution_id)"#,
        "codemode.journalRollback" => r#"codemode.journalRollback(execution_id)"#,
        "codemode.limits" => r#"codemode.limits()"#,
        _ => "(no example available)",
    }
}

fn related_methods(path: &str) -> Vec<&'static str> {
    match path {
        "codemode.search" => vec!["codemode.describe", "codemode.limits"],
        "codemode.describe" => vec!["codemode.search", "codemode.limits"],
        "codemode.journalDoctor" => vec!["codemode.journalInspect", "codemode.journalResume"],
        "codemode.journalInspect" => vec!["codemode.journalDoctor", "codemode.journalResume"],
        "codemode.journalResume" => vec!["codemode.journalInspect", "codemode.journalRollback"],
        "codemode.journalRollback" => vec!["codemode.journalInspect", "codemode.journalDoctor"],
        "codemode.limits" => vec!["codemode.search", "codemode.describe"],
        _ => vec![],
    }
}

pub fn describe_method(path: &str) -> Value {
    let path_lower = path.to_lowercase();
    if let Some(m) = METHOD_CATALOG
        .iter()
        .find(|m| m.path.to_lowercase() == path_lower)
    {
        let mut body = json!({
            "path": m.path,
            "description": m.description,
            "signature": m.signature,
            "example": make_example(m.path),
            "related": related_methods(m.path),
            "kind": "method",
            "operation_class": classify_method(m.path),
            "mutability": if classify_method(m.path) == OperationClass::ReadOnly { "read_only" } else { "mutating" },
            "limits": CodeModeLimits::default().as_json(),
            "safety": {
                "sandbox": "fresh isolated context per execution; no network/env/process/raw-fs/module/timer capabilities"
            }
        });
        // Structural I/O schemas from the operation ABI (tokenzero-irx9.1) so
        // CodeMode describe cannot drift from FastMCP tools/list envelopes.
        if let Some(op) = tokenzero_core::operation_abi::resolve_operation(m.path) {
            let obj = body.as_object_mut().expect("describe object");
            obj.insert("inputSchema".into(), op.args.schema.clone());
            obj.insert("outputSchema".into(), op.results.schema.clone());
            obj.insert("canonical_op".into(), json!(op.name));
        }
        body
    } else {
        json!({
            "path": path,
            "error": format!("no method found for path: {path}"),
            "available": METHOD_CATALOG.iter().map(|m| m.path).collect::<Vec<_>>(),
        })
    }
}

pub fn codemode_method_catalog() -> Value {
    json!({
        "schema_version": "tokenzero.codemode.catalog.v1",
        "methods": METHOD_CATALOG.iter().map(|m| {
            let mut entry = json!({
                "path": m.path,
                "connector": m.connector,
                "description": m.description,
                "signature": m.signature,
            });
            if let Some(op) = tokenzero_core::operation_abi::resolve_operation(m.path) {
                let obj = entry.as_object_mut().expect("method entry");
                obj.insert("inputSchema".into(), op.args.schema.clone());
                obj.insert("outputSchema".into(), op.results.schema.clone());
                obj.insert("canonical_op".into(), json!(op.name));
            }
            entry
        }).collect::<Vec<_>>(),
        "discovery": {
            "owner": "zerostack",
            "search_binding": "codemode.search",
            "describe_binding": "codemode.describe"
        },
        "limits": CodeModeLimits::default().as_json(),
        "aggregate_execution_owner": "zerostack",
        "local_execution": false,
        "worker_transport": "raw-worker-v2",
        "next_actions": [
            "Use the ZeroStack aggregate catalog search to rank bindings by keyword.",
            "Describe a catalog path such as codemode.search before composing a plan.",
            "Execute multi-step plans only through the aggregate host."
        ]
    })
}

