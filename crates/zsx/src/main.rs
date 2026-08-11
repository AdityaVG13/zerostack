#![forbid(unsafe_code)]

mod exec;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string(&response).expect("zsx response serializes")
            );
            if response.get("ok") != Some(&Value::Bool(true)) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            let response = json!({
                "protocol": exec::ZSX_PROTOCOL,
                "ok": false,
                "error": zsx_core::finalize_visible_error(&error.to_string()),
                "code": "internal",
            });
            println!(
                "{}",
                serde_json::to_string(&response).expect("zsx response serializes")
            );
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<Value, Box<dyn std::error::Error>> {
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            print_help();
            std::process::exit(0);
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("zsx {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        [command, rest @ ..] if command == "exec" => exec_command(rest),
        _ => Err("usage: zsx exec -C ROOT [--file PLAN] [--timeout-ms N]".into()),
    }
}

fn exec_command(args: &[String]) -> Result<Value, Box<dyn std::error::Error>> {
    let (mut root, mut file, mut timeout_ms) = (None, None, exec::DEFAULT_TIMEOUT_MS);
    let mut a = args.iter();
    while let Some(k) = a.next() {
        match k.as_str() {
            "-C" => root = a.next().map(PathBuf::from),
            "--file" => file = a.next().map(PathBuf::from),
            "--timeout-ms" => {
                timeout_ms = a
                    .next()
                    .ok_or("--timeout-ms requires a value")?
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --timeout-ms: {error}"))?;
                if timeout_ms == 0 {
                    return Err("--timeout-ms must be nonzero".into());
                }
            }
            _ => return Err(format!("unknown argument {k}").into()),
        }
    }
    let root = root.ok_or("missing -C ROOT")?;
    let source = if let Some(path) = file {
        std::fs::read_to_string(path)?
    } else {
        let mut source = String::new();
        std::io::stdin().read_to_string(&mut source)?;
        source
    };
    exec::exec(root, &source, Duration::from_millis(timeout_ms))
}

fn print_help() {
    println!("ZeroStack single-process CodeMode executable");
    println!();
    println!("Usage: zsx exec -C ROOT [--file PLAN] [--timeout-ms N]");
    println!();
    println!("exec    execute one CodeMode plan against the embedded zsx-core");
    println!("        session; the plan is read from --file or stdin. Runs");
    println!("        entirely in this process: no worker processes, no");
    println!("        session socket.");
    println!("-C ROOT          authorized engine root (canonicalized)");
    println!("--file PLAN      read the plan from a file instead of stdin");
    println!("--timeout-ms N   execution timeout in milliseconds (default 30000)");
    println!("-h, --help       show this help");
    println!("-V, --version    show version");
}
