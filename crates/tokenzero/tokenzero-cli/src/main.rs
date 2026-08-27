#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;
use tokenzero_core::McpToolSurface;
use tokenzero_core::{
    ContentType, Mode, ToolResponse, detect_content_type,
    shell_display_command_from_argv_for_platform,
};
#[cfg(feature = "surface-mcp")]
use tokenzero_engine::mcp_idle_timeout_from_secs;
use tokenzero_engine::{
    EditHunk, EngineConfig, TokenZeroEngine, cli_json, default_shell_timeout, render_text,
    request_full_cli_envelope, shell_timeout_from_secs, slim_envelope_enabled,
};
use tokenzero_install as install;

mod agent_surfaces;
mod artifact_contracts;
mod claim_actions;
mod cli_args;
mod competitor_adapters;
mod completion_handoff;
mod hook;
#[cfg(feature = "surface-mcp")]
mod mcp_artifact;
mod reach;
mod release_claims;
mod source_currency;
mod zerostack_store;
use agent_surfaces::{capabilities_json, mcp_name_to_cli_verb, robot_docs_guide};
use artifact_contracts::{json_artifact_path, release_candidate_id};
use cli_args::*;
use competitor_adapters::{
    competitor_adapter_matrix, competitor_adapter_rows, load_benchmark_adapter_approval,
};
use reach::{installed_tokenzero_command_audit, run_reach};
use release_claims::{ClaimEvidenceInputs, run_claim_audit};
use tokenzero_pulse::{
    SessionLedgerReport, default_ledger_path, doctor_jsonl_sqlite, export_jsonl, import_jsonl,
    report_for_path, sync_jsonl_to_sqlite,
};
use tokenzero_runtime::{
    ExecutionMode, contains_platform_shell_syntax, env_map, plan_command_for_platform, quote_for,
    split_command_string,
};
use zerostack_store::{
    allowed_roots_for_workspace, default_allowed_roots, resolve_recovery_cache_path,
    tokenzero_work_root,
};

fn is_broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

fn map_stdout_write(result: io::Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) if is_broken_pipe(&err) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn write_stdout(text: &str) -> Result<()> {
    let mut stdout = io::stdout();
    map_stdout_write(
        stdout
            .write_all(text.as_bytes())
            .and_then(|_| stdout.flush()),
    )
}

fn writeln_stdout(text: impl AsRef<str>) -> Result<()> {
    let text = text.as_ref();
    if text.ends_with('\n') {
        write_stdout(text)
    } else {
        write_stdout(&format!("{text}\n"))
    }
}

fn print_clap_error(err: clap::Error) -> ! {
    let rendered = err.to_string();
    if err.use_stderr() {
        let _ = writeln!(io::stderr(), "{}", rendered.trim_end());
    } else {
        let _ = write_stdout(&rendered);
    }
    std::process::exit(err.exit_code());
}

fn emit_json_md<T, F, J, M>(output_json: J, output_md: M, as_json: bool, run: F) -> Result<()>
where
    T: serde::Serialize,
    F: FnOnce(J, M) -> Result<T>,
{
    emit_value(run(output_json, output_md)?, as_json)
}

fn emit_migration_report(
    json: String,
    mut text: String,
    failed: bool,
    as_json: bool,
    safe_alternative: &str,
) -> Result<()> {
    if failed && !as_json {
        text.push_str(&format!("\nSafe alternative: {safe_alternative}\n"));
    }
    if as_json {
        writeln_stdout(json)?;
    } else {
        writeln_stdout(text)?;
    }
    if failed {
        if as_json {
            eprintln!("Safe alternative: {safe_alternative}");
        }
        std::process::exit(1);
    }
    Ok(())
}

fn emit_operator_migration(
    outcome: tokenzero_engine::OperatorMigrationOutcome,
    as_json: bool,
    safe_alternative: &str,
) -> Result<()> {
    emit_migration_report(
        outcome.json,
        outcome.text,
        outcome.failed,
        as_json,
        safe_alternative,
    )
}

macro_rules! dispatch_command {
($command:expr;
@emit { $($ev:ident => $eh:ident),* $(,)? }
@result { $($rv:ident => $rh:ident),* $(,)? }
@json_md { $($jv:ident => $jr:expr),* $(,)? }
@value { $($vv:ident($va:ident) => $value:expr;)* }
@special { $($sv:ident($sa:ident) => $special:block)* }
) => {
match $command {
$(Commands::$ev(args) => emit($eh(args)?)?,)*
$(Commands::$rv(args) => $rh(args)?,)*
$(Commands::$jv(args) => emit_json_md(args.output_json, args.output_md, args.json, $jr)?,)*
$(Commands::$vv($va) => { let as_json = $va.json; emit_value($value, as_json)? },)*
$(Commands::$sv($sa) => $special,)*
}
};
}

#[cfg(feature = "surface-mcp")]
fn run_mcp_artifact(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    iterations: usize,
) -> Result<serde_json::Value> {
    mcp_artifact::run_mcp_artifact(output_json, output_md, iterations)
}

#[cfg(not(feature = "surface-mcp"))]
fn run_mcp_artifact(
    _output_json: PathBuf,
    _output_md: Option<PathBuf>,
    _iterations: usize,
) -> Result<serde_json::Value> {
    anyhow::bail!(
        "mcp-smoke and mcp-soak require explicit compatibility feature surface-mcp; use the ZeroStack aggregate host for canonical MCP"
    )
}

fn main() -> Result<()> {
    let argv: Vec<OsString> = std::env::args_os().collect();

    // Fast path: avoid building the full clap command tree for --version/-V.
    if argv.len() == 2 && matches!(argv[1].to_str(), Some("--version" | "-V")) {
        writeln_stdout(format!("tokenzero {}", env!("CARGO_PKG_VERSION")))?;
        return Ok(());
    }

    let normalized_argv = normalize_agent_invocation_args(argv);
    let normalized_argv = normalize_json_envelope_args(normalized_argv);
    let cli = match Cli::try_parse_from(&normalized_argv) {
        Ok(cli) => cli,
        Err(err) => {
            // bara (R-016): an unknown subcommand that is an MCP tz_* tool name
            // must suggest the mapped CLI verb (tz_read -> read), never clap's
            // generic nearest string (which sent tz_read to 'tree').
            if let Some(verb) = mcp_name_to_cli_verb(
                normalized_argv
                    .get(1)
                    .and_then(|arg| arg.to_str())
                    .unwrap_or_default(),
            ) {
                if matches!(err.kind(), clap::error::ErrorKind::InvalidSubcommand) {
                    let corrected: Vec<String> = normalized_argv
                        .iter()
                        .skip(1)
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect();
                    let corrected = std::iter::once(verb.to_string())
                        .chain(corrected.into_iter().skip(1))
                        .collect::<Vec<_>>()
                        .join(" ");
                    eprintln!(
                        "error: unrecognized subcommand '{}'\n\n  tip: '{}' is an MCP tool name; the CLI verb is '{}'\n\n  corrected command: tokenzero {}\n",
                        normalized_argv[1].to_string_lossy(),
                        normalized_argv[1].to_string_lossy(),
                        verb,
                        corrected,
                    );
                    std::process::exit(2);
                }
            }
            // bdki (R-002): Levenshtein-1 flag typo recovery. Distance-1 typos
            // get did-you-mean + copy-pasteable corrected command; anything
            // else gets a plain error without clap's misleading nearest-string
            // tip (which sent --exlpain to --help).
            if matches!(err.kind(), clap::error::ErrorKind::UnknownArgument) {
                if let Some(message) = flag_typo_message(&normalized_argv, &err) {
                    eprintln!("{message}");
                    std::process::exit(2);
                }
            }
            // dzb2 (R-003): missing required args name the exact invocation.
            if matches!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument) {
                if let Some(hint) = missing_arg_message(&normalized_argv, &err) {
                    eprintln!("{hint}");
                    std::process::exit(2);
                }
            }
            print_clap_error(err);
        }
    };
    let Some(command) = cli.command else {
        let mut help = Vec::new();
        Cli::command().write_help(&mut help)?;
        write_stdout(&String::from_utf8_lossy(&help))?;
        writeln_stdout("")?;
        return Ok(());
    };
    dispatch_command!(command;
    @emit { Read => handle_read, Find => handle_find, Grep => handle_grep, Glob => handle_glob, Tree => handle_tree, Edit => handle_edit, Recall => handle_recall, Fetch => handle_fetch, Run => handle_run, Ingest => handle_ingest, Expand => handle_expand, }
    @result { Rewrite => emit_rewrite, Doctor => handle_doctor, Pulse => handle_pulse, SessionLedger => handle_session_ledger, Cache => handle_cache, Install => handle_install, Init => handle_init, Clients => handle_clients, ClientStatus => handle_client_status, Capabilities => handle_capabilities, CachePack => handle_cache_pack, Bench => handle_bench, Quote => handle_quote, }
    @json_md { McpSmoke => |j, m| run_mcp_artifact(j, m, 1), McpSoak => |j, m| run_mcp_artifact(j, m, 25), ExactRecoveryShell => run_exact_recovery_shell, ExactRecoveryAudit => run_exact_recovery_audit, HarmEval => run_harm_eval, ProtectedAnchorAudit => run_protected_anchor_audit, FalseSuccessShell => run_false_success_shell, RepoInventory => run_repo_inventory, PromptCachePack => run_prompt_cache_pack, ShellMatrix => run_shell_matrix, OneShotEval => run_one_shot_eval, AdapterApprovalTemplate => run_adapter_approval_template, CompletionAudit => run_completion_audit, SecurityPrivacyAudit => run_security_privacy_audit, ArtifactHandoff => run_artifact_handoff, WsSkeleton => run_ws_skeleton, }
    @value { SessionOpen(args) => engine_from_common(&args).session_boot_snapshot(); Stats(args) => handle_stats(args)?; InstallSmoke(args) => run_install_smoke(args.output_json, args.apply)?; PackageAudit(args) => handle_package_audit(args)?; OsReachAudit(args) => run_os_reach_audit(args.output_json, args.output_md, args.root, args.os_artifact, args.release_approval,)?; OsReleaseArtifact(args) => run_os_release_artifact(args.output_json, args.output_md, args.root,)?; SourceCurrencyAudit(args) => run_source_currency_audit(args.output_json, args.output_md, args.refresh_ledger, args.refresh_git_heads,)?; AdapterApprovalAudit(args) => run_adapter_approval_audit(args.output_json, args.output_md, args.approval_file, args.execution_approval,)?; ClaimAudit(args) => run_claim_audit(args.output_json, args.output_md, args.release_approval, ClaimEvidenceInputs { source_artifact: args.source_artifact, benchmark_artifact: args.benchmark_artifact, adapter_approval_artifact: args.adapter_approval_artifact, recovery_artifact: args.recovery_artifact, task_success_artifact: args.task_success_artifact, os_artifact: args.os_artifact, },)?; Reach(args) => run_reach(args.root, args.output_json)?; }
    @special {
    Mem(args) => {
        let engine = engine_from_common(&args);
        emit_with_json(dispatch_cli_tool(&engine, "tz_mem", json!({})), args.json)?;
    }
    Hook(args) => { hook::handle_hook(args); }
    Discover(args) => {
        let root = tokenzero_work_root(None);
        let engine = engine_new(
            &root,
            default_allowed_roots(&root),
            None,
            4000,
            Mode::Auto,
            default_shell_timeout(),
            None,
        );
        emit_with_json(dispatch_cli_tool(&engine, "tz_discover", json!({})), args.json)?;
    }
    RobotDocs(args) => { handle_robot_docs(args)?; }
    McpServer(args) => {
        #[cfg(feature = "surface-mcp")]
        {
            compatibility_server_niceness();
            enforce_surface_exclusivity(&args)?;
            tokenzero_mcp_compat::run_fastmcp_stdio(engine_config_for_mcp(&args)?)
        }
        #[cfg(not(feature = "surface-mcp"))]
        {
            let _ = args;
            anyhow::bail!("MCP compatibility adapter is not compiled; rebuild tokenzero-cli with --features surface-mcp")
        }
    }
    });
    Ok(())
}

