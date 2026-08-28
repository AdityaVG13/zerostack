//! Mutually exclusive package surfaces for FSZero (fszero-ncib.3).
//!
//! Product rule: users install **one** of `fszero-mcp` or `fszero-codemode`
//! from the same revision and shared core. The installer writes one client
//! registration and replaces any prior surface. Dual catalog / dual mode
//! startup fails closed.
//!
//! Process exclusivity: enabling **both** `surface-mcp` and `surface-codemode`
//! is a hard compile failure. One process never contains both surface runtimes.

pub mod completions;
#[cfg(feature = "dev-harness")]
pub mod release_smoke;
pub mod surface_bin;
pub mod waiver;

#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]
compile_error!(
    "fszero-ncib.3 process exclusivity: surface-mcp and surface-codemode cannot both be enabled. \
Build fszero-mcp (--features surface-mcp) OR fszero-codemode (--features surface-codemode), never both. \
The fszero binary is an installer/re-exec shim only (default features, no surface-*)."
);

use crate::core::{
    OPERATION_ABI_NAME, OPERATION_ABI_VERSION, operation_abi_digest, operation_abi_schemas_digest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Package artifact names (release binaries / packages).
pub const ARTIFACT_MCP: &str = "fszero-mcp";
pub const ARTIFACT_CODEMODE: &str = "fszero-codemode";
/// Compatibility shim name — never exposes both surfaces itself.
pub const ARTIFACT_SHIM: &str = "fszero";

/// Install-state filename under the install prefix / config dir.
pub const INSTALL_STATE_FILE: &str = "install-state.json";

/// Client registration file written by the installer (single surface).
pub const CLIENT_CONFIG_FILE: &str = "client-config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageSurface {
    Mcp,
    Codemode,
}

impl PackageSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => crate::core::SelectedSurface::Mcp.as_str(),
            Self::Codemode => crate::core::SelectedSurface::Codemode.as_str(),
        }
    }

    pub fn artifact_name(self) -> &'static str {
        match self {
            Self::Mcp => ARTIFACT_MCP,
            Self::Codemode => ARTIFACT_CODEMODE,
        }
    }

    /// The mutually exclusive counterpart surface.
    pub const fn other(self) -> Self {
        match self {
            Self::Mcp => Self::Codemode,
            Self::Codemode => Self::Mcp,
        }
    }

    /// CLI mode flag refused by this exclusive artifact (`--mode=<other>`).
    pub const fn forbidden_mode_flag(self) -> &'static str {
        match self {
            Self::Mcp => "--mode=codemode",
            Self::Codemode => "--mode=mcp",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        // Artifact names + shared SelectedSurface aliases.
        match s.trim().to_ascii_lowercase().as_str() {
            ARTIFACT_MCP => Ok(Self::Mcp),
            ARTIFACT_CODEMODE => Ok(Self::Codemode),
            other => match crate::core::SelectedSurface::parse(other) {
                Some(crate::core::SelectedSurface::Mcp) => Ok(Self::Mcp),
                Some(crate::core::SelectedSurface::Codemode) => Ok(Self::Codemode),
                None => Err(format!(
                    "unknown package surface {other:?}; require 'mcp' or 'codemode' (artifacts {ARTIFACT_MCP} / {ARTIFACT_CODEMODE})"
                )),
            },
        }
    }

    /// Selection matrix for client install docs (CodeMode-first).
    /// - Clients that speak CodeMode install the canonical `fszero-codemode`.
    /// - Legacy MCP-only clients install compatibility `fszero-mcp`.
    pub fn recommended_for_client(native_codemode_client: bool) -> Self {
        if native_codemode_client {
            Self::Codemode
        } else {
            Self::Mcp
        }
    }
}

