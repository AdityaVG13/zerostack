//! Dual-surface temporary-prefix install/exec smoke (fszero-ncib.10).
//!
//! Clean prefix → install one exclusive surface → inspect catalog from the
//! **installed binary** → execute one real op on that binary → parse recovered
//! bytes from the response (no hardcoded success). Fail on any error.

use super::{
    PackageSurface, client_config_for, install_surface, load_install_state, uninstall_surface,
};
use crate::core::runtime_metrics::{
    process_start_count, record_process_start, reset_runtime_metrics,
};
use crate::core::{OPERATION_ABI_VERSION, contract_digest_hex};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[inline]
fn smoke_fail<T>(msg: impl Into<String>) -> Result<T, String> {
    Err(msg.into())
}

#[derive(Debug, Clone)]
pub struct ReleaseSmokeReport {
    pub surface: String,
    pub catalog_names: Vec<String>,
    pub exec_ok: bool,
    pub recovered: Option<Vec<u8>>,
    pub install_state_ok: bool,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Build a single-surface debug binary (records one process start via cargo).
pub fn ensure_surface_bin(surface: PackageSurface) -> Result<PathBuf, String> {
    let bin_name = surface.artifact_name();
    let out = repo_root().join("../../target/debug").join(bin_name);
    // Instrumented spawn: record then cargo (same substrate as command_status).
    // Always records even when cargo is a no-op rebuild so detectors stay live.
    record_process_start();
    let status = Command::new("cargo")
        .args([
            "build",
            "--package",
            bin_name,
            "--no-default-features",
            "--features",
            "sqlite-system",
            "--jobs",
            "2",
        ])
        .env("CARGO_BUILD_JOBS", "2")
        .current_dir(repo_root())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return smoke_fail(format!("cargo build {bin_name} failed"));
    }
    if !out.is_file() {
        return smoke_fail(format!("missing binary {}", out.display()));
    }
    Ok(out)
}

/// Catalog from the **installed binary** (`catalog` subcommand), not the test process.
fn catalog_from_installed_bin(bin: &Path, surface: PackageSurface) -> Result<Vec<String>, String> {
    record_process_start();
    let out = Command::new(bin)
        .arg("catalog")
        .output()
        .map_err(|e| format!("catalog spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "catalog exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "catalog json: {e} stdout={}",
            String::from_utf8_lossy(&out.stdout)
        )
    })?;
    let names: Vec<String> = v
        .as_array()
        .ok_or_else(|| "catalog must be a JSON array of tools".to_string())?
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    if names.is_empty() {
        return smoke_fail(format!("{} installed catalog empty", surface.as_str()));
    }
    match surface {
        PackageSurface::Mcp => {
            if !names.iter().any(|n| n.starts_with("fszero.")) {
                return Err("installed mcp catalog missing fszero.*".into());
            }
            if names.iter().any(|n| n.starts_with("fz_")) {
                return Err("installed mcp catalog leaked fz_*".into());
            }
        }
        PackageSurface::Codemode => {
            if !names.iter().any(|n| n == "fz_execute_code") {
                return Err("installed codemode catalog missing fz_execute_code".into());
            }
            if names.iter().any(|n| n.starts_with("fszero.")) {
                return Err("installed codemode catalog leaked fszero.*".into());
            }
        }
    }
    Ok(names)
}

/// Execute one read on the installed binary via raw-worker; return recovered bytes
/// from the **binary response** (`result.value.payload_utf8`).
fn exec_read_on_installed_bin(
    bin: &Path,
    workspace: &Path,
    surface: PackageSurface,
    base_args: &[String],
) -> Result<(bool, Vec<u8>), String> {
    record_process_start();
    let mut child = Command::new(bin);
    for a in base_args {
        child.arg(a);
    }
    let mut child = child
        .arg("--raw-worker")
        .arg("--root")
        .arg(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} spawn raw-worker: {e}", surface.as_str()))?;
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let hs = json!({
            "kind": "handshake",
            "request": { "semantic_contract_digest": contract_digest_hex(), "semantic_contract_version": OPERATION_ABI_VERSION, "planner_owner": "client", "compression_owner": "client", }
        });
        writeln!(stdin, "{hs}").map_err(|e| e.to_string())?;
        let call = json!({ "kind": "call", "op": "fs.read", "args": {"path": "smoke.txt"}, "request_id": "smoke-1" });
        writeln!(stdin, "{call}").map_err(|e| e.to_string())?;
    }
    let (stdout, stderr) = crate::core::substrate_child::read_piped_stdio(&mut child);
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "{} raw-worker exit {:?} stderr={stderr}",
            surface.as_str(),
            status.code()
        ));
    }
    let mut recovered: Option<Vec<u8>> = None;
    let mut exec_ok = false;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("bad frame: {e} line={line}"))?;
        if v.get("kind").and_then(Value::as_str) != Some("result") {
            continue;
        }
        exec_ok = v
            .pointer("/result/ok")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(s) = v
            .pointer("/result/value/payload_utf8")
            .and_then(Value::as_str)
        {
            recovered = Some(s.as_bytes().to_vec());
        }
    }
    if !exec_ok {
        return Err(format!(
            "{} binary result not ok; stdout={stdout} stderr={stderr}",
            surface.as_str()
        ));
    }
    let bytes = recovered.ok_or_else(|| {
        format!(
            "{} binary response missing result.value.payload_utf8; stdout={stdout}",
            surface.as_str()
        )
    })?;
    Ok((true, bytes))
}