/// bdki (R-002): build an error message for an unknown long flag. Returns
/// None when the offending token cannot be identified (caller falls back to
/// clap's own rendering).
fn flag_typo_message(argv: &[OsString], err: &clap::Error) -> Option<String> {
    let root = Cli::command();
    let mut context = &root;
    // The flag may precede the verb (tokenzero --jsno read x): fall back to
    // the first positional that names a subcommand so the known-flag set is
    // the verb's, not just the top level's.
    let mut sub_name: Option<String> = None;
    for token in argv.iter().skip(1) {
        let Some(text) = token.to_str() else {
            continue;
        };
        if text.starts_with('-') {
            continue;
        }
        if let Some(sub) = root.find_subcommand(text) {
            context = sub;
            sub_name = Some(text.to_string());
            break;
        }
    }
    let mut known: Vec<String> = Vec::new();
    let mut canonical: Vec<String> = Vec::new();
    for arg in context.get_arguments() {
        if let Some(long) = arg.get_long() {
            let long = long.to_string();
            canonical.push(long.clone());
            known.push(long);
        }
        // Hidden aliases (jsno/jason/timout) count as known so a valid flag
        // placed before the verb is treated as mispositioned, not a typo.
        if let Some(aliases) = arg.get_aliases() {
            known.extend(aliases.into_iter().map(str::to_string));
        }
    }
    canonical.push("help".to_string());
    known.push("help".to_string());
    if sub_name.is_none() {
        canonical.push("version".to_string());
        known.push("version".to_string());
    }
    // Position of the verb token, so flags before it count as mispositioned.
    let sub_index = sub_name.as_ref().and_then(|sub| {
        argv.iter()
            .skip(1)
            .position(|arg| arg.to_str() == Some(sub.as_str()))
            .map(|pos| pos + 1)
    });
    let mut bad: Option<(String, String, bool)> = None;
    for (index, token) in argv.iter().enumerate().skip(1) {
        let Some(text) = token.to_str() else {
            continue;
        };
        if text == "--" {
            break;
        }
        let Some(name) = text.strip_prefix("--") else {
            continue;
        };
        let name = name.split('=').next().unwrap_or(name);
        if name.is_empty() {
            continue;
        }
        if known.iter().any(|flag| flag == name) {
            // A flag the verb owns, but placed before the verb: recoverable
            // by reordering, not renaming.
            if sub_index.is_some_and(|pos| index < pos) {
                bad = Some((text.to_string(), name.to_string(), true));
                break;
            }
            continue;
        }
        bad = Some((text.to_string(), name.to_string(), false));
        break;
    }
    let (bad_token, bad_name, mispositioned) = bad?;
    let mut candidates: Vec<&String> = known
        .iter()
        .filter(|flag| levenshtein(flag, &bad_name) == 1)
        .collect();
    candidates.sort();
    let mut out = format!("error: unexpected argument '{bad_token}' found\n\n");
    if mispositioned {
        let mut rest: Vec<String> = Vec::new();
        let mut sub_seen = false;
        for arg in argv.iter().skip(1) {
            let text = arg.to_string_lossy();
            if !sub_seen && sub_name.as_deref() == Some(text.as_ref()) {
                sub_seen = true;
                continue;
            }
            rest.push(text.into_owned());
        }
        let mut parts = vec!["tokenzero".to_string()];
        if let Some(sub) = &sub_name {
            parts.push(sub.clone());
        }
        parts.extend(rest);
        out.push_str(&format!(
            "  tip: '{bad_token}' belongs after the subcommand\n\n  corrected command: {}\n\n",
            parts.join(" ")
        ));
    } else if let Some(good) = candidates.first() {
        // Rebuild with the flag fixed; if the flag preceded the verb, move
        // the verb first so the corrected command actually parses.
        let mut rest: Vec<String> = Vec::new();
        let mut sub_seen = false;
        for arg in argv.iter().skip(1) {
            let text = arg.to_string_lossy();
            if !sub_seen && sub_name.as_deref() == Some(text.as_ref()) {
                sub_seen = true;
                continue;
            }
            if text == bad_token {
                rest.push(format!("--{good}"));
            } else if let Some(value) = text
                .strip_prefix(bad_token.as_str())
                .and_then(|tail| tail.strip_prefix('='))
            {
                rest.push(format!("--{good}={value}"));
            } else {
                rest.push(text.into_owned());
            }
        }
        let mut parts = vec!["tokenzero".to_string()];
        if let Some(sub) = &sub_name {
            parts.push(sub.clone());
        }
        parts.extend(rest);
        let corrected = parts.join(" ");
        out.push_str(&format!(
            "  tip: did you mean: '--{good}'?\n\n  corrected command: {corrected}\n\n"
        ));
    } else {
        canonical.sort();
        canonical.dedup();
        let valid = canonical
            .iter()
            .map(|flag| format!("--{flag}"))
            .collect::<Vec<_>>()
            .join(", ");
        let family = sub_name.as_deref().unwrap_or("tokenzero");
        out.push_str(&format!("  valid flags for '{family}': {valid}\n\n"));
        let usage = context.clone().render_usage().to_string();
        if sub_name.is_some() {
            out.push_str("Usage: tokenzero ");
            out.push_str(usage.trim().trim_start_matches("Usage: "));
        } else {
            out.push_str(usage.trim_end());
        }
        return Some(out);
    }
    let rendered = err.to_string();
    if let Some((_, tail)) = rendered.split_once("Usage:") {
        out.push_str(&format!("Usage:{}", tail.trim_end()));
    }
    Some(out)
}

/// dzb2 (R-003): for a missing required argument, append the exact
/// corrected invocation for the resolved subcommand.
fn missing_arg_message(argv: &[OsString], err: &clap::Error) -> Option<String> {
    let root = Cli::command();
    let sub_name = argv
        .iter()
        .skip(1)
        .filter_map(|arg| arg.to_str())
        .find(|text| !text.starts_with('-') && root.find_subcommand(text).is_some())?;
    let sub = root.find_subcommand(sub_name)?;
    if let Some(corrected) = match sub_name {
        "read" => Some("tokenzero read <path> --json"),
        "run" => Some("tokenzero run --json -- <command>"),
        "expand" => Some("tokenzero expand <tz-ref> --raw"),
        _ => None,
    } {
        return Some(format!(
            "{}\n  corrected command: {corrected}\n",
            err.to_string().trim_end()
        ));
    }
    let usage = sub
        .clone()
        .render_usage()
        .to_string()
        .trim()
        .trim_start_matches("Usage: ")
        .to_string();
    // usage renders as "<verb> [OPTIONS] <REQ>..."; drop the bin-prefixed
    // verb and rebuild as `tokenzero <verb> --json <required positionals>`.
    let positionals = usage
        .split_whitespace()
        .skip(1)
        .filter(|tok| *tok != "[OPTIONS]")
        .collect::<Vec<_>>()
        .join(" ");
    let rendered = err.to_string();
    Some(format!(
        "{}\n  corrected command: tokenzero {} --json {}\n",
        rendered.trim_end(),
        sub_name,
        positionals
    ))
}

/// bdki (R-002): classic Levenshtein distance over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            curr[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn normalize_agent_invocation_args(mut argv: Vec<OsString>) -> Vec<OsString> {
    if argv.len() <= 1 {
        return argv;
    }
    if argv.len() == 2 && matches!(argv[1].to_str(), Some("--robot-help" | "robot-help")) {
        argv[1] = OsString::from("robot-docs");
        argv.push(OsString::from("guide"));
        return argv;
    }
    // pec5 (R-001): root mega-command aliases -> doctor --robot-triage.
    if matches!(argv[1].to_str(), Some("--robot-triage" | "robot-triage")) {
        argv[1] = OsString::from("doctor");
        argv.insert(2, OsString::from("--robot-triage"));
        return argv;
    }
    if argv[1]
        .to_str()
        .is_some_and(|arg| arg == "--mode" || arg.starts_with("--mode="))
    {
        argv.insert(1, OsString::from("mcp-server"));
        return argv;
    }
    match argv[1].to_str() {
        // R-011: table-driven top-level verb recoveries. After the verb is
        // canonical, re-enter the normal pipeline so install subcommands still
        // flow through normalize_install_invocation_args.
        Some("rn" | "reed" | "instal") => {
            let mut normalized = argv;
            normalized[1] = OsString::from(match normalized[1].to_str() {
                Some("rn") => "run",
                Some("reed") => "read",
                Some("instal") => "install",
                _ => unreachable!("verb rewrite matched above"),
            });
            normalize_agent_invocation_args(normalized)
        }
        Some("run" | "shell") => normalize_run_invocation_args(argv),
        Some("install") => normalize_install_invocation_args(argv),
        _ => argv,
    }
}

