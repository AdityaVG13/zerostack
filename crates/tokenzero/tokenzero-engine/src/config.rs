//! Engine configuration, env toggles, and serve-flight guard.

use crate::admission::{AdmissionEstimator, AdmissionPolicy};
use crate::cache_crossover::EmissionCrossoverConfig;
use crate::session::ServeKey;
use crate::{
    DEFAULT_MCP_IDLE_TIMEOUT_SECS, DEFAULT_SHELL_TIMEOUT_SECS, DIFF_READS_ENV,
    MAX_MCP_IDLE_TIMEOUT_SECS, MAX_SHELL_TIMEOUT_SECS, RG_PATH_ENV, SEARCH_BACKEND_ENV,
    SESSION_DEDUP_ENV, TokenZeroEngine,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokenzero_core::{McpToolSurface, Mode};
use tokenzero_runtime::RunOutputPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchBackend {
    #[default]
    Auto,
    Rg,
    Internal,
}

impl SearchBackend {
    pub fn from_env() -> Self {
        match std::env::var(SEARCH_BACKEND_ENV).ok().as_deref() {
            Some("rg") => Self::Rg,
            Some("internal") => Self::Internal,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub allowed_roots: Vec<PathBuf>,
    /// Root against which relative paths are resolved before the allowlist check.
    pub call_root: PathBuf,
    pub cache_path: PathBuf,
    pub max_visible_tokens: usize,
    /// Local payloads larger than this default to an exact ref in Auto mode.
    pub capsule_exact_ref_threshold_bytes: usize,
    /// Capsule admission policy (ZS-VIEW-006). Default `ByteThreshold`
    /// reproduces the legacy fixed-threshold rule exactly.
    pub admission_policy: AdmissionPolicy,
    /// Horizon-cost estimator parameters; consulted only when
    /// `admission_policy == HorizonCost`.
    pub admission_estimator: AdmissionEstimator,
    /// Emission-path cache crossover knobs (ZS-CACHE-006). Defaults
    /// reproduce the historical `pick_cheaper` emission exactly.
    pub emission_crossover: EmissionCrossoverConfig,
    pub mode: Mode,
    pub shell_timeout: Duration,
    pub shell_capture_bytes: usize,
    pub shell_spill_bytes: usize,
    pub shell_inline_budget: usize,
    pub mcp_idle_timeout: Option<Duration>,
    pub search_backend: SearchBackend,
    /// Explicit rg binary path (`TOKENZERO_RG_PATH`); skips the PATH lookup.
    /// Tests set this field directly instead of mutating process-global env.
    pub rg_path_override: Option<PathBuf>,
    /// Session redundancy layer master switch (seen-set dedup; docs/codemode.md
    /// §5a). Default comes from `TOKENZERO_MCP_DEDUP`, parsed once at
    /// construction; tests set this field instead of mutating env.
    pub session_dedup: bool,
    /// Diff-aware re-reads (docs/codemode.md §5b). Default comes from
    /// `TOKENZERO_MCP_DIFF_READS`, parsed once at construction; only
    /// consulted while `session_dedup` is on.
    pub diff_reads: bool,
    /// Explicit curl binary for `tz_fetch` (`TOKENZERO_CURL_PATH`); tests set
    /// this field directly instead of mutating process-global env.
    pub curl_path_override: Option<PathBuf>,
    /// `tz_fetch` network access is off by default (SSRF surface); opt in
    /// with `TOKENZERO_FETCH=on`. Tests set this field directly.
    pub fetch_enabled: bool,
    /// Hosts (suffix match) explicitly trusted for fetch; they bypass the
    /// post-DNS IP checks. From `TOKENZERO_FETCH_ALLOW`, comma-separated.
    pub fetch_allow_hosts: Vec<String>,
    /// Hosts (suffix match) always refused. From `TOKENZERO_FETCH_DENY`.
    pub fetch_deny_hosts: Vec<String>,
    /// Installed MCP tool surface (`TOKENZERO_MCP_TOOL_SURFACE`).
    pub tool_surface: McpToolSurface,
    /// Programmatic shareable usage-telemetry choice; `None` defers to the environment.
    /// When enabled, only `{execution_path, raw_tokens, spent_tokens}` may be recorded.
    /// There is no exporter/upload path (`exporter=none`).
    pub telemetry_enabled: Option<bool>,
    /// RATC retry/fail weights. ADVISORY until E5 measurement fills real values.
    pub ratc: RatcWeights,
    /// Corridor handle/selector/CAS estimates (h, q, c). ADVISORY until E5.
    pub corridor: CorridorEstimates,
}

impl EngineConfig {
    pub fn for_root(root: &Path) -> Self {
        let output_policy = RunOutputPolicy::default();
        Self {
            allowed_roots: vec![root.to_path_buf()],
            call_root: root.to_path_buf(),
            cache_path: crate::workspace::default_recovery_cache_path(root),
            max_visible_tokens: 4000,
            capsule_exact_ref_threshold_bytes: capsule_exact_ref_threshold_from_env(),
            admission_policy: AdmissionPolicy::ByteThreshold,
            admission_estimator: AdmissionEstimator {
                exact_ref_threshold_bytes: capsule_exact_ref_threshold_from_env(),
                ..AdmissionEstimator::default()
            },
            emission_crossover: EmissionCrossoverConfig::default(),
            mode: Mode::Auto,
            shell_timeout: default_shell_timeout(),
            shell_capture_bytes: output_policy.per_stream_capture_bytes,
            shell_spill_bytes: output_policy.spill_threshold_bytes,
            shell_inline_budget: shell_inline_budget_from_env(),
            mcp_idle_timeout: default_mcp_idle_timeout(),
            search_backend: SearchBackend::from_env(),
            rg_path_override: std::env::var_os(RG_PATH_ENV).map(PathBuf::from),
            session_dedup: session_dedup_default(),
            diff_reads: diff_reads_default(),
            curl_path_override: std::env::var_os(CURL_PATH_ENV).map(PathBuf::from),
            fetch_enabled: env_opt_in(FETCH_ENABLED_ENV),
            fetch_allow_hosts: env_host_list(FETCH_ALLOW_ENV),
            fetch_deny_hosts: env_host_list(FETCH_DENY_ENV),
            tool_surface: mcp_tool_surface_from_env(),
            telemetry_enabled: None,
            ratc: ratc_weights_from_env().unwrap_or_else(|err| panic!("TOKENZERO_RATC: {err}")),
            corridor: corridor_estimates_from_env()
                .unwrap_or_else(|err| panic!("TOKENZERO_CORRIDOR: {err}")),
        }
    }
}

pub fn mcp_tool_surface_from_env() -> McpToolSurface {
    std::env::var(McpToolSurface::ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

pub const TELEMETRY_ENV: &str = "TOKENZERO_TELEMETRY";
pub const FETCH_ENABLED_ENV: &str = "TOKENZERO_FETCH";
pub const FETCH_ALLOW_ENV: &str = "TOKENZERO_FETCH_ALLOW";
pub const FETCH_DENY_ENV: &str = "TOKENZERO_FETCH_DENY";
pub const SHELL_INLINE_BUDGET_ENV: &str = "TOKENZERO_SHELL_INLINE_BUDGET";
pub const DEFAULT_SHELL_INLINE_BUDGET: usize = 2000;
pub const CAPSULE_EXACT_REF_THRESHOLD_ENV: &str = "TOKENZERO_CAPSULE_EXACT_REF_THRESHOLD_BYTES";
pub const DEFAULT_CAPSULE_EXACT_REF_THRESHOLD_BYTES: usize = 40 * 1024;
pub const RATC_ENV: &str = "TOKENZERO_RATC";
pub const CORRIDOR_ENV: &str = "TOKENZERO_CORRIDOR";
/// RATC/corridor numbers are placeholders until E5 measures them.
pub const RATC_STATUS_ADVISORY: &str = "advisory_until_e5";

/// Retry/failure weights for `ratc = visible + expand + rho_fail*retries + lambda_fail*fails`.
/// Defaults stay 0 so an unmeasured penalty is never fabricated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatcWeights {
    pub rho_fail: f64,
    pub lambda_fail: f64,
}

impl Default for RatcWeights {
    fn default() -> Self {
        Self {
            rho_fail: 0.0,
            lambda_fail: 0.0,
        }
    }
}

/// Corridor estimates in tokens: handle (h), selector serialization (q), CAS round-trip (c).
/// Defaults match the E5 (40, 20) note; `c` stays 0 until measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorridorEstimates {
    pub h: f64,
    pub q: f64,
    pub c: f64,
}

impl Default for CorridorEstimates {
    fn default() -> Self {
        Self {
            h: 40.0,
            q: 20.0,
            c: 0.0,
        }
    }
}

fn json_object_with_allowed_keys(
    raw: &str,
    allowed: &[&str],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| format!("invalid json: {err}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    for key in object.keys() {
        if !allowed.iter().any(|allowed| *allowed == key) {
            return Err(format!("unknown key '{key}'"));
        }
    }
    Ok(object.clone())
}

fn json_f64(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let Some(n) = value
        .as_f64()
        .or_else(|| value.as_u64().map(|n| n as f64))
        .or_else(|| value.as_i64().map(|n| n as f64))
    else {
        return Err(format!("{key} must be a number"));
    };
    if !n.is_finite() || n < 0.0 {
        return Err(format!("{key} must be a finite number >= 0"));
    }
    Ok(Some(n))
}

/// Parse `{"rho_fail":..,"lambda_fail":..}`. Unknown keys fail loud.
pub fn parse_ratc_weights(raw: &str) -> Result<RatcWeights, String> {
    let object = json_object_with_allowed_keys(raw, &["rho_fail", "lambda_fail"])?;
    Ok(RatcWeights {
        rho_fail: json_f64(&object, "rho_fail")?.unwrap_or(0.0),
        lambda_fail: json_f64(&object, "lambda_fail")?.unwrap_or(0.0),
    })
}

/// Parse `{"h":..,"q":..,"c":..}`. Unknown keys fail loud.
pub fn parse_corridor_estimates(raw: &str) -> Result<CorridorEstimates, String> {
    let object = json_object_with_allowed_keys(raw, &["h", "q", "c"])?;
    let defaults = CorridorEstimates::default();
    Ok(CorridorEstimates {
        h: json_f64(&object, "h")?.unwrap_or(defaults.h),
        q: json_f64(&object, "q")?.unwrap_or(defaults.q),
        c: json_f64(&object, "c")?.unwrap_or(defaults.c),
    })
}

pub fn ratc_weights_from_env() -> Result<RatcWeights, String> {
    match std::env::var(RATC_ENV) {
        Ok(raw) if !raw.trim().is_empty() => parse_ratc_weights(&raw),
        _ => Ok(RatcWeights::default()),
    }
}

pub fn corridor_estimates_from_env() -> Result<CorridorEstimates, String> {
    match std::env::var(CORRIDOR_ENV) {
        Ok(raw) if !raw.trim().is_empty() => parse_corridor_estimates(&raw),
        _ => Ok(CorridorEstimates::default()),
    }
}

fn matches_env_value(value: &str, accepted: &[&str]) -> bool {
    accepted
        .iter()
        .any(|word| value.trim().eq_ignore_ascii_case(word))
}

/// Parse a default-off opt-in value without touching process-global state.
pub fn telemetry_env_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches_env_value(value, &["1", "on", "true", "yes"]))
}

/// Resolve shareable usage-telemetry permission from highest to lowest precedence.
pub fn resolve_telemetry(
    cli_opt_in: bool,
    cli_opt_out: bool,
    programmatic: Option<bool>,
    env_value: Option<&str>,
) -> bool {
    if cli_opt_out {
        false
    } else if cli_opt_in {
        true
    } else {
        programmatic.unwrap_or_else(|| telemetry_env_enabled(env_value))
    }
}

/// Opt-in toggle parse: only `1`/`on`/`true`/`yes` (case-insensitive) enable.
pub(crate) fn env_opt_in(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| matches_env_value(&value, &["1", "on", "true", "yes"]))
}

