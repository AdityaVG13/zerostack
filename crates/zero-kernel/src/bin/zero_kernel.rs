use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use zero_abi::KernelBudget;
use zero_kernel::{ZeroKernel, direct_contract_digest};
#[cfg(feature = "mcp-carrier")]
use zero_mcp::{
    FastMcpZeroCarrier, McpDispatchError, McpTransportConfig, ZeroCarrierCapabilities,
    ZeroCarrierExecutor, ZeroCarrierSampling,
};
use zero_store::{import_legacy_store, read_and_verify_manifest};

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
    if matches!(command.as_str(), "help" | "--help" | "-h")
        || remaining
            .iter()
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    match command.as_str() {
        "doctor" | "health" => {
            doctor(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from(".")))
        }
        "exec" => execute(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from("."))),
        #[cfg(feature = "mcp-carrier")]
        "mcp" => mcp(parse_path_flag(&remaining, "-C").unwrap_or_else(|| PathBuf::from("."))),
        #[cfg(not(feature = "mcp-carrier"))]
        "mcp" => Err("mcp command requires the `mcp-carrier` feature".into()),
        "migrate" => migrate(&remaining),
        _ => Err(format!(
            "unknown command {command:?}; use doctor, health, exec, mcp, or migrate"
        )),
    }
}

const HELP: &str = "ZeroKernel\n\nUsage:\n  zero-kernel doctor|health [-C <workspace>]\n  zero-kernel exec [-C <workspace>]\n  zero-kernel mcp [-C <workspace>]\n  zero-kernel migrate <options>\n\nOptions:\n  -h, --help  Show this help and exit";

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
    let digest = native_package_digest();
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

fn native_package_digest() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "zero-kernel".into());
    format!("{}", blake3::hash(exe.as_bytes()).to_hex())
}

fn migrate(args: &[String]) -> Result<(), String> {
    let source = required_path_flag(args, "--source")?;
    let destination = required_path_flag(args, "--destination")?;
    let manifest = required_path_flag(args, "--manifest")?;
    let key_hex = required_string_flag(args, "--key-hex")?;
    let key = parse_key(&key_hex)?;
    let result = import_legacy_store(&source, &destination, &manifest, &key)
        .map_err(|error| error.to_string())?;
    let verified = read_and_verify_manifest(&manifest, &key).map_err(|error| error.to_string())?;
    if result != verified {
        return Err("written migration manifest did not verify identically".into());
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "objects": result.entries.len(),
            "bytes": result.total_bytes,
            "manifest": manifest,
            "signature": result.signature,
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
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

fn required_path_flag(args: &[String], flag: &str) -> Result<PathBuf, String> {
    parse_path_flag(args, flag).ok_or_else(|| format!("{flag} requires a path"))
}

fn required_string_flag(args: &[String], flag: &str) -> Result<String, String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_key(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("--key-hex must contain 64 lowercase hex characters".into());
    }
    let mut key = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
        key[index] = u8::from_str_radix(text, 16)
            .map_err(|_| "--key-hex must contain lowercase hexadecimal".to_string())?;
    }
    Ok(key)
}
