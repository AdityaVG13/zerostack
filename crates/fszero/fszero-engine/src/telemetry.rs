//! Shareable telemetry permission (default off) and local token counters.
//!
//! Local operational counters (CodeMode `codemode/telemetry`, recovery metrics,
//! doctor reports) remain local and are not this module. Shareable telemetry is
//! a separate opt-in permission whose only allowlisted payload fields are
//! aggregate `raw_tokens` and `saved_tokens` plus schema/version. FSZero has no
//! telemetry exporter: opting in or inspecting never uploads.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Environment opt-in for shareable telemetry inspection permission.
pub const TELEMETRY_ENV: &str = "FSZERO_TELEMETRY";

/// Closed schema id for the shareable dry-run payload.
pub const TELEMETRY_SCHEMA: &str = "fszero.telemetry";

/// Local-only counter file schema (never exported as shareable telemetry).
pub const LOCAL_COUNTERS_SCHEMA: &str = "fszero.local_counters";

/// Status string proving no exporter / upload path exists.
pub const TELEMETRY_EXPORTER: &str = "none";

/// Relative path under an FSZero store root for local token aggregates.
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

/// Read `telemetry` from a config JSON object (`.zerostack/config.json` or
/// `.fszero/config.json`). Missing key or non-boolean values yield `None`.
pub fn telemetry_from_config_value(value: &Value) -> Option<bool> {
    match value.get(TELEMETRY_CONFIG_KEY) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Resolve the durable store root used for telemetry config + local counters.
///
/// Prefer unified `.zerostack/` (or `ZEROSTACK_STORE_ROOT`); else legacy
/// `<repo>/.fszero`.
pub fn telemetry_store_root(repo_root: &Path) -> PathBuf {
    super::zerostack_store::zerostack_store_or_detect(repo_root)
        .unwrap_or_else(|| repo_root.join(".fszero"))
}

/// Load shareable-telemetry config override from `config.json` under the store.
pub fn load_telemetry_config(store_root: &Path) -> Option<bool> {
    let path = store_root.join("config.json");
    let bytes = fs::read(&path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    telemetry_from_config_value(&value)
}

/// Local operational token aggregates. Never include paths, queries, refs, or ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

    fn from_json(value: &Value) -> Option<Self> {
        let raw_tokens = value.get("raw_tokens")?.as_u64()?;
        let saved_tokens = value.get("saved_tokens")?.as_u64()?;
        let schema = value
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or(LOCAL_COUNTERS_SCHEMA)
            .to_string();
        Some(Self {
            schema: if schema.is_empty() {
                LOCAL_COUNTERS_SCHEMA.to_string()
            } else {
                schema
            },
            raw_tokens,
            saved_tokens,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "schema": if self.schema.is_empty() { LOCAL_COUNTERS_SCHEMA } else { self.schema.as_str() },
            "raw_tokens": self.raw_tokens, "saved_tokens": self.saved_tokens,
        })
    }
}

/// Exact shareable telemetry payload allowlist. Closed typed schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryPayload {
    pub schema: &'static str,
    pub version: &'static str,
    pub raw_tokens: u64,
    pub saved_tokens: u64,
}

/// Dry-run inspection result: permission + exporter status + exact payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryInspection {
    pub enabled: bool,
    pub exporter: &'static str,
    pub payload: TelemetryPayload,
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
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            LocalTokenCounters::from_json(&value).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "local counters missing raw_tokens/saved_tokens",
                )
            })
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
    let bytes = serde_json::to_vec_pretty(&out.to_json())
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

/// Summarize local counters into the exact inspect/dry-run envelope.
pub fn inspect_telemetry(store_root: &Path, enabled: bool) -> io::Result<TelemetryInspection> {
    let counters = read_local_counters(store_root)?;
    Ok(TelemetryInspection {
        enabled,
        exporter: TELEMETRY_EXPORTER,
        payload: shareable_payload_from_counters(&counters),
    })
}

/// Truthful exporter API: FSZero has no shareable telemetry exporter.
///
/// Always returns `None`. Exists so callers and tests can assert that enabling
/// permission never produces an outbound payload or network send.
pub fn export_shareable_telemetry(_inspection: &TelemetryInspection) -> Option<TelemetryPayload> {
    None
}

/// Serialize inspection JSON for CLI dry-run output.
///
/// R-IDEA-007 / fszero-8bu7.7: top-level privacy banner always names
/// `exporter=none`, default-off opt-in, and that inspect never uploads.
pub fn inspection_json(inspection: &TelemetryInspection) -> Value {
    json!({
        "privacy": {
            "exporter": TELEMETRY_EXPORTER,
            "default_opt_in": false,
            "opt_in_env": TELEMETRY_ENV,
            "shareable_when_enabled": true,
            "upload": "never",
            "note": "Shareable telemetry is default-off. exporter is always none; opt-in only unlocks local aggregate raw_tokens/saved_tokens inspect, never network upload.",
        },
        "enabled": inspection.enabled,
        "exporter": inspection.exporter,
        "payload": {
            "schema": inspection.payload.schema,
            "version": inspection.payload.version,
            "raw_tokens": inspection.payload.raw_tokens,
            "saved_tokens": inspection.payload.saved_tokens,
        },
    })
}
