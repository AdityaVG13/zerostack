//! Structured JSON errors on stderr for agent-ergonomic CLI failures.

use std::path::Path;

use graphzero_engine::query_surface::{QuerySurfaceError, SURFACE_NAMES};
use graphzero_store::store::expand::json_escape;
use serde_json::{Map, Value, json};

pub fn valid_surfaces_json() -> Value {
    json!(SURFACE_NAMES)
}

pub fn agent_error_json(error: &str, hint: &str, extra: Value) -> String {
    let mut obj = Map::new();
    obj.insert("error".into(), Value::String(error.into()));
    obj.insert("hint".into(), Value::String(hint.into()));
    if let Value::Object(extra) = extra {
        obj.extend(extra);
    }

    serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| {
        format!(
            "{{\"error\":\"{}\",\"hint\":\"{}\"}}",
            json_escape(error),
            json_escape(hint)
        )
    })
}

pub fn query_surface_error_json(err: &QuerySurfaceError, surface: &str, repo: &str) -> String {
    let surfaces = valid_surfaces_json();
    match err {
        QuerySurfaceError::UnknownSurface(bad) => agent_error_json(
            &format!("unknown surface {bad}"),
            "Use a valid query surface or MCP orient with surface=symbol|callers|…; CLI: graphzero query-surface <surface> with required flags",
            json!({
                "valid_surfaces": surfaces,
                "example": format!("graphzero orient --surface symbol --name <SYMBOL> --repo {repo}"),
            }),
        ),
        QuerySurfaceError::MissingArgument(arg) => {
            let (hint, example) = missing_arg_hint(surface, arg, repo);
            agent_error_json(
                &format!("missing argument {arg}"),
                &hint,
                json!({ "surface": surface, "example": example, "valid_surfaces": surfaces }),
            )
        }
        QuerySurfaceError::SymbolNotFound(name) => agent_error_json(
            &format!("SYMBOL_NOT_FOUND: {name}"),
            "Index the repo first, then retry with an exact symbol name",
            json!({
                "example_index": format!("graphzero index {repo}"),
                "example_orient": format!("graphzero orient --surface symbol --name {name} --repo {repo}"),
            }),
        ),
        QuerySurfaceError::EvidenceMissing => agent_error_json(
            "EVIDENCE_MISSING",
            "Run graphzero index on the repo to populate the store, then retry",
            json!({ "example": format!("graphzero index {repo}") }),
        ),
        QuerySurfaceError::MalformedIndex {
            blob_idx,
            blob_hash_count,
        } => agent_error_json(
            &format!(
                "MALFORMED_INDEX: blob_idx {blob_idx} out of range for {blob_hash_count} blob hashes"
            ),
            "The on-disk index is corrupt or truncated; re-run graphzero index to rebuild it, then retry",
            json!({ "example": format!("graphzero index {repo}") }),
        ),
    }
}

fn missing_arg_hint(surface: &str, arg: &str, repo: &str) -> (String, String) {
    match (surface, arg) {
        ("symbol" | "callers" | "deps", "name") => (
            "Pass --name <SYMBOL> (agents often want: graphzero orient --surface symbol --name <SYMBOL>)".into(),
            format!("graphzero orient --surface symbol --name <SYMBOL> --repo {repo}"),
        ),
        ("outline", "path") => (
            "Pass --path <FILE> relative to the repo root".into(),
            format!("graphzero orient --surface outline --path src/lib.rs --repo {repo}"),
        ),
        ("context" | "search" | "word" | "hot" | "changes", "query") => (
            "Pass --query <TEXT>".into(),
            format!("graphzero search --query \"<text>\" --repo {repo}"),
        ),
        _ => (
            format!("Pass --{arg} for surface '{surface}'"),
            format!("graphzero orient --surface {surface} --{arg} <value> --repo {repo}"),
        ),
    }
}

type CliErrorEnrichment = fn(&str, &str) -> Option<String>;

fn orient_help_enrichment(raw: &str, _repo: &str) -> Option<String> {
    (raw.contains("unrecognized subcommand") && raw.contains("'orient'")).then(|| {
        agent_error_json(
            raw,
            "Subcommand help: graphzero orient --help (not graphzero --help orient)",
            json!({ "example": "graphzero orient --help" }),
        )
    })
}

fn expand_ref_enrichment(raw: &str, _repo: &str) -> Option<String> {
    (raw.contains("not a gz:// ref") || raw.contains("not a gz://, g:, or q: ref")).then(|| {
        agent_error_json(
            raw,
            "Expand requires a gz://, g:<loc>, or q:<id> reference from snap, query-surface, or MCP tools",
            json!({ "example": "graphzero expand gz://blob/<64-hex>" }),
        )
    })
}

fn read_path_enrichment(raw: &str, repo: &str) -> Option<String> {
    if !(raw.starts_with("read ") || raw.contains("No such file or directory")) {
        return None;
    }
    let hint = if raw.contains(".scip") {
        "Provide an existing SCIP index path; index the repo with graphzero index if the store is empty"
    } else if raw.contains("os error 2") {
        "Path not found; for ingest scip use a real .scip file, or run graphzero index if the code graph store is empty"
    } else {
        "Check the file path exists and is readable"
    };
    let mut extra = json!({});
    if raw.contains(".scip") || raw.contains("os error 2") {
        extra["example_index"] = json!(format!("graphzero index {repo}"));
    }
    Some(agent_error_json(raw, hint, extra))
}

