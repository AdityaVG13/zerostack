//! FailureBundle v1. A partial bundle with provenance is more valuable than none.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mismatch::MismatchClassification;
use crate::repo::sha256_hex;

pub const SCHEMA_VERSION: &str = "failure_bundle.v1.0.0";
pub const FIRST_DIVERGENCE_JSONPTR: &str = "/failure/first_divergence";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureType {
    Assertion,
    Panic,
    Divergence,
    Timeout,
    SpecConflict,
    Invariant,
    WalRecovery,
    FileFormat,
    Extension,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstDivergence {
    pub kind: String,
    pub subject: String,
    pub oracle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureSection {
    pub first_divergence: FirstDivergence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub git_sha: String,
    pub toolchain_version: String,
    pub platform: String,
    pub feature_flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailureBundle {
    pub schema_version: String,
    pub failure_type: FailureType,
    pub seed: u64,
    pub fixture_id: String,
    pub schedule_fingerprint: String,
    pub exact_repro_command: String,
    pub first_divergence_jsonptr: String,
    pub artifact_sha256: Vec<String>,
    pub expected_vs_actual: String,
    pub classification: MismatchClassification,
    pub environment: Environment,
    pub failure: FailureSection,
    pub partial: bool,
    pub bead_id: Option<String>,
}

impl FailureBundle {
    pub fn new(
        failure_type: FailureType,
        seed: u64,
        fixture_id: impl Into<String>,
        classification: MismatchClassification,
        first: FirstDivergence,
    ) -> Self {
        let fixture_id = fixture_id.into();
        let schedule_fingerprint = schedule_fingerprint(seed, &fixture_id);
        let expected_vs_actual = format!("{} != {}", first.oracle, first.subject);
        Self {
            schema_version: SCHEMA_VERSION.to_owned(),
            failure_type,
            seed,
            fixture_id: fixture_id.clone(),
            schedule_fingerprint,
            exact_repro_command: repro_command(&fixture_id),
            first_divergence_jsonptr: FIRST_DIVERGENCE_JSONPTR.to_owned(),
            artifact_sha256: Vec::new(),
            expected_vs_actual,
            classification,
            environment: capture_environment(),
            failure: FailureSection {
                first_divergence: first,
            },
            partial: false,
            bead_id: None,
        }
    }

    /// Mark capture as incomplete but still write-ready.
    pub fn into_partial(mut self) -> Self {
        self.partial = true;
        self
    }

    pub fn with_repro_command(mut self, command: impl Into<String>) -> Self {
        self.exact_repro_command = command.into();
        self
    }

    /// Always writes a manifest. Capture holes become `partial: true`.
    pub fn write_manifest(&self, dir: &Path) -> Result<PathBuf, String> {
        fs::create_dir_all(dir).map_err(|error| format!("mkdir {}: {error}", dir.display()))?;
        let name = format!("{}.bundle.json", sanitize_id(&self.fixture_id));
        let path = dir.join(name);
        match write_json(&path, self) {
            Ok(()) => Ok(path),
            Err(error) => {
                let fallback = PartialOnDisk {
                    schema_version: SCHEMA_VERSION,
                    seed: self.seed,
                    fixture_id: &self.fixture_id,
                    schedule_fingerprint: &self.schedule_fingerprint,
                    exact_repro_command: &self.exact_repro_command,
                    first_divergence_jsonptr: FIRST_DIVERGENCE_JSONPTR,
                    environment: &self.environment,
                    partial: true,
                    write_error: error,
                };
                write_json(&path, &fallback)
                    .map_err(|second| format!("partial bundle write failed: {second}"))?;
                Ok(path)
            }
        }
    }
}

#[derive(Serialize)]
struct PartialOnDisk<'a> {
    schema_version: &'static str,
    seed: u64,
    fixture_id: &'a str,
    schedule_fingerprint: &'a str,
    exact_repro_command: &'a str,
    first_divergence_jsonptr: &'static str,
    environment: &'a Environment,
    partial: bool,
    write_error: String,
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn schedule_fingerprint(seed: u64, fixture_id: &str) -> String {
    sha256_hex(format!("{seed}:{fixture_id}").as_bytes())
}

pub fn repro_command(fixture_id: &str) -> String {
    format!("cargo test -p zerostack-harness -- --test-threads=1 {fixture_id} --exact --nocapture")
}

pub fn capture_environment() -> Environment {
    Environment {
        git_sha: git_sha(),
        toolchain_version: std::env::var("RUSTC_VERSION").unwrap_or_else(|_| "unknown".into()),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        feature_flags: Vec::new(),
    }
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mismatch::MismatchClassification;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("zs-failure-bundle-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch");
        path
    }

    #[test]
    fn jsonptr_is_first_divergence() {
        let bundle = FailureBundle::new(
            FailureType::Divergence,
            0x11,
            "fixture-a",
            MismatchClassification::TrueDivergence {
                description: "row 0".into(),
            },
            FirstDivergence {
                kind: "canonical".into(),
                subject: "left".into(),
                oracle: "right".into(),
                byte_offset: Some(0),
            },
        );
        assert_eq!(bundle.first_divergence_jsonptr, FIRST_DIVERGENCE_JSONPTR);
        assert_eq!(bundle.schema_version, SCHEMA_VERSION);
        assert!(!bundle.schedule_fingerprint.is_empty());
        assert!(bundle.exact_repro_command.contains("fixture-a"));
    }

    #[test]
    fn never_skips_manifest_when_partial() {
        let dir = scratch();
        let path = FailureBundle::new(
            FailureType::WalRecovery,
            7,
            "partial-capture",
            MismatchClassification::TrueDivergence {
                description: "crash recovery".into(),
            },
            FirstDivergence {
                kind: "root".into(),
                subject: "".into(),
                oracle: "absent-or-complete".into(),
                byte_offset: None,
            },
        )
        .into_partial()
        .write_manifest(&dir)
        .expect("partial still writes");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains(FIRST_DIVERGENCE_JSONPTR));
        assert!(text.contains("\"partial\": true"));
        assert!(text.contains("\"seed\": 7"));
        let _ = fs::remove_dir_all(dir);
    }
}
