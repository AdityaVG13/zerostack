//! Strict preflight CLI: `zerostack doctor|locate [--json]`, plus help/version.
//!
//! Exit codes: 0 complete, 1 checks failed, 2 usage error. JSON always goes to
//! stdout and stays parseable; only locate writes a failure summary, to stderr.

use std::path::Path;
use std::process::ExitCode;

use zero_codemode::MANIFEST_SCHEMA;
use zero_codemode::manifest::{locate_from_process, render_manifest_human};
use zero_codemode::preflight::{doctor_report, locate_missing, render_doctor_human};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            print_help();
            ExitCode::SUCCESS
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("zerostack {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        [flag] if flag == "doctor" => doctor(false),
        [flag, json] if flag == "doctor" && json == "--json" => doctor(true),
        [flag] if flag == "locate" => locate(false),
        [flag, json] if flag == "locate" && json == "--json" => locate(true),
        _ => {
            eprintln!("zerostack: unsupported arguments");
            eprintln!("usage: zerostack doctor [--json] | locate [--json] | --help | --version");
            ExitCode::from(2)
        }
    }
}

/// Run the nine-check doctor against the live process environment.
fn doctor(json: bool) -> ExitCode {
    let report = doctor_report(&locate_from_process(), &is_directory);
    if json {
        println!(
            "{}",
            serde_json::to_string(&report).expect("doctor report always serializes")
        );
    } else {
        print!("{}", render_doctor_human(&report));
    }
    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Emit the complete locate manifest, failing when a required entry is missing.
fn locate(json: bool) -> ExitCode {
    let manifest = locate_from_process();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&manifest).expect("manifest always serializes")
        );
    } else {
        print!("{}", render_manifest_human(&manifest));
    }
    let missing = locate_missing(&manifest);
    if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "zerostack: locate incomplete: missing {}",
            missing.join(", ")
        );
        ExitCode::from(1)
    }
}

fn is_directory(path: &Path) -> bool {
    path.is_dir()
}

fn print_help() {
    println!("ZeroStack preflight");
    println!();
    println!("Usage: zerostack doctor [--json] | locate [--json] | --help | --version");
    println!();
    println!(
        "doctor   run the nine-component preflight; exit 0 when every check passes, 1 otherwise"
    );
    println!(
        "locate   emit the complete {MANIFEST_SCHEMA} manifest; exit 1 when a required entry is unresolved"
    );
    println!("--json   emit JSON on stdout");
    println!("-h, --help     show this help");
    println!("-V, --version  show version");
}