pub(crate) fn env_host_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

const CURL_PATH_ENV: &str = "TOKENZERO_CURL_PATH";

pub(crate) fn session_dedup_default() -> bool {
    env_toggle_enabled(SESSION_DEDUP_ENV)
}

/// RAII guard releasing a set of in-flight ServeKeys when the serve finishes
/// (or unwinds), waking any request waiting on those keys.
pub(crate) struct ServeFlight<'a> {
    pub(crate) engine: &'a TokenZeroEngine,
    pub(crate) keys: Vec<ServeKey>,
}

impl Drop for ServeFlight<'_> {
    fn drop(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        let (lock, cvar) = &self.engine.in_flight;
        let mut set = lock.lock().unwrap_or_else(|p| p.into_inner());
        for key in &self.keys {
            set.remove(key);
        }
        drop(set);
        cvar.notify_all();
    }
}

pub(crate) fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("tz-{}-{nanos:x}", std::process::id())
}

pub(crate) fn diff_reads_default() -> bool {
    env_toggle_enabled(DIFF_READS_ENV)
}

pub fn shell_inline_budget_from_env() -> usize {
    std::env::var(SHELL_INLINE_BUDGET_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_SHELL_INLINE_BUDGET)
}

pub fn capsule_exact_ref_threshold(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CAPSULE_EXACT_REF_THRESHOLD_BYTES)
}

