//! Nine-check install preflight (`zerostack doctor`) and locate completeness.
//!
//! The doctor consumes the [crate::manifest::locate_manifest] report and adds
//! the two directory checks the manifest reports rather than resolves. File
//! checks copy path and source from the manifest, so the doctor and locate can
//! never disagree about what was found.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Versioned doctor schema, stable across surfaces.
pub const DOCTOR_SCHEMA: &str = "zerostack.doctor.v1";

/// Stable failure text for an unresolved executable check.
const EXECUTABLE_ERROR: &str = "no executable candidate resolved";
/// Stable failure text for an unresolved module check.
const MODULE_ERROR: &str = "no readable regular-file candidate resolved";
/// Stable failure text for a store root that is not a directory.
const STORE_ROOT_NOT_DIRECTORY: &str = "store root is not a directory";
/// Stable failure text for a journal directory that is not a directory.
const JOURNAL_DIR_NOT_DIRECTORY: &str = "journal directory is not a directory";
/// Stable failure text when no store root was reported at all.
const NO_STORE_ROOT: &str = "no store root resolved";
/// Stable failure text when no journal directory was reported at all.
const NO_JOURNAL_DIR: &str = "no journal directory resolved";

/// Engine-binary remediation: name the exact install location or pin.
const BINARY_REMEDIATION: &str = "install into $ZEROSTACK_HOME/bin or set ZEROSTACK_DEV_ROOT";
/// Node remediation: name the exact pin.
const NODE_REMEDIATION: &str = "set ZEROSTACK_NODE to a Node executable";
/// Runtime-module remediation: name the exact pin.
const RUNTIME_MODULE_REMEDIATION: &str = "set ZEROSTACK_RUNTIME_MODULE to a readable module file";
/// Substrate-module remediation: name the exact pin.
const SUBSTRATE_MODULE_REMEDIATION: &str =
    "set ZEROSTACK_SUBSTRATE_MODULE to a readable module file";
/// Store remediation when the manifest reported no store path.
const STORE_PIN_REMEDIATION: &str =
    "set ZEROSTACK_STORE_ROOT and opt in with ZEROSTACK_SHARED_STORE=1";

/// One preflight check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// Stable component id, in doctor order.
    pub component: String,
    /// True when the component passed.
    pub ok: bool,
    /// Resolved path, when the manifest reported one.
    pub path: Option<String>,
    /// Resolution rule that produced the path, when the manifest reported one.
    pub source: Option<String>,
    /// Stable failure text, present only when `ok` is false.
    pub error: Option<String>,
    /// Exact remediation, present only when `ok` is false.
    pub remediation: Option<String>,
}

/// The full doctor report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Versioned schema label.
    pub schema: String,
    /// True when every check passed.
    pub ok: bool,
    /// Exactly the nine checks, in doctor order.
    pub checks: Vec<DoctorCheck>,
}

/// What a check validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An executable resolved by [crate::discovery].
    Executable,
    /// A readable regular-file module resolved by the manifest.
    Module,
    /// A directory the manifest reports rather than resolves.
    Store,
}

/// The nine checks, in doctor order: component id and manifest lookup path.
const DOCTOR_CHECKS: [(&str, &str, Kind); 9] = [
    ("aggregate_host", "aggregate_host", Kind::Executable),
    ("binaries.fs", "binaries.fs", Kind::Executable),
    ("binaries.graph", "binaries.graph", Kind::Executable),
    ("binaries.token", "binaries.token", Kind::Executable),
    ("node", "node", Kind::Executable),
    ("runtime_module", "runtime_module", Kind::Module),
    ("substrate_module", "substrate_module", Kind::Module),
    ("store_root", "store_root", Kind::Store),
    ("journal_dir", "journal_dir", Kind::Store),
];

/// Required locate entries, in manifest order: every field a harness needs.
const LOCATE_REQUIRED: [&str; 7] = [
    "aggregate_host",
    "binaries.fs",
    "binaries.graph",
    "binaries.token",
    "node",
    "runtime_module",
    "substrate_module",
];

/// Run the nine ordered checks against a locate manifest.
///
/// `is_directory` admits directories, injected so the report is testable
/// without touching the filesystem. File checks never re-probe: they read the
/// manifest, which already probed the layout.
pub fn doctor_report(
    manifest: &serde_json::Value,
    is_directory: &dyn Fn(&Path) -> bool,
) -> DoctorReport {
    let checks: Vec<DoctorCheck> = DOCTOR_CHECKS
        .iter()
        .map(|(component, dotted, kind)| match kind {
            Kind::Executable => file_check(component, lookup(manifest, dotted), EXECUTABLE_ERROR),
            Kind::Module => file_check(component, lookup(manifest, dotted), MODULE_ERROR),
            Kind::Store => store_check(component, lookup(manifest, dotted), is_directory),
        })
        .collect();
    let ok = checks.iter().all(|check: &DoctorCheck| check.ok);
    DoctorReport {
        schema: DOCTOR_SCHEMA.to_owned(),
        ok,
        checks,
    }
}