fn normalize_json_envelope_args(mut argv: Vec<OsString>) -> Vec<OsString> {
    // Keep clap's boolean `--json` on every existing subcommand while adding
    // the exact compatibility spelling from the envelope contract. Stop at
    // `--`: child argv and authored source bytes must never be rewritten.
    for argument in argv.iter_mut().skip(1) {
        if argument == "--" {
            break;
        }
        if argument == "--json=full" {
            request_full_cli_envelope();
            *argument = OsString::from("--json");
        }
    }
    argv
}

fn normalize_install_invocation_args(argv: Vec<OsString>) -> Vec<OsString> {
    if argv.len() < 3 {
        return argv;
    }
    match argv[2].to_str() {
        Some("plan") => {
            let mut out = vec![argv[0].clone(), "install".into(), "--plan".into()];
            out.extend(argv[3..].iter().cloned());
            out
        }
        Some("status") => {
            let mut out = vec![argv[0].clone(), "clients".into(), "detect".into()];
            out.extend(
                argv[3..]
                    .iter()
                    .filter(|arg| {
                        !matches!(
                            arg.to_str(),
                            Some(
                                "--global"
                                    | "--mcp"
                                    | "--shell"
                                    | "--instructions"
                                    | "--cli"
                                    | "--plan"
                            )
                        )
                    })
                    .cloned(),
            );
            out
        }
        _ => argv,
    }
}

fn normalize_run_invocation_args(argv: Vec<OsString>) -> Vec<OsString> {
    if argv.iter().skip(2).any(|arg| arg.to_str() == Some("--")) {
        return argv;
    }
    let Some((options, command)) = split_run_args_without_delimiter(&argv[2..]) else {
        return argv;
    };
    let mut normalized = Vec::with_capacity(argv.len() + 1);
    normalized.push(argv[0].clone());
    normalized.push(argv[1].clone());
    normalized.extend(options);
    normalized.push(OsString::from("--"));
    normalized.extend(command);
    normalized
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunOptionKind {
    Flag,
    Value,
    Json,
}

const RUN_OPTIONS: &[(&str, RunOptionKind)] = &[
    ("--json", RunOptionKind::Json),
    ("--jsno", RunOptionKind::Json),
    ("--jason", RunOptionKind::Json),
    ("--no-rewrite", RunOptionKind::Flag),
    ("--stdin", RunOptionKind::Flag),
    ("--explain-runtime", RunOptionKind::Flag),
    ("--cwd", RunOptionKind::Value),
    ("--rewrite", RunOptionKind::Value),
    ("--env", RunOptionKind::Value),
    ("--runtime-platform", RunOptionKind::Value),
    ("--mode", RunOptionKind::Value),
    ("--budget", RunOptionKind::Value),
    ("--allowed-root", RunOptionKind::Value),
    ("--cache-path", RunOptionKind::Value),
    ("--timeout", RunOptionKind::Value),
    ("--timeout-seconds", RunOptionKind::Value),
    ("--timout", RunOptionKind::Value),
];

fn run_option(value: &str) -> Option<(RunOptionKind, bool)> {
    RUN_OPTIONS.iter().find_map(|&(option, kind)| {
        (value == option).then_some((kind, false)).or_else(|| {
            value
                .strip_prefix(option)
                .is_some_and(|suffix| suffix.starts_with('='))
                .then_some((kind, true))
        })
    })
}

fn split_run_args_without_delimiter(args: &[OsString]) -> Option<(Vec<OsString>, Vec<OsString>)> {
    let mut options = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        let value = args[idx].to_str()?;
        let width = match run_option(value) {
            Some((RunOptionKind::Value, false)) => {
                // Same rule as tokenzero-mcp `parse_flag`: a following flag is
                // missing value, not a path. Leave argv for clap to fail loud.
                let Some(next) = args.get(idx + 1) else {
                    return None;
                };
                if next.to_str().is_some_and(|text| text.starts_with('-')) {
                    return None;
                }
                2
            }
            Some(_) => 1,
            None if value.starts_with('-') => return None,
            None => break,
        };
        options.extend_from_slice(&args[idx..idx + width]);
        idx += width;
    }
    if idx >= args.len() {
        return None;
    }
    // Once the first child executable token is seen, every remaining token is
    // child argv. Trailing --json/--jsno/--jason must not be promoted to the
    // parent envelope (CE-P02-01); put parent options before the child or use `--`.
    let command = args[idx..].to_vec();
    (!command.is_empty()).then_some((options, command))
}

fn default_paths(path: Vec<PathBuf>) -> Vec<PathBuf> {
    if path.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        path
    }
}

fn tool_engine_mode(tool: &ToolArgs) -> Result<(TokenZeroEngine, Mode)> {
    Ok((engine_from_tool(tool)?, parse_mode(&tool.mode)?))
}

struct EmitResponse {
    responses: Vec<ToolResponse>,
    json: bool,
    complete_read_source: bool,
}

fn tools_emit(
    engine: &TokenZeroEngine,
    mut responses: Vec<ToolResponse>,
    json: bool,
    _tool: &str,
) -> Result<EmitResponse> {
    if json && slim_envelope_enabled() {
        for response in &mut responses {
            engine.apply_session_visible_ref_aliases(response);
        }
    }
    Ok(EmitResponse {
        responses,
        json,
        complete_read_source: false,
    })
}

fn tool_emit(
    engine: &TokenZeroEngine,
    response: ToolResponse,
    json: bool,
    tool: &str,
) -> Result<EmitResponse> {
    tools_emit(engine, vec![response], json, tool)
}

fn with_safe_alternative(mut response: ToolResponse, command: &str) -> ToolResponse {
    if let Some(error) = response.error.as_mut() {
        let safe = format!("Safe alternative: {command}");
        error.repair = Some(match error.repair.take() {
            Some(existing) if existing.contains(command) => existing,
            Some(existing) => format!("{existing} {safe}"),
            None => safe,
        });
    }
    response
}

/// Route a CLI domain op through the shared engine dispatcher exactly once.
fn dispatch_cli_tool(engine: &TokenZeroEngine, op: &str, args: serde_json::Value) -> ToolResponse {
    let outcome = tokenzero_engine::dispatch_cli(engine, op, &args);
    let response = if let Some(response) = outcome.tool_response {
        response
    } else if let Some(err) = outcome.domain_error {
        ToolResponse::error(op, err.kind.as_str(), err.message, None)
    } else {
        ToolResponse::error(
            op,
            "dispatch_empty",
            "domain dispatch returned no tool response",
            None,
        )
    };
    response
}

fn mode_json(mode: Mode) -> String {
    mode.to_string()
}

fn paths_json(paths: &[PathBuf]) -> serde_json::Value {
    json!(
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
    )
}

fn handle_find(args: FindArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let paths = default_paths(args.path);
    let response = dispatch_cli_tool(
        &engine,
        "tz_find",
        json!({
            "query": args.query,
            "path": paths_json(&paths),
            "mode": mode_json(mode),
            "max_files": args.max_files,
            "max_visible_tokens": args.max_visible_tokens,
        }),
    );
    tool_emit(&engine, response, args.tool.json, "find")
}

fn handle_recall(args: RecallArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let response = dispatch_cli_tool(
        &engine,
        "tz_recall",
        json!({
            "query": args.query,
            "max_hits": args.max_hits,
            "mode": mode_json(mode),
            "max_visible_tokens": args.max_visible_tokens,
        }),
    );
    tool_emit(&engine, response, args.tool.json, "recall")
}

fn handle_fetch(args: FetchArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let mut payload = json!({
        "url": args.url,
        "fresh": args.fresh,
        "mode": mode_json(mode),
        "max_visible_tokens": args.max_visible_tokens,
    });
    if let Some(ttl) = args.ttl_seconds {
        payload["ttl_seconds"] = json!(ttl);
    }
    let response = dispatch_cli_tool(&engine, "tz_fetch", payload);
    tool_emit(&engine, response, args.tool.json, "fetch")
}

fn handle_grep(args: FindArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let paths = default_paths(args.path);
    let response = dispatch_cli_tool(
        &engine,
        "tz_grep",
        json!({
            "query": args.query,
            "path": paths_json(&paths),
            "mode": mode_json(mode),
            "max_files": args.max_files,
            "max_visible_tokens": args.max_visible_tokens,
        }),
    );
    tool_emit(&engine, response, args.tool.json, "grep")
}

fn handle_glob(args: GlobArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let paths = default_paths(args.path);
    let response = dispatch_cli_tool(
        &engine,
        "tz_glob",
        json!({
            "pattern": args.pattern,
            "path": paths_json(&paths),
            "include_hidden": args.include_hidden,
            "mode": mode_json(mode),
            "max_files": args.max_files,
            "max_visible_tokens": args.max_visible_tokens,
        }),
    );
    tool_emit(&engine, response, args.tool.json, "glob")
}

fn handle_tree(args: TreeArgs) -> Result<EmitResponse> {
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let paths = default_paths(args.path);
    let response = dispatch_cli_tool(
        &engine,
        "tz_tree",
        json!({
            "path": paths_json(&paths),
            "depth": args.depth,
            "include_hidden": args.include_hidden,
            "mode": mode_json(mode),
            "max_files": args.max_files,
            "max_visible_tokens": args.max_visible_tokens,
        }),
    );
    tool_emit(&engine, response, args.tool.json, "tree")
}

fn handle_edit(args: EditArgs) -> Result<EmitResponse> {
    let edits_text = if args.stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        args.edits_json
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "edit requires --edits-json <json> or --stdin; safe alternative: tokenzero edit <path> --edits-json '<json>' --dry-run --json"
                )
            })?
    };
    let hunks: Vec<EditHunk> = serde_json::from_str(&edits_text).map_err(|err| {
        anyhow::anyhow!(
            "invalid edits JSON ({err}); expected [{{\"find\": \"...\", \"replace\": \"...\", \"replace_all\": false}}]; safe alternative: tokenzero edit <path> --edits-json '<json>' --dry-run --json"
        )
    })?;
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let edits_json: Vec<serde_json::Value> = hunks
        .iter()
        .map(|h| {
            json!({
                "find": h.find,
                "replace": h.replace,
                "replace_all": h.replace_all,
            })
        })
        .collect();
    let response = with_safe_alternative(
        dispatch_cli_tool(
            &engine,
            "tz_edit",
            json!({
                "path": args.path.display().to_string(),
                "edits": edits_json,
                "create": args.create,
                "dry_run": args.dry_run,
                "mode": mode_json(mode),
                "max_visible_tokens": args.max_visible_tokens,
            }),
        ),
        "tokenzero edit <path> --edits-json '<json>' --dry-run --json",
    );
    tool_emit(&engine, response, args.tool.json, "edit")
}