impl std::fmt::Display for PackageSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Shared CLI: `--prefix` / `--prefix=` (default install prefix).
/// Scan `args` for `--name value` or `--name=value` (and optional short aliases).
pub fn flag_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        for name in names {
            if a == *name {
                return args.get(i + 1).map(String::as_str);
            }
            if let Some(rest) = a.strip_prefix(name) {
                if let Some(rest) = rest.strip_prefix('=') {
                    if !rest.is_empty() {
                        return Some(rest);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// True if any of `tokens` appears as a whole argv element.
pub fn args_has(args: &[String], tokens: &[&str]) -> bool {
    args.iter().any(|a| tokens.iter().any(|t| a.as_str() == *t))
}

pub fn parse_prefix_flag(args: &[String]) -> PathBuf {
    flag_value(args, &["--prefix"])
        .map(PathBuf::from)
        .unwrap_or_else(default_install_prefix)
}

/// Shared CLI: `--binary` / `--binary=` (fallback to current_exe or `fallback`).
pub fn parse_binary_flag(args: &[String], fallback: &str) -> PathBuf {
    flag_value(args, &["--binary"])
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from(fallback)))
}

/// Shared CLI: `--surface` / `-s` / `--surface=` for the installer shim.
///
/// Also accepts a positional surface immediately after `install`
/// (`fszero install mcp`) so agents are not silently defaulted to codemode
/// (R-020 / fszero-uaak.2).
pub fn parse_surface_flag(args: &[String]) -> Result<Option<PackageSurface>, String> {
    if let Some(v) = flag_value(args, &["--surface", "-s"]) {
        return Ok(Some(PackageSurface::parse(v)?));
    }
    if let Some(pos) = args.iter().position(|a| a == "install") {
        if let Some(next) = args.get(pos + 1).map(String::as_str) {
            if !next.starts_with('-') {
                return match PackageSurface::parse(next) {
                    Ok(surface) => Ok(Some(surface)),
                    Err(_) => Err(format!(
                        "unknown install surface {next:?}; use --surface mcp|codemode \
(example: fszero install --surface mcp)"
                    )),
                };
            }
        }
    }
    Ok(None)
}

/// Opt-in env restoring legacy bare-argv stdio server for the packaging shim
/// (R-004 / fszero-x7n7.2). Dedicated surface binaries (`fszero-mcp` /
/// `fszero-codemode`) remain bare-server by design.
pub const ALLOW_BARE_SERVER_ENV: &str = "FSZERO_ALLOW_BARE_SERVER";

/// True when [`ALLOW_BARE_SERVER_ENV`] is a truthy opt-in (`1`/`on`/`true`/`yes`).
pub fn bare_server_opt_in() -> bool {
    env::var_os(ALLOW_BARE_SERVER_ENV)
        .map(|v| {
            matches!(
                v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
                "1" | "on" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Bare program name only (no subcommand or flags).
pub fn is_bare_invocation(args: &[String]) -> bool {
    args.len() <= 1
}

/// Explicit server start on the packaging shim (`serve` / `--serve` / `--mode=`).
pub fn explicit_server_request(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "serve" || a == "--serve" || a.starts_with("--mode="))
}

/// Whether the packaging shim should re-exec the selected surface as a server.
///
/// Bare `fszero` no longer starts a stdio server (agents hang forever). Use
/// `fszero serve`, `fszero --mode=…`, or set [`ALLOW_BARE_SERVER_ENV`].
pub fn shim_should_start_server(args: &[String]) -> bool {
    explicit_server_request(args) || (is_bare_invocation(args) && bare_server_opt_in())
}

/// Shim / surface verbs agents commonly type (for did-you-mean).
pub use completions::{COMPLETION_SHELLS, completion_script};

pub const SHIM_COMMANDS: &[&str] = &[
    "help",
    "install",
    "uninstall",
    "sbom",
    "doctor",
    "serve",
    "batch",
    "migrate-cas",
    "store-gc",
    "telemetry",
    "zeroref-fixture",
    "capabilities",
    "catalog",
    "tools",
    "layout",
    "robot-triage",
    "robot-docs",
    "completions",
];

/// Common flags for nearest-name suggestions.
pub const COMMON_FLAGS: &[&str] = &[
    "--help",
    "--json",
    "--root",
    "--prefix",
    "--surface",
    "--binary",
    "--dry-run",
    "--yes",
    "--mode=mcp",
    "--mode=codemode",
    "--serve",
    "--supervise",
    "--raw-worker",
    "--telemetry",
    "--no-telemetry",
];

/// True when `a` and `b` differ by at most one insertion, deletion, substitution,
/// or adjacent transposition (ASCII Damerau–Levenshtein distance ≤ 1).
/// Case-insensitive for verb matching.
pub fn edit_distance_at_most_one(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    if la == lb {
        let mut diffs = 0usize;
        let mut i = 0usize;
        while i < la {
            if a[i] == b[i] {
                i += 1;
                continue;
            }
            // Adjacent transposition counts as one edit.
            if i + 1 < la && a[i] == b[i + 1] && a[i + 1] == b[i] {
                diffs += 1;
                if diffs > 1 {
                    return false;
                }
                i += 2;
                continue;
            }
            diffs += 1;
            if diffs > 1 {
                return false;
            }
            i += 1;
        }
        return true;
    }
    let (short, long) = if la < lb { (a, b) } else { (b, a) };
    let mut i = 0usize;
    let mut j = 0usize;
    let mut skipped = false;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
        } else if !skipped {
            skipped = true;
            j += 1;
        } else {
            return false;
        }
    }
    true
}

/// Nearest catalog names within edit distance 1 (lowercased compare).
pub fn nearest_names<'a>(input: &str, candidates: &[&'a str], limit: usize) -> Vec<&'a str> {
    let needle = input.to_ascii_lowercase();
    let mut out = Vec::new();
    for &cand in candidates {
        if edit_distance_at_most_one(&needle, &cand.to_ascii_lowercase()) {
            if cand != input && !out.contains(&cand) {
                out.push(cand);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    out
}

/// Format a did-you-mean suffix, or empty string when nothing is close.
pub fn did_you_mean_suffix(input: &str, candidates: &[&str]) -> String {
    let hits = nearest_names(input, candidates, 3);
    if hits.is_empty() {
        return String::new();
    }
    format!("\ndid you mean: {}?", hits.join(", "))
}

/// Machine-readable capabilities document (R-001 / fszero-8n7a.1).

pub fn robot_triage_document(surface: Option<PackageSurface>) -> serde_json::Value {
    let doc = capabilities_document(surface);
    let root = std::env::var("FSZERO_ROOT").ok();
    serde_json::json!({
        "schema": "fszero.robot-triage/v1",
        "quick_ref": {
            "budgets": doc["budgets"].clone(),
            "commands": SHIM_COMMANDS,
            "exit_codes": doc["exit_codes"].clone(),
            "exit_code_policy": doc["exit_code_policy"].clone(),
            "env_vars": doc["env_vars"].clone(),
        },
        "health": {
            "root_env_set": root.is_some(),
            "root": root,
        },
        "next_commands": [
            "fszero doctor --json",
            "fszero capabilities",
            "fszero batch plan.json",
        ],
    })
}

pub fn capabilities_document(surface: Option<PackageSurface>) -> serde_json::Value {
    let identity = surface.map(package_identity);
    serde_json::json!({
        "schema": "fszero.capabilities/v1",
        "ok": true,
        "package_version": env!("CARGO_PKG_VERSION"),
        "semantic_contract_digest": semantic_contract_digest(),
        "surface": surface.map(|s| s.as_str()),
        "artifact": surface.map(|s| s.artifact_name()),
        "package": identity,
        "capability_concepts": {
            "cli": {
                "owner": "FSZero standalone operator CLI",
                "scope": "commands, environment, store health, refs, and exit codes",
                "schema": "fszero.capabilities/v1",
                "retrieve": "fszero capabilities --json",
                "value_pointer": "/"
            },
            "zerokernel": {
                "owner": "ZeroStack",
                "scope": "model-facing read, edit, and atomic effect orchestration",
                "operations": ["z.read", "z.edit", "z.apply"],
                "docs": ["README.md", "docs/architecture.md", "docs/install.md"]
            },
            "zeroref_store": {
                "owner": "the current FSZero recovery store",
                "scope": "ZeroRef syntax, fragments, store attachment, and live recovery state",
                "contract": "zeroref",
                "store_key": crate::core::CAPABILITY_STORE_KEY
            },
            "copy_rule": "Use the CLI document for operator discovery and ZeroKernel for model-facing work. Retired engine catalogs are not interchangeable aliases."
        },
        "commands": SHIM_COMMANDS,
        "exit_codes": {
            "0": "success",
            "1": "runtime / operational failure (die1)",
            "2": "usage / contract / argv error (die2)",
            "zeroref_fixture": crate::core::zeroref_fixture::exit_code_dictionary()
        },
        "exit_code_policy": {
            "surface_runtime": "exit 1 for server, raw-worker, and other operational failures",
            "usage_contract_argv": "exit 2 for invalid flags, modes, and argument or contract syntax",
            "explicit_root_validation": "exit 2 when explicit-root validation fails before operation dispatch",
            "supervise_child_1_to_255": "propagate the child exit unchanged",
            "supervise_zero_or_missing": "exit 1 (missing includes launch failure or signal without an exit code)",
            "supervise_out_of_range": "remap with c.rem_euclid(256), promoting zero to 1"
        },
        "env_vars": {
            "FSZERO_ROOT": "workspace root override (explicit --root takes precedence; $HOME refused)",
            "FSZERO_INSTALL_PREFIX": "default standalone CLI install prefix",
            "FSZERO_SHARED_STORE": "opt in to ZEROSTACK_STORE_ROOT",
            "ZEROSTACK_STORE_ROOT": "shared durable store pin (requires shared-store opt-in)",
            "ZEROSTACK_SHARED_STORE": "hub canonical shared-store opt-in",
            "FSZERO_TELEMETRY": "default-off local usage accounting",
            "FSZERO_STARTUP_INDEX": "set to 1 to build the standalone search index at session start",
            "FSZERO_QUARANTINE_MAX_BYTES": "max total bytes under the store quarantine before prune",
            "FSZERO_QUARANTINE_MAX_AGE_DAYS": "max age in days for quarantine entries before prune",
            "FSZERO_CAS_GC_GRACE_SECS": "grace period before unreferenced blobs are eligible for GC",
            "FSZERO_INDEX_LOCK": "set to 0 to opt out of the cross-process single-indexer lock",
            "FSZERO_INDEX_LOCK_WAIT_MS": "bounded wait for the index build lock",
            "NO_COLOR": "disable ANSI color",
            "CI": "non-interactive, color-off execution",
            "TERM": "TERM=dumb disables color"
        },
        "output_discipline": {
            "schema": "fszero.output_discipline/v1",
            "ansi_color": "off unless stdout is a TTY and NO_COLOR/CI/TERM=dumb are unset",
            "interactive_prompts": "never; mutations require explicit arguments",
            "json_paths": "doctor --json, capabilities --json, robot-triage, batch, and sbom write JSON only to stdout"
        },
        "model_surface": {
            "owner": "ZeroStack",
            "operations": ["z.read", "z.edit", "z.apply"],
            "engine_catalog_registered": false
        },
        "budgets": [
            {"stage": "zerokernel_frame", "owner": "ZeroStack", "note": "frame deadline and cancellation are host-owned"},
            {"stage": "fszero_operation", "owner": "FSZero", "note": "byte, file-count, and store limits remain engine-owned"}
        ]
    })
}

/// R-IDEA-005 / fszero-l0hb.1: inventory of store roots agents need for layout.
///
/// Lists workspace + store paths and simple exists/health flags. Paths are
/// absolute; no home-directory expansion of secrets. Does not create dirs.
pub fn layout_document(workspace_root: &std::path::Path) -> serde_json::Value {
    use crate::{fszero_store_sqlite_path, zerostack_store_or_detect};

    let abs = |p: &std::path::Path| -> String {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .into_owned()
    };
    let exists = |p: &std::path::Path| p.exists();

    let legacy_fszero = workspace_root.join(".fszero");
    let local_zerostack = workspace_root.join(".zerostack");
    let unified = zerostack_store_or_detect(workspace_root);
    let active_store = unified.clone().unwrap_or_else(|| legacy_fszero.clone());
    let config_json = active_store.join("config.json");
    let blobs = active_store.join("blobs");
    let sqlite = fszero_store_sqlite_path(workspace_root);
    let quarantine = active_store.join("fszero").join("quarantine");
    let pin = ["ZEROSTACK_STORE_ROOT", "ZERO_STACK_STORE_ROOT"]
        .iter()
        .find_map(|k| std::env::var_os(k).map(|v| (*k, PathBuf::from(v))));
    let shared_opt_in = std::env::var_os("FSZERO_SHARED_STORE").is_some()
        || std::env::var_os("ZEROSTACK_SHARED_STORE").is_some();

    serde_json::json!({
        "schema": "fszero.layout/v1",
        "workspace_root": abs(workspace_root),
        "paths": {
            "workspace_root": abs(workspace_root),
            "legacy_fszero": abs(&legacy_fszero),
            "local_zerostack": abs(&local_zerostack),
            "active_store_root": abs(&active_store),
            "config_json": abs(&config_json),
            "cas_blobs": abs(&blobs),
            "fszero_sqlite": abs(&sqlite),
            "quarantine": abs(&quarantine),
        },
        "health": {
            "workspace_root_exists": exists(workspace_root),
            "legacy_fszero_exists": exists(&legacy_fszero),
            "local_zerostack_exists": exists(&local_zerostack),
            "active_store_exists": exists(&active_store),
            "config_json_exists": exists(&config_json),
            "cas_blobs_exists": exists(&blobs),
            "fszero_sqlite_exists": exists(&sqlite),
            "quarantine_exists": exists(&quarantine),
            "unified_store_selected": unified.is_some(),
        },
        "env": {
            "FSZERO_ROOT_set": std::env::var_os("FSZERO_ROOT").is_some(),
            "shared_store_opt_in": shared_opt_in,
            "store_root_pin": pin.as_ref().map(|(k, p)| serde_json::json!({
                "env": k,
                "value": p.to_string_lossy(),
            })),
        },
        "note": "layout is read-only discovery; it does not create stores or print secrets",
    })
}

/// Print install success line (identical across surface artifacts and shim).
pub fn print_install_ok(state: &InstallState) {
    println!(
        "install: ok surface={} artifact={} prefix={} semantic_contract_digest={} client_config={}",
        state.surface.as_str(),
        state.artifact,
        state.prefix,
        state.semantic_contract_digest,
        state.client_config
    );
}

/// R-011 / fszero-8bu7.6: whether packaging CLI may emit ANSI color.
///
/// Always false when `NO_COLOR` is set (any value), `CI` is set, `TERM=dumb`,
/// or stdout is not a TTY. Packaging paths currently emit plain text only;
/// this gate exists so future color stays agent-safe by default.
pub fn cli_color_enabled() -> bool {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CI").is_some() {
        return false;
    }
    if std::env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// True when packaging CLI must avoid interactive prompts (non-TTY / CI / dumb TERM).
pub fn cli_noninteractive() -> bool {
    use std::io::IsTerminal;
    if std::env::var_os("CI").is_some() {
        return true;
    }
    if std::env::var("TERM").ok().as_deref() == Some("dumb") {
        return true;
    }
    !std::io::stdin().is_terminal()
}

/// Scan text for ESC/CSI sequences used by ANSI color (agent non-TTY safety).
pub fn contains_ansi_escape(s: &str) -> bool {
    s.as_bytes().contains(&0x1b)
}

/// Fatal CLI exit with message on stderr (exit code 1).
///
/// BrokenPipe on stderr is ignored so `… | head` does not panic agents (R-IDEA-008).
pub fn die1(msg: impl std::fmt::Display) -> ! {
    write_stderr_line(&format!("{msg}"));
    std::process::exit(1);
}

/// Fatal CLI exit with message on stderr (exit code 2).
pub fn die2(msg: impl std::fmt::Display) -> ! {
    write_stderr_line(&format!("{msg}"));
    std::process::exit(2);
}

fn write_stderr_line(msg: &str) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    match writeln!(err, "{msg}") {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(_) => {}
    }
}

/// True when a panic payload is stdout/stderr BrokenPipe (R-IDEA-008).
pub fn panic_payload_is_broken_pipe(payload: &(dyn std::any::Any + Send)) -> bool {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return s.contains("Broken pipe") || s.contains("BrokenPipe");
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.contains("Broken pipe") || s.contains("BrokenPipe");
    }
    false
}

/// Ignore SIGPIPE and convert println! BrokenPipe panics to exit 0.
///
/// Call once at packaging CLI process start (surface bins + shim). Piped
/// consumers like `fszero capabilities --json | head` must not kill agents.
pub fn install_stdout_pipe_safety() {
    #[cfg(unix)]
    {
        // SIGPIPE=13, SIG_IGN=1 on Linux/macOS. Avoids exit 141 process death.
        unsafe extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        const SIGPIPE: i32 = 13;
        const SIG_IGN: usize = 1;
        // SAFETY: installing SIG_IGN for SIGPIPE is process-global and standard for CLIs.
        unsafe {
            signal(SIGPIPE, SIG_IGN);
        }
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if panic_payload_is_broken_pipe(info.payload()) {
            std::process::exit(0);
        }
        previous(info);
    }));
}

