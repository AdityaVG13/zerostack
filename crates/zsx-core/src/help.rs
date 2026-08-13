//! `zero.help.search` — runtime discovery for the aggregate ZSX surface.
//!
//! Modeled on opencode codemode's always-registered `$codemode.search`: a
//! speculative discovery call never fails as unknown, and every result
//! carries the exact call signature so no second lookup is needed. The
//! connector completes help calls synchronously; no engine is dispatched.

use serde_json::{Value, json};

use crate::lower::METHODS;

/// Globals available inside a plan. Everything else is rejected by the
/// bounded interpreter with `unsupported syntax`.
pub const SANDBOX_GLOBALS: &[&str] = &["Promise", "JSON", "Array", "Object", "Map", "Set", "Math"];

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;

struct HelpEntry {
    surface: &'static str,
    method: &'static str,
    signature: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
}

/// One entry per `METHODS` pair; parity is enforced by test.
const HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        surface: "fs",
        method: "plan",
        signature: "zero.fs.plan(goal: string)",
        description: "Turn a free-form goal into a fanned-out repository search",
        keywords: &["goal", "orient", "discover", "overview"],
    },
    HelpEntry {
        surface: "fs",
        method: "structural",
        signature: "zero.fs.structural(query: string, target?: string)",
        description: "Structural code search scoped to an optional target",
        keywords: &["structure", "ast", "pattern"],
    },
    HelpEntry {
        surface: "fs",
        method: "compound",
        signature: "zero.fs.compound(name: \"read\"|\"search\"|\"list\"|\"resolve\"|\"edit\"|\"write\", args: object) — search takes {query, path?}; read/list take {path}; resolve takes {intent, engine, limit}",
        description: "One named filesystem operation with an args object",
        keywords: &["read", "search", "list", "grep", "resolve", "query"],
    },
    HelpEntry {
        surface: "fs",
        method: "world",
        signature: "zero.fs.world(action: string, options?: object)",
        description: "Fork, edit, preview, rebase, and commit overlay worlds",
        keywords: &["world", "overlay", "fork", "commit", "preview"],
    },
    HelpEntry {
        surface: "fs",
        method: "edit",
        signature: "zero.fs.edit({ path, find, replace, start_line?, end_line?, base? }) — base is fz://blob/<sha256> of the expected current content (CAS)",
        description: "Guarded single-file find/replace edit with optional CAS base gate",
        keywords: &["edit", "patch", "replace", "mutate", "cas", "base"],
    },
    HelpEntry {
        surface: "fs",
        method: "write",
        signature: "zero.fs.write({ path, content, base? }) — base: null requires create (must not exist); base: fz://blob/<sha256> is a compare-and-swap overwrite",
        description: "Atomic create or overwrite with optional CAS base gate and bounded diff result",
        keywords: &["write", "create", "overwrite", "put", "cas", "base"],
    },
    HelpEntry {
        surface: "fs",
        method: "transact",
        signature: "zero.fs.transact([{ op: \"edit\"|\"write\", path, find?, replace?, content?, base? }, ...])",
        description: "All-or-nothing multi-step mutation: every CAS gate checked before any apply; journaled rollback on failure",
        keywords: &["transaction", "atomic", "multi", "rollback", "batch", "refactor"],
    },
    HelpEntry {
        surface: "fs",
        method: "read_many",
        signature: "zero.fs.read_many([\"path\" | { path, range?, max_bytes? }, ...]) — positional array, not {paths: [...]}",
        description: "Bulk file reads in one fused kernel pass",
        keywords: &["read", "bulk", "many", "files", "batch"],
    },
    HelpEntry {
        surface: "fs",
        method: "list_many",
        signature: "zero.fs.list_many([\"path\", ...]) — positional array",
        description: "Bulk directory listings",
        keywords: &["list", "ls", "directories", "bulk", "many"],
    },
    HelpEntry {
        surface: "fs",
        method: "search_many",
        signature: "zero.fs.search_many([\"query\", ...]) — positional array",
        description: "Bulk content searches",
        keywords: &["search", "grep", "bulk", "many", "queries"],
    },
    HelpEntry {
        surface: "fs",
        method: "ast_search_many",
        signature: "zero.fs.ast_search_many([{ language, pattern, paths, limit }, ...])",
        description: "Bulk AST pattern searches",
        keywords: &["ast", "structural", "sgrep", "pattern", "many"],
    },
    HelpEntry {
        surface: "graph",
        method: "blast",
        signature: "zero.graph.blast({ intent, budget })",
        description: "Blast-radius view for a change intent under a token budget",
        keywords: &["blast", "impact", "radius", "change"],
    },
    HelpEntry {
        surface: "graph",
        method: "query",
        signature: "zero.graph.query(surface: string, query: string)",
        description: "Query the code graph (for example symbol lookups)",
        keywords: &["symbol", "lookup", "graph"],
    },
    HelpEntry {
        surface: "graph",
        method: "orient",
        signature: "zero.graph.orient(surface: string, query: string)",
        description: "Orient in the graph around a query",
        keywords: &["orient", "context", "overview"],
    },
    HelpEntry {
        surface: "graph",
        method: "recall",
        signature: "zero.graph.recall(query: string)",
        description: "Recall remembered facts and anchors",
        keywords: &["memory", "recall", "facts"],
    },
    HelpEntry {
        surface: "graph",
        method: "verify",
        signature: "zero.graph.verify(target: string, claim: string)",
        description: "Verify a structural claim about a target",
        keywords: &["verify", "claim", "check"],
    },
    HelpEntry {
        surface: "graph",
        method: "snap",
        signature: "zero.graph.snap(query: string, budget: number)",
        description: "Bounded structural snapshot for a query",
        keywords: &["snapshot", "snap", "budget"],
    },
    HelpEntry {
        surface: "graph",
        method: "reserve",
        signature: "zero.graph.reserve(action: string | { action, agent_id?, intent_ops?, ttl_seconds? })",
        description: "List or declare structural reservations",
        keywords: &["reserve", "reservation", "declare", "lock"],
    },
    HelpEntry {
        surface: "graph",
        method: "index",
        signature: "zero.graph.index()",
        description: "Build or refresh the graph index",
        keywords: &["index", "build", "refresh"],
    },
    HelpEntry {
        surface: "graph",
        method: "remember",
        signature: "zero.graph.remember({ text, anchors?, source? })",
        description: "Persist a fact anchored to graph entities",
        keywords: &["memory", "remember", "persist", "fact"],
    },
    HelpEntry {
        surface: "token",
        method: "compact",
        signature: "zero.token.compact(text: string)",
        description: "Compact text into a budget-friendly representation",
        keywords: &["compact", "compress", "tokens"],
    },
    HelpEntry {
        surface: "token",
        method: "expand",
        signature: "zero.token.expand(ref: string)",
        description: "Expand a prior ref back into exact content",
        keywords: &["expand", "ref", "bytes", "blob"],
    },
    HelpEntry {
        surface: "token",
        method: "find",
        signature: "zero.token.find(query: string, path?: string)",
        description: "Token-efficient content find",
        keywords: &["find", "search", "tokens"],
    },
    HelpEntry {
        surface: "token",
        method: "read",
        signature: "zero.token.read(path: string | string[], { mode?, start_line?, end_line?, raw?, fresh?, max_files?, max_visible_tokens? })",
        description: "Token-budgeted exact file read",
        keywords: &["read", "file", "exact", "budget"],
    },
    HelpEntry {
        surface: "token",
        method: "job",
        signature: "zero.token.job({ ... }) — background token job control",
        description: "Poll or control background token jobs",
        keywords: &["job", "background", "poll"],
    },
    HelpEntry {
        surface: "token",
        method: "shell",
        signature: "zero.token.shell(command: string | string[], { cwd?, mode?, timeout_seconds?, timeout_ms?, stdin?, rewrite?, no_rewrite?, background? })",
        description: "Run one shell command with token-budgeted output",
        keywords: &["shell", "command", "exec", "run"],
    },
    HelpEntry {
        surface: "help",
        method: "search",
        signature: "zero.help.search({ query, namespace?, limit?, offset? })",
        description: "Discover surface operations and exact call signatures; empty query browses everything",
        keywords: &["help", "discover", "catalog", "signature", "docs"],
    },
];

