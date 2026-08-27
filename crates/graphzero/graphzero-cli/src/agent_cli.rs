//! Agent-ergonomic surfaces: capabilities, robot-docs, triage mega-command.

use graphzero_store::{graphzero_index_present, resolve_graphzero_store_root};
use serde_json::{Value, json};
use std::path::Path;

fn render_agent_json(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|err| {
        let fallback = json!({
            "schema_version": 1,
            "kind": "agent_json_serialization_error",
            "error": err.to_string()
        });
        serde_json::to_string(&fallback).unwrap_or_else(|_| {
            r#"{"schema_version":1,"kind":"agent_json_serialization_error","error":"serialization failed"}"#.to_string()
        })
    })
}

const KNOWN_ENV_NAMES: &[&str] = &[
    "GRAPHZERO_JSON",
    "GRAPHZERO_TELEMETRY",
    "GRAPHZERO_INCLUDE_GIT_HISTORY",
    "GRAPHZERO_INSTALL_PREFIX",
    "GRAPHZERO_SHARED_STORE",
    "ZEROSTACK_STORE_ROOT",
    "ZEROSTACK_SHARED_STORE",
    "GZ_REPO_ROOT",
    "NO_COLOR",
];

fn env_key_is_typo_candidate(key: &str) -> bool {
    key.starts_with("GRAPHZERO")
        || key.starts_with("GRAPHZRO")
        || key.starts_with("GZ_")
        || key.starts_with("ZEROSTACK_STORE")
        || key.starts_with("ZEROSTACK_SHARED")
        || key.starts_with("NO_COL")
        || key == "NOCOLOR"
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Near-miss GRAPHZERO_*/GZ_*/ZEROSTACK_* env keys (edit distance 1-2). Warn-only.
fn detect_env_typos() -> Vec<Value> {
    let mut out = Vec::new();
    for (key, _) in std::env::vars() {
        if KNOWN_ENV_NAMES.iter().any(|k| *k == key) {
            continue;
        }
        if !env_key_is_typo_candidate(&key) {
            continue;
        }
        let mut best: Option<(&'static str, usize)> = None;
        for &known in KNOWN_ENV_NAMES {
            let d = levenshtein(&key, known);
            if (1..=2).contains(&d) {
                match best {
                    Some((_, bd)) if bd <= d => {}
                    _ => best = Some((known, d)),
                }
            }
        }
        if let Some((did_you_mean, _)) = best {
            out.push(json!({ "got": key, "did_you_mean": did_you_mean }));
        }
    }
    out.sort_by(|a, b| {
        a.get("got")
            .and_then(|v| v.as_str())
            .cmp(&b.get("got").and_then(|v| v.as_str()))
    });
    out
}

/// Machine-readable contract for agents (stdout = JSON only).
pub fn capabilities_json() -> String {
    let is_shim = crate::packaging::is_compatibility_shim_build() || cfg!(feature = "surface-mcp");
    let primary_transport = if is_shim {
        "cli"
    } else if crate::packaging::surface_compiled_in(crate::packaging::PackageSurface::Mcp) {
        "mcp_stdio"
    } else if crate::packaging::surface_compiled_in(crate::packaging::PackageSurface::Codemode) {
        "codemode_stdio"
    } else {
        "cli"
    };
    render_agent_json(json!({
        "schema_version": 1,
        "tool": "graphzero",
        "audience": "ai_agent_only",
        "contract_version": "2026-06-26",
        "primary_transport": primary_transport,
        "package_role": if is_shim { "shim" } else { "package_surface" },
        "is_compatibility_shim_build": is_shim,
        "first_commands": [
            "graphzero",
            "graphzero capabilities",
            "graphzero index .",
            "graphzero doctor --repo .".to_string()
        ],
        "workflow": [
            { "step": 1, "action": "index", "cli": "graphzero index <repo>", "mcp": "tools/call index" },
            { "step": 2, "action": "orient_symbol", "cli": "graphzero orient --surface symbol --name <sym>", "mcp": "tools/call orient surface=symbol name=<sym> budget=1" },
            { "step": 3, "action": "recipe_or_json_plan", "cli": "graphzero code-mode '<recipe-or-json-dag>'", "mcp": "tools/call via graphzero-mcp; JavaScript plans use zerostack-codemode-host or zsx" },
            { "step": 4, "action": "expand_evidence", "cli": "graphzero expand <gz://ref>", "mcp": "tools/call expand reference=<ref>" }
        ],
        "mcp_aliases": {
            "blast_intent": {
                "canonical": "blast",
                "status": "legacy_alias",
                "catalog": false,
                "removal": "major_version_after_clients_migrate"
            }
        },
        "mcp_tools": {
            "mcp": ["orient", "search", "snap", "remember", "recall", "expand", "index", "blast", "reserve", "verify"],
            "mcp_tool_count": 10,
            "codemode": []
        },
        "cli_agent_commands": [
            "agent-triage", "capabilities", "robot-docs", "doctor", "telemetry", "orient", "search", "symbol",
            "index", "snap", "expand", "serve", "code-mode", "code-mode-search", "code-mode-describe"
        ],
        "claims": [
            "no_remaining_callers",
            "no_outgoing_calls",
            "no_remaining_references",
            "no_remaining_dependencies",
            "symbol_removed"
        ],
        "telemetry": {
            "default": "off",
            "env": "GRAPHZERO_TELEMETRY",
            "inspect": "graphzero telemetry inspect [--telemetry|--no-telemetry] --repo .",
            "payload_schema": "usage-telemetry.jsonl",
            "payload_fields": ["execution_path", "raw_tokens", "spent_tokens"],
            "exporter": "none",
            "docs": "docs/telemetry.md"
        },
        "defaults": { "budget": 1, "repo": ".", "ref_scheme": "gz://" },
        "retrieval_tiers": {
            "keyword": {
                "ops": ["search", "orient.search", "orient.locate"],
                "kind": "exact_lexical",
                "returns": "snippet_plus_gz_ref",
                "preindex": "worktree exact match when no snapshot is published"
            },
            "expand": {
                "ops": ["expand"],
                "kind": "exact_bytes",
                "returns": "gz_ref_payload"
            },
            "semantic": {
                "ops": [],
                "kind": "not_shipped",
                "reason": "no embedding authority; do not invent semantic_search"
            },
            "pagination": {
                "cursor": "gz://query/<id>",
                "session_lru": 20,
                "durable": true
            },
            "ranking": {
                "scheme": "frecency_ai",
                "class": "heuristic",
                "half_life_ai": "3d",
                "half_life_human": "10d",
                "modify_buckets_ai": ["30s", "5m", "15m", "1h", "4h"],
                "as_of": "snapshot_timestamp",
                "sidecar": "frecency.json"
            }
        },
        "global_json": "graphzero --json <verb> wraps stdout in {schema_version,data,meta}; also on when GRAPHZERO_JSON=1",
        "stdout_stderr": {
            "stdout": "json_or_exact_bytes_for_expand; success payloads only",
            "stderr": "diagnostics_and_errors; JSON when --json or GRAPHZERO_JSON=1",
            "never": "do_not_print_error_bodies_on_stdout_with_exit_0",
            "clap_usage": "missing/invalid args → stderr (+ JSON in agent mode), exit 2"
        },
        "intent_aliases": {
            "orient": "graphzero orient --surface symbol --name <SYMBOL>",
            "search": "graphzero search --query <TEXT>",
            "snap": "graphzero snap <SYMBOL> --budget 1",
            "codemode": "graphzero code-mode 'callers:<SYMBOL>'",
            "wrong_serach": "graphzero search --query <TEXT>"
        },
        "exit_codes": {
            "0": "success",
            "1": "runtime_or_domain_failure_see_stderr_json",
            "2": "usage_or_package_surface_refuse_see_stderr"
        },
        "exit_code_classes": {
            "success": [0],
            "runtime_or_domain": [1],
            "usage_or_package_refuse": [2]
        },
        "dual_binaries": [
            {
                "name": "graphzero",
                "role": "full_cli_and_compat_shim",
                "install": "cargo build -p graphzero-cli --bin graphzero (or packaging/install.sh shim)",
                "when_to_use": "index, doctor, orient/search/snap/blast, package install/uninstall, MCP serve via shim",
                "not_for": "standalone CodeMode server; use graphzero-mcp for MCP and zerostack-codemode-host or zsx for JavaScript"
            },
            {
                "name": "gzero",
                "role": "direct_recipe_json_cli",
                "install": "cargo build -p graphzero-engine --bin gzero",
                "when_to_use": "direct recipe and JSON-DAG plans: gzero codemode|execute '<plan>'; auto-indexes structural store",
                "not_for": "JavaScript CodeMode plans; use zerostack-codemode-host or zsx"
            },
            {
                "name": "graphzero-mcp",
                "role": "package_surface_fastmcp",
                "install": "packaging/install.sh --surface mcp (or cargo build --features tokenzero,surface-mcp)",
                "when_to_use": "native CodeMode clients that want the lean 10-tool FastMCP catalog",
                "not_for": "CodeMode execute/search/describe catalog (install graphzero-codemode); dual-surface installs"
            },
            {
                "name": "graphzero-codemode",
                "role": "raw_worker",
                "install": "packaging/install.sh --surface codemode (or cargo build -p graphzero-worker --bin graphzero-codemode --no-default-features)",
                "when_to_use": "raw worker protocol only; aggregate CodeMode host owns JavaScript execution",
                "not_for": "standalone CodeMode server or MCP catalog; use graphzero-mcp for MCP"
            }
        ],
        "store_paths": {
            "default_legacy": "<repo>/.graphzero",
            "unified_when_present": "<repo>/.zerostack/graphzero",
            "shared_opt_in": "ZEROSTACK_STORE_ROOT + GRAPHZERO_SHARED_STORE/ZEROSTACK_SHARED_STORE → projects/<key>/graphzero",
            "doctor": "graphzero doctor --repo . | jq .store_resolution"
        },
        "env": {
            "GRAPHZERO_JSON": "CLI JSON mode (1|true): wrap stdout/stderr for agents; also --json",
            "GRAPHZERO_TELEMETRY": "Opt-in shareable telemetry (1|on|true|yes); default off",
            "ZEROSTACK_STORE_ROOT": "Shared/meta store pin; ignored unless shared-store opt-in is set",
            "GRAPHZERO_SHARED_STORE": "Opt-in (1|on|true|yes) to use ZEROSTACK_STORE_ROOT as shared namespaced store",
            "ZEROSTACK_SHARED_STORE": "Alias of GRAPHZERO_SHARED_STORE for shared-store opt-in",
            "GZ_REPO_ROOT": "gzero-only: override repo root for the standalone CodeMode CLI",
            "GRAPHZERO_INCLUDE_GIT_HISTORY": "Indexer: include git history when set truthy",
            "GRAPHZERO_INSTALL_PREFIX": "Install/uninstall state prefix (default ~/.graphzero-install)",
            "NO_COLOR": "Disable ANSI color in clap/help; also honored when CI is set"
        },
        "zeroref": graphzero_store::ZeroRefDescriptor::from_env().to_json()
    }))
}

pub fn robot_docs_guide() -> String {
    include_str!("agent_robot_docs.txt").to_string()
}

/// Structured machine guide (robot-docs --json / agent mode).
pub fn robot_docs_json() -> String {
    render_agent_json(json!({
        "schema_version": 1,
        "kind": "robot_docs",
        "audience": "ai_agent_only",
        "tool": "graphzero",
        "first_commands": [
            "graphzero",
            "graphzero capabilities",
            "graphzero index .",
            "graphzero doctor --repo ."
        ],
        "mcp_tool_count": 10,
        "mcp_tools": ["orient", "search", "snap", "remember", "recall", "expand", "index", "blast", "reserve", "verify"],
        "mcp_aliases": {
            "blast_intent": {
                "canonical": "blast",
                "status": "legacy_alias",
                "removal": "major_version_after_clients_migrate"
            }
        },
        "retrieval_tiers": "keyword=search|locate (index or pre-index worktree); expand=expand gz://; ranking=heuristic frecency_ai; semantic=not_shipped; cursor=gz://query/<id> pages",
        "codemode_tools": [],
        "workflow": [
            "graphzero index .",
            "graphzero orient --surface symbol --name <sym>",
            "graphzero expand <gz://ref>",
            "graphzero code-mode '<recipe-or-json-dag>'"
        ],
        "remember": "graphzero remember --text '...' --anchor <symbol-or-repo-relative-path> (no --path; path anchors use --anchor)",
        "recall": "graphzero recall <TARGET>; TARGET = symbol name | gz://mem/<id> | repo-relative path anchor",
        "dual_binaries": "graphzero capabilities | jq .dual_binaries",
        "store_paths": "graphzero capabilities | jq .store_paths",
        "guide_text": include_str!("agent_robot_docs.txt"),
        "exit_codes": {
            "0": "success",
            "1": "runtime_or_domain_failure_see_stderr_json",
            "2": "usage_or_package_surface_refuse_see_stderr"
        }
    }))
}

pub fn agent_triage_json(repo: &str) -> String {
    let indexed = graphzero_index_present(Path::new(repo));
    let has_codemode =
        crate::packaging::surface_compiled_in(crate::packaging::PackageSurface::Codemode);
    let has_mcp = crate::packaging::surface_compiled_in(crate::packaging::PackageSurface::Mcp);
    let is_shim = crate::packaging::is_compatibility_shim_build() || cfg!(feature = "surface-mcp");

    let mut next = Vec::new();
    if indexed {
        next.push("graphzero orient --surface symbol --name <symbol>".to_string());
        next.push("graphzero search --query <text>".to_string());
        // Prefer gzero / install recovery over dead code-mode* on shim builds.
        if has_codemode {
            next.push("graphzero code-mode 'callers:<symbol>'".to_string());
        } else {
            next.push("gzero codemode 'callers:<symbol>'".to_string());
            next.push(
                "packaging/install.sh --surface codemode  # or use gzero for CodeMode".to_string(),
            );
        }
        if has_mcp && !is_shim {
            next.push("graphzero serve".to_string());
        } else if is_shim {
            next.push(
                "install graphzero-mcp then graphzero serve  # shim cannot host MCP stdio"
                    .to_string(),
            );
        }
    } else {
        next.push(format!("graphzero index {repo}"));
        next.push("graphzero doctor --repo .".to_string());
        next.push("gzero --help".to_string());
    }

    render_agent_json(json!({
        "schema_version": 1,
        "kind": "agent_triage",
        "audience": "ai_agent_only",
        "repo": repo,
        "indexed": indexed,
        "package_gates": {
            "is_compatibility_shim_build": is_shim,
            "surface_codemode_compiled": has_codemode,
            "surface_mcp_compiled": has_mcp
        },
        "next": next,
        "capabilities": "graphzero capabilities",
        "robot_docs": "graphzero robot-docs guide",
        "dual_binaries": "graphzero capabilities | jq .dual_binaries",
        "gzero": "direct recipe and JSON-DAG CLI; JavaScript plans require zerostack-codemode-host or zsx",
        "remember": "budget=1 ref-first; expand gz:// for bytes; stderr has hint+example on errors; store default <repo>/.graphzero (or .zerostack/graphzero when unified root present)"
    }))
}

pub fn doctor_json(cwd: &Path) -> String {
    let graphzero_dir = resolve_graphzero_store_root(cwd);
    let exists = graphzero_dir.is_dir();
    let indexed = graphzero_index_present(cwd);
    let store_resolution = graphzero_store::store_resolution_json(cwd);
    let warnings = store_resolution
        .get("warnings")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    let provenance = graphzero_store::provenance_doctor_report(&graphzero_dir)
        .map(|r| {
            json!({
                "schema_version": r.schema_version,
                "enabled": r.enabled,
                "record_count": r.record_count,
                "orphaned_derivations": r.orphaned_derivations,
            })
        })
        .unwrap_or_else(|err| {
            json!({
                "schema_version": graphzero_store::PROVENANCE_SCHEMA_VERSION,
                "enabled": graphzero_store::provenance_enabled(),
                "record_count": 0,
                "orphaned_derivations": [],
                "error": err.to_string(),
            })
        });
    // Doctor reports install selection when present; shim has no baked surface.
    let package_surface =
        crate::packaging::resolve_startup_surface(&std::env::args().collect::<Vec<_>>())
            .ok()
            .filter(|_| {
                !crate::packaging::is_compatibility_shim_build() && !cfg!(feature = "surface-mcp")
            });
    let package = match package_surface {
        Some(s) => crate::packaging::package_identity(s),
        None => serde_json::json!({
            "artifact": crate::packaging::ARTIFACT_SHIM,
            "surface": "shim",
            "role": "shim",
            "shim": true,
            "is_compatibility_shim_build": crate::packaging::is_compatibility_shim_build(),
            "runtime_dependencies": crate::packaging::runtime_dependency_matrix(),
            "semantic_contract_digest": crate::packaging::semantic_contract_digest(),
            "note": "compatibility shim: install graphzero-mcp or graphzero-codemode for a package surface",
            "primary_transport": "cli",
        }),
    };
    let env_typos = detect_env_typos();
    let mut doctor = json!({
        "schema_version": 1,
        "kind": "doctor",
        "binary": "ok",
        "indexed": indexed,
        "jq_hint": "graphzero doctor --repo . | jq '.'",
        "package": package,
        "graphzero_dir": {
            "path": graphzero_dir.display().to_string(),
            "exists": exists,
            "store_bytes": graphzero_store::store::compaction::store_bytes(&graphzero_dir)
        },
        "store_resolution": store_resolution,
        "warnings": warnings,
        "provenance": provenance,
        "zeroref": graphzero_store::ZeroRefDescriptor::from_env().to_json(),
        "if_not_indexed": format!("graphzero index {}", cwd.display())
    });
    // Optional warn-only field; omit when empty (skip_serialize equivalent).
    if !env_typos.is_empty() {
        if let Some(obj) = doctor.as_object_mut() {
            obj.insert("env_typos".into(), Value::Array(env_typos));
        }
    }
    render_agent_json(doctor)
}
