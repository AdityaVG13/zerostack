use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;
use zerostack_codemode_conformance::fake_substrate::fake_mcp_main;
use zerostack_codemode_conformance::{run_conformance, Ns, RunConfig, Surface};

/// Infer the served surface from the artifact filename.
///
/// Engines ship `<name>-codemode` and `<name>-mcp`, so the filename carries the
/// install-time choice. Returns None for anything else, including the bare
/// compatibility shim, which is a selected symlink whose target we must not
/// assume.
fn infer_surface(bin: &std::path::Path) -> Option<Surface> {
    let stem = bin.file_name()?.to_string_lossy().to_ascii_lowercase();
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
#[command(about = "Run ZeroStack CodeMode v1.0 G1-G10 conformance checks")]
struct Args {
    /// Substrate namespace: fz, tz, or gz.
    #[arg(long)]
    ns: Option<String>,

    /// The installed substrate artifact under test.
    #[arg(long)]
    bin: Option<PathBuf>,

    /// Which surface that artifact serves: 'codemode' or 'mcp'.
    ///
    /// Surfaces are mutually exclusive; you install one or the other. Inferred
    /// from the artifact filename when omitted.
    #[arg(long)]
    surface: Option<String>,

    /// Directory for JSON reports. Defaults to conformance/reports relative to cwd.
    #[arg(long, default_value = "reports")]
    reports_dir: PathBuf,

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
    if raw_args.iter().any(|arg| arg == "--fake-codemode-mcp") {
        return fake_mcp_main(&raw_args);
    }

    let args = Args::parse();
    if args.fake_codemode_mcp {
        let mut fake_args = vec![
            "zerostack-codemode-conformance".to_string(),
            args.fake_ns.unwrap_or_else(|| "gz".to_string()),
        ];
        if args.bad_refs {
            fake_args.push("--bad-refs".to_string());
        }
        return fake_mcp_main(&fake_args);
    }

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
                "cannot tell which surface {:?} serves; pass --surface codemode or --surface mcp",
                bin.file_name().unwrap_or_default()
            )
        })?,
    };

    let mut config = RunConfig::new(ns, bin, surface, args.reports_dir);
    config.timeout = Duration::from_secs(args.timeout_seconds);
    let report = run_conformance(&config);
    let path = report.write_to_reports_dir(&config.reports_dir)?;

    println!("wrote {}", path.display());
    println!("{}", serde_json::to_string_pretty(&report)?);

    if report.passed {
        Ok(())
    } else {
        std::process::exit(1);
    }
}
