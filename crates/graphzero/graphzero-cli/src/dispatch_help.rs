use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, PartialEq, Eq)]
struct HelpRewrite {
    exe: PathBuf,
    subcommand: String,
}

fn help_rewrite_request(args: &[String], exe: PathBuf) -> Result<Option<HelpRewrite>> {
    if args.len() != 3 || args.get(1).map(String::as_str) != Some("--help") {
        return Ok(None);
    }
    let subcommand = args[2].clone();
    if !is_help_rewrite_subcommand(&subcommand) {
        anyhow::bail!("help target must be a subcommand name, got {subcommand}");
    }
    Ok(Some(HelpRewrite { exe, subcommand }))
}

fn is_help_rewrite_subcommand(subcommand: &str) -> bool {
    !subcommand.is_empty()
        && !subcommand.starts_with('-')
        && subcommand
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn execute_help_rewrite(rewrite: &HelpRewrite) -> Result<std::process::Output> {
    std::process::Command::new(&rewrite.exe)
        .args([rewrite.subcommand.as_str(), "--help"])
        .output()
        .with_context(|| format!("help rewrite via {}", rewrite.exe.display()))
}

fn help_rewrite_exit_code(output: &std::process::Output) -> i32 {
    output.status.code().unwrap_or(1)
}

fn run_help_rewrite(rewrite: HelpRewrite) -> Result<bool> {
    let output = execute_help_rewrite(&rewrite)?;
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    std::process::exit(help_rewrite_exit_code(&output));
}

pub(crate) fn maybe_rewrite_help_to_subcommand() -> Result<bool> {
    let args: Vec<String> = std::env::args().collect();
    let exe = std::env::current_exe().context("current executable for help rewrite")?;
    match help_rewrite_request(&args, exe)? {
        Some(rewrite) => run_help_rewrite(rewrite),
        None => Ok(false),
    }
}