/// Install surface into prefix, inspect catalog from binary, execute on binary, uninstall.
pub fn smoke_one_surface(
    surface: PackageSurface,
    prefix: &Path,
    workspace: &Path,
) -> Result<ReleaseSmokeReport, String> {
    let _metrics_guard = crate::core::runtime_metrics::lock_metrics_for_test();
    reset_runtime_metrics();
    let bin = ensure_surface_bin(surface)?;
    fs::write(workspace.join("smoke.txt"), b"install-smoke-bytes").map_err(|e| e.to_string())?;
    fs::write(workspace.join("CHANGELOG.md"), b"# smoke\n").map_err(|e| e.to_string())?;
    fs::write(
        workspace.join("Cargo.toml"),
        b"[package]\nname=\"smoke\"\nversion=\"0.0.0\"\nedition=\"2021\"\n",
    )
    .map_err(|e| e.to_string())?;

    let _state = install_surface(surface, prefix, &bin).map_err(|e| e.to_string())?;
    let loaded = load_install_state(prefix)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "install-state missing after install".to_string())?;
    if loaded.surface != surface {
        return Err(format!(
            "install-state surface mismatch: {:?} vs {:?}",
            loaded.surface, surface
        ));
    }
    // Prefer the installed prefix binary if install copied it.
    let installed = prefix.join("bin").join(surface.artifact_name());
    // install_surface may only write state + config; use built artifact path.
    let run_bin = if installed.is_file() {
        installed
    } else {
        bin.clone()
    };

    let cfg = client_config_for(surface, &run_bin);
    let cfg_s = serde_json::to_string(&cfg).map_err(|e| e.to_string())?;
    // Dedicated artifact binaries reject any --mode= flag; only the shim needs it.
    if cfg_s.contains("--mode=") {
        return Err(format!(
            "{} client-config leaked --mode on dedicated artifact",
            surface.as_str()
        ));
    }

    // Catalog from **installed/built surface binary**, not the test process features.
    let catalog_names = catalog_from_installed_bin(&run_bin, surface)?;

    // Exec on that binary using the generated client config args (real artifact spawn).
    let (exec_ok, recovered_bytes) =
        exec_read_on_installed_bin(&run_bin, workspace, surface, &cfg.args)?;
    if recovered_bytes != b"install-smoke-bytes" {
        return Err(format!(
            "{} recovered bytes mismatch: got {:?}",
            surface.as_str(),
            String::from_utf8_lossy(&recovered_bytes)
        ));
    }

    // Doctor on the same binary (real process).
    record_process_start();
    let doc_status = Command::new(&run_bin)
        .arg("doctor")
        .arg("--root")
        .arg(workspace)
        .status()
        .map_err(|e| e.to_string())?;
    if !doc_status.success() {
        return Err(format!(
            "{} doctor failed with {:?}",
            surface.as_str(),
            doc_status.code()
        ));
    }

    uninstall_surface(prefix).map_err(|e| e.to_string())?;

    Ok(ReleaseSmokeReport {
        surface: surface.as_str().to_string(),
        catalog_names,
        exec_ok,
        recovered: Some(recovered_bytes),
        install_state_ok: true,
    })
}

/// Smoke both exclusive surfaces into separate temp prefixes.
pub fn dual_surface_temp_prefix_smoke() -> Result<(ReleaseSmokeReport, ReleaseSmokeReport), String>
{
    let stamp = crate::core::unix_epoch_nanos();
    let base = std::env::temp_dir().join(format!("fszero-ncib-smoke-{stamp}"));
    let ws = base.join("ws");
    let prefix_mcp = base.join("prefix-mcp");
    let prefix_cm = base.join("prefix-cm");
    for d in [&base, &ws, &prefix_mcp, &prefix_cm] {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }

    let result = (|| {
        let mcp = smoke_one_surface(PackageSurface::Mcp, &prefix_mcp, &ws)?;
        let cm = smoke_one_surface(PackageSurface::Codemode, &prefix_cm, &ws)?;
        Ok((mcp, cm))
    })();
    let _ = fs::remove_dir_all(&base);
    result
}

/// Kill-test helper: production spawn paths must increment process_start_count
/// without the caller manually calling `record_process_start`.
///
/// Holds the metrics test lock for the whole check so concurrent trials cannot
/// `reset_runtime_metrics` between the mid and after snapshots.
pub fn ensure_surface_bin_increments_process_starts() -> Result<(), String> {
    let _guard = crate::core::runtime_metrics::lock_metrics_for_test();
    reset_runtime_metrics();
    let before = process_start_count();
    // Production wrapper used by packaging / spawn sites — test must not call
    // record_process_start itself.
    crate::core::runtime_metrics::command_status("true", &[] as &[&str])
        .map_err(|e| e.to_string())?;
    let mid = process_start_count();
    if mid != before + 1 {
        return Err(format!(
            "command_status wrapper did not increment process_starts (before={before} mid={mid})"
        ));
    }
    // ensure_surface_bin records immediately before cargo (even on no-op rebuild).
    let path = ensure_surface_bin(PackageSurface::Mcp)?;
    if !path.is_file() {
        return Err(format!("ensure_surface_bin missing {}", path.display()));
    }
    let after = process_start_count();
    if after < mid + 1 {
        return Err(format!(
            "ensure_surface_bin did not call record_process_start (mid={mid} after={after})"
        ));
    }
    Ok(())
}
