use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use graphzero_store::store::publish::{PublishOptions, publish_batch};

use crate::agent_cli;
use crate::agent_errors;
use crate::agent_output;
use crate::agent_subcommand_hints;
use crate::blast_tools;
use crate::cli_args::{
    Cli, Command, DaemonAction, ExportFormat as CliExportFormat, IngestCommand, ReserveCommand,
    RobotDocsAction, TelemetryCommand, WhyCommand, agent_json_mode, agent_typo_extra_hint,
    cli_command, json_version_mode, print_version, version_requested,
};
use crate::commands;
use crate::daemon;
use crate::mcp;
use crate::pack_cmd;
use crate::packaging::{
    CLIENT_CONFIG_FILE, PackageSurface, assert_server_surface_boundary, default_install_prefix,
    install_state_path, install_surface, resolve_startup_surface, sbom_document, uninstall_report,
    uninstall_surface, uninstall_surface_dry_run,
};
use crate::query_surface_tools;
use crate::reserve_tools;
use tempfile::NamedTempFile;

fn resolve_intent_ops(intent: &str) -> Result<Vec<graphzero_reserve::IntentOperation>> {
    let path = Path::new(intent);
    if let Some(ops) = path
        .extension()
        .filter(|_| path.exists())
        .and_then(|_| reserve_tools::intent_ops_from_intent_file(path).ok())
    {
        return Ok(ops);
    }
    if let Some(ops) = serde_json::from_str::<serde_json::Value>(intent)
        .ok()
        .and_then(|v| reserve_tools::parse_intent_ops(&v).ok())
    {
        return Ok(ops);
    }
    Ok(vec![graphzero_reserve::IntentOperation {
        kind: "change_signature".to_string(),
        target_symbol: Some(intent.to_string()),
        intent_text: Some(intent.to_string()),
    }])
}

fn run_reserve(action: ReserveCommand, repo: PathBuf) -> Result<()> {
    let (root, repo) = commands::repo_pair(repo)?;
    let out = match action {
        ReserveCommand::Declare {
            agent,
            intent,
            ttl,
            json: _,
        } => {
            let ops = resolve_intent_ops(&intent)?;
            reserve_tools::run_declare(&root, &repo, &agent, ops, ttl)?
        }
        ReserveCommand::Check {
            agent,
            intent,
            acquire,
            ttl,
            json: _,
        } => {
            let ops = resolve_intent_ops(&intent)?;
            reserve_tools::run_check(&root, &repo, &agent, &ops, acquire, ttl)?
        }
        ReserveCommand::Release {
            agent,
            reservation_id,
            json: _,
        } => reserve_tools::run_release(&root, &repo, &agent, &reservation_id)?,
        ReserveCommand::Query { json: _ } => reserve_tools::run_query(&root, &repo)?,
    };
    println!("{out}");
    Ok(())
}

fn cli_domain_ctx(repo: PathBuf) -> Result<(graphzero_engine::EngineContext, PathBuf, PathBuf)> {
    let (root, repo) = commands::repo_pair(repo)?;
    let ctx = graphzero_engine::EngineContext::for_paths(
        repo.clone(),
        root.clone(),
        graphzero_engine::AdapterKind::Cli,
    );
    Ok((ctx, root, repo))
}

#[tracing::instrument(skip_all, fields(path = %path.display()))]
fn run_index(path: PathBuf) -> Result<()> {
    // commands::index::run is a thin CLI facade over the same domain dispatcher.
    let out = commands::index::run(&path)?;
    let mut data = serde_json::json!({
        "snapshot": out.snapshot,
        "shards": out.shards,
        "store": out.store,
    });
    if let Some(phases) = out.phases {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("phases".into(), phases);
        }
    }
    agent_output::emit_agent_json("index", data);
    Ok(())
}

