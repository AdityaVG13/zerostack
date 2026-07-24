use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;
use zerostack_codemode_conformance::fake_substrate::fake_mcp_main;
use zerostack_codemode_conformance::{run_conformance, Ns, RunConfig};

#[derive(Debug, Parser)]
#[command(name = "zerostack-codemode-conformance")]
#[command(about = "Run ZeroStack CodeMode v1.0 G1-G10 conformance checks")]
struct Args {
    /// Substrate namespace: fz, tz, or gz.
    #[arg(long)]
    ns: Option<String>,

    /// Substrate MCP server binary path.
    #[arg(long)]
    bin: Option<PathBuf>,

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
        let mut fake_args = vec!["zerostack-codemode-conformance".to_string(), args.fake_ns.unwrap_or_else(|| "gz".to_string())];
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

    let mut config = RunConfig::new(ns, bin, args.reports_dir);
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
