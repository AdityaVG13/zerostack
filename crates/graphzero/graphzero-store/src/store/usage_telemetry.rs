//! Opt-in usage telemetry: token-accounting only. Disabled by default. When explicitly enabled,
//! GraphZero may persist only closed `{execution_path, raw_tokens, spent_tokens}` records.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use super::telemetry::{TELEMETRY_ENV, resolve_telemetry};

/// Relative path under a GraphZero store root for opt-in usage JSONL.
pub const USAGE_TELEMETRY_REL: &str = "telemetry/usage-telemetry.jsonl";

/// In-process execution path that produced the token-accounting sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionPath {
    Domain,
}

/// Complete allowlisted usage-telemetry record. Closed schema — unknown fields
/// fail deserialization so new fields cannot slip in accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRecord {
    pub execution_path: ExecutionPath,
    pub raw_tokens: u64,
    pub spent_tokens: u64,
}

impl UsageRecord {
    /// Build a record when the accounting contract holds; otherwise reject.
    pub fn try_new(
        execution_path: ExecutionPath,
        raw_tokens: u64,
        spent_tokens: u64,
    ) -> Result<Self, UsageTelemetryError> {
        if spent_tokens > raw_tokens {
            return Err(UsageTelemetryError::SpentExceedsRaw {
                spent_tokens,
                raw_tokens,
            });
        }
        Ok(Self {
            execution_path,
            raw_tokens,
            spent_tokens,
        })
    }

    /// Field names that form the complete allowlist (schema/snapshot tests).
    pub const ALLOWLISTED_FIELDS: &'static [&'static str] =
        &["execution_path", "raw_tokens", "spent_tokens"];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageTelemetryError {
    SpentExceedsRaw { spent_tokens: u64, raw_tokens: u64 },
    Io(String),
}

impl std::fmt::Display for UsageTelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpentExceedsRaw {
                spent_tokens,
                raw_tokens,
            } => write!(
                f,
                "spent_tokens ({spent_tokens}) exceeds raw_tokens ({raw_tokens})"
            ),
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for UsageTelemetryError {}

/// Resolve whether usage telemetry may record, using programmatic override then env.
pub fn usage_telemetry_enabled(programmatic: Option<bool>) -> bool {
    let env_value = std::env::var(TELEMETRY_ENV).ok();
    resolve_telemetry(false, false, programmatic, env_value.as_deref())
}

/// JSONL path under the store root for opt-in usage records.
pub fn usage_telemetry_path_for_store(store_root: &Path) -> PathBuf {
    store_root.join(USAGE_TELEMETRY_REL)
}

/// Persist one usage record when enabled. No-op when disabled (creates no file).
pub fn record_usage(
    path: &Path,
    enabled: bool,
    record: &UsageRecord,
) -> Result<(), UsageTelemetryError> {
    if !enabled {
        return Ok(());
    }
    if record.spent_tokens > record.raw_tokens {
        return Err(UsageTelemetryError::SpentExceedsRaw {
            spent_tokens: record.spent_tokens,
            raw_tokens: record.raw_tokens,
        });
    }
    append_record(path, record).map_err(|err| UsageTelemetryError::Io(err.to_string()))
}

/// Inspect opt-in usage telemetry. Never uploads; `exporter` is always `none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageTelemetryInspection {
    pub enabled: bool,
    pub exporter: &'static str,
    /// Allowlisted records only; empty when disabled or nothing recorded.
    pub records: Vec<UsageRecord>,
}

pub fn inspect_usage_telemetry(path: &Path, enabled: bool) -> io::Result<UsageTelemetryInspection> {
    let records = if enabled {
        read_records(path)?
    } else {
        Vec::new()
    };
    Ok(UsageTelemetryInspection {
        enabled,
        exporter: "none",
        records,
    })
}

fn append_record(path: &Path, record: &UsageRecord) -> io::Result<()> {
    let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
    line.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}

fn read_records(path: &Path) -> io::Result<Vec<UsageRecord>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(record) = serde_json::from_str::<UsageRecord>(&line) else {
            continue;
        };
        records.push(record);
    }
    Ok(records)
}