fn run_snap(
    symbol: String,
    budget: usize,
    repo: PathBuf,
    export: Option<PathBuf>,
    cli_format: CliExportFormat,
) -> Result<()> {
    if symbol.trim().is_empty() {
        let repo_hint = repo.display().to_string();
        eprintln!(
            "{}",
            agent_errors::agent_error_json(
                "empty symbol",
                "Pass a symbol name: graphzero snap <SYMBOL> --budget 1",
                serde_json::json!({ "example": format!("graphzero snap <SYMBOL> --repo {repo_hint}") }),
            )
        );
        std::process::exit(1);
    }
    let (ctx, _root, _repo) = cli_domain_ctx(repo)?;
    let mut args = serde_json::json!({ "query": symbol, "budget": budget });
    if let Some(export_path) = &export {
        if let Some(obj) = args.as_object_mut() {
            obj.insert(
                "export_path".into(),
                serde_json::Value::String(export_path.display().to_string()),
            );
            obj.insert(
                "format".into(),
                serde_json::Value::String(cli_format.as_str().to_string()),
            );
        }
    }
    let result = graphzero_engine::dispatch(&ctx, "snap", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;

    // Transport framing only: choose stdout shape from domain result.
    if export.is_some() {
        let meta = serde_json::json!({
            "exported": result.value.get("exported").cloned().unwrap_or(serde_json::Value::Null),
            "ref": result.value.get("export_ref").or_else(|| result.value.get("ref")).cloned().unwrap_or(serde_json::Value::Null),
            "size_bytes": result.value.get("export_size").or_else(|| result.value.get("size_bytes")).cloned().unwrap_or(serde_json::Value::Null),
            "format": result.value.get("export_format").or_else(|| result.value.get("format")).cloned().unwrap_or(serde_json::json!(cli_format.as_str())),
            "query": result.value.get("query").cloned().unwrap_or(serde_json::json!(symbol)),
            "snapshot_id": result.value.get("snapshot_id").cloned().unwrap_or(serde_json::Value::Null),
        });
        agent_output::emit_agent_json("snap-export", meta);
        return Ok(());
    }

    // Domain already returned a JSON Value. Never re-parse it as agent stdin
    // JSON: budget=1 snaps are often a JSON string (bare q:/gz:// ref), and
    // stripping quotes then calling emit_agent_json_from_str mislabels an
    // internal success as invalid_agent_json on stdout with exit 0.
    agent_output::emit_agent_json("snap", result.value);
    Ok(())
}

fn run_remember(
    text: String,
    anchors: Vec<String>,
    kind: Option<String>,
    supersedes: Vec<String>,
    repo: PathBuf,
) -> Result<()> {
    let (ctx, _root, _repo) = cli_domain_ctx(repo)?;
    let args = serde_json::json!({
        "text": text,
        "anchors": anchors,
        "kind": kind,
        "supersedes": supersedes,
    });
    let result = graphzero_engine::dispatch(&ctx, "remember", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    println!("{}", result.value);
    Ok(())
}

fn run_recall(target: String, budget: usize, repo: PathBuf) -> Result<()> {
    let (ctx, _root, _repo) = cli_domain_ctx(repo)?;
    let args = serde_json::json!({ "target": target, "query": target, "budget": budget });
    let result = graphzero_engine::dispatch(&ctx, "recall", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let text = match &result.value {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other)?,
    };
    println!("{text}");
    Ok(())
}

fn run_expand(reference: String, repo: PathBuf) -> Result<()> {
    let (ctx, _root, repo) = cli_domain_ctx(repo)?;
    let repo_display = repo.display().to_string();
    let args = serde_json::json!({ "reference": reference });
    match graphzero_engine::dispatch(&ctx, "expand", &args) {
        Ok(result) => {
            if let Some(text) = result.value.get("text").and_then(|v| v.as_str()) {
                std::io::stdout().write_all(text.as_bytes())?;
            } else {
                std::io::stdout().write_all(serde_json::to_string(&result.value)?.as_bytes())?;
            }
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{}",
                agent_errors::enrich_cli_error_message(&e.message, &repo_display)
            );
            std::process::exit(1);
        }
    }
}

fn run_daemon(action: DaemonAction, repo: PathBuf, yes: bool) -> Result<()> {
    match action {
        DaemonAction::Run => daemon::run_foreground(&repo)?,
        DaemonAction::Enable => {
            if !yes {
                let preview = serde_json::json!({
                    "dry_run": true,
                    "action": "daemon_enable",
                    "repo": repo.display().to_string(),
                    "would": [
                        "write enabled state under resolved GraphZero store",
                        "spawn detached `graphzero daemon run` with stdout/stderr discarded"
                    ],
                    "hint": "Re-run with --yes to confirm (agents: non-interactive opt-in)",
                    "mutates": false
                });
                agent_output::emit_agent_json("daemon", preview);
                return Ok(());
            }
            daemon::handle("enable", &repo)?
        }
        DaemonAction::Disable => daemon::handle("disable", &repo)?,
        DaemonAction::Status => daemon::handle("status", &repo)?,
    }
    Ok(())
}

fn run_query_surface(
    surface: String,
    name: Option<String>,
    query: Option<String>,
    path: Option<String>,
    budget: usize,
    repo: PathBuf,
) -> Result<()> {
    let (ctx, _root, repo) = cli_domain_ctx(repo)?;
    let surface = query_surface_tools::normalize_agent_surface(&surface);
    let repo_display = repo.display().to_string();
    // Validate via the router's parser, not SURFACE_NAMES: the parser is the
    // single source of truth and also accepts aliases (e.g. "orient" ->
    // symbol). SURFACE_NAMES stays catalog-only.
    if graphzero_engine::query_surface::QuerySurface::parse_surface(&surface).is_none() {
        eprintln!(
            "{}",
            agent_errors::enrich_cli_error_message(
                &format!("unknown surface {surface}"),
                &repo_display,
            )
        );
        std::process::exit(1);
    }
    let mut args = serde_json::Map::new();
    args.insert("surface".into(), serde_json::Value::String(surface.clone()));
    if let Some(n) = name {
        args.insert("name".into(), serde_json::Value::String(n));
    }
    if let Some(q) = query {
        args.insert("query".into(), serde_json::Value::String(q));
    }
    if let Some(p) = path {
        args.insert("path".into(), serde_json::Value::String(p));
    }
    args.insert("budget".into(), serde_json::json!(budget));
    match graphzero_engine::dispatch(&ctx, &surface, &serde_json::Value::Object(args)) {
        Ok(result) => {
            let out = match &result.value {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other)?,
            };
            agent_output::emit_agent_json_from_str(&surface, &out);
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{}",
                agent_errors::enrich_cli_error_message(&e.message, &repo_display)
            );
            std::process::exit(1);
        }
    }
}

