use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zerostack_shared_tests::fake_substrate::fake_mcp_main;
use zerostack_shared_tests::{
    CompletionStatus, ConformanceReport, Ns, RunConfig, Surface, production_provenance,
    run_conformance, valid_head,
};

/// Infer the served surface from the artifact filename.
///
/// Engines ship `<name>-codemode` (raw worker), `<name>-planner` (planner
/// host), and `<name>-mcp` (MCP server), so the filename carries the
/// install-time choice. Returns None for anything else.
fn infer_surface(bin: &std::path::Path) -> Option<Surface> {
    let stem = bin.file_name()?.to_string_lossy().to_ascii_lowercase();
    if stem.contains("planner") || stem.contains("plan-mode") {
        return Some(Surface::Planner);
    }
    if stem.contains("codemode") || stem.contains("code-mode") {
        return Some(Surface::Codemode);
    }
    if stem.contains("mcp") {
        return Some(Surface::Mcp);
    }
    None
}

#[derive(Debug, Parser)]
#[command(name = "zerostack-codemode-conformance")]
#[command(
    about = "Run ZeroStack conformance: plan G1-G10 (planner), raw-worker RW1-RW10 (codemode), or G1 exposure (mcp)"
)]
struct Args {
    /// Substrate namespace: fz, tz, or gz.
    #[arg(long)]
    ns: Option<String>,

    /// The installed substrate artifact under test.
    #[arg(long)]
    bin: Option<PathBuf>,

    /// Which surface that artifact serves: 'planner', 'codemode', or 'mcp'.
    ///
    /// Distinct layers: planner runs plan-level G1-G10; codemode runs raw-worker
    /// RW1-RW10; mcp runs G1 exposure only. Inferred from the artifact filename
    /// when omitted.
    #[arg(long)]
    surface: Option<String>,

    /// Directory for JSON reports. Defaults to conformance/reports relative to cwd.
    #[arg(long, default_value = "reports")]
    reports_dir: PathBuf,

    /// Exact explicit source repository head (40..=64 lowercase hex).
    ///
    /// Required: a production receipt is immutable and must name the exact
    /// source commit the harness was checked out at. Rejected when missing or
    /// malformed — the run fails closed instead of writing a receipt without
    /// provenance.
    #[arg(long)]
    source_head: Option<String>,

    /// Current hub repository head (40..=64 lowercase hex).
    ///
    /// Required: a production receipt must name the current hub commit at
    /// collection time. Rejected when missing or malformed.
    #[arg(long)]
    hub_head: Option<String>,

    /// Per-request timeout in seconds.
    #[arg(long, default_value_t = 5)]
    timeout_seconds: u64,

    /// Internal fake MCP fixture for harness self-tests.
    #[arg(long, hide = true)]
    fake_codemode_mcp: bool,

    /// Namespace for the hidden fake MCP fixture.
    #[arg(long, hide = true)]
    fake_ns: Option<String>,

    /// Emit bad refs from the hidden fake MCP fixture.
    #[arg(long, hide = true)]
    bad_refs: bool,
}

fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();
    if let Some(fake_args) = fake_mcp_argv(&raw_args) {
        return fake_mcp_main(&fake_args);
    }

    let args = Args::parse();
    let config = build_run_config(args)?;
    let report = run_conformance(&config);
    // Production receipts are immutable: bind the exact source head, hub head,
    // artifact hash/bytes, checks digest, and measured counts. Missing or
    // invalid provenance was already rejected above; this is the last gate.
    let provenance = production_provenance(&config, &report)?;
    let report = report.with_provenance(provenance);
    finish_with_report(&report, &config.reports_dir)
}

