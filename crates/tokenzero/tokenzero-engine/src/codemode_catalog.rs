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
    "zero.read", "zero", "Read file(s) with token-budget capsule compression and exact recovery refs" =>
        "zero.read(path: string | string[], opts?: { mode?, start_line?, end_line?, raw?, fresh?, max_files?, max_visible_tokens? }): Promise<{ text: string, ref: string, visible_tokens: number, raw_tokens: number }>";
    "zero.find", "zero", "Literal substring search with compact, recoverable matches (never regex; use zero.grep for regex)" =>
        "zero.find(pattern: string, path?: string | string[], opts?: { mode?, max_files?, max_visible_tokens? }): Promise<{ text: string, ref: string, status: string, visible_tokens?: number, raw_tokens?: number }>";
    "zero.grep", "zero", "Grep-style content search: regex when the ripgrep backend is active, literal substring otherwise (invalid regex fails typed as invalid_pattern)" =>
        "zero.grep(pattern: string, path?: string | string[], opts?: { mode?, max_files?, max_visible_tokens? }): Promise<{ text: string, ref: string, status: string, visible_tokens?: number, raw_tokens?: number }>";
    "zero.glob", "zero", "List file paths matching a glob pattern (no file contents)" =>
        "zero.glob(pattern: string, path?: string | string[], opts?: { max_files? }): Promise<{ text: string, ref: string, status: string, visible_tokens?: number, raw_tokens?: number }>";
    "zero.tree", "zero", "Inspect a bounded directory tree for orientation" =>
        "zero.tree(path?: string, opts?: { depth?, include_hidden?, max_files? }): Promise<{ text: string, ref: string }>";
    "zero.shell", "zero", "Run a shell command with status-truth telemetry and compact output; use mode: exact for contiguous output; background:true returns a durable job handle" =>
        "zero.shell(command: string | string[], opts?: { cwd?, mode?, rewrite?, no_rewrite?, stdin?, timeout_ms?, timeout_seconds?, background? }): Promise<{ text: string, ref: string, exit_code: number, success: boolean } | { job: string, cursor: number, version: number }>;";
    "zero.token.job", "zero.token", "Long-poll a background shell job; returns only bytes after since and a backoff hint when unchanged" =>
        "zero.token.job(id: string, opts?: { waitMs?: number, since?: number, tailBytes?: number }): Promise<{ status: string, cursor: number, version: number, tail?: string, tailBytes?: number, unchanged: boolean, nextPollMs?: number, exitCode?: number }>;";
    "zero.edit", "zero", "Apply multi-hunk find/replace edits to one file atomically" =>
        "zero.edit(path: string, edits: Array<{ find: string, replace: string, replace_all?: boolean }>, opts?: { dry_run?, create? }): Promise<{ text: string, ref: string, hunks_applied: number }>";
    "zero.token.expand", "zero.token", "Recover exact bytes from a tz:// ref" =>
        "zero.token.expand(ref: string | { ref, start_line?, end_line?, selector?, symbol?, since?, fresh? } | Array<string | { ref, ... }>, opts?: { start_line?, end_line?, selector?, symbol?, anchor_kind?, since?, fresh? }): Promise<{ text: string, status: string, ref?: string, visible_tokens?: number, raw_tokens?: number } | { items: Array<{ text: string }>, count: number }>";
    "zero.token.compact", "zero.token", "Store arbitrary text/data behind a tz:// recovery ref via ingest" =>
        "zero.token.compact(data: string): Promise<{ ref: string, raw_tokens: number }>";
    "zero.token.multiCompact", "zero.token", "Batch compact many payloads in one CodeMode step with one visible ack" =>
        "zero.token.multiCompact(items: Array<string | any>): Promise<{ items: Array<{ ref: string }>, refs: string[], count: number }>";
    "zero.token.multiExpand", "zero.token", "Batch expand many tz:// refs in one CodeMode step" =>
        "zero.token.multiExpand(items: Array<string | { ref, start_line?, end_line?, selector?, symbol?, since?, fresh? }>): Promise<{ items: Array<{ text: string }>, count: number }>";
    "zero.token.dedupe", "zero.token", "Deduplicate JSON/string values while preserving first occurrence order" =>
        "zero.token.dedupe(items: any[]): Promise<{ items: any[], count: number }>";
    "zero.expand", "zero", "Recover exact bytes from a tz:// ref (compatibility alias for zero.token.expand)" =>
        "zero.expand(ref: string | { ref, ... } | Array<string | { ref, ... }>, opts?: { start_line?, end_line?, selector? }): Promise<{ text: string, status: string, ref?: string, visible_tokens?: number, raw_tokens?: number } | { items: Array<{ text: string }>, count: number }>";
    "zero.compact", "zero", "Store arbitrary text/data behind a tz:// recovery ref via ingest (compatibility alias for zero.token.compact)" =>
        "zero.compact(data: string): Promise<{ ref: string, raw_tokens: number }>";
    "zero.ingest", "zero", "Ingest text into a compact TokenZero capsule with recovery ref" =>
        "zero.ingest(text: string, opts?: { mode?, source? }): Promise<{ text: string, ref: string, visible_tokens: number, raw_tokens: number }>";
    "zero.mem", "zero", "Inspect recovery-cache state and statistics" =>
        "zero.mem(): Promise<{ text: string }>";
    "zero.recall", "zero", "Search payloads already stored in the recovery cache" =>
        "zero.recall(query: string, opts?: { max_hits?, mode?, max_visible_tokens? }): Promise<{ text: string, ref: string, status: string }>";
    "zero.fetch", "zero", "Fetch an http(s) URL via curl with TTL cache and exact refs" =>
        "zero.fetch(url: string, opts?: { ttl_seconds?, fresh?, mode?, max_visible_tokens? }): Promise<{ text: string, ref: string, status: string }>";
    "zero.cache_pack", "zero", "Build a daemonless prompt-cache pack with stable prefix and volatile refs" =>
        "zero.cache_pack(opts?: { scope? }): Promise<{ text: string, ref: string }>";
    "zero.rewrite", "zero", "Plan a conservative shell command rewrite without executing it" =>
        "zero.rewrite(command: string, opts?: { mode? }): Promise<{ text: string }>";
    "zero.discover", "zero", "Report TokenZero filter and runtime readiness metadata" =>
        "zero.discover(): Promise<{ filters: object, runtime: object }>";
    "zero.batch", "zero", "Run several independent TokenZero ops in one step (max 16)" =>
        "zero.batch(ops: Array<{ tool: string, args: object }>): Promise<{ text: string, refs: string[] }>";
    "zero.pipe", "zero", "Execute a sequence of operations with result threading (_prev auto-binding)" =>
        "zero.pipe(steps: Array<{ method: string, args?: any[] }>): Promise<{ steps: number, results: any[], last: any }>";
    "zero.pick", "zero", "Extract specific keys from an object value" =>
        "zero.pick(source: object, keys: string[] | ...string): Promise<object>";
    "zero.filter_lines", "zero", "Filter lines in a text value by substring match" =>
        "zero.filter_lines(source: { text: string } | string, pattern: string): Promise<{ text: string, lines: number, pattern: string }>";
    "zero.compact_max", "zero", "Max compression with guaranteed byte-exact recovery: content-type-aware aggressive compaction with tz:// ref" =>
        "zero.compact_max(data: string | any): Promise<{ text: string, ref: string, raw_tokens: number, visible_tokens: number, compression_strategy: string, gross_savings_pct: string, savings_scope: string }>";
    "zero.count", "zero", "Count lines in a text value or items in an array without materializing extra payload" =>
        "zero.count(x: string | { text: string } | any[]): number";
    "zero.first", "zero", "Return the first line or array item, or the first n lines/items" =>
        "zero.first(x: string | { text: string } | any[], n?: number): any";
    "zero.verdict", "zero", "Return a compact one-line verdict object" =>
        "zero.verdict(ok: any | (() => any), detail?: string): { ok: boolean, detail: string }";
    "zero.raw", "zero", "Opt one final-return value out of automatic ref-first compaction" =>
        "zero.raw<T>(value: T): T";
    "zero.count_tokens", "zero", "Count tokens, bytes, and lines in a value without storing it (introspection helper)" =>
        "zero.count_tokens(data: string | any): Promise<{ tokens: number, bytes: number, lines: number }>";
    "zero.assert", "zero", "Fail the plan immediately if condition is falsy (plan-level guard)" =>
        "zero.assert(condition: any, message?: string): Promise<{ ok: true }>";
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
        "zero.read" => {
            r#"const f = zero.read("src/main.rs", { raw: true, fresh: true }); return f"#
        }
        "zero.find" => r#"zero.find("TODO", "src/")"#,
        "zero.grep" => r#"zero.grep("fn main", "crates/")"#,
        "zero.glob" => r#"zero.glob("**/*.rs", "crates/")"#,
        "zero.tree" => r#"zero.tree("src", { depth: 2 })"#,
        "zero.shell" => r#"zero.shell("cargo test --quiet", { mode: "exact" })"#,
        "zero.token.job" => {
            r#"zero.token.job(jobId, { waitMs: 30000, since: cursor, tailBytes: 8192 })"#
        }
        "zero.edit" => r#"zero.edit("src/lib.rs", [{ find: "old", replace: "new" }])"#,
        "zero.expand" | "zero.token.expand" => r#"zero.expand("tz://blob/abc123")"#,
        "zero.compact" | "zero.token.compact" => r#"zero.compact(large_output)"#,
        "zero.token.multiCompact" => r#"zero.token.multiCompact([payloadA, payloadB])"#,
        "zero.token.multiExpand" => r#"zero.token.multiExpand([refA, refB])"#,
        "zero.token.dedupe" => r#"zero.token.dedupe([refA, refA, refB])"#,
        "zero.compact_max" => r#"zero.compact_max(large_output)"#,
        "zero.ingest" => r#"zero.ingest("large text payload")"#,
        "zero.pipe" => {
            r#"zero.pipe([{ method: "zero.read", args: ["f.rs"] }, { method: "zero.compact", args: ["_prev.text"] }])"#
        }
        "zero.pick" => r#"const r = zero.read("f.rs"); zero.pick(r, ["text", "ref"])"#,
        "zero.filter_lines" => {
            r#"const r = zero.grep("fn", "src/"); zero.filter_lines(r.text, "pub")"#
        }
        "zero.batch" => {
            r#"zero.batch([{ tool: "read", args: { path: "a.rs" } }, { tool: "read", args: { path: "b.rs" } }])"#
        }
        "zero.recall" => r#"zero.recall("fn main")"#,
        "zero.fetch" => r#"zero.fetch("https://example.com/api")"#,
        "zero.mem" => r#"zero.mem()"#,
        "zero.rewrite" => r#"zero.rewrite("find . -name '*.rs'")"#,
        "zero.discover" => r#"zero.discover()"#,
        "zero.cache_pack" => r#"zero.cache_pack()"#,
        _ => "(no example available)",
    }
}