fn run_compact(repo: PathBuf) -> Result<()> {
    println!("{}", commands::compact::run(&repo)?);
    Ok(())
}

fn run_ingest_scip(scip_file: PathBuf, repo: PathBuf) -> Result<()> {
    let (root, repo) = commands::repo_pair(repo)?;
    let (entry, edges, tier_b) = graphzero_scip::ingest_scip_publish(&repo, &root, &scip_file)?;
    println!(
        "{{\"snapshot\":{},\"tier_b_edges\":{},\"tier_b_blobs\":{}}}",
        entry.snapshot_id, edges, tier_b
    );
    Ok(())
}

fn run_stats(repo: PathBuf) -> Result<()> {
    println!(
        "{}",
        commands::stats::to_json(&commands::stats::collect(&repo)?)
    );
    Ok(())
}

const CODEMODE_RETIRED: &str = "graphzero code-mode is retired. Model execution is ZeroKernel (`z.find`, `z.read`). Operator structural CLI remains `graphzero index|orient|search|snap|blast`.";

fn run_codemode(_plan: String, _repo: PathBuf) -> Result<()> {
    anyhow::bail!(CODEMODE_RETIRED)
}

fn run_neighborhood_command(
    repo: PathBuf,
    seed: Vec<String>,
    hops: u32,
    budget: usize,
) -> Result<()> {
    let repo_root = commands::paths::canonical_repo(&repo)?;
    let store_root = commands::paths::store_root(&repo_root);
    let json = blast_tools::run_neighborhood(&store_root, &repo_root, &seed, hops, budget)?;
    println!("{json}");
    Ok(())
}

