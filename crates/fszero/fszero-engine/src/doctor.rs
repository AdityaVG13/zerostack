//! Stable, bounded, redacted filesystem doctor diagnostics.

use super::FSZeroSession;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub const DOCTOR_SCHEMA: &str = "fszero-doctor/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorDiagnostic {
    pub code: &'static str,
    pub severity: DoctorSeverity,
    pub subsystem: &'static str,
    pub evidence: BTreeMap<String, Value>,
    pub remediation: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema: &'static str,
    pub ok: bool,
    pub diagnostics: Vec<DoctorDiagnostic>,
}

fn diagnostic(
    code: &'static str,
    severity: DoctorSeverity,
    subsystem: &'static str,
    evidence: impl IntoIterator<Item = (&'static str, Value)>,
    remediation: &'static str,
) -> DoctorDiagnostic {
    DoctorDiagnostic {
        code,
        severity,
        subsystem,
        evidence: evidence
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
        remediation,
    }
}

/// Inspect facts that can be established without repair. Evidence contains no
/// paths, file contents, or host error strings.
pub fn doctor_diagnostics(root: &Path) -> DoctorReport {
    let metadata = match std::fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                "FSZ-DOC-ROOT-001"
            } else {
                "FSZ-DOC-ROOT-003"
            };
            return DoctorReport {
                schema: DOCTOR_SCHEMA,
                ok: false,
                diagnostics: vec![diagnostic(
                    code,
                    DoctorSeverity::Error,
                    "workspace",
                    [("root_exists", Value::Bool(false))],
                    "select an existing readable workspace and rerun fszero doctor --root <workspace>",
                )],
            };
        }
    };
    if !metadata.is_dir() {
        return DoctorReport {
            schema: DOCTOR_SCHEMA,
            ok: false,
            diagnostics: vec![diagnostic(
                "FSZ-DOC-ROOT-002",
                DoctorSeverity::Error,
                "workspace",
                [("root_is_directory", Value::Bool(false))],
                "select a workspace directory and rerun fszero doctor --root <workspace>",
            )],
        };
    }

    let session = match FSZeroSession::try_with_repo_store(root) {
        Ok(session) => session,
        Err(_) => {
            return DoctorReport {
                schema: DOCTOR_SCHEMA,
                ok: false,
                diagnostics: vec![diagnostic(
                    "FSZ-DOC-STORE-002",
                    DoctorSeverity::Error,
                    "store",
                    [("store_open", Value::Bool(false))],
                    "preserve the store, verify permissions and free space, then rerun doctor",
                )],
            };
        }
    };
    let report = session.root_report();
    let mut diagnostics = Vec::with_capacity(6);

    if report["durable_degraded"].as_bool().unwrap_or(false) {
        diagnostics.push(diagnostic(
            "FSZ-DOC-STORE-001",
            DoctorSeverity::Warning,
            "store",
            [("durable", Value::Bool(false))],
            "preserve the workspace and restore a writable durable store before mutation",
        ));
    }
    let violations = report["store_health"]["integrity_violations"]
        .as_u64()
        .unwrap_or(0);
    if violations > 0 || !report["last_integrity_error"].is_null() {
        diagnostics.push(diagnostic(
            "FSZ-DOC-INTEGRITY-001",
            DoctorSeverity::Error,
            "store",
            [("integrity_violations", Value::from(violations))],
            "stop mutation, retain the store, and run the documented integrity verifier",
        ));
    }
    if report["fz_runtime_health"]["fail_open"]
        .as_bool()
        .unwrap_or(false)
    {
        diagnostics.push(diagnostic(
            "FSZ-DOC-RUNTIME-001",
            DoctorSeverity::Error,
            "runtime",
            [("fail_open", Value::Bool(true))],
            "use the native fallback until a healthy doctor smoke clears runtime fail-open",
        ));
    }
    let cas_tmps = session.recovery.cas_tmp_object_count();
    if cas_tmps > 0 {
        diagnostics.push(diagnostic(
            "FSZ-DOC-CAS-001",
            DoctorSeverity::Warning,
            "cas",
            [("tmp_objects", Value::from(cas_tmps))],
            "CAS temps are inert and not served; sweep after confirming no live writer",
        ));
    }
    // R-013 / fszero-ic6k.5: SKIP_STARTUP_INDEX is a dead alias; real control is STARTUP_INDEX=1.
    if std::env::var_os("FSZERO_SKIP_STARTUP_INDEX").is_some() {
        diagnostics.push(diagnostic(
            "FSZ-DOC-ENV-001",
            DoctorSeverity::Warning,
            "env",
            [
                ("var", Value::String("FSZERO_SKIP_STARTUP_INDEX".into())),
                ("status", Value::String("ignored_dead_name".into())),
                ("use_instead", Value::String("FSZERO_STARTUP_INDEX=1".into())),
            ],
            "unset FSZERO_SKIP_STARTUP_INDEX; it is never read. Opt into startup indexing with FSZERO_STARTUP_INDEX=1",
        ));
    }
    let legacy_rows = report["migration_legacy"]["legacy_blob_rows"]
        .as_u64()
        .unwrap_or(0);
    if legacy_rows > 0 {
        diagnostics.push(diagnostic(
            "FSZ-DOC-MIGRATION-001",
            DoctorSeverity::Warning,
            "migration",
            [("legacy_blob_rows", Value::from(legacy_rows))],
            "run fszero migrate-cas --root <workspace>; migration is idempotent and non-destructive",
        ));
    }
    let peer_incompatible = report["peer_incompatibility"]
        .as_object()
        .is_some_and(|value| !value.is_empty())
        || report["peer_incompatibility"]
            .as_str()
            .is_some_and(|value| !value.is_empty());
    if peer_incompatible {
        diagnostics.push(diagnostic(
            "FSZ-DOC-PROTOCOL-001",
            DoctorSeverity::Warning,
            "protocol",
            [("peer_compatible", Value::Bool(false))],
            "align peer capability major versions before cross-engine expansion",
        ));
    }

    if diagnostics.is_empty() {
        diagnostics.push(diagnostic(
            "FSZ-DOC-OK",
            DoctorSeverity::Info,
            "workspace",
            [
                ("root_is_directory", Value::Bool(true)),
                ("integrity_violations", Value::from(0)),
            ],
            "none",
        ));
    }
    let ok = !diagnostics
        .iter()
        .any(|row| row.severity == DoctorSeverity::Error);
    DoctorReport {
        schema: DOCTOR_SCHEMA,
        ok,
        diagnostics,
    }
}
