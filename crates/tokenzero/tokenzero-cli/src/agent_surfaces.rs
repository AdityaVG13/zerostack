use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokenzero_core::operation_abi::all_operations;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CommandSurface {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub category: &'static str,
    pub mutates: bool,
    pub json: bool,
    pub primary_invocation: &'static str,
    pub description: &'static str,
    pub available_in_this_build: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExitCode {
    pub code: i32,
    pub label: &'static str,
    pub meaning: &'static str,
    pub retryable: bool,
}

const fn cmd(
    name: &'static str,
    aliases: &'static [&'static str],
    category: &'static str,
    mutates: bool,
    json: bool,
    primary_invocation: &'static str,
    description: &'static str,
) -> CommandSurface {
    CommandSurface {
        name,
        aliases,
        category,
        mutates,
        json,
        primary_invocation,
        description,
        available_in_this_build: true,
    }
}

const fn cmd_if(
    available: bool,
    name: &'static str,
    aliases: &'static [&'static str],
    category: &'static str,
    mutates: bool,
    json: bool,
    primary_invocation: &'static str,
    description: &'static str,
) -> CommandSurface {
    CommandSurface {
        name,
        aliases,
        category,
        mutates,
        json,
        primary_invocation,
        description,
        available_in_this_build: available,
    }
}

const fn exit(code: i32, label: &'static str, meaning: &'static str, retryable: bool) -> ExitCode {
    ExitCode {
        code,
        label,
        meaning,
        retryable,
    }
}