/// Write `payload` atomically to `path` using a temp-file + rename.
fn write_atomic_export(path: &Path, payload: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create parent dirs for {}", path.display()))?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(payload)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Owned arguments for the `blast` command, mirroring `Command::Blast`.
struct BlastCmd {
    intent: String,
    budget: usize,
    depth: u32,
    repo: PathBuf,
    world_ref: Option<String>,
    world_envelope: Option<String>,
    planned_edit: Vec<String>,
    focus: Vec<String>,
    export: Option<PathBuf>,
    cli_format: CliExportFormat,
}

fn run_blast(cmd: BlastCmd) -> Result<()> {
    let BlastCmd {
        intent,
        budget,
        depth,
        repo,
        world_ref,
        world_envelope,
        planned_edit,
        focus,
        export,
        cli_format,
    } = cmd;
    let (root, repo) = commands::repo_pair(repo)?;
    let out = if let Some(world_ref) = &world_ref {
        blast_tools::run_speculative_blast(
            &root,
            &repo,
            &intent,
            budget,
            world_ref,
            world_envelope.as_deref(),
            &planned_edit,
            &focus,
        )?
    } else if let Some(envelope) = &world_envelope {
        blast_tools::run_speculative_blast(
            &root,
            &repo,
            &intent,
            budget,
            "",
            Some(envelope),
            &planned_edit,
            &focus,
        )?
    } else {
        blast_tools::run_blast(&root, &repo, &intent, budget, depth)?
    };

    if let Some(export_path) = &export {
        // For blast, treat the json out as capsule payload for now (or convert to minimal meta)
        // Re-use store export if possible; fallback to direct atomic for string out
        // Framing: write blast JSON payload (no re-execution).
        let payload = if matches!(cli_format, CliExportFormat::Minimal) {
            // tiny wrapper
            format!(
                "{{\"schema\":\"gz-snap/v1\",\"ref\":\"blast:{}\",\"size_bytes\":{}}}",
                intent,
                out.len()
            )
        } else {
            out.clone()
        };
        write_atomic_export(export_path, payload.as_bytes())?;
        let meta = serde_json::json!({
            "exported": export_path.display().to_string(),
            "ref": format!("blast:{}", intent),
            "size_bytes": payload.len(),
            "format": cli_format.as_str(),
            "intent": intent,
        });
        agent_output::emit_agent_json("blast-export", meta);
        return Ok(());
    }

    agent_output::emit_agent_json_from_str("blast", &out);
    Ok(())
}

fn run_publish(
    file: PathBuf,
    capability: Option<String>,
    unsafe_allow_anonymous_publish: bool,
    repo: PathBuf,
) -> Result<()> {
    let (root, repo) = commands::repo_pair(repo)?;
    let raw = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
    let cap = capability
        .or_else(|| std::env::var("GRAPHZERO_PUBLISH_TOKEN").ok())
        .map(|s| s.to_string());
    let cap_ref = cap.as_deref();
    let opts = PublishOptions {
        store_root: &root,
        repo_root: Some(&repo),
        capability: cap_ref,
        allow_anonymous: unsafe_allow_anonymous_publish,
    };
    match publish_batch(&raw, &opts) {
        Ok(ack) => {
            println!(
                "{{\"ok\":true,\"edges_accepted\":{},\"segment_id\":{},\"snapshot_id\":{}}}",
                ack.edges_accepted, ack.segment_id, ack.snapshot_id
            );
        }
        Err(e) => {
            eprintln!("{}", e.to_json());
            std::process::exit(1);
        }
    }
    Ok(())
}

fn run_verify(
    target: Option<String>,
    claim: Option<String>,
    repo: PathBuf,
    claims_file: Option<PathBuf>,
) -> Result<()> {
    if let Some(path) = claims_file {
        let out = commands::verify::verify_pr_claims_json(&repo, &path)?;
        let verified = serde_json::from_str::<serde_json::Value>(&out)
            .ok()
            .and_then(|v| v.get("verified").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        println!("{out}");
        if !verified {
            std::process::exit(1);
        }
        return Ok(());
    }
    let target = target.context("verify requires TARGET unless --claims-file is provided")?;
    let claim = claim.context("verify requires --claim unless --claims-file is provided")?;
    let out = commands::verify::verify_json(&repo, &target, &claim)?;
    let verified = serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("verified").and_then(|b| b.as_bool()))
        .unwrap_or(false);
    println!("{out}");
    if !verified {
        std::process::exit(1);
    }
    Ok(())
}

fn run_doctor(repo: PathBuf) -> Result<()> {
    let repo = commands::canonical_repo(repo)?;
    agent_output::emit_agent_json_from_str("doctor", &agent_cli::doctor_json(&repo));
    Ok(())
}

fn run_install(
    surface: String,
    prefix: Option<PathBuf>,
    binary: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let surface = PackageSurface::parse(&surface).map_err(anyhow::Error::msg)?;
    let prefix = prefix.unwrap_or_else(default_install_prefix);
    let binary = binary.unwrap_or_else(|| {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from(surface.artifact_name()))
    });
    if dry_run {
        agent_output::emit_agent_json(
            "install",
            serde_json::json!({
                "dry_run": true,
                "surface": surface.as_str(),
                "artifact": surface.artifact_name(),
                "prefix": prefix.display().to_string(),
                "binary_path": binary.display().to_string(),
                "would_write": [
                    install_state_path(&prefix).display().to_string(),
                    prefix.join(CLIENT_CONFIG_FILE).display().to_string(),
                    prefix.join("shim-target").display().to_string(),
                ],
                "mutates": false,
            }),
        );
        return Ok(());
    }
    let state = install_surface(surface, &prefix, &binary).map_err(anyhow::Error::msg)?;
    agent_output::emit_agent_json(
        "install",
        serde_json::json!({
            "ok": true,
            "surface": state.surface.as_str(),
            "artifact": state.artifact,
            "prefix": state.prefix,
            "binary_path": state.binary_path,
            "semantic_contract_digest": state.semantic_contract_digest,
            "client_config": state.client_config,
            "platform": state.platform,
        }),
    );
    Ok(())
}