/// True when every required locate entry resolved.
pub fn locate_complete(manifest: &serde_json::Value) -> bool {
    locate_missing(manifest).is_empty()
}

/// Required locate entries that did not resolve, in manifest order.
pub fn locate_missing(manifest: &serde_json::Value) -> Vec<&'static str> {
    LOCATE_REQUIRED
        .iter()
        .copied()
        .filter(|component| !component_resolved(manifest, component))
        .collect()
}

/// One `OK` line per pass, `ERROR` plus `FIX` per failure, then the summary.
pub fn render_doctor_human(report: &DoctorReport) -> String {
    let mut out = String::new();
    for check in &report.checks {
        if check.ok {
            out.push_str(&format!(
                "OK {}: {} [{}]\n",
                check.component,
                check.path.as_deref().unwrap_or("<unresolved>"),
                check.source.as_deref().unwrap_or("-"),
            ));
        } else {
            out.push_str(&format!(
                "ERROR {}: {}\n",
                check.component,
                check.error.as_deref().unwrap_or("check failed")
            ));
            out.push_str(&format!(
                "FIX {}: {}\n",
                check.component,
                check.remediation.as_deref().unwrap_or("")
            ));
        }
    }
    let failed = report.checks.iter().filter(|check| !check.ok).count();
    if report.ok {
        out.push_str(&format!(
            "ZeroStack doctor: OK ({} checks)\n",
            report.checks.len()
        ));
    } else {
        out.push_str(&format!(
            "ZeroStack doctor: FAILED ({}/{} checks failed)\n",
            failed,
            report.checks.len()
        ));
    }
    out
}

/// A manifest entry resolved through a dotted path such as `binaries.fs`.
fn lookup<'a>(manifest: &'a serde_json::Value, dotted: &str) -> Option<&'a serde_json::Value> {
    let mut current = manifest;
    for part in dotted.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn component_resolved(manifest: &serde_json::Value, component: &str) -> bool {
    component_resolved_entry(lookup(manifest, component))
}

fn component_resolved_entry(entry: Option<&serde_json::Value>) -> bool {
    entry
        .and_then(serde_json::Value::as_object)
        .and_then(|entry| entry.get("resolved"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// A binary or module check: pass copies path/source from the manifest.
fn file_check(
    component: &str,
    entry: Option<&serde_json::Value>,
    error_text: &'static str,
) -> DoctorCheck {
    if component_resolved_entry(entry) {
        DoctorCheck {
            component: component.to_owned(),
            ok: true,
            path: entry_str(entry, "path"),
            source: entry_str(entry, "source"),
            error: None,
            remediation: None,
        }
    } else {
        DoctorCheck {
            component: component.to_owned(),
            ok: false,
            path: None,
            source: None,
            error: Some(error_text.to_owned()),
            remediation: Some(remediation_for(component).to_owned()),
        }
    }
}

/// A store check: the reported path must exist as a directory.
fn store_check(
    component: &str,
    entry: Option<&serde_json::Value>,
    is_directory: &dyn Fn(&Path) -> bool,
) -> DoctorCheck {
    let path = entry.and_then(serde_json::Value::as_str).map(PathBuf::from);
    match path {
        Some(path) if is_directory(&path) => DoctorCheck {
            component: component.to_owned(),
            ok: true,
            path: Some(path.to_string_lossy().into_owned()),
            source: None,
            error: None,
            remediation: None,
        },
        Some(path) => DoctorCheck {
            component: component.to_owned(),
            ok: false,
            path: Some(path.to_string_lossy().into_owned()),
            source: None,
            error: Some(
                if component == "store_root" {
                    STORE_ROOT_NOT_DIRECTORY
                } else {
                    JOURNAL_DIR_NOT_DIRECTORY
                }
                .to_owned(),
            ),
            remediation: Some(format!("Create directory: {}", path.display())),
        },
        None => DoctorCheck {
            component: component.to_owned(),
            ok: false,
            path: None,
            source: None,
            error: Some(
                if component == "store_root" {
                    NO_STORE_ROOT
                } else {
                    NO_JOURNAL_DIR
                }
                .to_owned(),
            ),
            remediation: Some(STORE_PIN_REMEDIATION.to_owned()),
        },
    }
}

fn entry_str(entry: Option<&serde_json::Value>, key: &str) -> Option<String> {
    entry
        .and_then(serde_json::Value::as_object)
        .and_then(|entry| entry.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn remediation_for(component: &str) -> &'static str {
    match component {
        "node" => NODE_REMEDIATION,
        "runtime_module" => RUNTIME_MODULE_REMEDIATION,
        "substrate_module" => SUBSTRATE_MODULE_REMEDIATION,
        _ => BINARY_REMEDIATION,
    }
}