pub fn capsule_exact_ref_threshold_from_env() -> usize {
    capsule_exact_ref_threshold(
        std::env::var(CAPSULE_EXACT_REF_THRESHOLD_ENV)
            .ok()
            .as_deref(),
    )
}

/// Opt-out toggle parse: unset means enabled; `0`/`off`/`false`/`no`
/// (case-insensitive) disable.
pub(crate) fn env_toggle_enabled(name: &str) -> bool {
    std::env::var(name).map_or(true, |value| {
        !matches_env_value(&value, &["0", "off", "false", "no"])
    })
}

pub fn default_shell_timeout() -> Duration {
    let from_env = std::env::var("TOKENZERO_SHELL_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    shell_timeout_from_secs(from_env)
}

pub fn shell_timeout_from_secs(seconds: Option<u64>) -> Duration {
    let seconds = seconds
        .unwrap_or(DEFAULT_SHELL_TIMEOUT_SECS)
        .clamp(1, MAX_SHELL_TIMEOUT_SECS);
    Duration::from_secs(seconds)
}

/// Millisecond-precision counterpart of [`shell_timeout_from_secs`].
///
/// Callers that spell a shell deadline in milliseconds must get the deadline
/// they asked for, including sub-second ones. Routing them through the seconds
/// path would floor a 300ms request to 0 and then clamp it back up to the 1s
/// minimum, which is a *different* bound than the caller requested -- the
/// silent substitution this whole contract exists to prevent.
pub fn shell_timeout_from_millis(millis: Option<u64>) -> Duration {
    let Some(millis) = millis else {
        return Duration::from_secs(DEFAULT_SHELL_TIMEOUT_SECS);
    };
    // A zero/negative-ish request is not "no timeout"; it is an unusable value.
    // Keep it representable rather than silently disabling the deadline.
    let millis = millis.clamp(1, MAX_SHELL_TIMEOUT_SECS.saturating_mul(1_000));
    Duration::from_millis(millis)
}

pub fn default_mcp_idle_timeout() -> Option<Duration> {
    let from_env = std::env::var("TOKENZERO_MCP_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    mcp_idle_timeout_from_secs(from_env)
}

pub fn mcp_idle_timeout_from_secs(seconds: Option<u64>) -> Option<Duration> {
    let seconds = seconds.unwrap_or(DEFAULT_MCP_IDLE_TIMEOUT_SECS);
    if seconds == 0 {
        return None;
    }
    Some(Duration::from_secs(
        seconds.clamp(1, MAX_MCP_IDLE_TIMEOUT_SECS),
    ))
}