fn related_methods(path: &str) -> Vec<&'static str> {
    match path {
        "zero.read" => vec!["zero.expand", "zero.find", "zero.compact"],
        "zero.find" | "zero.grep" => vec!["zero.filter_lines", "zero.recall", "zero.read"],
        "zero.glob" => vec!["zero.tree", "zero.read"],
        "zero.tree" => vec!["zero.glob", "zero.read"],
        "zero.shell" => vec!["zero.token.job", "zero.compact", "zero.filter_lines"],
        "zero.token.job" => vec!["zero.shell"],
        "zero.edit" => vec!["zero.read", "zero.find"],
        "zero.expand" | "zero.token.expand" => vec!["zero.compact", "zero.read"],
        "zero.compact" | "zero.token.compact" | "zero.compact_max" => {
            vec!["zero.expand", "zero.ingest", "zero.token.multiCompact"]
        }
        "zero.token.multiCompact" => vec!["zero.token.multiExpand", "zero.token.dedupe"],
        "zero.token.multiExpand" => vec!["zero.token.multiCompact", "zero.expand"],
        "zero.token.dedupe" => vec!["zero.token.multiCompact"],
        "zero.pipe" => vec!["zero.batch", "zero.pick", "zero.filter_lines"],
        "zero.pick" => vec!["zero.pipe", "zero.filter_lines"],
        "zero.filter_lines" => vec!["zero.find", "zero.pick", "zero.pipe"],
        "zero.batch" => vec!["zero.pipe"],
        "zero.recall" => vec!["zero.find", "zero.expand"],
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
            "Describe a dotted binding such as zero.read before composing it.",
            "Execute multi-step plans only through the aggregate host."
        ]
    })
}