fn run_uninstall(prefix: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let prefix = prefix.unwrap_or_else(default_install_prefix);
    if dry_run {
        let preview = uninstall_surface_dry_run(&prefix).map_err(anyhow::Error::msg)?;
        agent_output::emit_agent_json("uninstall", preview);
        return Ok(());
    }
    let prev = uninstall_surface(&prefix).map_err(anyhow::Error::msg)?;
    agent_output::emit_agent_json("uninstall", uninstall_report(prev));
    Ok(())
}

fn run_sbom(surface: Option<String>) -> Result<()> {
    let surface = match surface {
        Some(s) => PackageSurface::parse(&s).map_err(anyhow::Error::msg)?,
        None => resolve_startup_surface(&std::env::args().collect::<Vec<_>>())
            .unwrap_or(PackageSurface::Mcp),
    };
    agent_output::emit_agent_json("sbom", sbom_document(surface));
    Ok(())
}

fn run_telemetry(action: TelemetryCommand) -> Result<()> {
    match action {
        TelemetryCommand::Inspect {
            telemetry,
            no_telemetry,
            repo,
        } => {
            let (store, _repo) = commands::repo_pair(repo)?;
            let env_value = std::env::var(graphzero_store::TELEMETRY_ENV).ok();
            let config = graphzero_store::load_telemetry_config(&store);
            let enabled = graphzero_store::resolve_telemetry(
                telemetry,
                no_telemetry,
                config,
                env_value.as_deref(),
            );
            let inspection = graphzero_store::inspect_telemetry(&store, enabled)
                .with_context(|| format!("inspect telemetry under {}", store.display()))?;
            // Truthful no-export: even when enabled, GraphZero has no exporter.
            let _ = graphzero_store::export_shareable_telemetry(&inspection);
            agent_output::emit_agent_json(
                "telemetry-inspect",
                graphzero_store::inspection_json(&inspection),
            );
            Ok(())
        }
    }
}