const COMMANDS: &[CommandSurface] = &[
    cmd(
        "read",
        &["reed"],
        "context",
        false,
        true,
        "tokenzero read <path> --json",
        "Read bounded file content with exact recovery refs.",
    ),
    cmd(
        "find",
        &["search"],
        "context",
        false,
        true,
        "tokenzero find <query> <path> --json",
        "Search local text and return compact matches (literal under both backends).",
    ),
    cmd(
        "grep",
        &[],
        "context",
        false,
        true,
        "tokenzero grep <query> <path> --json",
        "Grep-style search: regex under the ripgrep backend, literal otherwise.",
    ),
    cmd(
        "recall",
        &[],
        "context",
        false,
        true,
        "tokenzero recall <query> --json",
        "Search payloads already stored in the recovery cache.",
    ),
    cmd(
        "fetch",
        &[],
        "context",
        false,
        true,
        "tokenzero fetch <url> --json",
        "Fetch an http(s) URL via curl with a TTL cache and exact refs.",
    ),
    cmd(
        "glob",
        &[],
        "context",
        false,
        true,
        "tokenzero glob '<pattern>' <path> --json",
        "List matching paths without dumping file contents.",
    ),
    cmd(
        "tree",
        &[],
        "context",
        false,
        true,
        "tokenzero tree <path> --json",
        "Inspect a bounded directory tree.",
    ),
    cmd(
        "edit",
        &[],
        "execution",
        true,
        true,
        "tokenzero edit <path> --edits-json '<json>' --json",
        "Apply multi-hunk find/replace edits to one file: all-or-nothing, atomic write, undo ref.",
    ),
    cmd(
        "run",
        &[
            "shell",
            "rn",
            "run <command>",
            "run --json <command>",
            "run <command> --json",
            "--jsno",
            "--jason",
            "--timout",
        ],
        "execution",
        false,
        true,
        "tokenzero run --json -- <command>",
        "Run a command with status-truth telemetry and refs; common JSON/timeout typos and missing -- delimiters are recovered.",
    ),
    cmd(
        "expand",
        &[],
        "recovery",
        false,
        true,
        "tokenzero expand <tz-ref> --raw",
        "Recover exact bytes from a prior TokenZero ref.",
    ),
    cmd(
        "mem",
        &["cache status", "cache statuz"],
        "state",
        false,
        true,
        "tokenzero mem --json",
        "Inspect recovery-cache state.",
    ),
    cmd(
        "pulse",
        &["pulse stats", "pulse status"],
        "state",
        false,
        true,
        "tokenzero pulse --json",
        "Inspect local Pulse telemetry; stats/status recover to the read-only report.",
    ),
    cmd(
        "doctor",
        &["doctor health", "doctor status", "doctor statuz"],
        "health",
        false,
        true,
        "tokenzero doctor --json",
        "Check local TokenZero health and next steps.",
    ),
    cmd(
        "install",
        &["install plan", "install status", "instal"],
        "setup",
        true,
        true,
        "tokenzero install --plan --json",
        "Plan or apply local integration writes with rollback data; --hooks wires the Claude Code PreToolUse hook, --shims installs the universal PATH shims, and install status recovers to clients detect.",
    ),
    cmd(
        "hook claude-code",
        &[],
        "setup",
        false,
        true,
        "tokenzero hook claude-code",
        "Claude Code PreToolUse adapter: reads hook JSON on stdin and rewrites Bash commands under `tokenzero run`; valid pass-through events exit 0, while empty or malformed stdin exits 2 with an exact usage example.",
    ),
    cmd(
        "capabilities",
        &["capability", "capabilites", "--jsno", "--jason"],
        "agent-contract",
        false,
        true,
        "tokenzero capabilities --json",
        "Emit the machine-readable CLI contract for agents.",
    ),
    cmd(
        "ingest",
        &[],
        "context",
        false,
        true,
        "tokenzero ingest <path> --json",
        "Ingest text or a file into a compact capsule with exact refs.",
    ),
    cmd(
        "rewrite",
        &["rewrite-command"],
        "execution",
        false,
        true,
        "tokenzero rewrite --json -- <command>",
        "Plan a TokenZero-safe rewrite of a shell command without executing it.",
    ),
    cmd(
        "discover",
        &[],
        "agent-contract",
        false,
        true,
        "tokenzero discover --json",
        "List local TokenZero tool-discovery metadata.",
    ),
    cmd(
        "stats",
        &[],
        "state",
        false,
        true,
        "tokenzero stats --json --cachezero",
        "Print local TokenZero usage statistics, or CacheZero shadow graduation with --cachezero.",
    ),
    cmd(
        "session-ledger",
        &["ledger"],
        "state",
        false,
        true,
        "tokenzero session-ledger --json",
        "Session cost ledger: token-turns; headline DPMT.",
    ),
    cmd(
        "cache",
        &["cache stats", "cache prune"],
        "state",
        true,
        true,
        "tokenzero cache stats --json",
        "Inspect or prune recovery-cache state; prune mutates.",
    ),
    cmd(
        "clients",
        &["client", "client-status"],
        "setup",
        false,
        true,
        "tokenzero clients detect --json",
        "Inspect AI client TokenZero integration state.",
    ),
    cmd(
        "session-open",
        &[],
        "context",
        false,
        true,
        "tokenzero session-open --json",
        "Open a bounded manifest+delta session.",
    ),
    cmd_if(
        cfg!(feature = "surface-mcp"),
        "mcp-server",
        &[],
        "setup",
        false,
        false,
        "tokenzero mcp-server --mode mcp",
        "Classic MCP stdio adapter. Not compiled in this build unless surface-mcp is live; tokenzero-mcp is not a workspace [[bin]].",
    ),
    cmd(
        "cache-pack",
        &[],
        "agent-contract",
        false,
        true,
        "tokenzero cache-pack --json",
        "Build a daemonless prompt-cache pack with stable prefix and volatile refs.",
    ),
    cmd(
        "quote",
        &[],
        "execution",
        false,
        true,
        "tokenzero quote --platform <os> -- <args>",
        "Quote shell arguments safely for the given platform.",
    ),
    cmd(
        "--robot-triage",
        &["robot-triage", "doctor --robot-triage"],
        "health",
        false,
        true,
        "tokenzero --robot-triage",
        "One-shot health + findings + planned actions + next command in one JSON object.",
    ),
    cmd(
        "robot-docs guide",
        &[
            "robot-doc guide",
            "robotdocs guide",
            "--robot-help",
            "robot-help",
            "robot-docs manual",
            "robot-docs commands",
            "robot-docs examples",
        ],
        "agent-contract",
        false,
        false,
        "tokenzero robot-docs guide",
        "Print a paste-ready agent guide with canonical commands.",
    ),
];

