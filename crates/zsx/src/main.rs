#![forbid(unsafe_code)]

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use zsx_cli::exec;
use zsx_cli::mcp;

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
        [command, rest @ ..] if command == "mcp" => {
            mcp_command(rest)?;
            // Serve already wrote framed MCP replies. A leftover JSON line
            // after EOF would be a protocol violation.
            std::process::exit(0);
        }
        _ => Err("usage: zsx exec -C ROOT | zsx mcp [-C ROOT]".into()),
    }
}

fn mcp_command(args: &[String]) -> Result<Value, Box<dyn std::error::Error>> {
    let mut root = std::env::current_dir()?;
    let mut a = args.iter();
    while let Some(k) = a.next() {
        match k.as_str() {
            "-C" => {
                root = PathBuf::from(a.next().ok_or("mcp -C requires a root")?);
            }
            _ => return Err(format!("unknown argument {k}").into()),
        }
    }
    mcp::serve(root)?;
    Ok(json!({
        "protocol": exec::ZSX_PROTOCOL,
        "ok": true,
        "result": "eof",
    }))
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
    println!("Usage:");
    println!("  zsx exec -C ROOT [--file PLAN] [--timeout-ms N]");
    println!("  zsx mcp  [-C ROOT]");
    println!();
    println!("exec    one-shot CodeMode plan (stdin or --file). Process exits.");
    println!("mcp     harness-owned stdio MCP: zero_execute / zero_wait.");
    println!("        Idle is a blocking stdin read (no background work).");
    println!("        The host starts this process and kills it with the session.");
    println!("        Not a sidecar. Stores stay warm across calls.");
    println!("-C ROOT          authorized engine root (canonicalized)");
    println!("--file PLAN      read the plan from a file instead of stdin");
    println!("--timeout-ms N   execution timeout in milliseconds (default 30000)");
    println!("-h, --help       show this help");
    println!("-V, --version    show version");
}
