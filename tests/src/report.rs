//! Canonical machine-readable conformance report writer.
//!
//! Production report filenames are surface-disjoint and collision-safe:
//! `{ns}-{surface}-{stamp}-{checks-digest-prefix}.json`. The surface in the
//! name keeps planner / codemode / mcp receipts in disjoint namespaces even
//! for the same namespace, and the content-addressed checks digest prefix
//! makes two runs with different results never share a filename. The writer
//! never clobbers: if the exact name already exists, a numeric suffix is
//! appended.

use crate::ConformanceReport;
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Length of the content-addressed identity prefix in report filenames.
pub const REPORT_IDENTITY_HEX: usize = 16;

/// Surface-disjoint, collision-safe report filename for a given timestamp.
///
/// `stamp` is the wall-clock stamp (e.g. `2026-08-09-120000`); identity is the
/// first [`REPORT_IDENTITY_HEX`] hex chars of the report's self-binding checks
/// digest. Two reports with different checks never collide; two reports with
/// identical checks are the same evidence and may share the name safely (the
/// writer still refuses to clobber).
pub fn report_filename(report: &ConformanceReport, stamp: &str) -> String {
    let identity = &report.checks_digest()[..REPORT_IDENTITY_HEX];
    format!(
        "{}-{}-{}-{}.json",
        report.ns, report.surface, stamp, identity
    )
}

/// Writes the report to `reports_dir` under a surface-disjoint, collision-safe
/// filename and returns the written path. Never overwrites an existing file:
/// on a same-name collision a numeric suffix is appended.
pub fn write_report_to_reports_dir(
    report: &ConformanceReport,
    reports_dir: &Path,
) -> Result<PathBuf> {
    std::fs::create_dir_all(reports_dir)
        .with_context(|| format!("creating {}", reports_dir.display()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
    let stem = report_filename(report, &stamp.to_string());
    let base = stem.trim_end_matches(".json");
    let json = serde_json::to_string_pretty(report).context("serialize report")?;
    for suffix in 1u64.. {
        let path = if suffix == 1 {
            reports_dir.join(&stem)
        } else {
            reports_dir.join(format!("{base}-{suffix}.json"))
        };
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(json.as_bytes())
                    .with_context(|| format!("writing {}", path.display()))?;
                file.sync_all()
                    .with_context(|| format!("syncing {}", path.display()))?;
                return Ok(path);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", path.display()));
            }
        }
    }
    unreachable!("u64 report suffix space exhausted")
}