/// 45lv (R-004): eval/audit verbs that exist on the CLI but are not
/// agent-primary. Listed so capabilities neither hides nor promotes them.
/// q41g (SURF-H-002): experimental_commands_policy provides one
/// machine-readable exclusion status and rationale tied to the full list.
const EXPERIMENTAL_SURFACE_RATIONALE: &str = "CLI-only audit, evaluation, and diagnostic commands are excluded from the agent-primary FeatureUniverse because they are not stable agent-contract routes; invoke them directly as CLI verbs.";
const EXPERIMENTAL_COMMANDS: &[&str] = &[
    "bench",
    "mcp-smoke",
    "mcp-soak",
    "exact-recovery-shell",
    "exact-recovery-audit",
    "harm-eval",
    "protected-anchor-audit",
    "false-success-shell",
    "repo-inventory",
    "prompt-cache-pack",
    "install-smoke",
    "package-audit",
    "shell-matrix",
    "os-reach-audit",
    "os-release-artifact",
    "one-shot-eval",
    "source-currency-audit",
    "adapter-approval-audit",
    "adapter-approval-template",
    "claim-audit",
    "completion-audit",
    "security-privacy-audit",
    "artifact-handoff",
    "reach",
    "ws-skeleton",
    "init",
];

const EXIT_CODES: &[ExitCode] = &[
    exit(0, "success", "The requested command completed.", false),
    exit(
        1,
        "blocked",
        "TokenZero refused or could not complete a requested operation; JSON includes a stable error or finding.",
        false,
    ),
    exit(
        2,
        "usage",
        "The CLI invocation was malformed; rerun with the exact command shown in the error or help output.",
        false,
    ),
];

const FEATURES_ALWAYS: &[&str] = &[
    "capabilities_json",
    "exact_recovery_refs",
    "intent_inference_aliases",
    "json_output",
    "non_tty_output_discipline",
    "pipeline_rerun_guidance",
    "robot_docs_guide",
    "status_truth_shell",
];

fn features() -> Vec<&'static str> {
    let mut out = FEATURES_ALWAYS.to_vec();
    out.sort();
    out
}

fn commands_by_name() -> BTreeMap<&'static str, CommandSurface> {
    COMMANDS
        .iter()
        .copied()
        .map(|command| (command.name, command))
        .collect()
}

/// Map a classic MCP tool spelling to the canonical local CLI route.
/// Aggregate CodeMode bindings intentionally have no engine-local CLI route.
pub(crate) fn mcp_name_to_cli_verb(name: &str) -> Option<&'static str> {
    Some(match name {
        "tz_read" => "read",
        "tz_find" => "find",
        "tz_grep" => "grep",
        "tz_glob" => "glob",
        "tz_tree" => "tree",
        "tz_edit" => "edit",
        "tz_recall" => "recall",
        "tz_fetch" => "fetch",
        "tz_shell" => "run",
        "tz_ingest" => "ingest",
        "tz_expand" => "expand",
        "tz_batch" | "tz_execute_code" | "tz_codemode_search" | "tz_codemode_describe" => {
            return None;
        }
        "tz_mem" => "mem",
        "tz_discover" => "discover",
        "tz_rewrite" => "rewrite",
        "tz_cache_pack" => "cache-pack",
        _ => return None,
    })
}

fn mcp_tool_rows() -> Vec<Value> {
    all_operations()
        .iter()
        .filter(|operation| operation.exposure.fastmcp_tool || operation.exposure.codemode_mcp_tool)
        .map(|operation| {
            let mut mcp_surfaces = Vec::new();
            if operation.exposure.fastmcp_tool {
                mcp_surfaces.push("classic");
            }
            if operation.exposure.codemode_mcp_tool {
                mcp_surfaces.push("codemode");
            }
            let cli_verb = mcp_name_to_cli_verb(operation.name);
            let route_relationship = match cli_verb {
                Some(_) => "shared_operation_surface_specific_contract",
                None if operation.exposure.codemode_mcp_tool => "aggregate_control_only",
                None if operation.exposure.codemode_binding.is_some() => "aggregate_binding_only",
                None => "mcp_only",
            };
            let available_in_this_build =
                operation.exposure.fastmcp_tool && cfg!(feature = "surface-mcp");
            json!({
                "mcp_tool": operation.name,
                "mcp_surfaces": mcp_surfaces,
                "available_in_this_build": available_in_this_build,
                "cli_verb": cli_verb,
                "codemode_binding": operation.exposure.codemode_binding,
                "aliases": operation.aliases,
                "route_relationship": route_relationship,
                "schema_relationship": "operation_abi_args_surface_specific_envelopes",
                "behavioral_parity": "not_claimed",
            })
        })
        .collect()
}

