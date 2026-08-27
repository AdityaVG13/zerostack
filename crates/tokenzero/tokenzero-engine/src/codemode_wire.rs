//! Aggregate binding wire types, limits, and operation classification.
//!
//! These compatibility types preserve the zero.* envelope consumed by the
//! ZeroStack aggregate host. TokenZero owns domain metadata and serialization,
//! but no longer embeds a planner or an execution hook.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokenzero_core::{AckClass, render_ack};

pub const CODEMODE_LIMITS_SCHEMA: &str = "tokenzero.codemode.limits.v1";
pub const DEFAULT_MAX_LOGICAL_OPS: usize = 1000;
pub const DEFAULT_MAX_PHYSICAL_OPS: usize = 256;
pub const HARD_MAX_WALL_MS: u64 = 5000;

/// Compatibility override for the aggregate binding wall ceiling, clamped to
/// [1s, 300s]. Hubs set `TOKENZERO_CODEMODE_HARD_MAX_WALL_MS` to trade latency
/// for headroom while per-call limits still clamp to this ceiling.
pub fn hard_max_wall_ms() -> u64 {
    static VALUE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("TOKENZERO_CODEMODE_HARD_MAX_WALL_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .map(|ms| ms.clamp(1_000, 300_000))
            .unwrap_or(HARD_MAX_WALL_MS)
    })
}
pub const DEFAULT_MAX_MICROTASKS: usize = 4096;
pub const DEFAULT_MAX_MEMORY_BYTES: usize = 32 * 1024 * 1024;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_RESULT_REF_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_MAX_REFS_EMITTED: usize = 256;
pub const DEFAULT_MAX_PARALLEL_WIDTH: usize = 2;
pub const DEFAULT_MAX_CODE_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_VISIBLE_TOKENS: usize = 4000;

/// Deployment default for the recipe and response token envelope. Per-call
/// limits.max_visible_tokens remains authoritative when supplied.
pub fn default_max_visible_tokens() -> usize {
    std::env::var("TOKENZERO_CODEMODE_MAX_VISIBLE_TOKENS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .map(|tokens| tokens.clamp(1, 1_000_000))
        .unwrap_or(DEFAULT_MAX_VISIBLE_TOKENS)
}

// serde(default): tool callers send PARTIAL limits objects (the documented
// contract — e.g. {"max_output_bytes": 1024}); without per-field defaults a
// partial object fails deserialization and tools.rs's `if let Ok` silently
// DROPS the caller's limits (observed in PR 16 review — the exact
// silent-failure class this codebase hunts).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeModeLimits {
    pub max_logical_ops: usize,
    pub max_physical_ops: usize,
    pub max_wall_ms: u64,
    pub hard_max_wall_ms: u64,
    pub max_microtasks: usize,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub max_result_ref_bytes: usize,
    pub max_refs_emitted: usize,
    pub max_parallel_width: usize,
    pub max_code_bytes: usize,
    pub max_visible_tokens: usize,
}

impl Default for CodeModeLimits {
    fn default() -> Self {
        Self {
            max_logical_ops: DEFAULT_MAX_LOGICAL_OPS,
            max_physical_ops: DEFAULT_MAX_PHYSICAL_OPS,
            max_wall_ms: hard_max_wall_ms(),
            hard_max_wall_ms: hard_max_wall_ms(),
            max_microtasks: DEFAULT_MAX_MICROTASKS,
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_result_ref_bytes: DEFAULT_MAX_RESULT_REF_BYTES,
            max_refs_emitted: DEFAULT_MAX_REFS_EMITTED,
            max_parallel_width: DEFAULT_MAX_PARALLEL_WIDTH,
            max_code_bytes: DEFAULT_MAX_CODE_BYTES,
            max_visible_tokens: default_max_visible_tokens(),
        }
    }
}

