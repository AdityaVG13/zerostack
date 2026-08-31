use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "mcp-carrier")]
use std::sync::Arc;
#[cfg(feature = "mcp-carrier")]
use std::time::Duration;

use serde_json::json;
use zero_abi::KernelBudget;
use zero_gate::{ProgramEvidenceError, ProgramEvidenceManifest, assemble_program_evidence};
use zero_kernel::{Observation, ZeroKernel, direct_contract_digest, paired_savings_report};
#[cfg(feature = "mcp-carrier")]
use zero_mcp::{
    FastMcpZeroCarrier, McpDispatchError, McpTransportConfig, ZeroCarrierCapabilities,
    ZeroCarrierExecutor, ZeroCarrierSampling,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("ZeroKernel: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "doctor".into());
    let remaining = args.collect::<Vec<_>>();
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        print_help();
        return Ok(());
    }
    if remaining
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        match command.as_str() {
            "program-evidence" => println!("{PROGRAM_EVIDENCE_USAGE}"),
            "savings-report" => println!("{SAVINGS_REPORT_USAGE}"),
            _ => print_help(),
        }
        return Ok(());
    }
    match command.as_str() {
        "doctor" | "health" => {
            doctor(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from(".")))
        }
        "exec" => execute(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from("."))),
        "program-evidence" => program_evidence(&remaining),
        "savings-report" => savings_report(&remaining),
        #[cfg(feature = "mcp-carrier")]
        "mcp" => mcp(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from("."))),
        #[cfg(not(feature = "mcp-carrier"))]
        "mcp" => Err("mcp command requires the `mcp-carrier` feature".into()),
        _ => Err(format!(
            "unknown command {command:?}; use doctor, health, exec, mcp, program-evidence, or savings-report"
        )),
    }
}

const HELP: &str = "ZeroKernel\n\nUsage:\n  zero-kernel doctor|health [-C <workspace>]\n  zero-kernel exec [-C <workspace>]\n  zero-kernel mcp [-C <workspace>]\n  zero-kernel program-evidence --manifest <manifest.json> --out <receipt.json>\n  zero-kernel savings-report --native <native.json> --zero <zero.json>\n\nOptions:\n  -h, --help  Show this help and exit";
const PROGRAM_EVIDENCE_USAGE: &str =
    "zero-kernel program-evidence --manifest <manifest.json> --out <receipt.json>";
const SAVINGS_REPORT_USAGE: &str =
    "zero-kernel savings-report --native <native.json> --zero <zero.json>";

fn print_help() {
    println!("{HELP}");
}

fn doctor(root: PathBuf) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let store = root.join(".zerostack");
    let kernel = ZeroKernel::canonical(
        &root,
        &store,
        format!("doctor-{}", std::process::id()),
        default_budget(),
    )
    .map_err(|error| error.to_string())?;
    let transactions_dir = store.join("transactions");
    let mut quarantined: Vec<String> = Vec::new();
    if let Ok(sessions) = std::fs::read_dir(&transactions_dir) {
        for session in sessions.flatten() {
            let Ok(cells) = std::fs::read_dir(session.path()) else {
                continue;
            };
            for cell in cells.flatten() {
                if cell
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".poisoned.json")
                {
                    quarantined.push(cell.path().display().to_string());
                }
            }
        }
    }
    quarantined.sort();
    println!(
        "{}",
        serde_json::to_string(&json!({
            "runtime": "ZeroKernel",
            "status": "healthy",
            "project_root": root,
            "store_root": store,
            "contract_digest": direct_contract_digest(),
            "live_frames": kernel.live_frames(),
            "live_tasks": kernel.live_tasks(),
            "live_processes": kernel.live_processes(),
            "quarantined_journals": quarantined.len(),
            "quarantined_paths_bounded": quarantined.iter().take(10).collect::<Vec<_>>(),
            "daemon": false,
            "listener": false,
            "kernel_child": false,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn execute(root: PathBuf) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .map_err(|error| error.to_string())?;
    let kernel = ZeroKernel::canonical(
        &root,
        root.join(".zerostack"),
        format!("cli-{}", std::process::id()),
        default_budget(),
    )
    .map_err(|error| error.to_string())?;
    let response = kernel
        .execute_cell(&source)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}
fn program_evidence(args: &[String]) -> Result<(), String> {
    let (manifest_path, out_path) =
        parse_two_path_flags(args, "--manifest", "--out", PROGRAM_EVIDENCE_USAGE)?;
    let bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("cannot read manifest {}: {error}", manifest_path.display()))?;
    let manifest =
        ProgramEvidenceManifest::from_canonical_bytes(&bytes).map_err(|error| error.to_string())?;
    let receipt =
        assemble_program_evidence(&manifest, load_evidence).map_err(|error| error.to_string())?;
    let canonical = receipt
        .canonical_bytes()
        .map_err(|error| error.to_string())?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_path)
        .map_err(|error| {
            format!(
                "cannot create immutable receipt {}: {error}",
                out_path.display()
            )
        })?;
    output
        .write_all(&canonical)
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", out_path.display()))?;
    println!("wrote {}", out_path.display());
    Ok(())
}

fn load_evidence(path: &Path) -> Result<Vec<u8>, ProgramEvidenceError> {
    std::fs::read(path)
        .map_err(|error| ProgramEvidenceError::io(format!("reading {}: {error}", path.display())))
}

fn savings_report(args: &[String]) -> Result<(), String> {
    let (native_path, zero_path) =
        parse_two_path_flags(args, "--native", "--zero", SAVINGS_REPORT_USAGE)?;
    let native = read_observation(&native_path, "native")?;
    let zero = read_observation(&zero_path, "zero")?;
    let report = paired_savings_report(native, zero).map_err(|error| error.to_string())?;
    println!("{}", report.canonical_render());
    Ok(())
}

fn read_observation(path: &Path, label: &str) -> Result<Observation, String> {
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "cannot read {label} observation {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "cannot decode {label} observation {}: {error}",
            path.display()
        )
    })
}