pub fn capabilities_json() -> serde_json::Value {
    // n3fx (R-012): every read-side command advertised by capabilities lists
    // its output schema here. Tool-backed verbs share the tokenzero.cli.v1
    // ToolResponse envelope; state/health/plan verbs carry their own version.
    let cli_tool_schema = json!({
        "schema_version": "tokenzero.cli.v1",
        "shape": "tool_response",
        "status_fields": ["ack", "status", "tool", "schema_version", "error", "telemetry", "refs"]
    });
    let mut output_schemas = json!({
        "capabilities": {
            "schema_version": "tokenzero.capabilities.v1",
            "required_keys": [
                "schema_version",
                "tool",
                "version",
                "contract_version",
                "features",
                "feature_flags",
                "commands",
                "commands_by_name",
                "mcp_tools",
                "surface_parity",
                "kernel_orifices",
                "packaging_orifices",
                "exit_codes",
                "env_vars"
            ]
        },
        "run": {
            "schema_version": "tokenzero.cli.v1",
            "shape": "tool_response",
            "status_fields": [
                "status",
                "tool",
                "telemetry.command_success",
                "telemetry.status_label",
                "telemetry.failed_segment",
                "refs"
            ]
        },
        "doctor_robot_triage": {
            "schema_version": "tokenzero.doctor.robot_triage.v1",
            "invocations": [
                "tokenzero --robot-triage",
                "tokenzero robot-triage",
                "tokenzero doctor --robot-triage"
            ],
            "required_keys": [
                "schema_version",
                "status",
                "ok",
                "health",
                "summary",
                "findings",
                "actions_planned",
                "recommendations",
                "recommended_command",
                "quick_ref",
                "commands",
                "mutation_policy"
            ]
        },
        "pulse": {
            "schema_version": "tokenzero.pulse.v1",
            "required_keys": ["schema_version", "status", "event_count", "visible_tokens", "recovery_tokens", "tokenizer_id", "counts_class", "certified", "savings_commensurate"]
        },
        "stats": {
            "schema_version": "tokenzero.pulse.v1",
            "required_keys": ["schema_version", "status", "event_count", "cache_hits", "recovery_blobs", "tokenizer_id", "counts_class", "certified", "savings_commensurate"]
        },
        "doctor": {
            "schema_version": "tokenzero.doctor.v1",
            "required_keys": ["schema_version", "status", "ok", "tool", "summary", "findings", "exit_code"]
        },
        "session-ledger": {
            "schema_version": "session-ledger-v3",
            "required_keys": ["schema_version", "tokenizer_id", "counts_class", "certified", "savings_commensurate", "total_sessions", "total_turns", "total_raw_tokens"]
        },
        "session-open": {
            "schema_version": "tokenzero.session-boot.v1",
            "required_keys": ["schema", "manifest_id", "manifest_path", "delta_path", "delta_ref"]
        },
        "clients": {
            "schema_version": "tokenzero.clients.v1",
            "required_keys": ["schema_version", "command", "status", "agents", "surfaces"]
        },
        "quote": {
            "shape": "quote_result",
            "required_keys": ["platform", "argv", "command"]
        }
    });
    for tool in [
        "read",
        "find",
        "grep",
        "glob",
        "tree",
        "recall",
        "fetch",
        "expand",
        "mem",
        "ingest",
        "discover",
        "cache-pack",
        "rewrite",
    ] {
        output_schemas[tool] = cli_tool_schema.clone();
    }
    json!({
        "schema_version": "tokenzero.capabilities.v1",
        "tool": "tokenzero",
        "version": env!("CARGO_PKG_VERSION"),
        "contract_version": 1,
        "features": features(),
        "stdout_contract": {
            "rule": "stdout is data; stderr is diagnostics",
            "json_flag": "--json",
            "refs_are_recoverable_with": "tokenzero expand <tz-ref> --raw"
        },
        "feature_flags": {
            "json_output": true,
            "exact_recovery_refs": true,
            "status_truth_shell": true,
            "pipeline_rerun_guidance": true,
            "intent_inference_aliases": true,
            "capabilities_json": true,
            "robot_docs_guide": true
        },
        "commands": COMMANDS,
        "commands_by_name": commands_by_name(),
        "mcp_tools": mcp_tool_rows(),
        "surface_parity": {
            "inventory_source": "canonical operation ABI all_operations()",
            "table": "mcp_tools",
            "behavioral_parity": "not_claimed",
            "schema_relationship": "operation_abi_args_surface_specific_envelopes",
            "availability_rule": "available_in_this_build is true only for classic FastMCP tools when the surface-mcp feature is compiled; CodeMode-only control tools stay false on this CLI",
            "name_contract": {
                "mcp": "tz_* tool names",
                "cli": "bare verbs selected by cli_verb",
                "codemode": "codemode.* journal/search/describe/limits control methods; V6 zero.* bindings are retired",
                "kernel": "TokenEngine measure/certify/project/compress/expand via z.measure/z.project/z.compress/z.expand"
            },
            "route_relationships": {
                "shared_operation_surface_specific_contract": "the operation ABI owns classic MCP and aggregate binding argument schemas; CLI spelling, envelopes, and availability remain surface-specific",
                "aggregate_binding_only": "the dotted binding is consumed by the ZeroStack aggregate host and has no engine-local CLI route",
                "aggregate_control_only": "the control schema is aggregate-host metadata and is not registered by TokenZero classic MCP",
                "mcp_only": "no CLI verb is claimed"
            }
        },
        "experimental_commands": EXPERIMENTAL_COMMANDS,
        "experimental_commands_policy": {
            "status": "excluded_with_rationale",
            "rationale": EXPERIMENTAL_SURFACE_RATIONALE,
        },
        "output_schemas": output_schemas,
        "exit_codes": EXIT_CODES,
        "env_vars": [
            {
                "name": "NO_COLOR",
                "effect": "suppress color where supported"
            },
            {
                "name": "CI",
                "effect": "non-interactive output discipline"
            },
            {
                "name": "TOKENZERO_CACHE_PATH",
                "effect": "override recovery cache path when configured by wrappers"
            }
        ],
        "canonical_invocations": [
            "tokenzero capabilities --json",
            "tokenzero --robot-help",
            "tokenzero robot-help",
            "tokenzero robot-docs guide",
            "tokenzero robot-docs commands",
            "tokenzero read <path> --json",
            "tokenzero find <query> <path> --json",
            "tokenzero search <query> <path> --json",
            "tokenzero run --json -- <command>",
            "tokenzero doctor --json",
            "tokenzero doctor status --json",
            "tokenzero pulse stats --json",
            "tokenzero cache statuz --json",
            "tokenzero install plan --json",
            "tokenzero install status --json",
            "tokenzero install --hooks --plan --json",
            "tokenzero install --shims --plan --json",
            "tokenzero hook claude-code"
        ],
        "aggregate_codemode": {
            "owner": "zerostack",
            "local_execution": false,
            "binding_source": "mcp_tools[].codemode_binding",
            "worker_transport": "raw-worker-v2",
            "status": "retired"
        },
        "kernel_orifices": {
            "owner": "tokenzero",
            "api": "zero_abi::TokenEngine",
            "methods": ["measure", "certify", "project", "compress", "expand"],
            "model_facing": ["z.measure", "z.project", "z.compress", "z.expand"],
            "certify": "TokenEngine::certify is the kernel honesty gate, not a model-facing z.* verb",
            "not_token_engine": [
                "z.read",
                "z.find",
                "z.run",
                "zero.read",
                "zero.token.expand"
            ],
            "codemode_binding_status": "retired",
            "note": "TokenZero owns measurement, projection, compression, and expand. z.read/z.find/z.run are ZeroStack host operations that may invoke TokenEngine at response boundaries. V6 dotted zero.* CodeMode bindings are retired, not the canonical kernel API."
        },
        "packaging_orifices": {
            "tokenzero": {
                "bin": true,
                "crate": "tokenzero-cli",
                "path": "crates/tokenzero/tokenzero-cli/src/main.rs"
            },
            "tokenzero-mcp": {
                "bin": false,
                "artifact": "tokenzero-mcp",
                "source_present": "crates/tokenzero/tokenzero-cli/src/bin/tokenzero_mcp.rs",
                "status": "not_a_workspace_bin",
                "available_in_this_build": cfg!(feature = "surface-mcp"),
                "reason": "autobins=false and no [[bin]] tokenzero-mcp; tokenzero_mcp_compat is not a workspace crate"
            },
            "tokenzero-codemode": {
                "bin": false,
                "artifact": "tokenzero-codemode",
                "status": "not_a_workspace_bin",
                "reason": "raw-worker artifact is not built in this workspace; ZeroStack owns aggregate plan execution"
            }
        },
        "dangerous_operations": [
            {
                "command": "edit",
                "safe_default": "tokenzero edit <path> --edits-json '<json>' --dry-run --json",
                "mutation_gate": "omit --dry-run only after reviewing the diff"
            },
            {
                "command": "install",
                "safe_default": "tokenzero install --plan --json",
                "mutation_gate": "--apply"
            },
            {
                "command": "install rollback",
                "safe_default": "tokenzero doctor --json",
                "mutation_gate": "--rollback <id>"
            },
            {
                "command": "cache prune",
                "safe_default": "tokenzero cache prune --json",
                "mutation_gate": "--apply"
            },
            {
                "command": "cache migrate-refs",
                "safe_default": "tokenzero cache migrate-refs --json",
                "mutation_gate": "--apply"
            },
            {
                "command": "cache migrate-rollback",
                "safe_default": "tokenzero cache migrate-rollback --json",
                "mutation_gate": "--apply"
            },
            {
                "command": "cache migrate-cleanup",
                "safe_default": "tokenzero cache migrate-verify --json",
                "mutation_gate": "--apply --confirm-cleanup"
            },
            {
                "command": "clients rollback",
                "safe_default": "tokenzero clients doctor --json",
                "mutation_gate": "clients rollback <id>"
            }
        ],
        "agent_next_steps": [
            "Start with `tokenzero capabilities --json` to discover the contract.",
            "Use `--json` for read/find/tree/run/doctor when composing with jq or another agent.",
            "`tokenzero search <query> <path> --json` is accepted as an agent-friendly alias for `find`.",
            "Use refs from JSON responses with `tokenzero expand <tz-ref> --raw` instead of re-reading broad files.",
            "If you type `tokenzero run true --json` or `tokenzero run --jason true`, TokenZero recovers to `tokenzero run --json -- true`.",
            "If you type `tokenzero rn true --json`, TokenZero recovers to `tokenzero run --json -- true`.",
            "`tokenzero doctor status --json`, `tokenzero pulse stats --json`, `tokenzero cache statuz --json`, and `tokenzero install plan --json` recover to safe read-side or plan surfaces.",
            "`tokenzero install status --json` recovers to `tokenzero clients detect --json`.",
            "Use `tokenzero run --json -- <command>` for command telemetry; inspect `command_success`, not only process exit.",
            "Use the ZeroStack aggregate host for multi-step CodeMode plans; TokenZero exposes dotted aggregate bindings and a planner-free raw-worker v2 artifact."
        ]
    })
}

