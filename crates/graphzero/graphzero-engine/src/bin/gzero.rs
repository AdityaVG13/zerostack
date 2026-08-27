//! gzero CLI (GraphZero) for CodeMode.
//! `gzero codemode '<plan using zero.graph.*>'`
//! Returns codemode ack + gz:// refs + visible_tokens/internal_actions telemetry.
//! Auto-indexes on first use (structural index only; git history tier off unless GRAPHZERO_INCLUDE_GIT_HISTORY=1).

use graphzero_engine::codemode_execute_plan;
use graphzero_store::Snapshot;
use graphzero_store::resolve_graphzero_store_root;
use graphzero_store::store::indexer;
use serde_json::json;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Core graphzero verbs agents often try on the wrong binary (R-014).
const GRAPHZERO_VERBS: &[&str] = &[
    "index",
    "snap",
    "remember",
    "recall",
    "expand",
    "daemon",
    "compact",
    "stats",
    "query-surface",
    "ingest",
    "why",
    "neighborhood",
    "blast",
    "publish",
    "reserve",
    "pack",
    "code-mode",
    "code-mode-search",
    "code-mode-describe",
    "robot-docs",
    "agent-triage",
    "doctor",
    "install",
    "uninstall",
    "sbom",
    "telemetry",
    "orient",
    "symbol",
    "search",
    "verify",
    "scip",
    "declare",
    "check",
    "release",
    "query",
    "status",
    "replay",
    "evidence-check",
    "zeroref-fixture",
];

fn wants_json(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
        || env::var_os("GRAPHZERO_JSON").is_some_and(|v| v != "0" && v != "false")
}

fn strip_json_flag(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| a.as_str() != "--json")
        .cloned()
        .collect()
}

fn triage_json() -> String {
    serde_json::to_string(&json!({
        "schema_version": 1,
        "kind": "gzero_triage",
        "tool": "gzero",
        "version": VERSION,
        "sibling": "graphzero",
        "verbs": ["codemode", "execute"],
        "aliases": { "code-mode": "codemode", "codmode": "codemode" },
        "gzero_only": ["codemode", "execute", "code-mode"],
        "examples": [
            "gzero codemode 'callers:main'",
            "gzero execute 'return await zero.graph.query(\"symbol\", \"main\")'",
            "gzero codemode 'return 1' --json"
        ],
        "exit_codes": {
            "0": "success_or_triage_help",
            "1": "plan_validation_or_runtime_failure",
            "2": "usage_error_or_wrong_binary"
        },
        "env": ["GZ_REPO_ROOT", "GRAPHZERO_INCLUDE_GIT_HISTORY", "GRAPHZERO_JSON"],
        "help": "Standalone CodeMode CLI. Prefer `gzero codemode '<plan>'`. Full GraphZero CLI is `graphzero`."
    }))
    .expect("triage json")
}

fn print_triage() {
    let _ = writeln!(io::stdout(), "{}", triage_json());
}

fn emit_wrong_binary(verb: &str, rest: &[String]) {
    let mut try_args = vec!["graphzero".to_string(), verb.to_string()];
    try_args.extend(rest.iter().cloned());
    let _ = writeln!(
        io::stderr(),
        "{}",
        json!({
            "error": "wrong_binary",
            "got": verb,
            "try": try_args,
            "gzero_only": ["codemode", "execute", "code-mode"],
            "hint": format!("Use `graphzero {verb} …` or `gzero codemode '<plan>'`")
        })
    );
}

fn emit_ack_json(ok: bool, line: &str) {
    let kind = if ok { "ok" } else { "err" };
    let payload = line
        .strip_prefix("ok ")
        .or_else(|| line.strip_prefix("err "))
        .unwrap_or(line);
    let _ = writeln!(
        io::stdout(),
        "{}",
        json!({
            "schema_version": 1,
            "kind": "gzero_ack",
            "ok": ok,
            "status": kind,
            "line": line,
            "payload": payload,
            "tool": "gzero",
            "version": VERSION
        })
    );
}

fn main() {
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let json_mode = wants_json(&raw_args);
    let args = strip_json_flag(&raw_args);

    if args.is_empty()
        || args
            .iter()
            .any(|a| a == "--help" || a == "-h" || a == "help")
    {
        print_triage();
        process::exit(0);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let _ = writeln!(io::stdout(), "gzero {VERSION}");
        process::exit(0);
    }

    let verb = args[0].as_str();
    let verb = match verb {
        "codemode" | "execute" | "code-mode" => verb,
        "codmode" => {
            let _ = writeln!(
                io::stderr(),
                "{}",
                json!({
                    "error": "unknown_verb",
                    "got": "codmode",
                    "hint": "Did you mean `codemode`?",
                    "example": "gzero codemode 'callers:main'"
                })
            );
            process::exit(2);
        }
        other if GRAPHZERO_VERBS.contains(&other) => {
            emit_wrong_binary(other, &args[1..]);
            process::exit(2);
        }
        other => {
            let _ = writeln!(
                io::stderr(),
                "{}",
                json!({
                    "error": "unknown_verb",
                    "got": other,
                    "hint": "Use `gzero codemode '<plan>'` or `gzero --help`",
                    "verbs": ["codemode", "execute"],
                    "sibling": "graphzero"
                })
            );
            process::exit(2);
        }
    };
    if args.len() < 2 {
        let _ = writeln!(
            io::stderr(),
            "{}",
            json!({
                "error": "missing_plan",
                "hint": "Pass a CodeMode plan string",
                "example": "gzero codemode 'callers:main'"
            })
        );
        process::exit(2);
    }

    let plan = args[1..].join(" ");
    let _ = verb; // execute / code-mode are aliases of codemode
    let repo_root = env::var("GZ_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| ".".into()));
    let store_root = resolve_graphzero_store_root(&repo_root);

    let has_index = store_root.join(".manifest").exists();
    if !has_index {
        eprintln!(
            "(GraphZero CodeMode: indexing {} — git-history tier disabled unless GRAPHZERO_INCLUDE_GIT_HISTORY=1)",
            repo_root.display()
        );
        let _ = fs::create_dir_all(&store_root);
        if let Err(e) = indexer::index_repo(&repo_root, &store_root) {
            eprintln!("(GraphZero auto-index failed: {})", e);
            let line = format!(
                "codemode:0 index failed: {} visible_tokens~{} internal_actions=0",
                e,
                (e.to_string().len() / 4).max(1)
            );
            if json_mode {
                emit_ack_json(false, &line);
            } else {
                println!("{line}");
            }
            process::exit(1);
        }
    }

    let snap = Snapshot::open(&store_root, Some(&repo_root));

    match snap {
        Ok(s) => {
            let line = codemode_execute_plan(&s, &plan);
            let ok = line.starts_with("ok ");
            if json_mode {
                emit_ack_json(ok, &line);
            } else {
                println!("{line}");
            }
            process::exit(if ok { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("(GraphZero snapshot open failed: {})", e);
            let line = format!(
                "codemode:0 snapshot: {} visible_tokens~{} internal_actions=0",
                e,
                (e.to_string().len() / 4).max(1)
            );
            if json_mode {
                emit_ack_json(false, &line);
            } else {
                println!("{line}");
            }
            process::exit(1);
        }
    }
}
