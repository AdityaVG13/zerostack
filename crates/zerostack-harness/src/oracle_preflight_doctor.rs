//! Greenfield oracle preflight. `certifying` is true only when aggregate is green.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine_identity::{
    ORACLE_CLIPPY, ORACLE_MIRI, ORACLE_PROPERTY_SUITE_V1, ORACLE_ROUND_TRIP, ORACLE_SPEC_V1,
    SUBJECT_IDENTITY_LABEL, oracle_label_is_allowed,
};
use crate::golden;
use crate::repo::repo_root;
use crate::spec_oracle::{all_verifiers, report_advisory_spec_sources, verify_spec_source_hashes};

pub const SCHEMA_VERSION: &str = "oracle-preflight-doctor.v1";

#[derive(Clone, Debug, Serialize)]
pub struct PreflightCheck {
    pub name: String,
    pub outcome: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    pub schema_version: String,
    pub aggregate_outcome: String,
    pub certifying: bool,
    pub first_failure_diagnosis: Option<String>,
    pub subject_identity: String,
    pub oracle_identities: Vec<String>,
    pub verifier_count: usize,
    pub resolved_zsx_path: Option<String>,
    pub checks: Vec<PreflightCheck>,
    pub deterministic_replay_command: String,
}

impl PreflightReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("preflight report serializes")
    }
}

pub fn discover_root() -> PathBuf {
    if let Some(arg) = std::env::args().skip_while(|item| item != "--root").nth(1) {
        return PathBuf::from(arg);
    }
    repo_root()
}

fn locate_zsx(root: &Path) -> Option<PathBuf> {
    if let Ok(value) = std::env::var("ZSX_BIN") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates = vec![
        root.join("target/debug/zsx"),
        root.join("target/release/zsx"),
    ];
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/zsx"));
        candidates.push(home.join(".local/share/zerostack/current/zsx"));
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = Path::new(dir).join("zsx");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn push(checks: &mut Vec<PreflightCheck>, name: &str, ok: bool, detail: impl Into<String>) {
    checks.push(PreflightCheck {
        name: name.to_owned(),
        outcome: if ok { "green".into() } else { "red".into() },
        detail: detail.into(),
    });
}

pub fn run(root: &Path) -> PreflightReport {
    let mut checks = Vec::new();

    let oracle_identities = vec![
        ORACLE_SPEC_V1.to_owned(),
        ORACLE_PROPERTY_SUITE_V1.to_owned(),
        "prior-commit-deadbeef".to_owned(),
        ORACLE_ROUND_TRIP.to_owned(),
        ORACLE_MIRI.to_owned(),
        ORACLE_CLIPPY.to_owned(),
    ];

    let identities_ok = SUBJECT_IDENTITY_LABEL == "zerostack"
        && oracle_identities.iter().all(|label| {
            oracle_label_is_allowed(label) && label.as_str() != SUBJECT_IDENTITY_LABEL
        });
    push(
        &mut checks,
        "engine_identity_distinct",
        identities_ok,
        if identities_ok {
            format!("subject={SUBJECT_IDENTITY_LABEL} oracles={oracle_identities:?}")
        } else {
            "subject/oracle identity collision or disallowed label".into()
        },
    );

    let verifiers = all_verifiers();
    let verifier_ok = !verifiers.is_empty();
    push(
        &mut checks,
        "spec_verifier_nonempty",
        verifier_ok,
        format!("{} wired spec verifiers", verifiers.len()),
    );

    let hash_result = verify_spec_source_hashes(root);
    match &hash_result {
        Ok(rows) => push(
            &mut checks,
            "spec_source_sha256",
            true,
            format!("{} certifying spec sources match contract", rows.len()),
        ),
        Err(error) => push(&mut checks, "spec_source_sha256", false, error.to_string()),
    }

    match report_advisory_spec_sources(root) {
        Ok(rows) if rows.is_empty() => checks.push(PreflightCheck {
            name: "spec_source_sha256_advisory".into(),
            outcome: "green".into(),
            detail: "no advisory spec sources".into(),
        }),
        Ok(rows) => {
            let drifted = rows
                .iter()
                .any(|(_, detail)| detail.contains("drifted") || detail.contains("absent"));
            checks.push(PreflightCheck {
                name: "spec_source_sha256_advisory".into(),
                outcome: if drifted {
                    "yellow".into()
                } else {
                    "green".into()
                },
                detail: rows
                    .iter()
                    .map(|(_, detail)| detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        Err(error) => checks.push(PreflightCheck {
            name: "spec_source_sha256_advisory".into(),
            outcome: "yellow".into(),
            detail: format!("advisory check failed to load: {error}"),
        }),
    }

    match golden::verify_all(root) {
        Ok(detail) => push(&mut checks, "golden_three_tier", true, detail),
        Err(error) => push(&mut checks, "golden_three_tier", false, error),
    }

    let zsx = locate_zsx(root);
    push(
        &mut checks,
        "subject_binary_zsx",
        zsx.is_some(),
        match &zsx {
            Some(path) => path.display().to_string(),
            None => "zsx not found on PATH, ZSX_BIN, or zerostack current release".into(),
        },
    );

    let reds: Vec<&PreflightCheck> = checks
        .iter()
        .filter(|check| check.outcome == "red")
        .collect();
    let aggregate_outcome = if reds.is_empty() { "green" } else { "red" };
    let first_failure_diagnosis = reds
        .first()
        .map(|check| format!("{}: {}", check.name, check.detail));

    PreflightReport {
        schema_version: SCHEMA_VERSION.to_owned(),
        aggregate_outcome: aggregate_outcome.to_owned(),
        certifying: aggregate_outcome == "green",
        first_failure_diagnosis,
        subject_identity: SUBJECT_IDENTITY_LABEL.to_owned(),
        oracle_identities,
        verifier_count: verifiers.len(),
        resolved_zsx_path: zsx.map(|path| path.display().to_string()),
        checks,
        deterministic_replay_command: format!(
            "cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json --root {}",
            root.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_does_not_panic() {
        let report = run(&repo_root());
        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert!(!report.checks.is_empty());
        assert_eq!(report.certifying, report.aggregate_outcome == "green");
    }
}
