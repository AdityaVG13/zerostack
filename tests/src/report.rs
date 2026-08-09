//! Canonical machine-readable conformance report writer.

use crate::ConformanceReport;
use anyhow::{Context, Result};
use std::path::Path;

pub fn write_report(path: &Path, report: &ConformanceReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(report).context("serialize report")?;
    std::fs::write(path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckResult, CompletionStatus, Ns, Surface};
    use tempfile::tempdir;

    #[test]
    fn report_round_trips_with_the_canonical_shape() {
        let report = crate::ConformanceReport::new(
            Ns::Gz,
            "fake-graphzero",
            Surface::Planner,
            (1..=10)
                .map(|gate| CheckResult::pass(&format!("G{gate}"), "semantic"))
                .collect(),
        );
        let value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(value["completion_status"], "complete");
        assert_eq!(value["passed"], true);
        assert_eq!(value["checks"][3]["id"], "G4");
        assert_eq!(value["checks"][3]["name"], "semantic");
        let back: crate::ConformanceReport = serde_json::from_value(value).expect("deserialize");
        assert_eq!(back.completion_status, CompletionStatus::Complete);
    }

    #[test]
    fn write_report_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("reports").join("gz.json");
        let report = crate::ConformanceReport::new(Ns::Gz, "fake", Surface::Mcp, vec![]);
        write_report(&path, &report).expect("write");
        assert!(path.is_file());
    }
}