fn exit_fail(label: &str, e: impl std::fmt::Display) -> ! {
    die1(format!("{label}: FAIL {e}"))
}

/// Exit the process after install (shared by shim + exclusive surface bins).
pub fn exit_install_result(result: Result<InstallState, String>) -> ! {
    match result {
        Ok(state) => {
            print_install_ok(&state);
            std::process::exit(0);
        }
        Err(e) => exit_fail("install", e),
    }
}

/// Exit the process after uninstall (shared by shim + exclusive surface bins).
pub fn exit_uninstall_result(result: Result<Option<InstallState>, String>) -> ! {
    match result {
        Ok(prev) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&uninstall_report(prev)).unwrap()
            );
            std::process::exit(0);
        }
        Err(e) => exit_fail("uninstall", e),
    }
}

/// Machine-readable plan schema for mutating packaging commands.
pub const MUTATION_PLAN_SCHEMA: &str = "fszero-mutation-plan/v1";

/// Pure install plan. Constructing it performs no filesystem writes.
pub fn install_dry_run_document(
    surface: PackageSurface,
    prefix: &Path,
    binary_path: &Path,
) -> serde_json::Value {
    serde_json::json!({
        "schema": MUTATION_PLAN_SCHEMA, "action": "install", "dry_run": true,
        "would_mutate": true, "requires_yes": false, "surface": surface.as_str(),
        "artifact": surface.artifact_name(), "prefix": prefix.display().to_string(),
        "binary": binary_path.display().to_string(),
        "targets": [INSTALL_STATE_FILE, CLIENT_CONFIG_FILE, "shim-target"],
    })
}

