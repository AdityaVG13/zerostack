//! Machine-readable conformance report (`conformance/reports/<ns>-<date>.json`).

use crate::checks::{CheckOutcome, HarnessReport};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportFile {
    pub contract_version: String,
    pub ns: String,
    pub generated_at: String,
    pub substrate_binary: String,
    pub checks: Vec<CheckOutcome>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl ReportFile {
    pub fn from_harness(report: &HarnessReport) -> Self {
        Self {
            contract_version: report.contract_version.clone(),
            ns: report.ns.clone(),
            generated_at: Utc::now().to_rfc3339(),
            substrate_binary: report.substrate_binary.clone(),
            passed: report.passed(),
            failed: report.failed(),
            skipped: report.skipped(),
            checks: report.checks.clone(),
        }
    }
}

pub fn write_report(path: &Path, report: &HarnessReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file = ReportFile::from_harness(report);
    let json = serde_json::to_string_pretty(&file).context("serialize report")?;
    std::fs::write(path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{CheckId, CheckOutcome, CheckStatus, HarnessReport};
    use tempfile::tempdir;

    #[test]
    fn report_round_trips_through_json_with_check_outcomes() {
        let harness = HarnessReport {
            contract_version: "1.0".into(),
            ns: "gz".into(),
            substrate_binary: "/tmp/fake-graphzero".into(),
            checks: vec![
                CheckOutcome {
                    id: CheckId::G2Refs,
                    status: CheckStatus::Pass,
                    detail: None,
                },
                CheckOutcome {
                    id: CheckId::G3Telemetry,
                    status: CheckStatus::Fail,
                    detail: Some("raw_leak present".into()),
                },
            ],
        };
        let file = ReportFile::from_harness(&harness);
        let json = serde_json::to_string(&file).expect("serialize");
        let back: ReportFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.ns, "gz");
        assert_eq!(back.passed, 1);
        assert_eq!(back.failed, 1);
        assert_eq!(back.checks.len(), 2);
    }

    #[test]
    fn write_report_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("reports").join("gz-2026-07-01.json");
        let harness = HarnessReport {
            contract_version: "1.0".into(),
            ns: "gz".into(),
            substrate_binary: "fake".into(),
            checks: vec![],
        };
        write_report(&path, &harness).expect("write");
        assert!(path.is_file());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"ns\": \"gz\""));
    }
}