pub fn write_report(path: &Path, report: &ConformanceReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(report).context("serialize report")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create {} without overwriting", path.display()))?;
    file.write_all(json.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckResult, CompletionStatus, Ns, Surface};
    use tempfile::tempdir;

    fn report(surface: Surface, checks: Vec<CheckResult>) -> ConformanceReport {
        ConformanceReport::new(Ns::Gz, "fake-graphzero", surface, checks)
    }

    fn all_pass_checks() -> Vec<CheckResult> {
        (1..=10)
            .map(|gate| CheckResult::pass(&format!("G{gate}"), "semantic"))
            .collect()
    }

    fn one_failing_check() -> Vec<CheckResult> {
        let mut checks = all_pass_checks();
        checks[2] = CheckResult::fail("G3", "telemetry", "bad telemetry");
        checks
    }

    #[test]
    fn report_round_trips_with_the_canonical_shape() {
        let report = report(Surface::Planner, all_pass_checks());
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
        let report = ConformanceReport::new(Ns::Gz, "fake", Surface::Mcp, vec![]);
        write_report(&path, &report).expect("write");
        assert!(path.is_file());
    }

    #[test]
    fn report_filename_is_surface_disjoint() {
        let stamp = "2026-08-09-120000";
        let checks = all_pass_checks();
        let planner = report(Surface::Planner, checks.clone());
        let codemode = report(Surface::Codemode, checks);
        let planner_name = report_filename(&planner, stamp);
        let codemode_name = report_filename(&codemode, stamp);
        assert_ne!(
            planner_name, codemode_name,
            "surfaces must not share a name"
        );
        assert!(planner_name.starts_with("gz-planner-2026-08-09-120000-"));
        assert!(codemode_name.starts_with("gz-codemode-2026-08-09-120000-"));
    }

    #[test]
    fn report_filename_is_collision_safe_across_results() {
        let stamp = "2026-08-09-120000";
        let passing = report(Surface::Planner, all_pass_checks());
        let failing = report(Surface::Planner, one_failing_check());
        assert_ne!(
            report_filename(&passing, stamp),
            report_filename(&failing, stamp),
            "different results must never collide"
        );
    }

    #[test]
    fn report_filename_is_content_addressed() {
        // Identical checks produce the identical content-addressed identity;
        // a single changed check changes the identity.
        let a = report(Surface::Planner, all_pass_checks());
        let b = report(Surface::Planner, all_pass_checks());
        assert_eq!(report_filename(&a, "s"), report_filename(&b, "s"));
        let mut changed = all_pass_checks();
        changed[0] = CheckResult::skip("G1", "exposure", "fixture");
        assert_ne!(
            report_filename(&a, "s"),
            report_filename(&report(Surface::Planner, changed), "s"),
            "checks digest must bind every check"
        );
    }

    #[test]
    fn writer_never_clobbers_an_existing_report() {
        let dir = tempdir().unwrap();
        let a = report(Surface::Planner, all_pass_checks());
        let b = report(Surface::Planner, one_failing_check());
        let first = write_report_to_reports_dir(&a, dir.path()).expect("write first");
        let second = write_report_to_reports_dir(&b, dir.path()).expect("write second");
        assert_ne!(
            first, second,
            "different evidence must land in different files"
        );
        let third = write_report_to_reports_dir(&a, dir.path()).expect("write third");
        assert_ne!(
            first, third,
            "identical evidence must not clobber an existing file"
        );
        assert!(first.is_file());
        assert!(second.is_file());
        assert!(third.is_file());
    }

    #[test]
    fn explicit_writer_refuses_to_overwrite() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("report.json");
        let original = b"preserve me";
        std::fs::write(&path, original).unwrap();
        let error = write_report(&path, &report(Surface::Planner, all_pass_checks())).unwrap_err();
        assert!(error.to_string().contains("without overwriting"));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn concurrent_writers_claim_unique_paths_atomically() {
        let dir = tempdir().unwrap();
        let report = std::sync::Arc::new(report(Surface::Planner, all_pass_checks()));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let report = std::sync::Arc::clone(&report);
                let reports_dir = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    write_report_to_reports_dir(&report, &reports_dir).expect("atomic report write")
                })
            })
            .collect();
        let paths: std::collections::BTreeSet<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(paths.len(), 8);
        assert!(paths.iter().all(|path| path.is_file()));
    }

    #[test]
    fn checks_digest_and_counts_are_measured_not_claimed() {
        let report = report(
            Surface::Mcp,
            vec![
                CheckResult::pass("G1", "exposure"),
                CheckResult::fail("G2", "refs", "bad ref"),
                CheckResult::skip("G3", "telemetry", "mcp"),
                CheckResult::pass("G4", "leak-proof"),
            ],
        );
        assert_eq!(report.measured_counts(), (2, 1, 1));
        assert_eq!(report.checks_digest().len(), 64);
        let provenance =
            report.build_provenance("a".repeat(40), "b".repeat(40), "c".repeat(64), 12345);
        assert_eq!(provenance.pass_count, 2);
        assert_eq!(provenance.fail_count, 1);
        assert_eq!(provenance.skip_count, 1);
        assert_eq!(provenance.checks_digest, report.checks_digest());
        assert_eq!(provenance.artifact_bytes, 12345);
        // The digest binds every check: flipping one status changes it.
        let flipped = ConformanceReport::new(
            Ns::Gz,
            "fake",
            Surface::Mcp,
            vec![
                CheckResult::pass("G1", "exposure"),
                CheckResult::fail("G2", "refs", "bad ref"),
                CheckResult::skip("G3", "telemetry", "mcp"),
                CheckResult::fail("G4", "leak-proof", "leak"),
            ],
        );
        assert_ne!(flipped.checks_digest(), report.checks_digest());
        assert_eq!(flipped.measured_counts(), (1, 2, 1));
    }

    #[test]
    fn provenance_round_trips_and_counts_statuses() {
        let report = report(Surface::Planner, all_pass_checks());
        let provenance = report.build_provenance(
            "0123456789abcdef0123456789abcdef01234567",
            "fedcba9876543210fedcba9876543210fedcba98",
            "ab".repeat(32),
            4096,
        );
        let value = serde_json::to_value(&report.with_provenance(provenance.clone())).unwrap();
        let back: crate::ConformanceReport = serde_json::from_value(value).unwrap();
        assert_eq!(back.provenance(), Some(&provenance));
        assert_eq!(back.provenance().unwrap().pass_count, 10);
        assert_eq!(back.provenance().unwrap().fail_count, 0);
        assert_eq!(back.provenance().unwrap().skip_count, 0);
    }

    #[test]
    fn valid_head_accepts_only_lowercase_hex_40_to_64() {
        assert!(crate::valid_head(&"a".repeat(40)));
        assert!(crate::valid_head(&"b".repeat(64)));
        assert!(!crate::valid_head(&"a".repeat(39)));
        assert!(!crate::valid_head(&"a".repeat(65)));
        assert!(!crate::valid_head(&"A".repeat(40)));
        assert!(!crate::valid_head(&"z".repeat(40)));
        assert!(!crate::valid_head(""));
    }
}