/// Read-only uninstall plan. The current state and target existence are
/// inspected, but no target is removed.
pub fn uninstall_dry_run_document(prefix: &Path) -> Result<serde_json::Value, String> {
    let state = load_install_state(prefix)?;
    let targets = [
        (INSTALL_STATE_FILE, install_state_path(prefix)),
        (CLIENT_CONFIG_FILE, prefix.join(CLIENT_CONFIG_FILE)),
        ("shim-target", prefix.join("shim-target")),
    ];
    let would_mutate = targets.iter().any(|(_, path)| path.exists());
    let target_rows = targets
        .iter()
        .map(|(name, path)| serde_json::json!({"name": name, "exists": path.exists()}))
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema": MUTATION_PLAN_SCHEMA, "action": "uninstall", "dry_run": true,
        "would_mutate": would_mutate, "requires_yes": true,
        "prefix": prefix.display().to_string(),
        "current_install": state.as_ref().map(|installed| serde_json::json!({
            "surface": installed.surface.as_str(), "artifact": installed.artifact.as_str(),
            "semantic_contract_digest": installed.semantic_contract_digest.as_str(),
        })),
        "targets": target_rows,
    }))
}

pub fn mutation_confirmation_error(action: &str) -> String {
    format!(
        "fszero {action}: confirmation required; rerun with --yes, or inspect without mutation using --dry-run"
    )
}