impl CodeModeLimits {
    pub fn as_json(&self) -> Value {
        json!({
            "schema": CODEMODE_LIMITS_SCHEMA,
            "max_logical_ops": self.max_logical_ops,
            "max_physical_ops": self.max_physical_ops,
            "max_wall_ms": self.max_wall_ms,
            "hard_max_wall_ms": self.hard_max_wall_ms,
            "max_microtasks": self.max_microtasks,
            "max_memory_bytes": self.max_memory_bytes,
            "max_output_bytes": self.max_output_bytes,
            "max_result_ref_bytes": self.max_result_ref_bytes,
            "max_refs_emitted": self.max_refs_emitted,
            "max_parallel_width": self.max_parallel_width,
            "max_code_bytes": self.max_code_bytes,
            "max_visible_tokens": self.max_visible_tokens,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationClass {
    ReadOnly,
    ReversibleStoreMutation,
    IrreversibleExternal,
    Unknown,
}

/// Classification table derived from the canonical PR18 operation descriptor.
pub fn classify_method(method: &str) -> OperationClass {
    let bare = method
        .strip_prefix("zero.token.")
        .or_else(|| method.strip_prefix("zero."))
        .or_else(|| method.strip_prefix("tz_"))
        .unwrap_or(method)
        .replace('-', "_");
    let classes = [
        (
            "read,find,grep,glob,tree,expand,multiExpand,multi_expand,dedupe,mem,recall,rewrite,discover,pick,filter_lines,count,first,verdict,raw,count_tokens,assert,codemode.search,codemode.describe,codemode.limits,codemode.journalDoctor,journalDoctor,journal_doctor,codemode.journalInspect,journalInspect,journal_inspect,codemode.journalResume,journalResume,journal_resume,search,describe,limits",
            OperationClass::ReadOnly,
        ),
        (
            "edit,codemode.journalRollback,journalRollback,journal_rollback,compact,multiCompact,multi_compact,compact_max,ingest,cache_pack,store_put,store_alias,migration_apply",
            OperationClass::ReversibleStoreMutation,
        ),
        (
            "shell,fetch,network,external",
            OperationClass::IrreversibleExternal,
        ),
    ];
    classes
        .into_iter()
        .find(|(methods, _)| methods.split(',').any(|candidate| candidate == bare))
        .map_or(OperationClass::Unknown, |(_, class)| class)
}

pub fn classify_descriptor_tool(tool: &str) -> OperationClass {
    match tool {
        "tz_execute_code" | "tz_batch" => OperationClass::Unknown,
        "tz_report_tool_issue" => OperationClass::IrreversibleExternal,
        other => classify_method(other),
    }
}
pub const CODEMODE_SCHEMA: &str = "tokenzero.codemode.v1";

// ─── Result types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeModeResult {
    /// Legacy schema key retained for existing CodeMode readers.
    pub schema: &'static str,
    /// Stable envelope schema key used across agent-facing JSON surfaces.
    pub schema_version: &'static str,
    pub status: CodeModeStatus,
    pub tool: &'static str,
    /// Stable envelope acknowledgement; constructors and `set_visible_ack`
    /// keep it aligned with `visible_ack`.
    pub ack: String,
    /// Legacy acknowledgement key retained for existing CodeMode readers.
    pub visible_ack: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_refs: Option<Value>,
    pub telemetry: CodeModeTelemetry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CodeModeError>,
    /// vz89.11 output channel separation; present only when the harness opted
    /// in via TOKENZERO_CHANNEL_SEPARATION.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<tokenzero_core::ChannelSeparation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeModeStatus {
    Completed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

impl CodeModeError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.message.contains(needle)
    }
}

impl std::ops::Deref for CodeModeError {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeModeTelemetry {
    #[serde(skip)]
    pub operations: usize,
    #[serde(default)]
    pub visible_tokens: usize,
    /// Exact tokens recovered by expand and consumed during this plan.
    #[serde(default)]
    pub recovery_tokens: usize,
    #[serde(default)]
    pub raw_tokens: usize,
    /// Signed percent: (raw - visible - recovery) / raw. May be negative.
    #[serde(default)]
    pub recovery_adjusted_savings_pct: f64,
    #[serde(default)]
    pub measurement_coverage_pct: u8,
    /// Measured plan-input tokens billed at the CodeMode boundary.
    #[serde(default)]
    pub billed_input_tokens: usize,
    /// Input tokens served from a provider cache, when reported.
    #[serde(default)]
    pub cached_input_tokens: usize,
    /// Measured visible output tokens billed at the CodeMode boundary.
    #[serde(default)]
    pub billed_output_tokens: usize,
    /// Output tokens satisfied by the prefix-cache measurement.
    #[serde(default)]
    pub cached_output_tokens: usize,
    #[serde(skip)]
    pub steps_run: Option<usize>,
    #[serde(skip)]
    pub parallel_groups: Option<usize>,
    #[serde(skip)]
    pub refs_count: Option<usize>,
    #[serde(skip)]
    pub equivalent_calls: Option<usize>,
    pub kind: String,
    pub status: String,
    pub logical_ops: usize,
    pub physical_ops: usize,
    pub batched_ops: usize,
    pub internal_actions: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// Per-session prefix cache hit count (provider cached_tokens when available;
    /// otherwise byte-prefix estimate). Serde default keeps old telemetry readable.
    #[serde(default)]
    pub prefix_cache_hits: usize,
    /// Per-session prefix cache denominator for the hit-rate metric.
    #[serde(default)]
    pub prefix_cache_total: usize,
    pub store_writes: usize,
    pub wall_ms: u64,
    pub bytes_materialized: usize,
    pub envelope_tokens: usize,
    pub payload_tokens: usize,
    /// Token attribution buckets for envelope overhead audit (6ot).
    pub ack_tokens: usize,
    pub ref_string_tokens: usize,
    pub framing_tokens: usize,
    pub preview_tokens: usize,
    /// Counterfactual prevented-read bytes: bytes that would have been read
    /// if graph queries, search hits, or ref expansion had not satisfied the
    /// request without a full file read. Measured as a lower-bound estimate
    /// from available accounting (raw vs. visible tokens, plus exact expand
    /// payload bytes); see exec.rs for the counterfactual methodology.
    pub prevented_read_bytes: usize,
    /// Count of expand calls that returned a capsule instead of the full body (wqw.13).
    #[serde(default)]
    pub prevented_full_body_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

impl CodeModeTelemetry {
    pub fn operations(&self) -> usize {
        self.operations
    }

    pub fn visible_tokens(&self) -> usize {
        self.visible_tokens
    }

    pub fn raw_tokens(&self) -> usize {
        self.raw_tokens
    }

    pub fn recovery_tokens(&self) -> usize {
        self.recovery_tokens
    }
}

// pn93: a plan value under this bound must inline fully with its ref
// attached; hiding 1-4KB outputs behind a ref forces a re-fetch round-trip
// that costs more than the bytes it claims to save.
const DEFAULT_REF_FIRST_BUDGET: usize = 1024;

/// Deployment default for the ref-first inline budget. Per-call
/// limits.ref_first_budget remains authoritative when supplied.
pub fn default_ref_first_budget() -> usize {
    std::env::var("TOKENZERO_REF_FIRST_BUDGET")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|tokens| tokens.clamp(1, 1_000_000))
        .unwrap_or(DEFAULT_REF_FIRST_BUDGET)
}

#[derive(Debug, Clone)]
pub struct CodeModeOptions {
    pub root: Option<PathBuf>,
    pub allowed_roots: Vec<PathBuf>,
    pub cache_path: Option<PathBuf>,
    pub max_visible_tokens: usize,
    pub timeout_seconds: Option<u64>,
    pub max_output_bytes: usize,
    pub max_refs_emitted: usize,
    pub max_logical_ops: usize,
    pub max_physical_ops: usize,
    pub max_microtasks: usize,
    pub max_memory_bytes: usize,
    pub max_code_bytes: usize,
    /// Soft plan wall clock (ms). Defaults match product CodeModeLimits.
    pub max_wall_ms: u64,
    /// Hard plan wall clock (ms); plans abort past this even if soft is higher.
    pub hard_max_wall_ms: u64,
    /// Bounded in-plan Promise.all / fan-out width for QuickJS host ops.
    pub max_parallel_width: usize,
    pub envelope: Option<String>,
    pub ref_first: bool,
    pub ref_first_budget: usize,
    /// Session crash-only health shared with the MCP engine (wqw.9).
    /// When set, plan expand/read outcomes update the same gate as tools/call.
    pub surface_health: Option<std::sync::Arc<crate::surface_health::SurfaceHealth>>,
    /// Programmatic shareable usage-telemetry choice; `None` defers to env.
    pub telemetry_enabled: Option<bool>,
}

impl Default for CodeModeOptions {
    fn default() -> Self {
        Self {
            root: None,
            allowed_roots: Vec::new(),
            cache_path: None,
            max_visible_tokens: default_max_visible_tokens(),
            timeout_seconds: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_refs_emitted: DEFAULT_MAX_REFS_EMITTED,
            max_logical_ops: DEFAULT_MAX_LOGICAL_OPS,
            max_physical_ops: DEFAULT_MAX_PHYSICAL_OPS,
            max_microtasks: DEFAULT_MAX_MICROTASKS,
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_code_bytes: DEFAULT_MAX_CODE_BYTES,
            max_wall_ms: hard_max_wall_ms(),
            hard_max_wall_ms: hard_max_wall_ms(),
            max_parallel_width: DEFAULT_MAX_PARALLEL_WIDTH,
            envelope: None,
            ref_first: true,
            ref_first_budget: default_ref_first_budget(),
            surface_health: None,
            telemetry_enabled: None,
        }
    }
}

fn telemetry(ops: usize, visible: usize, raw: usize, refs: usize, ok: bool) -> CodeModeTelemetry {
    let mut extra = serde_json::json!({
        "operations": ops, "visible_tokens": visible, "raw_tokens": raw,
        "refs_count": refs, "parallel_groups": 0, "envelope_tokens": 0,
        "payload_tokens": visible, "prevented_read_bytes": 0
    });
    if ok {
        extra["equivalent_calls"] = serde_json::json!(ops.saturating_add(1));
    }
    CodeModeTelemetry {
        operations: ops,
        visible_tokens: visible,
        recovery_tokens: 0,
        raw_tokens: raw,
        recovery_adjusted_savings_pct: if raw == 0 {
            0.0
        } else {
            (raw as f64 - visible as f64) * 100.0 / raw as f64
        },
        measurement_coverage_pct: 100,
        billed_input_tokens: 0,
        cached_input_tokens: 0,
        billed_output_tokens: 0,
        cached_output_tokens: 0,
        steps_run: None,
        parallel_groups: Some(0),
        refs_count: Some(refs),
        equivalent_calls: ok.then(|| ops.saturating_add(1)),
        kind: "codemode.execute".into(),
        status: if ok { "ok" } else { "error" }.into(),
        logical_ops: ops,
        physical_ops: ops,
        batched_ops: 0,
        internal_actions: ops,
        cache_hits: 0,
        cache_misses: ops,
        prefix_cache_hits: 0,
        prefix_cache_total: 0,
        store_writes: if ok { refs } else { 0 },
        wall_ms: 0,
        bytes_materialized: raw,
        envelope_tokens: 0,
        payload_tokens: visible,
        ack_tokens: 0,
        ref_string_tokens: 0,
        framing_tokens: 0,
        preview_tokens: 0,
        prevented_read_bytes: 0,
        prevented_full_body_count: 0,
        extra: Some(extra),
    }
}

impl CodeModeResult {
    fn new(
        value: Option<Value>,
        refs: Vec<String>,
        telemetry: CodeModeTelemetry,
        error: Option<CodeModeError>,
    ) -> Self {
        let ok = error.is_none();
        let ack: String = if ok {
            render_ack(AckClass::Success, false).into()
        } else {
            let error = error.as_ref().expect("error result");
            render_ack(
                AckClass::from_error_kind(&error.kind, error.retryable),
                false,
            )
            .into()
        };
        Self {
            schema: CODEMODE_SCHEMA,
            schema_version: CODEMODE_SCHEMA,
            status: if ok {
                CodeModeStatus::Completed
            } else {
                CodeModeStatus::Error
            },
            tool: "codemode",
            ack: ack.clone(),
            visible_ack: ack,
            detail_ref: None,
            execution_id: None,
            value,
            refs,
            execution_refs: None,
            telemetry,
            error,
            channels: None,
        }
    }

    pub fn set_visible_ack(&mut self, ack: impl Into<String>) {
        let ack = ack.into();
        self.ack.clone_from(&ack);
        self.visible_ack = ack;
    }

    pub fn completed(
        value: Value,
        refs: Vec<String>,
        ops: usize,
        visible: usize,
        raw: usize,
    ) -> Self {
        let info = telemetry(ops, visible, raw, refs.len(), true);
        Self::new(Some(value), refs, info, None)
    }

    pub fn error(msg: impl Into<String>, ops: usize) -> Self {
        let message = msg.into();
        Self::error_with_kind(classify_error_kind(&message), message, ops, false)
    }

    pub fn error_with_kind(
        kind: impl Into<String>,
        msg: impl Into<String>,
        ops: usize,
        retryable: bool,
    ) -> Self {
        Self::new(
            None,
            Vec::new(),
            telemetry(ops, 0, 0, 0, false),
            Some(CodeModeError::new(kind, msg, retryable)),
        )
    }

    pub fn to_line(&self) -> String {
        match self.status {
            CodeModeStatus::Completed => {
                let refs = if !self.refs.is_empty() {
                    format!(" refs={}", self.refs.join(","))
                } else {
                    Default::default()
                };
                let mut line = format!(
                    "codemode:ok {} ops={} visible_tokens={} raw_tokens={}{}",
                    self.visible_ack,
                    self.telemetry.operations(),
                    self.telemetry.visible_tokens(),
                    self.telemetry.raw_tokens(),
                    refs
                );
                if let Some(warning) = self
                    .telemetry
                    .extra
                    .as_ref()
                    .and_then(|extra| extra.get("root_fallback_warning"))
                    .and_then(Value::as_str)
                {
                    line.push_str(&format!("\n# warning: root_fallback: {warning}"));
                }
                line
            }
            CodeModeStatus::Error => format!(
                "codemode:error {} ops={} {}",
                self.visible_ack,
                self.telemetry.operations(),
                self.error
                    .as_ref()
                    .map(|error| structured_error_message(&error.message))
                    .unwrap_or_else(|| "unknown".to_string())
            ),
        }
    }
}

fn structured_error_message(message: &str) -> String {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(message) else {
        return message.replace(['\n', '\r'], " ");
    };
    let Some(error) = fields.get("error") else {
        return message.replace(['\n', '\r'], " ");
    };
    let render = |value: &Value| match value {
        Value::String(text) => text.replace(['\n', '\r'], " "),
        Value::Array(values) => values
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_string)
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    let mut rendered = render(error);
    if let Some(hint) = fields.get("hint") {
        rendered.push_str("; hint: ");
        rendered.push_str(&render(hint));
    }
    for (name, value) in fields {
        if name == "error" || name == "hint" {
            continue;
        }
        rendered.push_str("; ");
        rendered.push_str(&name);
        rendered.push_str(": ");
        rendered.push_str(&render(&value));
    }
    rendered
}

struct ErrorKindRule {
    kind: &'static str,
    any_starts_with: &'static [&'static str],
    any_contains: &'static [&'static str],
    all_contains: &'static [&'static str],
}