fn path_of(entry: &HelpEntry) -> String {
    format!("{}.{}", entry.surface, entry.method)
}

fn singular(term: &str) -> Option<String> {
    term.strip_suffix("es")
        .or_else(|| term.strip_suffix('s'))
        .filter(|stem| stem.len() >= 3)
        .map(str::to_owned)
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .flat_map(|term| {
            // camelCase boundaries split.
            let mut parts = Vec::new();
            let mut current = String::new();
            for ch in term.chars() {
                if ch.is_ascii_uppercase() && !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
                current.push(ch.to_ascii_lowercase());
            }
            if !current.is_empty() {
                parts.push(current);
            }
            parts
        })
        .filter(|term| !term.is_empty() && term != "*")
        .collect()
}

fn term_matches(term: &str, haystack: &str) -> bool {
    haystack.contains(term)
        || singular(term).is_some_and(|stem| haystack.contains(&stem))
}

fn score_entry(entry: &HelpEntry, terms: &[String]) -> u64 {
    let path = path_of(entry).to_ascii_lowercase();
    let segments: Vec<&str> = path.split(['.', '_']).collect();
    let description = entry.description.to_ascii_lowercase();
    let searchable = format!(
        "{} {} {}",
        entry.keywords.join(" "),
        entry.signature.to_ascii_lowercase(),
        entry.method.replace('_', " ")
    );
    let mut score = 0u64;
    for term in terms {
        let exact_segment = segments.iter().any(|segment| {
            *segment == term.as_str()
                || singular(term).is_some_and(|stem| *segment == stem.as_str())
        });
        if path == *term || exact_segment {
            score += 20;
        } else if term_matches(term, &path) {
            score += 8;
        }
        if term_matches(term, &description) {
            score += 4;
        }
        if term_matches(term, &searchable) {
            score += 2;
        }
    }
    score
}

