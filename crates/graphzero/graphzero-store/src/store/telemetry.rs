//! Shareable usage-telemetry permission (default off) and local token counters.
//!
//! Local operational counters (CodeMode `telemetry_ref`, query `accounting`,
//! daemon metrics, and `telemetry/local_counters.json`) remain local and are
//! not shareable usage telemetry. When opted in, durable usage records live in
//! [`super::usage_telemetry`] as closed `{execution_path, raw_tokens,
//! spent_tokens}` JSONL only. GraphZero has no telemetry exporter: opting in
//! or inspecting never uploads.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment opt-in for shareable telemetry inspection permission.
pub const TELEMETRY_ENV: &str = "GRAPHZERO_TELEMETRY";

/// Closed schema id for the shareable dry-run payload.
pub const TELEMETRY_SCHEMA: &str = "graphzero.telemetry.v1";

/// Local-only counter file schema (never exported as shareable telemetry).
pub const LOCAL_COUNTERS_SCHEMA: &str = "graphzero.local_counters.v1";

/// Status string proving no exporter / upload path exists.
pub const TELEMETRY_EXPORTER: &str = "none";

/// Relative path under a GraphZero store root for local token aggregates.
pub const LOCAL_COUNTERS_REL: &str = "telemetry/local_counters.json";

/// Config key / file field name for shareable telemetry opt-in.
pub const TELEMETRY_CONFIG_KEY: &str = "telemetry";

fn matches_env_value(value: &str, accepted: &[&str]) -> bool {
    accepted
        .iter()
        .any(|word| value.trim().eq_ignore_ascii_case(word))
}

/// Parse a default-off opt-in value without touching process-global state.
pub fn telemetry_env_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches_env_value(value, &["1", "on", "true", "yes"]))
}

/// Resolve shareable telemetry permission from highest to lowest precedence:
/// CLI opt-out, CLI opt-in, config/programmatic override, environment, then off.
pub fn resolve_telemetry(
    cli_opt_in: bool,
    cli_opt_out: bool,
    config: Option<bool>,
    env_value: Option<&str>,
) -> bool {
    if cli_opt_out {
        false
    } else if cli_opt_in {
        true
    } else {
        config.unwrap_or_else(|| telemetry_env_enabled(env_value))
    }
}

/// Read `telemetry` from a GraphZero config JSON object (`.graphzero/config.json`).
///
/// Missing file, missing key, or non-boolean values yield `None` (defer to env).
pub fn telemetry_from_config_value(value: &serde_json::Value) -> Option<bool> {
    match value.get(TELEMETRY_CONFIG_KEY) {
        Some(serde_json::Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Load shareable-telemetry config override from `config.json` next to the store.
pub fn load_telemetry_config(store_root: &Path) -> Option<bool> {
    let path = store_root.join("config.json");
    let bytes = fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    telemetry_from_config_value(&value)
}

/// Local operational token aggregates. Never include paths, queries, refs, or ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LocalTokenCounters {
    pub schema: String,
    pub raw_tokens: u64,
    pub saved_tokens: u64,
}

impl LocalTokenCounters {
    pub fn empty() -> Self {
        Self {
            schema: LOCAL_COUNTERS_SCHEMA.to_string(),
            raw_tokens: 0,
            saved_tokens: 0,
        }
    }
}

/// Legacy aggregate payload view (local counters only; not the durable usage
/// JSONL schema). Kept for local dry-run helpers that still surface aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetryPayload {
    pub schema: &'static str,
    pub version: &'static str,
    pub raw_tokens: u64,
    pub saved_tokens: u64,
}

/// Dry-run inspection: permission + exporter status + allowlisted usage records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetryInspection {
    pub enabled: bool,
    pub exporter: &'static str,
    pub records: Vec<super::usage_telemetry::UsageRecord>,
}

/// Path to the local counters file under a store root.
pub fn local_counters_path(store_root: &Path) -> PathBuf {
    store_root.join(LOCAL_COUNTERS_REL)
}

/// Read local token aggregates; missing file is empty counters (not an error).
pub fn read_local_counters(store_root: &Path) -> io::Result<LocalTokenCounters> {
    let path = local_counters_path(store_root);
    match fs::read(&path) {
        Ok(bytes) => {
            let mut counters: LocalTokenCounters = serde_json::from_slice(&bytes)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            if counters.schema.is_empty() {
                counters.schema = LOCAL_COUNTERS_SCHEMA.to_string();
            }
            Ok(counters)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(LocalTokenCounters::empty()),
        Err(err) => Err(err),
    }
}

/// Persist local token aggregates (local only; never shareable export).
pub fn write_local_counters(store_root: &Path, counters: &LocalTokenCounters) -> io::Result<()> {
    let path = local_counters_path(store_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = counters.clone();
    if out.schema.is_empty() {
        out.schema = LOCAL_COUNTERS_SCHEMA.to_string();
    }
    let bytes = serde_json::to_vec_pretty(&out)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(path, bytes)
}

/// Add raw/saved token mass to local counters. Does not enable shareable export.
pub fn record_local_tokens(
    store_root: &Path,
    raw_tokens: u64,
    saved_tokens: u64,
) -> io::Result<()> {
    let mut counters = read_local_counters(store_root)?;
    counters.raw_tokens = counters.raw_tokens.saturating_add(raw_tokens);
    counters.saved_tokens = counters.saved_tokens.saturating_add(saved_tokens);
    write_local_counters(store_root, &counters)
}

/// Build the exact shareable dry-run payload from local counters.
pub fn shareable_payload_from_counters(counters: &LocalTokenCounters) -> TelemetryPayload {
    TelemetryPayload {
        schema: TELEMETRY_SCHEMA,
        version: env!("CARGO_PKG_VERSION"),
        raw_tokens: counters.raw_tokens,
        saved_tokens: counters.saved_tokens,
    }
}

/// Inspect opt-in usage JSONL (empty records when disabled or nothing recorded).
pub fn inspect_telemetry(store_root: &Path, enabled: bool) -> io::Result<TelemetryInspection> {
    let path = super::usage_telemetry::usage_telemetry_path_for_store(store_root);
    let inspection = super::usage_telemetry::inspect_usage_telemetry(&path, enabled)?;
    Ok(TelemetryInspection {
        enabled: inspection.enabled,
        exporter: TELEMETRY_EXPORTER,
        records: inspection.records,
    })
}

/// Truthful exporter API: GraphZero has no shareable telemetry exporter.
///
/// Always returns `None`. Exists so callers and tests can assert that enabling
/// permission never produces an outbound payload or network send.
pub fn export_shareable_telemetry(_inspection: &TelemetryInspection) -> Option<TelemetryPayload> {
    None
}

/// Serialize inspection JSON for CLI dry-run output.
pub fn inspection_json(inspection: &TelemetryInspection) -> serde_json::Value {
    serde_json::json!({
        "enabled": inspection.enabled,
        "exporter": inspection.exporter,
        "records": inspection.records,
    })
}
