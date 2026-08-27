//! Unresolved `fszero doctor` surface must match CodeMode-first install.
//!
//! `resolve_startup_surface` already returns Codemode when nothing is selected.
//! The shim still hits `Err` when `--mode=` is present without a baked surface
//! or install-state; doctor must default that unresolved path to CodeMode, not MCP.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn fszero() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fszero"));
    for key in [
        "FSZERO_PRIVATE_WORKER",
        "FSZERO_ALLOW_BARE_SERVER",
        "FSZERO_STARTUP_INDEX",
        "FSZERO_ROOT",
        "FSZERO_PACKAGE_SURFACE",
        "FSZERO_SURFACE",
        "FSZERO_ENABLE_MCP",
        "FSZERO_ENABLE_CODEMODE",
        "FSZERO_INSTALL_PREFIX",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn isolated_dirs() -> (PathBuf, PathBuf) {
    let stamp = format!(
        "fszero-doctor-surface-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let base = std::env::temp_dir().join(stamp);
    let root = base.join("root");
    let prefix = base.join("prefix");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&prefix).unwrap();
    (root, prefix)
}

fn doctor_json(extra: &[&str]) -> Value {
    let (root, prefix) = isolated_dirs();
    let mut cmd = fszero();
    cmd.env("FSZERO_INSTALL_PREFIX", &prefix)
        .args(["doctor", "--json", "--root"])
        .arg(&root)
        .args(extra);
    let out = cmd.output().expect("fszero doctor --json");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "doctor JSON parse failed: {e}; status={:?} stdout={stdout} stderr={stderr}",
            out.status.code()
        )
    });
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
    doc
}

fn assert_codemode_identity(doc: &Value, context: &str) {
    assert_eq!(
        doc["schema"].as_str(),
        Some("fszero-doctor/v1"),
        "{context}: schema: {doc}"
    );
    assert_eq!(
        doc["package"]["surface"].as_str(),
        Some("codemode"),
        "{context}: unresolved doctor must report CodeMode, not MCP: {doc}"
    );
    assert_eq!(
        doc["package"]["artifact"].as_str(),
        Some("fszero-codemode"),
        "{context}: artifact: {doc}"
    );
    assert_eq!(
        doc["package"]["selection_matrix"]["canonical_default"].as_str(),
        Some("fszero-codemode"),
        "{context}: selection_matrix: {doc}"
    );
}

#[test]
fn doctor_unresolved_surface_defaults_to_codemode() {
    let doc = doctor_json(&[]);
    assert_codemode_identity(&doc, "no surface selection");
}

#[test]
fn doctor_unresolved_mode_codemode_defaults_to_codemode() {
    // Shim `--mode=` cannot bake a surface; resolve_startup_surface returns Err
    // and doctor must unwrap_or(Codemode), not Mcp.
    let doc = doctor_json(&["--mode=codemode"]);
    assert_codemode_identity(&doc, "--mode=codemode on shim");
}

#[test]
fn doctor_unresolved_mode_mcp_defaults_to_codemode() {
    let doc = doctor_json(&["--mode=mcp"]);
    assert_codemode_identity(&doc, "--mode=mcp on shim");
}

#[test]
fn doctor_explicit_package_surface_mcp_is_honored() {
    let (root, prefix) = isolated_dirs();
    let out = fszero()
        .env("FSZERO_INSTALL_PREFIX", &prefix)
        .env("FSZERO_PACKAGE_SURFACE", "mcp")
        .args(["doctor", "--json", "--root"])
        .arg(&root)
        .output()
        .expect("fszero doctor --json");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "doctor JSON parse failed: {e}; status={:?} stdout={} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(doc["package"]["surface"].as_str(), Some("mcp"));
    assert_eq!(doc["package"]["artifact"].as_str(), Some("fszero-mcp"));
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