fn maybe_print_version() -> bool {
    if version_requested() {
        print_version(json_version_mode());
        true
    } else {
        false
    }
}

fn emit_default_triage() -> Result<()> {
    let cwd = std::env::current_dir().context("cwd")?;
    agent_output::emit_agent_json_from_str(
        "graphzero",
        &agent_cli::agent_triage_json(&cwd.display().to_string()),
    );
    Ok(())
}

pub fn handle_pre_parse_shortcuts() -> Result<bool> {
    if maybe_print_version() {
        return Ok(true);
    }
    let args: Vec<String> = std::env::args().collect();
    // Dual mode on argv always fails closed (even on the compatibility shim).
    crate::packaging::modes_from_args(&args).map_err(anyhow::Error::msg)?;
    crate::packaging::reject_dual_env_selection().map_err(anyhow::Error::msg)?;

    let mode = match args.as_slice() {
        [_, arg] => arg.strip_prefix("--mode="),
        [_, flag, mode] if flag == "--mode" => Some(mode.as_str()),
        _ => None,
    };
    if let Some(mode) = mode {
        // Server entry only: resolve surface + exclusivity (shim cannot host servers).
        let surface = resolve_startup_surface(&args).map_err(anyhow::Error::msg)?;
        assert_server_surface_boundary(surface).map_err(anyhow::Error::msg)?;
        let server_mode = mcp::ServerMode::parse(mode)?;
        match (surface, server_mode) {
            (PackageSurface::Mcp, mcp::ServerMode::Mcp) => {
                #[cfg(feature = "surface-mcp")]
                {
                    crate::fastmcp_mode::run();
                }
                #[cfg(not(feature = "surface-mcp"))]
                {
                    anyhow::bail!(
                        "graphzero: FastMCP is not compiled into this operator CLI. Model execution is ZeroKernel (`z.find`)."
                    );
                }
            }
            (PackageSurface::Codemode, mcp::ServerMode::Mcp) => {
                anyhow::bail!(
                    "graphzero: artifact surface is 'codemode'; refused --mode=mcp. Install graphzero-mcp (mutually exclusive)."
                );
            }
        }
        return Ok(true);
    }
    if std::env::args_os().nth(1).is_none()
        || std::env::args()
            .skip(1)
            .all(|a| a == "--json" || a == "--agent")
    {
        emit_default_triage()?;
        return Ok(true);
    }
    crate::dispatch_help::maybe_rewrite_help_to_subcommand()
}

fn is_display_help_or_version(error: &clap::Error) -> bool {
    error.kind() == clap::error::ErrorKind::DisplayHelp
        || error.kind() == clap::error::ErrorKind::DisplayVersion
}

fn emit_cli_parse_error(msg: &str, repo: &str, example: Option<&str>) {
    if let Some(ex) = example {
        if agent_json_mode() {
            eprintln!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "error": msg,
                    "hint": ex,
                    "example": ex,
                }))
                .unwrap_or_else(|_| msg.to_string())
            );
        } else {
            eprintln!("{msg}");
            eprintln!("hint: try: {ex}");
        }
    } else if agent_json_mode() {
        eprintln!("{}", agent_errors::enrich_cli_error_message(msg, repo));
    } else {
        eprintln!("{msg}");
        eprintln!(
            "hint: graphzero agent-triage | graphzero orient --surface symbol --name <sym> | graphzero search --query <q>"
        );
    }
}