/// Execute one `zero.help.search` call. Never fails: malformed args degrade
/// to an empty-query browse with a note.
pub fn help_search(input: &Value) -> Value {
    // Accept `{...}` or `[{...}]` (positional) or a bare query string.
    let args = match input {
        Value::Array(items) => items.first().cloned().unwrap_or(Value::Null),
        other => other.clone(),
    };
    let (query, namespace, limit, offset) = match &args {
        Value::String(query) => (query.clone(), None, DEFAULT_LIMIT, 0),
        Value::Object(map) => (
            map.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            map.get("namespace")
                .and_then(Value::as_str)
                .map(str::to_owned),
            map.get("limit")
                .and_then(Value::as_u64)
                .map(|limit| (limit as usize).clamp(1, MAX_LIMIT))
                .unwrap_or(DEFAULT_LIMIT),
            map.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
        ),
        _ => (String::new(), None, DEFAULT_LIMIT, 0),
    };
    let entries: Vec<&HelpEntry> = HELP_ENTRIES
        .iter()
        .filter(|entry| {
            namespace
                .as_deref()
                .is_none_or(|namespace| entry.surface == namespace)
        })
        .collect();

    let normalized = query.trim().to_ascii_lowercase();
    // Exact path lookup returns that entry alone.
    let exact: Vec<(&HelpEntry, u64)> = entries
        .iter()
        .filter(|entry| {
            let path = path_of(entry);
            normalized == path
                || normalized == format!("zero.{path}")
                || normalized == format!("zero.{path}(...)")
        })
        .map(|entry| (*entry, 100))
        .collect();

    let mut scored: Vec<(&HelpEntry, u64)> = if !exact.is_empty() {
        exact
    } else if normalized.is_empty() {
        entries.iter().map(|entry| (*entry, 0)).collect()
    } else {
        let terms = tokenize(&normalized);
        entries
            .iter()
            .map(|entry| (*entry, score_entry(entry, &terms)))
            .filter(|(_, score)| *score > 0)
            .collect()
    };
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| path_of(a.0).cmp(&path_of(b.0))));

    let total = scored.len();
    let page: Vec<Value> = scored
        .iter()
        .skip(offset)
        .take(limit)
        .map(|(entry, score)| {
            json!({
                "path": path_of(entry),
                "signature": entry.signature,
                "description": entry.description,
                "score": score,
            })
        })
        .collect();
    let shown = page.len();
    let remaining = total.saturating_sub(offset + shown);
    json!({
        "operation": "help.search",
        "ok": true,
        "total": total,
        "count": shown,
        "remaining": remaining,
        "next": if remaining > 0 { json!({"offset": offset + shown}) } else { Value::Null },
        "sandbox_globals": SANDBOX_GLOBALS,
        "results": page,
    })
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx-core/unit/help.rs"]
mod tests;
