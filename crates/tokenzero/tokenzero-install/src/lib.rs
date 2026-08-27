//! TokenZero install surface: thin wrapper over the hub install engine.
//!
//! The engine (plan/apply/rollback, doctor, agent detection, archive
//! integrity) lives in the ZeroStack hub (`zerostack-install`). This crate
//! supplies the TokenZero payload identity and re-exports the engine API so
//! existing consumers keep their import paths.

#![forbid(unsafe_code)]

pub use zerostack_install::*;

use serde_json::{Value, json};
use std::path::Path;

/// TokenZero payload identity (artifact names differ per repo; the engine
/// itself is product-neutral).
pub const ARTIFACT_MCP: &str = "tokenzero-mcp";
pub const ARTIFACT_RAW_WORKER: &str = "tokenzero-codemode";
pub const ARTIFACT_SHIM: &str = "tokenzero";

/// Source of the classic MCP stdio binary. Not a workspace `[[bin]]`.
pub const MCP_BIN_SOURCE: &str = "crates/tokenzero/tokenzero-cli/src/bin/tokenzero_mcp.rs";

/// Classic MCP is live only when this crate compiled `surface-mcp`.
/// The default TokenZero workspace build does not: `tokenzero_mcp.rs` is not
/// a `[[bin]]` (`autobins = false`) and `tokenzero_mcp_compat` is not a crate.
pub fn classic_mcp_orifice_live() -> bool {
    packaging::surface_compiled_in(packaging::PackageSurface::Mcp)
}

/// Doctor/install/capabilities honesty block for the classic MCP orifice.
pub fn mcp_orifice_json() -> Value {
    let live = classic_mcp_orifice_live();
    json!({
        "live": live,
        "compiled": live,
        "ready": live,
        "artifact": ARTIFACT_MCP,
        "cli_verb": "mcp-server",
        "source": MCP_BIN_SOURCE,
        "status": if live { "compiled" } else { "not_a_workspace_bin" },
        "server": if live { Value::String("tokenzero mcp-server".into()) } else { Value::Null },
        "note": "autobins=false and no [[bin]] tokenzero-mcp; tokenzero mcp-server is cfg(feature = \"surface-mcp\") and that feature has no tokenzero_mcp_compat crate"
    })
}

fn honest_mcp_doctor_capabilities(mut caps: Value) -> Value {
    let live = classic_mcp_orifice_live();
    if let Some(detectors) = caps.get_mut("detectors").and_then(Value::as_array_mut) {
        for detector in detectors {
            if detector.get("id").and_then(Value::as_str)
                == Some("tz-mcp-server-entrypoint-declared")
            {
                detector["severity"] = json!(if live { "ok" } else { "info" });
                detector["description"] = json!(if live {
                    "MCP server entrypoint is declared as tokenzero mcp-server"
                } else {
                    "classic MCP is not compiled; tokenzero-mcp is not a workspace [[bin]]"
                });
            }
        }
    }
    caps["mcp_orifice"] = mcp_orifice_json();
    caps
}

fn honest_mcp_doctor(mut report: Value) -> Value {
    let live = classic_mcp_orifice_live();
    let orifice = mcp_orifice_json();
    report["mcp"] = orifice.clone();
    report["mcp_orifice"] = orifice;
    if let Some(checks) = report.get_mut("checks").and_then(Value::as_array_mut) {
        for check in checks {
            if check.get("id").and_then(Value::as_str) == Some("mcp_server_entrypoint_declared") {
                check["ok"] = json!(live);
                check["severity"] = json!(if live { "ok" } else { "info" });
                check["evidence"] = json!(if live {
                    "tokenzero mcp-server"
                } else {
                    "tokenzero-mcp is not a workspace bin; mcp-server is not compiled"
                });
            }
        }
    }
    if report.get("capabilities").is_some() {
        report["capabilities"] =
            honest_mcp_doctor_capabilities(zerostack_install::doctor_capabilities());
    }
    report
}

/// Hub doctor claims `mcp.ready` from env parse + workspace root, which is
/// not dispatch. Overlay the compile-time orifice so doctor JSON cannot
/// advertise `tokenzero mcp-server` / `tokenzero-mcp` when they are not live.
pub fn doctor(root: &Path, cache_path: Option<&Path>) -> Value {
    honest_mcp_doctor(zerostack_install::doctor(root, cache_path))
}

pub fn doctor_capabilities() -> Value {
    honest_mcp_doctor_capabilities(zerostack_install::doctor_capabilities())
}

pub fn doctor_robot_triage(root: &Path, cache_path: Option<&Path>) -> Value {
    let mut triage = zerostack_install::doctor_robot_triage(root, cache_path);
    triage["mcp_orifice"] = mcp_orifice_json();
    triage
}

/// Legacy alias retained for tests importing `packaging::*` directly.
pub mod packaging {
    pub use zerostack_install::packaging::*;
    pub const ARTIFACT_MCP: &str = super::ARTIFACT_MCP;
    pub const ARTIFACT_RAW_WORKER: &str = super::ARTIFACT_RAW_WORKER;
    pub const ARTIFACT_SHIM: &str = super::ARTIFACT_SHIM;
}

pub mod package_audit {
    pub use zerostack_install::package_audit::*;
}