fn parse_two_path_flags(
    args: &[String],
    first_flag: &str,
    second_flag: &str,
    usage: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let mut first = None;
    let mut second = None;
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{argument} requires a path\n{usage}"))?;
        let slot = match argument.as_str() {
            flag if flag == first_flag => &mut first,
            flag if flag == second_flag => &mut second,
            other => return Err(format!("unknown argument {other:?}\n{usage}")),
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("duplicate argument {argument}\n{usage}"));
        }
    }
    let first = first.ok_or_else(|| format!("missing {first_flag}\n{usage}"))?;
    let second = second.ok_or_else(|| format!("missing {second_flag}\n{usage}"))?;
    Ok((first, second))
}

#[cfg(feature = "mcp-carrier")]
fn mcp(root: PathBuf) -> Result<(), String> {
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let kernel = ZeroKernel::canonical(
        &root,
        root.join(".zerostack"),
        format!("mcp-{}", std::process::id()),
        default_budget(),
    )
    .map_err(|error| error.to_string())?;
    let digest = native_package_digest()?;
    let capabilities = ZeroCarrierCapabilities {
        cancellation: true,
        progress: true,
        sampling: ZeroCarrierSampling::Unavailable,
        maximum_inbound_bytes: 16 * 1024 * 1024,
        maximum_outbound_bytes: 16 * 1024 * 1024,
        native_package_digest: digest,
    };
    let executor = KernelStdioExecutor { kernel };
    let carrier = FastMcpZeroCarrier::new(
        Arc::new(executor),
        capabilities,
        McpTransportConfig {
            tool_timeout: Duration::from_secs(60),
            max_inflight: 1,
        },
    )
    .map_err(|error| error.to_string())?
    .with_server_identity("zero", env!("CARGO_PKG_VERSION"))
    .map_err(|error| error.to_string())?;
    carrier.run_stdio()
}

#[cfg(feature = "mcp-carrier")]
struct KernelStdioExecutor {
    kernel: ZeroKernel,
}

#[cfg(feature = "mcp-carrier")]
impl ZeroCarrierExecutor for KernelStdioExecutor {
    fn execute(
        &self,
        plan: &str,
        context: &zero_mcp::McpCallContext,
    ) -> Result<zero_abi::ZeroKernelResponse, McpDispatchError> {
        context.check()?;
        self.kernel
            .execute_cell(plan)
            .map_err(|error| McpDispatchError::new("kernel", error.to_string(), false))
    }
}

#[cfg(feature = "mcp-carrier")]
fn native_package_digest() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate zero-kernel executable: {error}"))?;
    let mut file = std::fs::File::open(&executable).map_err(|error| {
        format!(
            "cannot open zero-kernel executable {}: {error}",
            executable.display()
        )
    })?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| {
        format!(
            "cannot hash zero-kernel executable {}: {error}",
            executable.display()
        )
    })?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn default_budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 30_000,
        cpu_ms: 30_000,
        memory_bytes: 256 * 1024 * 1024,
        call_limit: 64,
        task_limit: 16,
        output_byte_limit: 64 * 1024,
    }
}

fn parse_path_flag(args: &[String], flag: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| PathBuf::from(&pair[1]))
}