fn exit_cli_parse_error(error: clap::Error) -> ! {
    if is_display_help_or_version(&error) {
        error.print().ok();
        std::process::exit(0);
    }
    let msg = error.to_string();
    let repo = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    let example = agent_typo_extra_hint(&msg)
        .map(str::to_string)
        .or_else(|| agent_subcommand_hints::subcommand_hint_from_parse_error(&msg));
    emit_cli_parse_error(&msg, &repo, example.as_deref());
    // Usage / clap parse failures are exit 2 (capabilities.exit_codes).
    std::process::exit(2);
}

pub fn parse_cli_or_exit() -> Cli {
    use clap::FromArgMatches;
    // Apply NO_COLOR / CI before clap parse so help/errors are not colored.
    let cmd = cli_command();
    match cmd.try_get_matches() {
        Ok(matches) => match Cli::from_arg_matches(&matches) {
            Ok(cli) => cli,
            Err(error) => exit_cli_parse_error(error),
        },
        Err(error) => exit_cli_parse_error(error),
    }
}

fn dispatch_primary_command(command: Command) -> Result<()> {
    match command {
        Command::Reserve { action, repo } => run_reserve(action, repo),
        Command::Index { path, repo } => run_index(repo.unwrap_or(path)),
        Command::Snap {
            symbol,
            budget,
            repo,
            export,
            format,
        } => run_snap(symbol, budget, repo, export, format),
        Command::Remember {
            text,
            anchors,
            kind,
            supersedes,
            repo,
        } => run_remember(text, anchors, kind, supersedes, repo),
        Command::Recall {
            target,
            budget,
            repo,
        } => run_recall(target, budget, repo),
        Command::Expand { reference, repo } => run_expand(reference, repo),
        Command::Daemon { action, repo, yes } => run_daemon(action, repo, yes),
        Command::QuerySurface {
            surface,
            name,
            query,
            path,
            budget,
            repo,
        } => run_query_surface(surface, name, query, path, budget, repo),
        Command::Serve => {
            // `serve` is FastMCP-only; shim and codemode-only packages fail closed.
            #[cfg(feature = "surface-mcp")]
            {
                assert_server_surface_boundary(PackageSurface::Mcp).map_err(anyhow::Error::msg)?;
                crate::fastmcp_mode::run()
            }
            #[cfg(not(feature = "surface-mcp"))]
            {
                Err(anyhow::anyhow!(
                    "graphzero serve is not an operator CLI surface. Model execution is ZeroKernel (`z.find`)."
                ))
            }
        }
        Command::Compact { repo } => run_compact(repo),
        other => dispatch_secondary_command(other),
    }
}