fn handle_ingest(args: IngestArgs) -> Result<EmitResponse> {
    let mut text = String::new();
    if args.stdin || args.input.is_none() || args.input.as_deref() == Some(Path::new("-")) {
        std::io::stdin().read_to_string(&mut text)?;
    } else if let Some(input) = &args.input {
        match fs::read_to_string(input) {
            Ok(loaded) => text = loaded,
            Err(err) => {
                // n3fx (R-012): --json must emit a structured JSON error, never
                // a bare text error, so JSON consumers can parse every path.
                if args.tool.json {
                    writeln_stdout(serde_json::to_string_pretty(&json!({
                        "schema_version": "tokenzero.cli.v1",
                        "status": "error",
                        "tool": "ingest",
                        "ack": "9",
                        "error": {
                            "code": "ingest_read_failed",
                            "message": format!("could not read {}: {}", input.display(), err),
                        },
                    }))?)?;
                    std::process::exit(1);
                }
                return Err(err.into());
            }
        }
    }
    let kind = content_type_from_kind(&args.kind, &text, args.input.as_deref());
    let source = args
        .input
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".to_string());
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let response = dispatch_cli_tool(
        &engine,
        "tz_ingest",
        json!({
            "text": text,
            "mode": mode_json(mode),
            "source": source,
            "content_type": kind.to_string(),
        }),
    );
    tool_emit(&engine, response, args.tool.json, "ingest")
}

fn handle_read(args: ReadArgs) -> Result<EmitResponse> {
    let mut paths = args.path;
    if let Some(paths_from) = args.paths_from {
        let root = tokenzero_work_root(None);
        let allowed_roots = allowed_roots_for_workspace(&root, &args.tool.allowed_root);
        if !existing_path_is_within_allowed_roots(&paths_from, &allowed_roots) {
            return Ok(EmitResponse {
                responses: vec![ToolResponse::error(
                    "read",
                    "path_not_allowed",
                    "paths-from file is outside allowed roots",
                    Some(
                        "Move the paths-from file under an allowed root or pass an explicit --allowed-root for that file"
                            .to_string(),
                    ),
                )],
                json: args.tool.json,
                complete_read_source: false,
            });
        }
        let text = fs::read_to_string(paths_from)?;
        paths.extend(
            text.lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .map(PathBuf::from),
        );
    }
    if paths.is_empty() {
        anyhow::bail!("read requires a path\n\n  corrected command: tokenzero read <path> --json");
    }
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let mut payload = json!({
        "path": paths_json(&paths),
        "mode": mode_json(mode),
        "raw": args.raw,
        "max_files": args.max_files,
        "max_visible_tokens": args.max_visible_tokens,
    });
    if let Some(s) = args.start_line {
        payload["start_line"] = json!(s);
    }
    if let Some(e) = args.end_line {
        payload["end_line"] = json!(e);
    }
    let response = dispatch_cli_tool(&engine, "tz_read", payload);
    let mut emitted = tool_emit(&engine, response, args.tool.json, "read")?;
    emitted.complete_read_source =
        paths.len() == 1 && args.start_line.is_none() && args.end_line.is_none();
    Ok(emitted)
}

fn handle_run(args: RunArgs) -> Result<EmitResponse> {
    if args.command.is_empty() && !args.stdin {
        anyhow::bail!(
            "run requires a command after --\n\n  corrected command: tokenzero run --json -- <command>"
        );
    }
    if args.explain_runtime {
        let argv = normalize_command(&args.command);
        let platform = args
            .runtime_platform
            .clone()
            .unwrap_or_else(|| tokenzero_runtime::current_platform().to_string());
        let plan = plan_command_for_platform(&argv, args.cwd.as_deref(), false, &platform)?;
        writeln_stdout(serde_json::to_string_pretty(&plan)?)?;
        std::process::exit(0);
    }
    let mut stdin_payload = None;
    if args.stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        stdin_payload = Some(buffer);
    }
    let env = env_map(&args.env_overrides)?;
    let normalized_command = normalize_command(&args.command);
    let command = display_command_for_platform(
        &normalized_command,
        args.cwd.as_deref(),
        tokenzero_runtime::current_platform(),
    );
    let (engine, mode) = tool_engine_mode(&args.tool)?;
    let mut payload = json!({
        "command": command,
        "argv": normalized_command,
        "mode": mode_json(mode),
        "no_rewrite": args.no_rewrite,
    });
    if let Some(cwd) = &args.cwd {
        payload["cwd"] = json!(cwd.display().to_string());
    }
    if let Some(rewrite) = &args.rewrite {
        payload["rewrite"] = json!(rewrite);
    }
    if let Some(stdin) = &stdin_payload {
        payload["stdin"] = json!(stdin);
    }
    if !env.is_empty() {
        payload["env"] = json!(env);
    }
    let response = dispatch_cli_tool(&engine, "tz_shell", payload);
    tool_emit(&engine, response, args.tool.json, "shell")
}

fn display_command_for_platform(argv: &[String], cwd: Option<&Path>, platform: &str) -> String {
    match plan_command_for_platform(argv, cwd, false, platform) {
        Ok(plan) if plan.execution_mode == ExecutionMode::Shell => argv.join(" "),
        _ => shell_display_command_from_argv_for_platform(argv, platform),
    }
}

fn handle_expand(args: ExpandArgs) -> Result<EmitResponse> {
    let mut refs = args.refs.clone();
    if let Some(refs_from) = &args.refs_from {
        refs.extend(
            fs::read_to_string(refs_from)?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
        );
    }
    if refs.is_empty() {
        anyhow::bail!(
            "expand requires a ref\n\n  corrected command: tokenzero expand <tz-ref> --raw"
        );
    }
    let root = tokenzero_work_root(None);
    let engine = engine_new(
        &root,
        default_allowed_roots(&root),
        args.cache_path.clone(),
        4000,
        Mode::Exact,
        default_shell_timeout(),
        None,
    );
    let (selector, start, end) = expand_selector(&args);
    let mut common = json!({});
    // yevj: --raw is the explicit raw-recovery authorization (cap-gated,
    // secret-gate bypass), not just the legacy "raw" selector shape.
    if args.raw {
        common["raw"] = json!(true);
    }
    if let Some(sel) = selector {
        common["selector"] = json!(sel);
    }
    if let Some(s) = start {
        common["start_line"] = json!(s);
    }
    if let Some(e) = end {
        common["end_line"] = json!(e);
    }
    if let Some(k) = &args.anchor_kind {
        common["anchor_kind"] = json!(k);
    }
    if let Some(sym) = &args.symbol {
        common["symbol"] = json!(sym);
    }
    let responses = refs
        .into_iter()
        .map(|ref_id| {
            let mut payload = common.clone();
            payload["ref"] = json!(ref_id);
            dispatch_cli_tool(&engine, "tz_expand", payload)
        })
        .collect();
    tools_emit(&engine, responses, args.json, "expand")
}

fn emit_rewrite(args: RewriteArgs) -> Result<()> {
    let command = match (&args.command, args.argv.is_empty()) {
        (Some(command), _) => command.clone(),
        (None, false) => display_command_for_platform(
            &normalize_command(&args.argv),
            None,
            tokenzero_runtime::current_platform(),
        ),
        (None, true) => anyhow::bail!(
            "rewrite requires a command string or `-- <command...>`\n\n  corrected command: tokenzero rewrite --json -- <command>"
        ),
    };
    let root = tokenzero_work_root(None);
    let engine = engine_new(
        &root,
        default_allowed_roots(&root),
        None,
        4000,
        Mode::Auto,
        default_shell_timeout(),
        None,
    );
    let response = dispatch_cli_tool(
        &engine,
        "tz_rewrite",
        json!({ "command": command, "mode": args.mode }),
    );
    emit_with_json(response, args.json)
}

fn path_display(p: &Path) -> String {
    p.display().to_string()
}

fn doctor_report(args: &DoctorArgs) -> serde_json::Value {
    let root = tokenzero_work_root(args.root.clone());
    let mut report = install::doctor(&root, args.cache_path.as_deref());
    let effective = allowed_roots_for_workspace(&root, &[]);
    report["effective_allowed_roots"] = json!(
        effective
            .iter()
            .map(|p| path_display(p))
            .collect::<Vec<_>>()
    );
    report["allowlist_algorithm"] = json!(
        "effective roots = doctor/call root union configured --allowed-root entries, deduped by canonical path. Relative CodeMode paths join to execute root."
    );
    let store = zerostack_store::store_resolution_report(&root, args.cache_path.clone());
    report["store_resolution"] =
        zerostack_store::store_resolution_json(&root, args.cache_path.clone());
    report["effective_store_root"] =
        json!(store.effective_store_root.as_ref().map(|p| path_display(p)));
    report["effective_cache_path"] = json!(path_display(&store.effective_cache_path));
    report["migration"] = tokenzero_engine::recovery_migration_state(&store.effective_cache_path);
    report["recovery_blobs"] =
        tokenzero_engine::recovery_blob_status_json(&store.effective_cache_path);
    report["engine_binaries"] = tokenzero_engine::engine_binaries_json();
    if let Some(summary) = &store.mismatch_summary {
        let mismatch = store.store_project_mismatch;
        let finding = json!({"id": if mismatch {"tz-store-project-mismatch"} else {"tz-store-global-pin-ignored"}, "severity": if mismatch {"warning"} else {"info"}, "status": "detected", "check": "store_resolution", "summary": summary, "evidence": {"project_root": path_display(&root), "effective_cache_path": path_display(&store.effective_cache_path), "effective_store_root": store.effective_store_root.as_ref().map(|p| path_display(p)), "shared_store_opt_in": store.shared_store_opt_in, "global_pin_set": store.global_pin_set, "isolation_mode": store.isolation_mode}, "auto_fix": false, "fix_supported": false, "next_step": if mismatch {"Use a per-project store (unset TOKENZERO_SHARED_STORE / ZEROSTACK_SHARED_STORE) or pass --cache-path under the project root."} else {"Default is per-project isolation (wqw.2). Set TOKENZERO_SHARED_STORE=1 only for intentional meta-workspace sharing."}});
        if let Some(findings) = report.get_mut("findings").and_then(|v| v.as_array_mut()) {
            findings.push(finding);
        }
    }
    if args.runtime {
        let plan =
            tokenzero_runtime::plan_command(&["echo".into(), "ok".into()], Some(&root), false).ok();
        report["runtime"] = serde_json::to_value(plan).unwrap_or(json!(null));
    }
    report
}