/// Single early gate for the hidden fixture.
///
/// Spawn form is `current_exe --fake-codemode-mcp <ns> [--bad-refs]`.
/// Rebuilds argv as `[prog, ns, optional --bad-refs]` for `fake_mcp_main`.
fn fake_mcp_argv(raw_args: &[String]) -> Option<Vec<String>> {
    if !raw_args.iter().any(|a| a == "--fake-codemode-mcp") {
        return None;
    }
    let prog = raw_args
        .first()
        .cloned()
        .unwrap_or_else(|| "zerostack-codemode-conformance".into());
    let mut out = vec![prog, fake_ns_from_raw(raw_args)];
    if raw_args.iter().any(|a| a == "--bad-refs") {
        out.push("--bad-refs".into());
    }
    Some(out)
}

fn fake_ns_from_raw(raw_args: &[String]) -> String {
    if let Some(i) = raw_args.iter().position(|a| a == "--fake-codemode-mcp")
        && let Some(ns) = raw_args.get(i + 1)
        && !ns.starts_with('-')
    {
        // Positional ns after the flag (spawn path). Skip other long-opts.
        return ns.clone();
    }
    if let Some(i) = raw_args.iter().position(|a| a == "--fake-ns")
        && let Some(ns) = raw_args.get(i + 1)
    {
        return ns.clone();
    }
    "gz".into()
}

fn build_run_config(args: Args) -> Result<RunConfig> {
    let ns = Ns::parse(args.ns.as_deref().context("--ns fz|tz|gz is required")?)?;
    let bin = args.bin.context("--bin <substrate-binary> is required")?;
    if !bin.is_file() {
        bail!("substrate binary is not a file: {}", bin.display());
    }

    // One artifact serves exactly one surface, chosen at install time. Infer it
    // from the filename so the common case needs no flag, but never guess
    // silently: an unrecognizable name is an error, not a default, because
    // running the wrong surface's checks would produce misleading evidence.
    let surface = match args.surface.as_deref() {
        Some(value) => Surface::parse(value)?,
        None => infer_surface(&bin).with_context(|| {
            format!(
                "cannot tell which surface {:?} serves; pass --surface planner|codemode|mcp",
                bin.file_name().unwrap_or_default()
            )
        })?,
    };

    // Production provenance: the receipt is immutable, so it must name the
    // exact explicit source head, the current hub head, and the tested
    // artifact's SHA-256 and byte length. Missing or invalid provenance is a
    // hard error BEFORE any check runs: a receipt without exact commits is
    // not production evidence.
    let source_head = args
        .source_head
        .context("--source-head <40..64 lowercase hex> is required for production provenance")?;
    let hub_head = args
        .hub_head
        .context("--hub-head <40..64 lowercase hex> is required for production provenance")?;
    if !valid_head(&source_head) {
        bail!("--source-head {source_head:?} is not 40..=64 lowercase hex");
    }
    if !valid_head(&hub_head) {
        bail!("--hub-head {hub_head:?} is not 40..=64 lowercase hex");
    }

    let artifact = std::fs::read(&bin)
        .with_context(|| format!("reading artifact bytes of {}", bin.display()))?;
    let artifact_bytes = artifact.len() as u64;
    let artifact_sha256 = zero_abi::sha256_hex(&artifact);

    let mut config = RunConfig::new(ns, bin, surface, args.reports_dir);
    config.timeout = Duration::from_secs(args.timeout_seconds);
    config.source_head = Some(source_head);
    config.hub_head = Some(hub_head);
    config.artifact_sha256 = Some(artifact_sha256);
    config.artifact_bytes = Some(artifact_bytes);
    Ok(config)
}

fn finish_with_report(report: &ConformanceReport, reports_dir: &Path) -> Result<()> {
    let path = report.write_to_reports_dir(reports_dir)?;
    println!("wrote {}", path.display());
    println!("{}", serde_json::to_string_pretty(report)?);

    eprintln!("completion status: {:?}", report.completion_status);
    match report.completion_status {
        CompletionStatus::Complete if report.passed => Ok(()),
        CompletionStatus::Failed => std::process::exit(1),
        CompletionStatus::Partial => std::process::exit(2),
        CompletionStatus::Complete => std::process::exit(1),
    }
}
