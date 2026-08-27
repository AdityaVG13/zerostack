//! Release artifact `graphzero-mcp`: FastMCP per-op surface only (graphzero-o2uq.3).
//!
//! Packaging subcommands (install/uninstall/sbom/doctor/help) always exit without
//! opening a stdio server. Shared core with `graphzero-codemode`.

use graphzero::packaging::{
    PackageSurface, assert_packaged_surface_features, assert_stdio_only_args,
    assert_surface_compiled, assert_surface_runtime_exclusivity, default_install_prefix,
    install_surface, package_identity, sbom_document, semantic_contract_digest, uninstall_report,
    uninstall_surface, uninstall_surface_dry_run,
};
use std::env;
use std::path::PathBuf;
use std::process;

const SURFACE: PackageSurface = PackageSurface::Mcp;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args
        .iter()
        .any(|a| a == "help" || a == "--help" || a == "-h")
    {
        let id = package_identity(SURFACE);
        println!(
            "graphzero-mcp — FastMCP per-operation surface (mutually exclusive with graphzero-codemode)\n\
             semantic_contract_digest: {}\n\
             selection: native CodeMode clients install this package\n\
             usage: graphzero-mcp | graphzero-mcp doctor | graphzero-mcp sbom | graphzero-mcp install|uninstall [--dry-run]\n\
             identity: {}",
            semantic_contract_digest(),
            id
        );
        process::exit(0);
    }

    if args.iter().any(|a| a == "sbom") {
        println!(
            "{}",
            serde_json::to_string_pretty(&sbom_document(SURFACE)).unwrap()
        );
        process::exit(0);
    }

    if args.iter().any(|a| a == "install") {
        run_install(&args);
        process::exit(0);
    }
    if args.iter().any(|a| a == "uninstall") {
        run_uninstall(&args);
        process::exit(0);
    }

    if args.iter().any(|a| a == "doctor" || a == "--doctor") {
        if let Err(e) = assert_surface_compiled(SURFACE) {
            eprintln!("{e}");
            process::exit(2);
        }
        let id = package_identity(SURFACE);
        println!(
            "package: artifact={} surface={} semantic_contract_digest={}",
            id["artifact"], id["surface"], id["semantic_contract_digest"]
        );
        println!(
            "{}",
            graphzero::agent_cli::doctor_json(std::path::Path::new("."))
        );
        process::exit(0);
    }

    if args.iter().any(|a| a == "--mode=codemode") {
        eprintln!(
            "graphzero-mcp: artifact is locked to surface 'mcp'; refused --mode=codemode. \
Run JavaScript through zerostack-codemode-host or zsx against graphzero-worker (mutually exclusive)."
        );
        process::exit(2);
    }

    if let Err(error) = assert_stdio_only_args(&args) {
        eprintln!("graphzero-mcp: {error}");
        process::exit(2);
    }

    if let Err(e) = assert_surface_compiled(SURFACE) {
        eprintln!("{e}");
        process::exit(2);
    }
    // Unconditional process/artifact exclusivity (no env opt-out).
    if let Err(e) = assert_packaged_surface_features(SURFACE) {
        eprintln!("{e}");
        process::exit(2);
    }
    if let Err(e) = assert_surface_runtime_exclusivity(SURFACE) {
        eprintln!("{e}");
        process::exit(2);
    }

    // FastMCP run never returns (`!`).
    graphzero::fastmcp_mode::run()
}

fn parse_flag(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        if let Some(rest) = args[i].strip_prefix(&format!("{name}=")) {
            return Some(rest.to_string());
        }
        i += 1;
    }
    None
}

fn run_install(args: &[String]) {
    if let Some(requested) = parse_flag(args, "--surface") {
        if PackageSurface::parse(&requested).ok() != Some(SURFACE) {
            eprintln!(
                "graphzero-mcp: install --surface must be 'mcp' for this artifact (got {requested:?})"
            );
            process::exit(2);
        }
    }
    let prefix = parse_flag(args, "--prefix")
        .map(PathBuf::from)
        .unwrap_or_else(default_install_prefix);
    let binary = parse_flag(args, "--binary")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_exe().unwrap_or_else(|_| PathBuf::from("graphzero-mcp")));
    match install_surface(SURFACE, &prefix, &binary) {
        Ok(state) => {
            println!(
                "install: ok surface={} artifact={} prefix={} semantic_contract_digest={} client_config={}",
                state.surface.as_str(),
                state.artifact,
                state.prefix,
                state.semantic_contract_digest,
                state.client_config
            );
        }
        Err(e) => {
            eprintln!("install: FAIL {e}");
            process::exit(1);
        }
    }
}

fn run_uninstall(args: &[String]) {
    let prefix = parse_flag(args, "--prefix")
        .map(PathBuf::from)
        .unwrap_or_else(default_install_prefix);
    if args.iter().any(|a| a == "--dry-run") {
        match uninstall_surface_dry_run(&prefix) {
            Ok(preview) => {
                println!("{}", serde_json::to_string_pretty(&preview).unwrap());
            }
            Err(e) => {
                eprintln!("uninstall: FAIL {e}");
                process::exit(1);
            }
        }
        return;
    }
    match uninstall_surface(&prefix) {
        Ok(prev) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&uninstall_report(prev)).unwrap()
            );
        }
        Err(e) => {
            eprintln!("uninstall: FAIL {e}");
            process::exit(1);
        }
    }
}
