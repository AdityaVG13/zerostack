//! Opt-in usage telemetry: token-accounting only.
//!
//! Disabled by default. When explicitly enabled, TokenZero may persist only
//! closed `{execution_path, raw_tokens, spent_tokens}` records. This path never
//! stores prompts, responses, commands, paths, refs, tool names, errors,
//! durations, timestamps, or identifiers.
//!
//! ## Counter semantics
//!
//! - `raw_tokens`: uncompressed source token mass from the authoritative
//!   accounting path (`Accounting.raw_tokens` for MCP; CodeMode plan
//!   `raw_tokens` for CodeMode).
//! - `spent_tokens`: tokens actually presented to the caller
//!   (`Accounting.visible_tokens` / CodeMode `visible_tokens`).
//!
//! The contract requires `spent_tokens <= raw_tokens`. Records that violate it
//! are rejected and not persisted.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::config::{TELEMETRY_ENV, resolve_telemetry, telemetry_env_enabled};

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

/// JSONL path beside the recovery cache for opt-in usage records.
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

/// Record MCP accounting when opted in. Fail-open on I/O; reject bad contracts.
pub fn record_mcp_accounting(
    cache_path: &Path,
    enabled: bool,
    raw_tokens: usize,
    spent_tokens: usize,
) {
    if !enabled {
        return;
    }
    let Ok(record) = UsageRecord::try_new(
        ExecutionPath::Mcp,
        u64::try_from(raw_tokens).unwrap_or(u64::MAX),
        u64::try_from(spent_tokens).unwrap_or(u64::MAX),
    ) else {
        return;
    };
    let path = usage_telemetry_path_for_cache(cache_path);
    let _ = record_usage(&path, true, &record);
}

/// Record CodeMode accounting when opted in. Fail-open on I/O; reject bad contracts.
pub fn record_codemode_accounting(
    cache_path: &Path,
    enabled: bool,
    raw_tokens: usize,
    spent_tokens: usize,
) {
    if !enabled {
        return;
    }
    let Ok(record) = UsageRecord::try_new(
        ExecutionPath::Codemode,
        u64::try_from(raw_tokens).unwrap_or(u64::MAX),
        u64::try_from(spent_tokens).unwrap_or(u64::MAX),
    ) else {
        return;
    };
    let path = usage_telemetry_path_for_cache(cache_path);
    let _ = record_usage(&path, true, &record);
}

/// Inspect opt-in usage telemetry. Never uploads; `exporter` is always `none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetryInspection {
    pub enabled: bool,
    pub exporter: &'static str,
    /// Allowlisted records only; empty when disabled or nothing recorded.
    pub records: Vec<UsageRecord>,
}

pub fn inspect_usage_telemetry(path: &Path, enabled: bool) -> io::Result<TelemetryInspection> {
    let records = if enabled {
        read_records(path)?
    } else {
        Vec::new()
    };
    Ok(TelemetryInspection {
        enabled,
        exporter: "none",
        records,
    })
}

fn append_record<T: Serialize>(path: &Path, record: &T) -> io::Result<()> {
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

/// Coarse operation classes keep telemetry useful without persisting tool arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationClass {
    Read,
    Search,
    Mutate,
    Shell,
    Expand,
    Compact,
    Plan,
    Other,
}