pub fn robot_docs_guide() -> &'static str {
    r#"# TokenZero Robot Guide

TokenZero is an agent-facing context runtime. Use it when you need bounded file reads, search, trees, command telemetry, and exact recovery refs.

## First Commands

```bash
tokenzero capabilities --json
tokenzero --robot-help
tokenzero robot-help
tokenzero robot-docs guide
tokenzero robot-docs commands
tokenzero doctor --json
tokenzero doctor status --json
tokenzero pulse stats --json
tokenzero install status --json
```

## Context

```bash
tokenzero read <path> --json
tokenzero find <query> <path> --json
tokenzero search <query> <path> --json
tokenzero tree <path> --json
tokenzero expand <tz-ref> --raw
```

Prefer refs from TokenZero responses over broad re-reads. `expand` recovers exact bytes from `tz://...` refs.

## Shell

```bash
tokenzero run --json -- <command>
```

For shell results, inspect `telemetry.command_success`, `telemetry.status_label`, `telemetry.failed_segment`, and `telemetry.pipeline_rerun_command`. Do not infer success from transport exit alone.
Common recoveries: `tokenzero run true --json`, `tokenzero run --json true`, `tokenzero run --jsno true`, `tokenzero run --jason true`, and `tokenzero run --timout 5 true` are normalized to the canonical run shape.
`tokenzero rn true --json` is treated as the common typo for `tokenzero run --json -- true`.
Setup/status recoveries are read-side by default: `tokenzero doctor status --json`, `tokenzero pulse stats --json`, `tokenzero cache statuz --json`, `tokenzero install plan --json`, and `tokenzero install status --json` all avoid unintended writes.