fn dispatch_secondary_command(command: Command) -> Result<()> {
    match command {
        Command::Ingest { action } => match action {
            IngestCommand::Scip { scip_file, repo } => run_ingest_scip(scip_file, repo),
        },
        Command::Stats { repo } => run_stats(repo),
        Command::Why { action } => match action {
            WhyCommand::Ingest { repo, fixtures } => {
                let repo = crate::why_cmd::repo_canonical(&repo)?;
                crate::why_cmd::run_ingest(&repo, fixtures.as_deref())
            }
            WhyCommand::Status { repo } => {
                let repo = crate::why_cmd::repo_canonical(&repo)?;
                crate::why_cmd::run_status(&repo)
            }
            WhyCommand::Replay { repo, fixtures } => {
                let repo = crate::why_cmd::repo_canonical(&repo)?;
                crate::why_cmd::run_replay(&repo, &fixtures)
            }
            WhyCommand::EvidenceCheck { repo } => {
                let repo = crate::why_cmd::repo_canonical(&repo)?;
                crate::why_cmd::run_evidence_check(&repo)
            }
        },
        Command::Neighborhood {
            seed,
            hops,
            budget,
            repo,
        } => run_neighborhood_command(repo, seed, hops, budget),
        Command::Blast {
            intent,
            budget,
            depth,
            repo,
            world_ref,
            world_envelope,
            planned_edit,
            focus,
            export,
            format,
        } => run_blast(BlastCmd {
            intent,
            budget,
            depth,
            repo,
            world_ref,
            world_envelope,
            planned_edit,
            focus,
            export,
            cli_format: format,
        }),
        Command::Publish {
            file,
            capability,
            unsafe_allow_anonymous_publish,
            repo,
        } => run_publish(file, capability, unsafe_allow_anonymous_publish, repo),
        Command::Pack { action } => pack_cmd::run(action),
        Command::ZerorefFixture { action } => crate::zeroref_fixture::run(action),
        Command::CodeMode { plan, repo } => run_codemode(plan, repo),
        Command::CodeModeSearch { query: _ } => anyhow::bail!(CODEMODE_RETIRED),
        Command::CodeModeDescribe { name: _ } => anyhow::bail!(CODEMODE_RETIRED),
        Command::Capabilities => {
            agent_output::emit_agent_json_from_str("capabilities", &agent_cli::capabilities_json());
            Ok(())
        }
        Command::RobotDocs { action } => {
            let action = action.unwrap_or(RobotDocsAction::Guide);
            match action {
                RobotDocsAction::Guide => {
                    if agent_json_mode() {
                        agent_output::emit_agent_json_from_str(
                            "robot-docs",
                            &agent_cli::robot_docs_json(),
                        );
                    } else {
                        print!("{}", agent_cli::robot_docs_guide());
                    }
                    Ok(())
                }
            }
        }
        other => dispatch_tertiary_command(other),
    }
}

fn dispatch_tertiary_command(command: Command) -> Result<()> {
    match command {
        Command::AgentTriage { repo } => {
            let repo = commands::canonical_repo(repo)?;
            agent_output::emit_agent_json_from_str(
                "agent-triage",
                &agent_cli::agent_triage_json(&repo.display().to_string()),
            );
            Ok(())
        }
        Command::Doctor { repo } => run_doctor(repo),
        Command::Install {
            surface,
            prefix,
            binary,
            dry_run,
        } => run_install(surface, prefix, binary, dry_run),
        Command::Uninstall { prefix, dry_run } => run_uninstall(prefix, dry_run),
        Command::Sbom { surface } => run_sbom(surface),
        Command::Telemetry { action } => run_telemetry(action),
        Command::Orient {
            surface,
            name,
            positional_name,
            query,
            path,
            budget,
            repo,
            export: _export,
            format: _format,
        } => {
            let name = name.or(positional_name);
            run_query_surface(surface, name, query, path, budget, repo)
        }
        Command::Search {
            query,
            positional_query,
            budget,
            repo,
            export: _export,
            format: _format,
        } => {
            let query = query.or(positional_query).ok_or_else(|| {
                anyhow::anyhow!(
                    "missing argument query (try: graphzero search --query <TEXT> or graphzero search <TEXT>)"
                )
            })?;
            run_query_surface("search".into(), None, Some(query), None, budget, repo)
        }
        Command::Verify {
            target,
            claim,
            repo,
            claims_file,
        } => run_verify(target, claim, repo, claims_file),
        Command::Symbol {
            name,
            budget,
            repo,
            export: _export,
            format: _format,
        } => run_query_surface("symbol".into(), Some(name), None, None, budget, repo),
        command => anyhow::bail!(
            "unhandled command category: {:?}",
            std::mem::discriminant(&command)
        ),
    }
}

pub fn run() -> Result<()> {
    if handle_pre_parse_shortcuts()? {
        return Ok(());
    }
    let cli = parse_cli_or_exit();
    agent_output::set_json_envelope(cli.json || agent_json_mode());
    dispatch_primary_command(cli.command)
}