/// Parse safety flags and uninstall (shared by shim + exclusive surface bins).
pub fn exit_uninstall_from_args(args: &[String]) -> ! {
    let prefix = parse_prefix_flag(args);
    if args_has(args, &["--dry-run"]) {
        match uninstall_dry_run_document(&prefix) {
            Ok(plan) => {
                println!("{}", serde_json::to_string_pretty(&plan).unwrap());
                std::process::exit(0);
            }
            Err(error) => exit_fail("uninstall --dry-run", error),
        }
    }
    if !args_has(args, &["--yes"]) {
        die2(mutation_confirmation_error("uninstall"));
    }
    exit_uninstall_result(uninstall_surface(&prefix))
}

/// Compile-time surfaces enabled in this binary (feature matrix).
///
/// At most one entry. Empty = packaging shim / shared core without a server surface.
pub fn compile_time_surfaces() -> Vec<PackageSurface> {
    #[allow(unused_mut)] // mut only needed when a surface feature is enabled
    let mut out = Vec::new();
    #[cfg(feature = "surface-mcp")]
    out.push(PackageSurface::Mcp);
    #[cfg(feature = "surface-codemode")]
    out.push(PackageSurface::Codemode);
    // Dual features cannot reach runtime: compile_error! above.
    debug_assert!(
        out.len() <= 1,
        "process exclusivity violated: more than one surface feature compiled in"
    );
    out
}

/// Immutable package surface for this **artifact** when built as a single-surface
/// release binary (`fszero-mcp` / `fszero-codemode`). None means the compatibility
/// shim (no surface runtime compiled in) — selection is install-state / re-exec only.
pub fn baked_package_surface() -> Option<PackageSurface> {
    let surfaces = compile_time_surfaces();
    if surfaces.len() == 1 {
        return Some(surfaces[0]);
    }
    // Symlink installs: process name is fszero-mcp / fszero-codemode.
    if let Ok(name) = env::current_exe() {
        if let Some(stem) = name.file_name().and_then(|s| s.to_str()) {
            if stem == ARTIFACT_MCP || stem.starts_with("fszero-mcp") {
                return Some(PackageSurface::Mcp);
            }
            if stem == ARTIFACT_CODEMODE || stem.starts_with("fszero-codemode") {
                return Some(PackageSurface::Codemode);
            }
        }
    }
    if let Ok(v) = env::var("FSZERO_PACKAGE_SURFACE") {
        if let Ok(s) = PackageSurface::parse(&v) {
            return Some(s);
        }
    }
    None
}

/// Combined semantic contract digest advertised by help/doctor/SBOM/uninstall.
/// Print package version + semantic_contract_digest (R-IDEA-001 / fszero-8n7a.16).
/// Stdout only; exit 0. Shared by shim and surface bins.
pub fn print_version_line() {
    println!(
        "fszero {} semantic_contract_digest={}",
        env!("CARGO_PKG_VERSION"),
        semantic_contract_digest()
    );
}

pub fn version_flag_requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--version" || a == "-V")
}

pub fn semantic_contract_digest() -> String {
    let abi = operation_abi_digest();
    let schemas = operation_abi_schemas_digest();
    let payload = format!(
        "fszero-semantic-contract\nabi_name={OPERATION_ABI_NAME}\nabi_version={OPERATION_ABI_VERSION}\nabi_digest={abi}\nschemas_digest={schemas}\n"
    );
    crate::core::operation_schemas::hex_encode_pub(&Sha256::digest(payload.as_bytes()))
}