## Output Contract

Stdout is data. Stderr is diagnostics. JSON commands include `schema_version` or `tool`/`status` fields and stable refs when recovery is available.

## Exit Codes

0 means success. 1 means TokenZero blocked or could not complete the operation. 2 means command-line usage error. For command telemetry, inspect the JSON telemetry fields because the wrapper can transport a failed child command successfully.

## Safe Mutation Defaults

`tokenzero install` defaults to a plan. Use `tokenzero install --plan --json` before any `--apply`. `tokenzero cache prune --json` is a dry run unless `--apply` is supplied.

## Kernel, MCP, CLI, and Aggregate Bindings

TokenZero's canonical TokenEngine orifices are `z.measure`, `z.project`, `z.compress`, and `z.expand` (plus `certify` as the honesty gate). Classic MCP uses `tz_*` names and the local CLI uses bare verbs. Classic MCP `codemode_binding` rows are null; V6 `zero.read` / `zero.token.*` aggregate routes are retired. TokenZero does not execute plans locally.

Run `tokenzero capabilities --json` to inspect `kernel_orifices`, classic MCP availability, CLI routes, aggregate bindings, schemas, refs, effects, and output contracts.
"#
}

pub fn robot_docs_commands() -> &'static str {
    r#"# TokenZero Robot Commands