fn daemon_socket_enrichment(raw: &str, repo: &str) -> Option<String> {
    (raw.contains("bind ") && raw.contains(".sock")).then(|| {
        agent_error_json(
            raw,
            "Another daemon may already be bound; try graphzero daemon status or graphzero daemon disable",
            json!({ "example_status": format!("graphzero daemon status --repo {repo}") }),
        )
    })
}

fn unknown_surface_enrichment(raw: &str, repo: &str) -> Option<String> {
    raw.contains("unknown surface").then(|| {
        agent_error_json(
            raw,
            "Use graphzero orient --surface symbol --name <SYMBOL> for symbol lookup, or query-surface with a valid surface name",
            json!({
                "valid_surfaces": valid_surfaces_json(),
                "example": format!("graphzero orient --surface symbol --name <SYMBOL> --repo {repo}"),
            }),
        )
    })
}

fn missing_argument_enrichment(raw: &str, _repo: &str) -> Option<String> {
    let clap_missing = raw.contains("required arguments were not provided")
        || raw.contains("missing argument")
        || raw.contains("required argument");
    clap_missing.then(|| {
        agent_error_json(
            raw,
            "See graphzero agent-triage or add the flag named in the error",
            json!({
                "valid_surfaces": valid_surfaces_json(),
                "agent_triage": "graphzero agent-triage",
                "example": "graphzero blast --intent <SYMBOL>",
            }),
        )
    })
}

fn codemode_surface_enrichment(raw: &str, _repo: &str) -> Option<String> {
    (raw.contains("surface-codemode") || raw.contains("code-mode requires")).then(|| {
        agent_error_json(
            raw,
            "Install graphzero-codemode package OR use standalone gzero for CodeMode plans",
            json!({
                "error_kind": "missing_codemode_surface",
                "surface": "codemode",
                "try": [
                    "packaging/install.sh --surface codemode",
                    "gzero codemode 'callers:<symbol>'",
                    "graphzero capabilities | jq .dual_binaries"
                ],
                "example": "gzero codemode 'return 1'",
                "install_command": "packaging/install.sh --surface codemode",
            }),
        )
    })
}

fn mcp_surface_enrichment(raw: &str, _repo: &str) -> Option<String> {
    (raw.contains("surface-mcp") || raw.contains("serve requires")).then(|| {
        agent_error_json(
            raw,
            "Install graphzero-mcp package for FastMCP stdio serve",
            json!({
                "error_kind": "missing_mcp_surface",
                "surface": "mcp",
                "try": [
                    "packaging/install.sh --surface mcp",
                    "graphzero capabilities | jq .dual_binaries"
                ],
                "example": "graphzero-mcp  # or graphzero serve after install",
                "install_command": "packaging/install.sh --surface mcp",
            }),
        )
    })
}

fn evidence_missing_enrichment(raw: &str, repo: &str) -> Option<String> {
    raw.contains("EVIDENCE_MISSING").then(|| {
        let indexed = graphzero_store::graphzero_index_present(Path::new(repo));
        if !indexed {
            agent_error_json(
                raw,
                "Store unindexed or unavailable; run index then doctor",
                json!({
                    "error_kind": "store_unavailable",
                    "example_index": format!("graphzero index {repo}"),
                    "example_doctor": "graphzero doctor --repo .",
                }),
            )
        } else {
            agent_error_json(
                raw,
                "Indexed store could not resolve evidence for this query; retry after reindex or try a different needle",
                json!({
                    "error_kind": "evidence_missing",
                    "example_index": format!("graphzero index {repo}"),
                    "example_doctor": "graphzero doctor --repo .",
                    "example_search": "graphzero search --query <TEXT>",
                }),
            )
        }
    })
}

fn missing_snapshot_enrichment(raw: &str, repo: &str) -> Option<String> {
    (raw.contains("No snapshots") || (raw.contains("snapshot") && raw.contains("not found"))).then(
        || {
            agent_error_json(
                raw,
                "No index yet at the resolved store (<repo>/.graphzero or .zerostack/graphzero); run graphzero index on the repository root",
                json!({ "example": format!("graphzero index {repo}") }),
            )
        },
    )
}

const CLI_ERROR_RULES: &[CliErrorEnrichment] = &[
    orient_help_enrichment,
    expand_ref_enrichment,
    read_path_enrichment,
    daemon_socket_enrichment,
    unknown_surface_enrichment,
    missing_argument_enrichment,
    codemode_surface_enrichment,
    mcp_surface_enrichment,
    evidence_missing_enrichment,
    missing_snapshot_enrichment,
];

/// Map common anyhow/display errors to teaching JSON on stderr.
pub fn enrich_cli_error_message(raw: &str, repo: &str) -> String {
    CLI_ERROR_RULES
        .iter()
        .find_map(|rule| rule(raw, repo))
        .unwrap_or_else(|| {
            agent_error_json(raw, "graphzero agent-triage or graphzero --help", json!({}))
        })
}