fn handle_doctor(args: DoctorArgs) -> Result<()> {
    let root = || tokenzero_work_root(args.root.clone());
    let cache = args.cache_path.as_deref();
    match args.command.clone() {
        Some(DoctorCommand::Capabilities) => emit_exit_json(install::doctor_capabilities()),
        Some(DoctorCommand::Health) => emit_doctor_health(&args),
        Some(DoctorCommand::Fix) => {
            emit_exit_json(install::doctor_fix(&root(), cache, args.dry_run))
        }
        Some(DoctorCommand::Undo { run_id }) => {
            emit_exit_json(install::doctor_undo(&root(), &run_id))
        }
        Some(DoctorCommand::Ls) => emit_exit_json(install::doctor_ls(&root())),
        Some(DoctorCommand::RobotDocs) => {
            write_stdout(&install::doctor_robot_docs())?;
            Ok(())
        }
        Some(DoctorCommand::Explain { finding_id }) => {
            emit_exit_json(install::doctor_explain(&root(), cache, &finding_id))
        }
        Some(DoctorCommand::Diagnose) | None => {
            if args.fix {
                return emit_exit_json(install::doctor_fix(&root(), cache, args.dry_run));
            }
            if let Some(finding_id) = args.explain.as_deref() {
                return emit_exit_json(install::doctor_explain(&root(), cache, finding_id));
            }
            if args.robot_triage {
                return emit_exit_json(install::doctor_robot_triage(&root(), cache));
            }
            emit_exit_json(doctor_report(&args))
        }
    }
}

fn emit_doctor_health(args: &DoctorArgs) -> Result<()> {
    let report = doctor_report(args);
    let u64f = |v: &serde_json::Value| v.as_u64().unwrap_or(0);
    let (ok, status) = (
        report["ok"].as_bool().unwrap_or(false),
        report["status"].as_str().unwrap_or("blocked"),
    );
    let (finding_count, blocking, info) = (
        u64f(&report["finding_count"]),
        u64f(&report["summary"]["blocking_findings"]),
        u64f(&report["summary"]["informational_findings"]),
    );
    let exit_code = doctor_exit_code(&report);
    let doctor_ver = report["doctor_version"]
        .as_str()
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    let line = format!(
        "{status} tokenzero={} doctor={doctor_ver} findings={finding_count} blocking={blocking} info={info}",
        env!("CARGO_PKG_VERSION")
    );
    if args.json {
        emit_exit_json(
            json!({"schema_version": "tokenzero.doctor.health.v1", "status": status, "ok": ok, "line": line, "finding_count": finding_count, "blocking_findings": blocking, "informational_findings": info, "exit_code": exit_code}),
        )
    } else {
        writeln_stdout(line)?;
        exit_if_nonzero(exit_code);
        Ok(())
    }
}

fn print_pretty<T: serde::Serialize>(value: &T) -> Result<()> {
    writeln_stdout(serde_json::to_string_pretty(value)?)
}
fn exit_if_nonzero(code: i32) {
    if code != 0 {
        std::process::exit(code);
    }
}
fn emit_exit_json(value: serde_json::Value) -> Result<()> {
    print_pretty(&value)?;
    exit_if_nonzero(doctor_exit_code(&value));
    Ok(())
}
fn doctor_exit_code(value: &serde_json::Value) -> i32 {
    if let Some(code) = value.get("exit_code").and_then(serde_json::Value::as_i64) {
        return code.clamp(0, 255) as i32;
    }
    if value.get("ok") == Some(&json!(false)) || value.get("status") == Some(&json!("blocked")) {
        1
    } else {
        0
    }
}

fn handle_stats(args: StatsArgs) -> Result<serde_json::Value> {
    let root = tokenzero_work_root(args.root);
    let cache = resolve_recovery_cache_path(&root, args.cache_path);
    if args.cachezero {
        return Ok(tokenzero_engine::cachezero_stats_json(&cache));
    }
    let mut report = serde_json::to_value(report_for_path(&default_ledger_path(&root))?)?;
    report["recovery_blobs"] = tokenzero_engine::recovery_blob_status_json(&cache);
    Ok(report)
}

fn handle_pulse(args: PulseArgs) -> Result<()> {
    let ledger_path = default_ledger_path(&tokenzero_work_root(args.root));
    match args.command {
        Some(PulseCommand::Sync) => {
            emit_pulse_result("pulse sync", sync_jsonl_to_sqlite(&ledger_path), args.json)
        }
        Some(PulseCommand::Doctor) => {
            emit_pulse_result("pulse doctor", doctor_jsonl_sqlite(&ledger_path), args.json)
        }
        Some(PulseCommand::ExportJsonl(a)) => emit_pulse_result(
            "pulse export-jsonl",
            export_jsonl(&ledger_path, &a.output),
            args.json,
        ),
        Some(PulseCommand::ImportJsonl(a)) => emit_pulse_result(
            "pulse import-jsonl",
            import_jsonl(&a.input, &ledger_path),
            args.json,
        ),
        Some(PulseCommand::Stats) | None => {
            let _ = sync_jsonl_to_sqlite(&ledger_path);
            let report = report_for_path(&ledger_path)?;
            if args.json {
                print_pretty(&report)
            } else {
                write_stdout(&tokenzero_pulse::render_text(&report))?;
                Ok(())
            }
        }
    }
}

fn handle_session_ledger(args: SessionLedgerArgs) -> Result<()> {
    let root = tokenzero_work_root(args.root);
    let pulse_ledger_path = default_ledger_path(&root);
    let response_ledger_path =
        tokenzero_engine::ledger::ledger_path_for_cache(&resolve_recovery_cache_path(&root, None));
    match args.command {
        Some(SessionLedgerCommand::Schema) => print_pretty(&SessionLedgerReport::schema_json())?,
        Some(SessionLedgerCommand::Inspect(flags)) => {
            let env_value = std::env::var(tokenzero_engine::ledger::TELEMETRY_ENV).ok();
            let enabled = tokenzero_engine::ledger::resolve_telemetry(
                flags.telemetry,
                flags.no_telemetry,
                None,
                env_value.as_deref(),
            );
            let usage_path = tokenzero_engine::ledger::usage_telemetry_path_for_cache(
                &resolve_recovery_cache_path(&root, None),
            );
            emit_value(
                tokenzero_engine::ledger::inspect_telemetry(&usage_path, enabled)?,
                args.json,
            )?;
        }
        Some(SessionLedgerCommand::Export) => {
            print_pretty(&SessionLedgerReport::from_ledger(&pulse_ledger_path)?)?
        }
        Some(SessionLedgerCommand::Stats) | None => {
            let report = SessionLedgerReport::from_ledger(&pulse_ledger_path)?;
            if args.json {
                print_pretty(&report)?;
            } else {
                write_stdout(&report.render_text())?;
            }
        }
        Some(SessionLedgerCommand::Query { query }) => {
            let since_ms = |days: u64| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| {
                        u64::try_from(d.as_millis())
                            .unwrap_or(u64::MAX)
                            .saturating_sub(days.saturating_mul(86_400_000))
                    })
                    .unwrap_or(0)
            };
            if let LedgerQueryCommand::TaskCost { json_out, csv_out } = &query {
                let report = tokenzero_engine::ledger::write_task_cost_report(
                    &response_ledger_path,
                    json_out,
                    csv_out,
                )?;
                emit_value(
                    json!({
                        "schema": report.schema,
                        "task_count": report.task_count,
                        "successful_tasks": report.successful_tasks,
                        "success_rate": report.success_rate,
                        "json_path": json_out,
                        "csv_path": csv_out,
                    }),
                    args.json,
                )?;
            } else {
                let query = match query {
                    LedgerQueryCommand::Repo { repo, days } => {
                        tokenzero_engine::ledger::LedgerQuery::RepoCost {
                            repo: repo.to_string_lossy().into_owned(),
                            since_ms: since_ms(days),
                        }
                    }
                    LedgerQueryCommand::VersionDelta {
                        baseline,
                        candidate,
                        days,
                    } => tokenzero_engine::ledger::LedgerQuery::VersionDelta {
                        baseline,
                        candidate,
                        since_ms: since_ms(days),
                    },
                    LedgerQueryCommand::AgentSpend { days } => {
                        tokenzero_engine::ledger::LedgerQuery::AgentSpend {
                            since_ms: since_ms(days),
                        }
                    }
                    LedgerQueryCommand::TaskCost { .. } => unreachable!(),
                };
                emit_value(
                    tokenzero_engine::ledger::query_ledger(&response_ledger_path, &query)?,
                    args.json,
                )?;
            }
        }
    }
    Ok(())
}

fn emit_pulse_result<T: serde::Serialize>(
    operation: &str,
    result: std::io::Result<T>,
    as_json: bool,
) -> Result<()> {
    match result {
        Ok(value) => emit_value(value, as_json),
        Err(err) if as_json => {
            let kind = err.kind();
            writeln_stdout(serde_json::to_string_pretty(
                &json!({"schema_version": "tokenzero.pulse.error.v1", "ok": false, "status": "error", "operation": operation, "error_kind": io_error_kind_name(kind), "retryable": kind == std::io::ErrorKind::WouldBlock, "error": err.to_string(), "exit_code": 1}),
            )?)?;
            std::process::exit(1);
        }
        Err(err) => Err(err.into()),
    }
}

fn io_error_kind_name(kind: std::io::ErrorKind) -> &'static str {
    use std::io::ErrorKind as K;
    match kind {
        K::NotFound => "not_found",
        K::PermissionDenied => "permission_denied",
        K::ConnectionRefused => "connection_refused",
        K::ConnectionReset => "connection_reset",
        K::ConnectionAborted => "connection_aborted",
        K::NotConnected => "not_connected",
        K::AddrInUse => "addr_in_use",
        K::AddrNotAvailable => "addr_not_available",
        K::BrokenPipe => "broken_pipe",
        K::AlreadyExists => "already_exists",
        K::WouldBlock => "would_block",
        K::InvalidInput => "invalid_input",
        K::InvalidData => "invalid_data",
        K::TimedOut => "timed_out",
        K::WriteZero => "write_zero",
        K::Interrupted => "interrupted",
        K::Unsupported => "unsupported",
        K::UnexpectedEof => "unexpected_eof",
        K::OutOfMemory => "out_of_memory",
        _ => "other",
    }
}