```bash
tokenzero capabilities --json
tokenzero robot-docs guide
tokenzero robot-docs commands
tokenzero read <path> --json
tokenzero find <query> <path> --json
tokenzero search <query> <path> --json
tokenzero tree <path> --json
tokenzero run --json -- <command>
tokenzero doctor --json
tokenzero doctor status --json
tokenzero pulse stats --json
tokenzero install status --json
```

Recoveries: `capability`, `capabilites`, `robot-help`, `--robot-help`, `rn`, `reed`, `instal`, `shell`, `search`, `--jsno`, `--jason`, `--timout`, `cache statuz`, `doctor status`, `doctor statuz`, `pulse stats`, `pulse status`, `install plan`, and `install status` redirect to safe canonical surfaces.
"#
}

pub fn robot_docs_examples() -> &'static str {
    r#"# TokenZero Robot Examples

```bash
tokenzero capabilities --json | jq '.commands'
tokenzero search TokenZero AGENTS.md --json
tokenzero read Cargo.toml --json
tokenzero tree crates/tokenzero --json
tokenzero rn rustc --version --json
tokenzero run --json -- cargo test -p tokenzero
tokenzero doctor status --json
tokenzero pulse stats --json
tokenzero install status --json
```

For `run`, inspect `telemetry.command_success`, `telemetry.failed_segment`, and `telemetry.pipeline_rerun_command`.
"#
}