/// Package identity block shared by doctor / help / SBOM / uninstall.
pub fn package_identity(surface: PackageSurface) -> serde_json::Value {
    serde_json::json!({
        "artifact": surface.artifact_name(), "surface": surface.as_str(), "shim": ARTIFACT_SHIM, "package_version": env!("CARGO_PKG_VERSION"),
        "abi_name": OPERATION_ABI_NAME, "abi_version": OPERATION_ABI_VERSION, "abi_digest": operation_abi_digest(), "schemas_digest": operation_abi_schemas_digest(),
        "semantic_contract_digest": semantic_contract_digest(),
        "selection_matrix": {
            "canonical_default": ARTIFACT_CODEMODE,
            "legacy_mcp_client": ARTIFACT_MCP,
            "rule": "CodeMode-first: fresh/default -> fszero-codemode; legacy MCP-only -> fszero-mcp"
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallState {
    pub surface: PackageSurface,
    pub artifact: String,
    pub binary_path: String,
    pub prefix: String,
    pub semantic_contract_digest: String,
    pub abi_digest: String,
    pub schemas_digest: String,
    pub package_version: String,
    pub installed_at_unix: u64,
    pub platform: String,
    /// Client config path written by this install (relative or absolute).
    pub client_config: String,
}

impl InstallState {
    pub fn for_surface(surface: PackageSurface, prefix: &Path, binary_path: &Path) -> Self {
        Self {
            surface,
            artifact: surface.artifact_name().to_string(),
            binary_path: binary_path.display().to_string(),
            prefix: prefix.display().to_string(),
            semantic_contract_digest: semantic_contract_digest(),
            abi_digest: operation_abi_digest(),
            schemas_digest: operation_abi_schemas_digest(),
            package_version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at_unix: now_unix(),
            platform: current_platform().to_string(),
            client_config: prefix.join(CLIENT_CONFIG_FILE).display().to_string(),
        }
    }
}

fn now_unix() -> u64 {
    crate::core::unix_epoch_secs() as u64
}

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}

/// Default install prefix: `$FSZERO_INSTALL_PREFIX` or `~/.fszero` or `./.fszero-install`.
pub fn default_install_prefix() -> PathBuf {
    if let Ok(p) = env::var("FSZERO_INSTALL_PREFIX") {
        return PathBuf::from(p);
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".fszero");
    }
    PathBuf::from(".fszero-install")
}

pub fn install_state_path(prefix: &Path) -> PathBuf {
    prefix.join(INSTALL_STATE_FILE)
}

pub fn load_install_state(prefix: &Path) -> Result<Option<InstallState>, String> {
    let path = install_state_path(prefix);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read install state: {e}"))?;
    let state: InstallState =
        serde_json::from_str(&raw).map_err(|e| format!("parse install state: {e}"))?;
    Ok(Some(state))
}

/// Create `prefix` and atomically write pretty-printed JSON to `path`.
fn write_pretty_json(
    prefix: &Path,
    path: &Path,
    value: &impl Serialize,
    what: &str,
) -> Result<(), String> {
    fs::create_dir_all(prefix).map_err(|e| format!("create install prefix: {e}"))?;
    let raw = serde_json::to_string_pretty(value).map_err(|e| format!("serialize {what}: {e}"))?;
    // Unify on core path atomic_write (perms/xattr carry); drop packaging-local duplicate.
    crate::core::path::atomic_write(path, raw.as_bytes()).map_err(|e| format!("write {what}: {e}"))
}

pub fn write_install_state(prefix: &Path, state: &InstallState) -> Result<(), String> {
    write_pretty_json(prefix, &install_state_path(prefix), state, "install state")
}

/// Client registration payload — single surface only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientConfig {
    pub name: String,
    pub surface: PackageSurface,
    pub command: String,
    pub args: Vec<String>,
    pub semantic_contract_digest: String,
    pub package_version: String,
}

pub fn client_config_for(surface: PackageSurface, binary_path: &Path) -> ClientConfig {
    let command = binary_path.display().to_string();
    // Dedicated artifact binaries are locked to a single surface and reject any
    // --mode= flag. The compatibility shim `fszero` needs --mode so it can re-exec
    // the selected artifact.
    let args = if binary_path.file_name().and_then(|s| s.to_str()) == Some(surface.artifact_name())
    {
        Vec::new()
    } else {
        vec![format!("--mode={}", surface.as_str())]
    };
    ClientConfig {
        name: format!("FSZero ({})", surface.as_str()),
        surface,
        command,
        args,
        semantic_contract_digest: semantic_contract_digest(),
        package_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

pub fn write_client_config(prefix: &Path, config: &ClientConfig) -> Result<PathBuf, String> {
    let path = prefix.join(CLIENT_CONFIG_FILE);
    write_pretty_json(prefix, &path, config, "client config")?;
    Ok(path)
}

/// Install one surface. Replaces prior surface cleanly (binary selection + client config).
pub fn install_surface(
    surface: PackageSurface,
    prefix: &Path,
    binary_path: &Path,
) -> Result<InstallState, String> {
    // Fail closed if both surfaces somehow requested via env.
    reject_dual_env_selection()?;

    let prev = load_install_state(prefix)?;
    if let Some(prev) = &prev {
        if prev.surface != surface {
            let _ = uninstall_surface(prefix);
        }
    }

    let config = client_config_for(surface, binary_path);
    write_client_config(prefix, &config)?;
    let state = InstallState::for_surface(surface, prefix, binary_path);
    write_install_state(prefix, &state)?;

    // Compatibility shim marker: `fszero` resolves to the selected surface.
    let shim_path = prefix.join("shim-target");
    fs::write(&shim_path, surface.as_str()).map_err(|e| format!("write shim-target: {e}"))?;

    Ok(state)
}

/// Remove install state, client config, and shim marker for this prefix.
pub fn uninstall_surface(prefix: &Path) -> Result<Option<InstallState>, String> {
    let prev = load_install_state(prefix)?;
    let _ = fs::remove_file(install_state_path(prefix));
    let _ = fs::remove_file(prefix.join(CLIENT_CONFIG_FILE));
    let _ = fs::remove_file(prefix.join("shim-target"));
    Ok(prev)
}

/// Detect dual surface selection attempts (env / argv) and fail closed.
pub fn reject_dual_env_selection() -> Result<(), String> {
    if let Ok(v) = env::var("FSZERO_SURFACE") {
        let lower = v.to_ascii_lowercase();
        if lower.contains(',')
            || lower.contains('+')
            || lower == "both"
            || lower == "all"
            || (lower.contains("mcp") && lower.contains("codemode"))
        {
            return Err(dual_surface_diagnostic(
                "FSZERO_SURFACE requests more than one package surface",
            ));
        }
    }
    if env::var_os("FSZERO_ENABLE_MCP").is_some() && env::var_os("FSZERO_ENABLE_CODEMODE").is_some()
    {
        return Err(dual_surface_diagnostic(
            "both FSZERO_ENABLE_MCP and FSZERO_ENABLE_CODEMODE are set",
        ));
    }
    Ok(())
}

pub fn dual_surface_diagnostic(detail: &str) -> String {
    format!(
        "fszero: dual package surface rejected (fail closed): {detail}. Install exactly one artifact ({ARTIFACT_MCP} or {ARTIFACT_CODEMODE}); CodeMode-first default is {ARTIFACT_CODEMODE}; legacy MCP-only clients install {ARTIFACT_MCP}. Do not register both catalogs in one client session."
    )
}

/// Parse CLI args for dual `--mode=` selection.
pub fn modes_from_args(args: &[String]) -> Result<Option<PackageSurface>, String> {
    let mut mcp = false;
    let mut codemode = false;
    for a in args {
        if let Some(rest) = a.strip_prefix("--mode=") {
            match rest {
                "mcp" => mcp = true,
                "codemode" => codemode = true,
                "both" | "all" | "mcp+codemode" | "codemode+mcp" => {
                    return Err(dual_surface_diagnostic(&format!("invalid --mode={rest}")));
                }
                _ => {}
            }
            continue;
        }
        if a == "--mode" {
            continue;
        }
    }
    if mcp && codemode {
        return Err(dual_surface_diagnostic(
            "both --mode=mcp and --mode=codemode present on argv",
        ));
    }
    if mcp {
        return Ok(Some(PackageSurface::Mcp));
    }
    if codemode {
        return Ok(Some(PackageSurface::Codemode));
    }
    Ok(None)
}

/// Resolve the single surface this process is allowed to start.
/// Gate: if args request a surface, it must match `selected` (immutable selection).
fn accept_surface(
    selected: PackageSurface,
    from_args: Option<PackageSurface>,
    mismatch: impl FnOnce(PackageSurface, PackageSurface) -> String,
) -> Result<PackageSurface, String> {
    if let Some(requested) = from_args {
        if requested != selected {
            return Err(mismatch(requested, selected));
        }
    }
    Ok(selected)
}

pub fn resolve_startup_surface(args: &[String]) -> Result<PackageSurface, String> {
    reject_dual_env_selection()?;
    let from_args = modes_from_args(args)?;

    if let Some(baked) = baked_package_surface() {
        return accept_surface(baked, from_args, |requested, baked| {
            format!(
                "fszero: artifact {} is locked to surface '{}'; refused request for '{}'. Reinstall the matching package or use the {} compatibility shim after selecting one surface.",
                baked.artifact_name(),
                baked.as_str(),
                requested.as_str(),
                ARTIFACT_SHIM
            )
        });
    }

    // Install state is the immutable selection for the packaging shim (re-exec target).
    let prefix = default_install_prefix();
    if let Some(state) = load_install_state(&prefix)? {
        return accept_surface(state.surface, from_args, |requested, installed| {
            format!(
                "fszero: installed surface is '{}' (immutable); refused request for '{}'. Reinstall with packaging/install.sh --surface {} to change selection.",
                installed.as_str(),
                requested.as_str(),
                requested.as_str()
            )
        });
    }

    // Shim-target file without full state.
    let shim = prefix.join("shim-target");
    if let Ok(raw) = fs::read_to_string(&shim) {
        let installed = PackageSurface::parse(raw.trim())?;
        return accept_surface(installed, from_args, |requested, installed| {
            format!(
                "fszero: shim-target surface is '{}' (immutable); refused '{}'",
                installed.as_str(),
                requested.as_str()
            )
        });
    }

    if let Ok(v) = env::var("FSZERO_SURFACE") {
        let env_surface = PackageSurface::parse(&v)?;
        return accept_surface(env_surface, from_args, |requested, env_surface| {
            format!(
                "fszero: FSZERO_SURFACE={} disagrees with --mode={}",
                env_surface.as_str(),
                requested.as_str()
            )
        });
    }

    // Args alone are not enough for the packaging shim (no surface runtime in-process).
    // Surface artifacts already returned via baked_package_surface above.
    if from_args.is_some() {
        return Err(
            "fszero: packaging shim has no surface runtime compiled in. \
Install fszero-mcp or fszero-codemode (selection is immutable), or invoke that artifact directly. \
Do not pass --mode= to a dual-capable process — dual surfaces are forbidden."
                .into(),
        );
    }

    // CodeMode-first: default to fszero-codemode when no selection is found.
    // This makes fresh shim invocations resolve to the canonical surface.
    Ok(PackageSurface::Codemode)
}

/// Resolve the absolute path of the selected surface binary for re-exec from the shim.
pub fn resolve_selected_binary(surface: PackageSurface) -> Result<PathBuf, String> {
    let prefix = default_install_prefix();
    if let Some(state) = load_install_state(&prefix)? {
        if state.surface != surface {
            return Err(format!(
                "fszero: install-state surface {} != requested {}",
                state.surface.as_str(),
                surface.as_str()
            ));
        }
        let p = PathBuf::from(&state.binary_path);
        if p.is_file() {
            return Ok(p);
        }
    }
    // Same directory as this executable (install.sh layout).
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(surface.artifact_name());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    // PATH lookup
    if let Ok(path) = env::var("PATH") {
        for entry in env::split_paths(&path) {
            let candidate = entry.join(surface.artifact_name());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "fszero: selected surface binary {} not found; run packaging/install.sh --surface {}",
        surface.artifact_name(),
        surface.as_str()
    ))
}

/// Resolve startup surface and its on-disk binary in one step (shim re-exec / supervise).
pub fn resolve_surface_binary(args: &[String]) -> Result<(PackageSurface, PathBuf), String> {
    let surface = resolve_startup_surface(args)?;
    let path = resolve_selected_binary(surface)?;
    Ok((surface, path))
}

/// Whether this build can start the given surface (feature matrix).
pub fn surface_compiled_in(surface: PackageSurface) -> bool {
    compile_time_surfaces().contains(&surface)
}

pub fn assert_surface_compiled(surface: PackageSurface) -> Result<(), String> {
    if surface_compiled_in(surface) {
        Ok(())
    } else {
        Err(format!(
            "fszero: package surface '{}' was not compiled into this binary (artifact features). Rebuild with --features surface-{} or install {}.",
            surface.as_str(),
            surface.as_str(),
            surface.artifact_name()
        ))
    }
}

/// Software bill of materials identity for the selected surface.
pub fn sbom_document(surface: PackageSurface) -> serde_json::Value {
    let mut identity = package_identity(surface);
    if let serde_json::Value::Object(ref mut map) = identity {
        map.insert(
            "sbom".into(),
            serde_json::json!({
                "format": "fszero-sbom-v1",
                "components": [
                    {
                        "type": "application", "name": surface.artifact_name(),
                        "version": env!("CARGO_PKG_VERSION"), "surface": surface.as_str(), },
                    { "type": "library", "name": "fs-zero", "version": env!("CARGO_PKG_VERSION"), "role": "shared-core", "semantic_contract_digest": semantic_contract_digest(), }
                ],
                "platform": current_platform(), "mutually_exclusive_with": surface.other().artifact_name(),
                "support_class": match surface { PackageSurface::Codemode => "canonical", PackageSurface::Mcp => "compatibility" },
                "support_policy": match surface { PackageSurface::Codemode => "canonical: full feature growth", PackageSurface::Mcp => "compatibility: security/correctness fixes only; no new features; removal planned after 180 days and two stable releases" },
            }),
        );
    }
    identity
}

/// Uninstall report identity line for CLI.
pub fn uninstall_report(prev: Option<InstallState>) -> serde_json::Value {
    match prev {
        Some(s) => serde_json::json!({
            "uninstalled": true, "artifact": s.artifact, "surface": s.surface.as_str(), "semantic_contract_digest": s.semantic_contract_digest,
            "package_version": s.package_version, "prefix": s.prefix,
        }),
        None => serde_json::json!({"uninstalled": false, "reason": "no install state present"}),
    }
}

/// Paste-ready markdown handbook for agents (`fszero robot-docs` / `--robot-help`).
pub fn robot_docs_guide() -> String {
    let caps = capabilities_document(None);
    let mut out = String::new();
    out.push_str("# FSZero agent handbook\n\n");
    out.push_str("## Quick start\n\n");
    out.push_str("1. Install the shim: `fszero install`\n");
    out.push_str("2. Point it at a sandbox root: `export FSZERO_ROOT=/path/to/workspace`\n");
    out.push_str("3. Verify: `fszero doctor --json`, then `fszero capabilities --json`\n\n");
    out.push_str("## Command reference\n\n");
    for cmd in SHIM_COMMANDS {
        out.push_str(&format!(
            "- `fszero {}` - {}\n",
            cmd,
            shim_command_purpose(cmd)
        ));
    }
    out.push_str("\n## Capability concepts (not interchangeable)\n\n");
    if let Some(concepts) = caps["capability_concepts"].as_object() {
        for name in [
            "cli",
            "package",
            "zeroref_store",
            "codemode_manifest",
            "handshake",
        ] {
            let Some(entry) = concepts.get(name) else {
                continue;
            };
            let owner = entry
                .get("owner")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let scope = entry
                .get("scope")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let retrieve = entry
                .get("retrieve")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            out.push_str(&format!(
                "- `{name}` - owner: {owner}; scope: {scope}; retrieve: `{retrieve}`\n"
            ));
        }
        if let Some(rule) = concepts
            .get("copy_rule")
            .and_then(serde_json::Value::as_str)
        {
            out.push_str(&format!("- `copy_rule` - {rule}\n"));
        }
    }
    out.push_str("\n## Exit codes\n\n");
    if let Some(codes) = caps["exit_codes"].as_object() {
        for (code, meaning) in codes {
            out.push_str(&format!("- `{}` - {}\n", code, render_scalar(meaning)));
        }
    }
    out.push_str("\n## Exit-code policy\n\n");
    if let Some(policy) = caps["exit_code_policy"].as_object() {
        for (name, meaning) in policy {
            out.push_str(&format!("- `{}` - {}\n", name, render_scalar(meaning)));
        }
    }
    // R-FAM-007 / fszero-2qdw.12: portable ZeroRef grammar is not shared-CAS recover.
    out.push_str("\n## Ref interop: portable grammar ≠ shared-CAS recover\n\n");
    out.push_str("- Portable ZeroRef v1 subset is blob identity only: `(fz|gz|tz)://blob/<64-hex>` with exact `#B`/`#L` fragments.\n");
    out.push_str("- **Parse** validates grammar. **Retag** (scheme rewrite, e.g. `gz://blob/H` → `fz://blob/H`) is an identity label change only -- it does not copy or invent bytes.\n");
    out.push_str("- **Recover / expand** requires the hash already present in that engine's recovery store or a shared CAS all engines pin. Matching grammar alone never implies bytes are available.\n");
    out.push_str("- Never fix a failed expand by rewriting the scheme. Never treat seq/file/execution/engine-private refs as portable by retag.\n");
    out.push_str("- Normative: `docs/architecture.md` and `docs/design/zeroref-v1-annex.md`.\n");
    out.push_str("\n## Environment variables\n\n");
    match &caps["env_vars"] {
        serde_json::Value::Object(map) => {
            for (name, meaning) in map {
                out.push_str(&format!("- `{}` - {}\n", name, render_scalar(meaning)));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                out.push_str(&format!("- `{}`\n", render_scalar(item)));
            }
        }
        _ => {}
    }
    out.push_str("\n## Recipes\n\n");
    out.push_str("- Health snapshot: `fszero doctor --json`\n");
    out.push_str("- Contract and command inventory: `fszero capabilities --json`\n");
    out.push_str("- Batch execution: `fszero batch plan.json`\n");
    out.push_str("- Machine triage bundle: `fszero robot-triage`\n\n");
    out.push_str("## Troubleshooting\n\n");
    out.push_str("- Home root refused: FSZERO_ROOT must not be your home directory; point it at a project subdirectory.\n");
    out.push_str("- Index lock busy: another session holds the single-indexer lock (store.db.indexlock); wait for it to finish or set FSZERO_INDEX_LOCK=0 to opt out.\n");
    out
}

fn render_scalar(value: &serde_json::Value) -> String {
    match value.as_str() {
        Some(text) => text.to_string(),
        None => value.to_string(),
    }
}

fn shim_command_purpose(cmd: &str) -> &str {
    match cmd {
        "help" => "print shim usage",
        "install" => "install or repair the selected surface binaries",
        "uninstall" => "remove installed surfaces and state",
        "sbom" => "emit the software bill of materials",
        "doctor" => "diagnose install, root and surface health",
        "serve" => "start the MCP or codemode server surface",
        "batch" => "execute a JSON plan of filesystem operations",
        "migrate-cas" => "migrate the content-addressed store",
        "store-gc" => "apply snapshot retention to forensic/salvage store siblings",
        "telemetry" => "inspect local telemetry records",
        "zeroref-fixture" => "generate zeroref test fixtures",
        "codemode" => "retired; model execution is ZeroKernel z.read/z.edit/z.apply",
        "capabilities" => "print the machine-readable capability contract",
        "catalog" => "list catalog entries",
        "tools" => "list callable tools for the active surface",
        "layout" => "print the read-only workspace/store layout inventory",
        "robot-triage" => "emit the JSON triage bundle for agents",
        "robot-docs" => "print this paste-ready agent handbook",
        _ => "see `fszero help`",
    }
}