fn handle_cache(args: CacheArgs) -> Result<()> {
    match args.command {
        CacheCommand::Status(args) => {
            let engine = engine_from_common(&args);
            emit_with_json(dispatch_cli_tool(&engine, "tz_mem", json!({})), args.json)?
        }
        CacheCommand::Prune(args) => {
            let root = tokenzero_work_root(args.root);
            let cache = resolve_recovery_cache_path(&root, args.cache_path);
            let dry_run = !args.apply;
            let report = tokenzero_engine::prune_stale_cache(&cache, dry_run).map_err(|err| {
                anyhow::anyhow!(
                    "cache prune failed ({err}); safe alternative: tokenzero cache prune --json"
                )
            })?;
            emit_value(report, args.json)?;
        }
        CacheCommand::MigrateRefs(args) => emit_operator_migration(
            tokenzero_engine::cache_migrate_refs(args.root, args.cache_path, !args.apply),
            args.json,
            "tokenzero cache migrate-refs --json",
        )?,
        CacheCommand::MigrateVerify(args) => emit_operator_migration(
            tokenzero_engine::cache_migrate_verify(args.root, args.cache_path),
            args.json,
            "tokenzero cache migrate-refs --json",
        )?,
        CacheCommand::MigrateRollback(args) => emit_operator_migration(
            tokenzero_engine::cache_migrate_rollback(args.root, args.cache_path, args.apply),
            args.json,
            "tokenzero cache migrate-rollback --json",
        )?,
        CacheCommand::MigrateCleanup(args) => emit_operator_migration(
            tokenzero_engine::cache_migrate_cleanup(
                args.root,
                args.cache_path,
                args.apply,
                args.confirm_cleanup,
            ),
            args.json,
            "tokenzero cache migrate-verify --json",
        )?,
    }
    Ok(())
}

fn handle_cache_pack(args: CachePackArgs) -> Result<()> {
    let root = tokenzero_work_root(args.root.clone());
    let engine = engine_new(
        &root,
        default_allowed_roots(&root),
        args.cache_path.clone(),
        4000,
        Mode::Structured,
        default_shell_timeout(),
        None,
    );
    let response = dispatch_cli_tool(&engine, "tz_cache_pack", json!({ "scope": args.scope }));
    emit_with_json(response, args.json)
}

fn handle_bench(args: BenchArgs) -> Result<()> {
    let BenchCommand::Competitors(args) = args.command;
    let report = run_bench_competitors(args)?;
    print_pretty(&report)
}

/// Hub install engine owns its own `McpToolSurface` (zerostack-install).
/// TokenZero core owns the CLI/MCP wire enum. Same names, distinct types.
fn install_mcp_surface(surface: McpToolSurface) -> install::McpToolSurface {
    match surface {
        McpToolSurface::Classic => install::McpToolSurface::Classic,
        McpToolSurface::CodeMode => install::McpToolSurface::CodeMode,
    }
}

fn install_apply_or_plan(
    root: &Path,
    global: bool,
    capabilities: &[String],
    agents: &[String],
    surface: McpToolSurface,
    apply: bool,
    as_json: bool,
) -> Result<()> {
    let surface = install_mcp_surface(surface);
    if apply {
        let applied = install::apply_for_agents(root, global, capabilities, agents, surface)
            .with_context(
                || "install apply failed; safe alternative: tokenzero install --plan --json",
            )?;
        emit_value(stamp_mcp_orifice(applied)?, as_json)
    } else {
        emit_value(
            stamp_mcp_orifice(install::plan_for_agents(
                root,
                global,
                capabilities,
                agents,
                surface,
            ))?,
            as_json,
        )
    }
}

fn stamp_mcp_orifice<T: serde::Serialize>(value: T) -> Result<serde_json::Value> {
    let mut stamped = serde_json::to_value(value)?;
    if let Some(object) = stamped.as_object_mut() {
        object.insert("mcp_orifice".into(), install::mcp_orifice_json());
    }
    Ok(stamped)
}

fn handle_install(args: InstallArgs) -> Result<()> {
    let agents = install_agents(&args.agents, args.grok)?;
    let capabilities = install_capabilities(&args);
    let surface = parse_mcp_surface(&args.surface)?;
    let root = install_root(args.root.clone(), args.global);
    if let Some(id) = args.rollback {
        let rollback = install::rollback(&root, &id).with_context(
            || "install rollback failed; safe alternative: tokenzero doctor --json",
        )?;
        emit_value(rollback, args.json)
    } else {
        install_apply_or_plan(
            &root,
            args.global,
            &capabilities,
            &agents,
            surface,
            args.apply,
            args.json,
        )
    }
}

fn handle_init(args: InitArgs) -> Result<()> {
    let _plan_requested = args.plan;
    install_apply_or_plan(
        &install_root(args.root.clone(), args.global),
        args.global,
        &init_capabilities(&args),
        &install_agents(&args.agents, false)?,
        parse_mcp_surface(&args.surface)?,
        args.apply,
        args.json,
    )
}

fn handle_clients(args: ClientsArgs) -> Result<()> {
    match args.command {
        ClientsCommand::Detect(args) => handle_client_status(args),
        ClientsCommand::Scan(args) => handle_clients_scan(args),
        ClientsCommand::Plan(args) => handle_clients_plan(args),
        ClientsCommand::Doctor(args) => handle_clients_doctor(args),
        ClientsCommand::Rollback(args) => handle_clients_rollback(args),
    }
}

/// Presence scan: which AI harnesses live on this machine, and the install
/// invocation that wires the supported ones. Detection only — nothing is
/// written.
fn handle_clients_scan(args: ClientStatusArgs) -> Result<()> {
    let home = install_root(args.root.clone(), true);
    let detected = install::detect_present_agents(&home, std::env::var("PATH").ok().as_deref());
    let supported: Vec<&str> = detected
        .iter()
        .filter(|a| a.supported)
        .map(|a| a.agent.as_str())
        .collect();
    let next_step = if supported.is_empty() {
        "no supported harnesses detected; docs/install.md covers manual adapters".to_string()
    } else {
        format!(
            "tokenzero install --global --apply --hooks{}",
            supported
                .iter()
                .map(|a| format!(" --agent {a}"))
                .collect::<String>()
        )
    };
    emit_value(
        json!({"schema_version": "tokenzero.clients.v1", "command": "clients scan", "status": "ok", "home": path_display(&home), "detected": detected, "unsupported_note": "supported=false entries need the manual adapter snippets in docs/install.md", "next_step": next_step}),
        args.json,
    )
}

fn handle_client_status(args: ClientStatusArgs) -> Result<()> {
    emit_value(
        client_status_report(
            &install_root(args.root.clone(), true),
            &install_agents(&args.agents, args.grok)?,
            "detect",
        )?,
        args.json,
    )
}

fn handle_clients_plan(args: ClientsPlanArgs) -> Result<()> {
    let profile = clients_profile(&args.profile)?;
    let agents = install_agents(&args.agents, args.grok)?;
    let root = install_root(args.root.clone(), true);
    let mut value = stamp_mcp_orifice(install::plan_for_agents(
        &root,
        true,
        &clients_capabilities(&profile),
        &agents,
        install_mcp_surface(clients_mcp_surface(&profile)),
    ))?;
    if let Some(object) = value.as_object_mut() {
        object.extend([
            ("schema_version".into(), json!("tokenzero.clients.plan.v1")),
            ("command".into(), json!("clients plan")),
            ("profile".into(), json!(profile)),
            ("root".into(), json!(path_display(&root))),
            ("agents".into(), json!(clients_agent_labels(&agents))),
        ]);
    }
    emit_value(value, args.json)
}

const CLIENTS_DOCTOR_FINDINGS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "installed",
        "tz-clients-installed",
        "info",
        "TokenZero client integration surfaces are present",
        "Run tokenzero doctor --json for runtime health.",
    ),
    (
        "mixed",
        "tz-clients-mixed",
        "warning",
        "Some TokenZero client integration surfaces are present and some are missing",
        "Run tokenzero clients plan --profile standard --json, review the plan, then use tokenzero install --global --apply --mcp --json if approved.",
    ),
    (
        "",
        "tz-clients-missing",
        "info",
        "No TokenZero client integration surfaces were detected at the planned target paths",
        "Run tokenzero clients plan --profile standard --json to inspect the read-only integration plan.",
    ),
];

fn clients_doctor_findings(status: &str) -> Vec<serde_json::Value> {
    let row = CLIENTS_DOCTOR_FINDINGS
        .iter()
        .find(|(s, ..)| *s == status)
        .unwrap_or(&CLIENTS_DOCTOR_FINDINGS[2]);
    vec![json!({"id": row.1, "severity": row.2, "summary": row.3, "next_step": row.4})]
}

fn handle_clients_doctor(args: ClientStatusArgs) -> Result<()> {
    let mut report = client_status_report(
        &install_root(args.root.clone(), true),
        &install_agents(&args.agents, args.grok)?,
        "doctor",
    )?;
    let status = report
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let findings = clients_doctor_findings(status);
    if let Some(object) = report.as_object_mut() {
        object.insert("findings".to_string(), json!(findings));
    }
    emit_value(report, args.json)
}

fn handle_clients_rollback(args: ClientsRollbackArgs) -> Result<()> {
    let rollback = install::rollback(&install_root(args.root.clone(), true), &args.id)
        .with_context(
            || "clients rollback failed; safe alternative: tokenzero clients doctor --json",
        )?;
    emit_value(rollback, args.json)
}

fn handle_capabilities(args: CapabilitiesArgs) -> Result<()> {
    emit_value(capabilities_json(), args.json)
}

fn handle_robot_docs(args: RobotDocsArgs) -> Result<()> {
    write_stdout(match args.command {
        RobotDocsCommand::Guide => robot_docs_guide(),
        RobotDocsCommand::Commands => agent_surfaces::robot_docs_commands(),
        RobotDocsCommand::Examples => agent_surfaces::robot_docs_examples(),
    })
}