// First match wins. Policy sits above sandbox so "denied" does not steal those rows.
const ERROR_KIND_RULES: &[ErrorKindRule] = &[
    ErrorKindRule {
        kind: "policy",
        any_starts_with: &[],
        any_contains: &["mutating binding denied", "mutation", "edit denied"],
        all_contains: &[],
    },
    ErrorKindRule {
        kind: "sandbox",
        any_starts_with: &["sandbox:"],
        any_contains: &["denied", "quickjs"],
        all_contains: &[],
    },
    ErrorKindRule {
        kind: "validation",
        any_starts_with: &[],
        any_contains: &[
            "parse error",
            "invalid json",
            "empty plan",
            "missing method",
            "requires a steps array",
        ],
        all_contains: &["missing", "argument"],
    },
    ErrorKindRule {
        kind: "substrate",
        any_starts_with: &[],
        any_contains: &[
            "outside allowed roots",
            "absolute path rejected",
            "bad path",
            "not found",
            "no such",
            "missing target",
            "missing_target",
        ],
        all_contains: &[],
    },
];

fn classify_error_kind(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    for rule in ERROR_KIND_RULES {
        if rule
            .any_starts_with
            .iter()
            .any(|prefix| lower.starts_with(prefix))
            || rule
                .any_contains
                .iter()
                .any(|needle| lower.contains(needle))
            || (!rule.all_contains.is_empty()
                && rule
                    .all_contains
                    .iter()
                    .all(|needle| lower.contains(needle)))
        {
            return rule.kind;
        }
    }
    "runtime"
}

