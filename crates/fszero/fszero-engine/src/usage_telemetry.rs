//! Opt-in usage telemetry: token-accounting only.
//!
//! Disabled by default. When explicitly enabled, FSZero may persist only
//! closed `{execution_path, raw_tokens, spent_tokens}` records. This path never
//! stores prompts, responses, commands, paths, refs, tool names, errors,
//! durations, timestamps, or identifiers.
//!
//! In-session CodeMode ack fields (`codemode/telemetry` logical_ops, etc.) are
//! protocol metadata and are not this module.
//!
//! ## Counter semantics
//!
//! - `raw_tokens`: uncompressed source token mass from the authoritative
//!   accounting path (MCP result body mass; CodeMode plan `raw_tokens`).
//! - `spent_tokens`: tokens actually presented to the caller (MCP visible ack /
//!   CodeMode `visible_tokens`).
//!
//! The contract requires `spent_tokens <= raw_tokens`. Records that violate it
//! are rejected and not persisted.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::telemetry::telemetry_env_enabled;
use crate::telemetry::{TELEMETRY_ENV, resolve_telemetry};

/// Execution surface that produced the token-accounting sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionPath {
    Mcp,
    Codemode,
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

/// JSONL path beside the recovery cache / store db for opt-in usage records.
pub fn usage_telemetry_path_for_cache(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name("usage-telemetry.jsonl")
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

/// Record path accounting when opted in. Fail-open on I/O; reject bad contracts.
pub fn record_path_accounting(
    execution_path: ExecutionPath,
    cache_path: &Path,
    enabled: bool,
    raw_tokens: usize,
    spent_tokens: usize,
) {
    if !enabled {
        return;
    }
    let Ok(record) = UsageRecord::try_new(
        execution_path,
        u64::try_from(raw_tokens).unwrap_or(u64::MAX),
        u64::try_from(spent_tokens).unwrap_or(u64::MAX),
    ) else {
        return;
    };
    let path = usage_telemetry_path_for_cache(cache_path);
    let _ = record_usage(&path, true, &record);
}

/// Record MCP accounting when opted in. Fail-open on I/O; reject bad contracts.
pub fn record_mcp_accounting(
    cache_path: &Path,
    enabled: bool,
    raw_tokens: usize,
    spent_tokens: usize,
) {
    record_path_accounting(
        ExecutionPath::Mcp,
        cache_path,
        enabled,
        raw_tokens,
        spent_tokens,
    );
}

/// Record CodeMode accounting when opted in. Fail-open on I/O; reject bad contracts.
pub fn record_codemode_accounting(
    cache_path: &Path,
    enabled: bool,
    raw_tokens: usize,
    spent_tokens: usize,
) {
    record_path_accounting(
        ExecutionPath::Codemode,
        cache_path,
        enabled,
        raw_tokens,
        spent_tokens,
    );
}

/// Opt-in path accounting from visible text mass (MCP / CodeMode result bodies).
/// No-op when telemetry disabled or `cache_path` is missing.
pub fn record_opt_in_visible_accounting(
    execution_path: ExecutionPath,
    cache_path: Option<&Path>,
    raw_text: &str,
    spent_text: &str,
) {
    if !usage_telemetry_enabled(None) {
        return;
    }
    let Some(cache_path) = cache_path else {
        return;
    };
    let raw_tokens = super::session::estimate_visible_tokens(raw_text).max(1);
    let spent_tokens = super::session::estimate_visible_tokens(spent_text)
        .max(1)
        .min(raw_tokens);
    record_path_accounting(execution_path, cache_path, true, raw_tokens, spent_tokens);
}

/// Inspect opt-in usage telemetry. Never uploads; `exporter` is always `none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageTelemetryInspection {
    pub enabled: bool,
    pub exporter: &'static str,
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