fn handle_package_audit(args: PackageAuditArgs) -> Result<serde_json::Value> {
    let artifacts = collect_package_audit_artifacts(&args.dist)?;
    Ok(install::package_audit(
        &tokenzero_work_root(None),
        &artifacts,
    ))
}

/// `--dist .` (the default) audits workspace packaging files. Any other path
/// must exist and be readable; an empty or unreadable dist must not silently
/// fall back to those defaults (which can report ok:true with no archives).
fn collect_package_audit_artifacts(dist: &Path) -> Result<Vec<PathBuf>> {
    if dist == Path::new(".") {
        return Ok(Vec::new());
    }
    if dist.is_file() {
        return Ok(vec![dist.to_path_buf()]);
    }
    if !dist.exists() {
        anyhow::bail!("package-audit --dist {} does not exist", dist.display());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dist)
        .with_context(|| format!("package-audit --dist {} is unreadable", dist.display()))?
    {
        let entry = entry
            .with_context(|| format!("package-audit --dist {} is unreadable", dist.display()))?;
        paths.push(entry.path());
    }
    if paths.is_empty() {
        anyhow::bail!(
            "package-audit --dist {} contains no artifacts",
            dist.display()
        );
    }
    Ok(paths)
}

fn handle_quote(args: QuoteArgs) -> Result<()> {
    let argv = normalize_command(&args.args);
    let quoted = quote_for(&args.platform, &argv);
    if args.json {
        print_pretty(&json!({"platform": args.platform, "argv": argv, "command": quoted}))
    } else {
        writeln_stdout(quoted)?;
        Ok(())
    }
}

fn engine_config(
    root: &Path,
    allowed_roots: Vec<PathBuf>,
    cache_path: PathBuf,
    max_visible_tokens: usize,
    mode: Mode,
    shell_timeout: std::time::Duration,
    mcp_idle_timeout: Option<std::time::Duration>,
) -> EngineConfig {
    EngineConfig {
        allowed_roots,
        cache_path,
        max_visible_tokens,
        mode,
        shell_timeout,
        mcp_idle_timeout,
        ..EngineConfig::for_root(root)
    }
}

fn engine_new(
    root: &Path,
    allowed_roots: Vec<PathBuf>,
    cache_path: Option<PathBuf>,
    budget: usize,
    mode: Mode,
    shell_timeout: std::time::Duration,
    mcp_idle: Option<std::time::Duration>,
) -> TokenZeroEngine {
    TokenZeroEngine::new_cli(engine_config(
        root,
        allowed_roots,
        resolve_recovery_cache_path(root, cache_path),
        budget,
        mode,
        shell_timeout,
        mcp_idle,
    ))
}

fn engine_from_tool(args: &ToolArgs) -> Result<TokenZeroEngine> {
    let root = tokenzero_work_root(None);
    Ok(engine_new(
        &root,
        allowed_roots_for_workspace(&root, &args.allowed_root),
        args.cache_path.clone(),
        args.budget.unwrap_or(4000),
        parse_mode(&args.mode)?,
        shell_timeout_from_secs(args.timeout_seconds),
        None,
    ))
}

fn engine_from_common(args: &CommonArgs) -> TokenZeroEngine {
    let root = tokenzero_work_root(args.root.clone());
    engine_new(
        &root,
        default_allowed_roots(&root),
        args.cache_path.clone(),
        4000,
        Mode::Auto,
        default_shell_timeout(),
        None,
    )
}

#[cfg(feature = "surface-mcp")]
fn engine_config_for_mcp(args: &McpServerArgs) -> Result<EngineConfig> {
    let root = mcp_work_root(&args.allowed_root);
    let tool_surface = args
        .tool_surface
        .as_deref()
        .unwrap_or(&args.mode)
        .parse::<McpToolSurface>()
        .map_err(anyhow::Error::msg)?;
    if tool_surface != McpToolSurface::Classic {
        anyhow::bail!(
            "engine-local CodeMode was removed; tokenzero-mcp serves only classic MCP and ZeroStack owns aggregate plan execution"
        );
    }
    let mut config = engine_config(
        &root,
        allowed_roots_for_workspace(&root, &args.allowed_root),
        resolve_recovery_cache_path(&root, args.cache_path.clone()),
        4000,
        parse_mode(&args.default_mode)?,
        shell_timeout_from_secs(args.shell_timeout_seconds),
        mcp_idle_timeout_from_secs(args.idle_timeout_seconds),
    );
    config.tool_surface = tool_surface;
    Ok(config)
}

#[cfg(feature = "surface-mcp")]
fn mcp_work_root(allowed_roots: &[PathBuf]) -> PathBuf {
    tokenzero_work_root(allowed_roots.first().cloned())
}

/// The long-lived classic MCP compatibility server runs at reduced scheduling
/// priority so it cannot starve interactive sessions. `TOKENZERO_NO_RENICE=1`
/// opts out.
#[cfg(all(unix, feature = "surface-mcp"))]
fn compatibility_server_niceness() {
    if std::env::var_os("TOKENZERO_NO_RENICE").is_some() {
        return;
    }
    let _ = std::process::Command::new("renice")
        .args(["-n", "5", "-p", &std::process::id().to_string()])
        .output();
}

#[cfg(all(not(unix), feature = "surface-mcp"))]
fn compatibility_server_niceness() {}

/// Keep classic MCP compatibility registration separate from the aggregate host.
///
/// Engine-local CodeMode is absent. The remaining runtime guard refuses any
/// non-classic mode and, when the hub owns a root, refuses a competing classic
/// MCP registration unless the explicit debug sentinel override is present.
#[cfg(feature = "surface-mcp")]
fn enforce_surface_exclusivity(args: &McpServerArgs) -> Result<()> {
    if let Err(err) = install::packaging::reject_dual_compiled_surfaces() {
        anyhow::bail!("{err}");
    }
    let argv: Vec<String> = std::env::args().collect();
    let resolved =
        install::packaging::resolve_startup_surface(&argv).map_err(|e| anyhow::anyhow!("{e}"))?;

    let requested = args.tool_surface.as_deref().unwrap_or(&args.mode);
    let requested_surface =
        install::packaging::PackageSurface::parse(requested).map_err(|e| anyhow::anyhow!("{e}"))?;
    if requested_surface != resolved {
        anyhow::bail!(
            "tokenzero: process surface is locked to '{}'; refused request for '{}'. \
Install {} for that surface (mutually exclusive — one process, one catalog).",
            resolved.as_str(),
            requested_surface.as_str(),
            requested_surface.artifact_name()
        );
    }
    install::packaging::assert_surface_compiled(resolved).map_err(|e| anyhow::anyhow!("{e}"))?;

    #[cfg(not(unix))]
    {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let surface = args.tool_surface.as_deref().unwrap_or(&args.mode);
        if surface != "mcp" || std::env::var_os("TOKENZERO_ALLOW_DUAL").is_some() {
            return Ok(());
        }
        let root = mcp_work_root(&args.allowed_root);
        let sentinel = root.join(".zerostack").join("codemode.active");
        let Ok(raw) = std::fs::read_to_string(&sentinel) else {
            return Ok(());
        };
        let pid = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|value| value.get("pid").and_then(serde_json::Value::as_u64));
        let live = pid.is_some_and(|pid| {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|status| status.success())
        });
        if live {
            anyhow::bail!(
                "CodeMode hub is active for {} (pid {} via {}); per-op MCP and CodeMode must not run together for one repo. Stop the hub or set TOKENZERO_ALLOW_DUAL=1 (hub sentinel only — never dual catalogs).",
                root.display(),
                pid.unwrap_or(0),
                sentinel.display()
            );
        }
        Ok(())
    }
}

fn existing_path_is_within_allowed_roots(path: &Path, allowed_roots: &[PathBuf]) -> bool {
    let Ok(candidate) = path.canonicalize() else {
        return true;
    };
    allowed_roots.iter().any(|root| {
        root.canonicalize()
            .is_ok_and(|allowed| candidate == allowed || candidate.starts_with(allowed))
    })
}

fn install_root(root: Option<PathBuf>, global: bool) -> PathBuf {
    root.or_else(|| global.then(platform_home_dir).flatten())
        .unwrap_or_else(|| tokenzero_work_root(None))
}

fn platform_home_dir() -> Option<PathBuf> {
    home_dir_from_env(|name| std::env::var_os(name), cfg!(windows))
}

fn home_dir_from_env<F>(mut var: F, windows: bool) -> Option<PathBuf>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let mut nonempty = |name: &str| var(name).filter(|value| !value.as_os_str().is_empty());
    if windows {
        if let Some(userprofile) = nonempty("USERPROFILE") {
            return Some(PathBuf::from(userprofile));
        }
        if let (Some(mut drive), Some(path)) = (nonempty("HOMEDRIVE"), nonempty("HOMEPATH")) {
            drive.push(path);
            return Some(PathBuf::from(drive));
        }
    }
    nonempty("HOME").map(PathBuf::from)
}

fn parse_mode(value: &str) -> Result<Mode> {
    value.parse::<Mode>().map_err(anyhow::Error::msg)
}

fn normalize_command(command: &[String]) -> Vec<String> {
    let parts = match command {
        [first, rest @ ..] if first == "--" => rest,
        _ => command,
    };
    match parts {
        [part] if !contains_platform_shell_syntax(part, tokenzero_runtime::current_platform()) => {
            split_command_string(part)
        }
        _ => parts.to_vec(),
    }
}

fn content_type_from_kind(kind: &str, text: &str, path: Option<&Path>) -> ContentType {
    match kind {
        "code" => ContentType::Code,
        "shell" | "tool-output" => ContentType::ShellOutput,
        "diff" => ContentType::Diff,
        "json" => ContentType::JsonConfig,
        "markdown" | "pack" => ContentType::Markdown,
        "log" => ContentType::Logs,
        _ => detect_content_type(text, path),
    }
}

fn parse_line_token(value: &str) -> Option<usize> {
    value.trim().trim_start_matches('L').parse().ok()
}