impl OperationClass {
    pub fn classify(name: &str) -> Self {
        match name {
            "read" | "tree" | "list" | "glob" | "inventory" => Self::Read,
            "search" | "grep" | "find" => Self::Search,
            "edit" | "write" | "mutate" => Self::Mutate,
            "shell" => Self::Shell,
            "expand" | "multi_expand" => Self::Expand,
            "compact" | "cache_pack" => Self::Compact,
            "execute_code" | "codemode" => Self::Plan,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionTokens {
    pub raw: u64,
    pub visible: u64,
    pub billed: u64,
    pub cached: u64,
}

impl DirectionTokens {
    pub fn measured(raw: usize, visible: usize, billed: usize, cached: usize) -> Self {
        Self {
            raw: raw as u64,
            visible: visible as u64,
            billed: billed as u64,
            cached: cached as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmplificationRecord {
    pub execution_path: ExecutionPath,
    pub operation_class: OperationClass,
    pub input: DirectionTokens,
    pub output: DirectionTokens,
    pub decision_atoms: u64,
    pub pointer_tokens: u64,
    pub novel_bpe_tokens: u64,
    pub floor_tokens: u64,
    pub amplification_milli: u64,
}

impl AmplificationRecord {
    pub fn new(
        execution_path: ExecutionPath,
        operation_class: OperationClass,
        input: DirectionTokens,
        output: DirectionTokens,
        decision_atoms: usize,
        pointer_tokens: usize,
        novel_bpe_tokens: usize,
    ) -> Self {
        let floor_tokens = (decision_atoms as u64)
            .saturating_add(pointer_tokens as u64)
            .saturating_add(novel_bpe_tokens as u64)
            .max(1);
        let amplification_milli = output.visible.saturating_mul(1_000) / floor_tokens;
        Self {
            execution_path,
            operation_class,
            input,
            output,
            decision_atoms: decision_atoms as u64,
            pointer_tokens: pointer_tokens as u64,
            novel_bpe_tokens: novel_bpe_tokens as u64,
            floor_tokens,
            amplification_milli,
        }
    }
}

pub const TA_REGISTRY: &[(OperationClass, u64)] = &[
    (OperationClass::Read, 8_000),
    (OperationClass::Search, 8_000),
    (OperationClass::Mutate, 4_000),
    (OperationClass::Shell, 12_000),
    (OperationClass::Expand, 4_000),
    (OperationClass::Compact, 4_000),
    (OperationClass::Plan, 16_000),
    (OperationClass::Other, 16_000),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaClassReport {
    pub operation_class: OperationClass,
    pub samples: u64,
    pub max_amplification_milli: u64,
    pub registered_bound_milli: u64,
    pub within_bound: bool,
}

pub fn replay_ta_table(records: &[AmplificationRecord]) -> Vec<TaClassReport> {
    use std::collections::BTreeMap;
    let mut groups = BTreeMap::<OperationClass, (u64, u64)>::new();
    for record in records {
        let entry = groups.entry(record.operation_class).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(record.amplification_milli);
    }
    groups
        .into_iter()
        .map(|(operation_class, (samples, max))| {
            let bound = TA_REGISTRY
                .iter()
                .find(|(class, _)| *class == operation_class)
                .map_or(u64::MAX, |(_, bound)| *bound);
            TaClassReport {
                operation_class,
                samples,
                max_amplification_milli: max,
                registered_bound_milli: bound,
                within_bound: max <= bound,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaCostLockViolation {
    pub operation_class: OperationClass,
    pub observed_amplification_milli: u64,
    pub registered_bound_milli: u64,
}

/// Enforce registered per-operation token-amplification bounds.
/// Callers must treat any violation as a hard CI/release failure.
pub fn enforce_ta_cost_locks(
    records: &[AmplificationRecord],
) -> Result<Vec<TaClassReport>, Vec<TaCostLockViolation>> {
    let table = replay_ta_table(records);
    let violations = table
        .iter()
        .filter(|row| !row.within_bound)
        .map(|row| TaCostLockViolation {
            operation_class: row.operation_class,
            observed_amplification_milli: row.max_amplification_milli,
            registered_bound_milli: row.registered_bound_milli,
        })
        .collect::<Vec<_>>();
    if violations.is_empty() {
        Ok(table)
    } else {
        Err(violations)
    }
}

pub fn record_operation_amplification(
    cache_path: &Path,
    enabled: bool,
    execution_path: ExecutionPath,
    operation: &str,
    input: DirectionTokens,
    output: DirectionTokens,
    pointer_tokens: usize,
) {
    if !enabled {
        return;
    }
    let record = AmplificationRecord::new(
        execution_path,
        OperationClass::classify(operation),
        input,
        output,
        1,
        pointer_tokens,
        input.raw as usize,
    );
    let path = cache_path.with_file_name("token-amplification.jsonl");
    let _ = append_record(&path, &record);
}