fn expand_selector(args: &ExpandArgs) -> (Option<String>, Option<usize>, Option<usize>) {
    let mut selector = args.selector.clone();
    if args.raw || selector.is_none() {
        selector = Some("raw".into());
    }
    if args.summary {
        selector = Some("summary".into());
    }
    let mut start = args.start_line;
    let mut end = args.end_line;
    if let Some(line) = args.line {
        start = Some(line);
        end = Some(line);
    }
    if let Some(lines) = args.lines.as_deref() {
        let value = lines.trim().trim_start_matches('L');
        if let Some((s, e)) = value.split_once('-') {
            start = s.parse().ok();
            end = e.parse().ok();
        } else {
            start = value.parse().ok();
            end = start;
        }
    }
    if let Some(around) = args.around.as_deref() {
        let (line, radius) = around.split_once(':').unwrap_or((around, "3"));
        let line = parse_line_token(line).unwrap_or(1);
        let radius = radius.parse::<usize>().unwrap_or(3);
        start = Some(line.saturating_sub(radius).max(1));
        end = Some(line + radius);
    }
    (selector, start, end)
}

fn capability_list(
    want_mcp: bool,
    shell: bool,
    instructions: bool,
    cli: bool,
    hooks: bool,
    shims: bool,
    default_mcp_if_empty: bool,
) -> Vec<String> {
    let flags = [
        (want_mcp, "mcp"),
        (shell, "shell"),
        (instructions, "instructions"),
        (cli, "cli"),
        (hooks, "hooks"),
        (shims, "shim"),
    ];
    let mut caps: Vec<String> = flags
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name.to_string())
        .collect();
    if default_mcp_if_empty && caps.is_empty() {
        caps.push("mcp".to_string());
    }
    caps
}

fn install_capabilities(args: &InstallArgs) -> Vec<String> {
    capability_list(
        args.mcp || args.grok || !args.agents.is_empty(),
        args.shell,
        args.instructions,
        args.cli,
        args.hooks,
        args.shims,
        false,
    )
}

fn init_capabilities(args: &InitArgs) -> Vec<String> {
    capability_list(
        args.mcp || !args.agents.is_empty(),
        args.shell,
        args.instructions,
        args.cli,
        args.hooks,
        args.shims,
        true,
    )
}

fn clients_profile(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "standard" | "" => Ok("standard".to_string()),
        "codemode" | "code-mode" => Ok("codemode".to_string()),
        other => anyhow::bail!(
            "unsupported clients profile '{other}'; supported profiles: standard, codemode"
        ),
    }
}

fn clients_capabilities(profile: &str) -> Vec<String> {
    let mut caps = vec!["mcp".to_string(), "hooks".to_string()];
    if profile == "codemode" {
        caps.push("instructions".to_string());
    }
    caps
}

fn clients_mcp_surface(_profile: &str) -> McpToolSurface {
    McpToolSurface::Classic
}
fn parse_mcp_surface(raw: &str) -> Result<McpToolSurface> {
    raw.parse()
        .map_err(|message: String| anyhow::anyhow!(message))
}
fn clients_agent_labels(agents: &[String]) -> Vec<String> {
    if agents.is_empty() {
        vec!["all".to_string()]
    } else {
        agents.to_vec()
    }
}

fn client_status_report(
    root: &Path,
    agents: &[String],
    command: &str,
) -> Result<serde_json::Value> {
    let plan = install::plan_for_agents(
        root,
        true,
        &clients_capabilities("standard"),
        agents,
        install_mcp_surface(McpToolSurface::Classic),
    );
    let surfaces: Vec<serde_json::Value> = plan
        .writes
        .iter()
        .map(|write| client_surface_status(write, root))
        .collect::<Result<_>>()?;
    let count = |state: &str| surfaces.iter().filter(|s| s["state"] == state).count();
    let (installed, mixed, missing) = (count("installed"), count("mixed"), count("missing"));
    let status = if installed > 0 && missing == 0 && mixed == 0 {
        "installed"
    } else if installed > 0 || mixed > 0 {
        "mixed"
    } else {
        "missing"
    };
    Ok(
        json!({"schema_version": "tokenzero.clients.v1", "status": status, "ok": status == "installed", "exit_code": 0, "command": format!("clients {command}"), "root": path_display(root), "global": true, "profile": "standard", "agents": clients_agent_labels(agents), "summary": {"installed": installed, "mixed": mixed, "missing": missing, "total": surfaces.len(), "raw_bypass_risk": status != "installed"}, "surfaces": surfaces, "next_action": if status == "installed" {"Run tokenzero doctor --json to verify runtime health."} else {"Run tokenzero clients plan --profile standard --json to review the read-only integration plan."}}),
    )
}

fn client_surface_status(write: &install::InstallWrite, root: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(install::inspect_client_surface(
        write, root,
    ))?)
}

fn install_agents(raw_agents: &[String], grok: bool) -> Result<Vec<String>> {
    let mut agents = Vec::new();
    if grok {
        push_agent(&mut agents, "grok")?;
    }
    for raw in raw_agents {
        push_agent(&mut agents, raw)?;
    }
    Ok(agents)
}

const AGENT_ALIASES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("claude-code", "claude"),
    ("claude-desktop", "claude"),
    ("codex", "codex"),
    ("cursor", "cursor"),
    ("droid", "droid"),
    ("factory-droid", "droid"),
    ("factory", "factory"),
    ("gemini", "gemini"),
    ("grok", "grok"),
    ("opencode", "opencode"),
    ("open-code", "opencode"),
];

fn push_agent(agents: &mut Vec<String>, raw: &str) -> Result<()> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if normalized == "all" {
        return Ok(());
    }
    if normalized.is_empty() {
        anyhow::bail!("--agent requires a non-empty agent name");
    }
    let Some((_, agent)) = AGENT_ALIASES.iter().find(|(alias, _)| *alias == normalized) else {
        anyhow::bail!(
            "unsupported agent '{normalized}'; expected one of claude, codex, cursor, droid, factory, gemini, grok, opencode, or all"
        );
    };
    if !agents.iter().any(|existing| existing == *agent) {
        agents.push((*agent).to_string());
    }
    Ok(())
}

fn emit(value: EmitResponse) -> Result<()> {
    let complete_read_source = value.complete_read_source;
    let mut responses = value.responses;
    if responses.len() == 1 {
        let response = responses
            .pop()
            .ok_or_else(|| anyhow::anyhow!("internal error: command produced no response"))?;
        return emit_with_json_options(response, value.json, complete_read_source);
    }
    if responses.is_empty() {
        anyhow::bail!("internal error: command produced no response");
    }

    let exit_error = responses.iter().any(|response| response.status == "error");
    if value.json {
        let values = responses
            .iter()
            .map(|response| serde_json::from_str::<serde_json::Value>(&cli_json(response)))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        writeln_stdout(serde_json::to_string_pretty(&values)?)?;
    } else {
        for response in &responses {
            if response.tool == "expand" && response.status == "ok" {
                if let Some(visible) = &response.visible {
                    write_stdout(&visible.text)?;
                }
            } else {
                write_stdout(&render_cli_text_options(response, complete_read_source))?;
            }
        }
    }
    if exit_error {
        std::process::exit(1);
    }
    Ok(())
}

fn render_cli_text_options(response: &ToolResponse, complete_read_source: bool) -> String {
    let rendered = if complete_read_source {
        tokenzero_engine::render::render_text_with_complete_read(response)
    } else {
        render_text(response)
    };
    let Some(telemetry) = response
        .telemetry
        .as_ref()
        .filter(|_| response.tool == "shell")
    else {
        return rendered;
    };
    if telemetry
        .get("output_strategy")
        .and_then(serde_json::Value::as_str)
        != Some("inline_shell")
    {
        return rendered;
    }

    let mut out = String::new();
    if telemetry
        .pointer("/stdout_capture/bytes")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|bytes| bytes > 0)
    {
        out.push_str("stdout:\n");
    }
    out.push_str(&rendered);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(exit_code) = telemetry
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
    {
        out.push_str(&format!("exit_code: {exit_code}\n"));
    }
    out
}

fn emit_with_json(response: ToolResponse, as_json: bool) -> Result<()> {
    emit_with_json_options(response, as_json, false)
}

fn emit_with_json_options(
    response: ToolResponse,
    as_json: bool,
    complete_read_source: bool,
) -> Result<()> {
    let exit_error = response.status == "error";
    if as_json {
        writeln_stdout(cli_json(&response))?;
    } else if response.tool == "expand" && response.status == "ok" {
        if let Some(visible) = &response.visible {
            write_stdout(&visible.text)?;
        }
    } else {
        write_stdout(&render_cli_text_options(&response, complete_read_source))?;
    }
    if exit_error {
        std::process::exit(1);
    }
    // nt0i: text mode always mirrors the child; JSON mode historically keeps
    // exit 0 (machine consumers read telemetry.command_success). The opt-in
    // gate mirrors the child in JSON mode too, for harnesses that gate on the
    // process exit code. Default flip rides the 1cwf envelope contract bump.
    if !as_json || run_child_exit_enabled() {
        if let Some(code) = child_failure_exit_code(&response) {
            std::process::exit(code);
        }
    }
    Ok(())
}

/// nt0i (1cwf flip): --json `run` mirrors the child exit code by default so
/// harnesses gating on process exit observe child failure. Set
/// TOKENZERO_RUN_CHILD_EXIT=0/off/false/no to keep the legacy exit-0 envelope
/// contract; envelope content is unchanged either way (status/telemetry stay
/// truthful: `telemetry.command_success` and `telemetry.exit_code`).
pub fn run_child_exit_enabled() -> bool {
    std::env::var("TOKENZERO_RUN_CHILD_EXIT")
        .map(|raw| {
            !matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
        .unwrap_or(true)
}

/// `run` mirrors the child's exit status so `&&`/`||` chains and CI wrappers
/// observe failures; --json does the same unless explicitly opted out.
fn child_failure_exit_code(response: &ToolResponse) -> Option<i32> {
    if response.tool != "shell" {
        return None;
    }
    let telemetry = response.telemetry.as_ref()?;
    if telemetry
        .get("command_success")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        return None;
    }
    match telemetry
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
    {
        Some(0) => None,
        Some(code) => Some(code.clamp(1, 255) as i32),
        None => Some(1),
    }
}

fn emit_value<T: serde::Serialize>(value: T, _as_json: bool) -> Result<()> {
    let json_value = serde_json::to_value(value)?;
    print_pretty(&json_value)?;
    exit_if_nonzero(doctor_exit_code(&json_value));
    Ok(())
}

mod audits;
use audits::bench::*;
use audits::os_reach::*;
use audits::recovery::*;
use audits::release::*;
use audits::shared::*;

#[cfg(test)]
#[path = "../../../../tests/tokenzero/unit/tokenzero/main_package_audit_dist_tests.rs"]
mod package_audit_dist_tests;
