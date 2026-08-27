//! CodeMode data types, limits, and serialization helpers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use graphzero_store::store::query::tokens_for_str;

use super::envelope_v1_enabled;
use super::utils::first_chars_flat;
use super::utils::safe_execution_path_component;

// ── limits ──

pub(crate) const MAX_LOGICAL_OPS: u64 = 1000;
pub(crate) const MAX_PHYSICAL_OPS: u64 = 256;
/// Default per-plan wall budget.
///
/// Contract (bead zerostack-155n): the deadline a plan runs under is
/// `limits.max_wall_ms`, resolved in this order:
///   1. an explicit `limits.max_wall_ms` supplied by the caller/host, else
///   2. `GRAPHZERO_CODEMODE_MAX_WALL_MS` (host timeout propagation), else
///   3. this default.
///
/// The previous 1s default made ordinary multi-hop analysis (for example
/// `graph.blast` at depth 3) fail with `deadline_exceeded` before it could
/// finish. The default is a real analysis budget; hosts with a tighter deadline
/// pass it down explicitly rather than relying on GraphZero to guess low.
pub(crate) const MAX_WALL_MS: u128 = 30_000;

/// Environment override used by hosts to propagate their own call deadline.
pub(crate) const MAX_WALL_MS_ENV: &str = "GRAPHZERO_CODEMODE_MAX_WALL_MS";

/// Resolve the default wall budget, honouring host timeout propagation.
pub(crate) fn default_max_wall_ms() -> u128 {
    std::env::var(MAX_WALL_MS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u128>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(MAX_WALL_MS)
}
pub(crate) const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RESULT_REF_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_REFS_EMITTED: usize = 256;
pub(crate) const MAX_CODE_BYTES: usize = 64 * 1024;
pub(crate) const REF_FIRST_STRING_TOKENS: usize = 64;
pub(crate) const REF_FIRST_PREVIEW_CHARS: usize = 48;
pub(crate) const MAX_MICROTASKS: u64 = 4096;
pub(crate) const MAX_MEMORY_BYTES: usize = 32 * 1024 * 1024;
/// Default in-plan JSON parallel fanout. Kept low so concurrent CodeMode
/// sessions share the machine analysis budget (contract v1).
pub(crate) const MAX_PARALLEL_WIDTH: usize = 2;

// ── structs ──

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeModeLimits {
    pub max_logical_ops: u64,
    pub max_physical_ops: u64,
    pub max_wall_ms: u128,
    pub max_microtasks: u64,
    pub max_memory_bytes: usize,
    pub max_output_bytes: usize,
    pub max_result_ref_bytes: usize,
    pub max_refs_emitted: usize,
    pub max_parallel_width: usize,
    pub max_code_bytes: usize,
}

impl Default for CodeModeLimits {
    fn default() -> Self {
        Self {
            max_logical_ops: MAX_LOGICAL_OPS,
            max_physical_ops: MAX_PHYSICAL_OPS,
            max_wall_ms: default_max_wall_ms(),
            max_microtasks: MAX_MICROTASKS,
            max_memory_bytes: MAX_MEMORY_BYTES,
            max_output_bytes: MAX_OUTPUT_BYTES,
            max_result_ref_bytes: MAX_RESULT_REF_BYTES,
            max_refs_emitted: MAX_REFS_EMITTED,
            max_parallel_width: MAX_PARALLEL_WIDTH,
            max_code_bytes: MAX_CODE_BYTES,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CodeModeTelemetry {
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub plan_kind: String,
    pub logical_ops: u64,
    pub physical_ops: u64,
    pub batched_ops: u64,
    pub internal_actions: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub store_writes: u64,
    pub wall_ms: u128,
    pub bytes_materialized: usize,
    #[serde(default)]
    pub raw_token_estimate: usize,
    #[serde(default)]
    pub visible_token_estimate: usize,
    #[serde(default)]
    pub measurement_coverage_pct: u8,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(skip_serializing)]
    pub execution_id: String,
    #[serde(skip_serializing)]
    pub visible_ack: String,
    #[serde(skip_serializing)]
    pub steps_run: u64,
    #[serde(skip_serializing)]
    pub parallel_groups: u64,
    #[serde(skip_serializing)]
    pub refs: Vec<String>,
    #[serde(skip_serializing)]
    pub started_at: String,
    #[serde(skip_serializing)]
    pub finished_at: String,
    #[serde(skip_serializing)]
    pub round_trips: u64,
    #[serde(skip_serializing)]
    pub visible_ack_count: u64,
}

impl CodeModeTelemetry {
    /// Project telemetry to the frozen contract vocabulary (G3): only the 11
    /// required top-level keys plus optional `extra`. Non-frozen internal
    /// metrics (plan_kind, token estimates, coverage) move into `extra` in the
    /// projection; the struct itself and persisted telemetry keep them.
    pub fn contract_json(&self) -> Value {
        let mut extra = self.extra.clone();
        if !self.plan_kind.is_empty() {
            extra.insert("plan_kind".into(), json!(self.plan_kind));
        }
        if self.raw_token_estimate != 0 {
            extra.insert("raw_token_estimate".into(), json!(self.raw_token_estimate));
        }
        if self.visible_token_estimate != 0 {
            extra.insert(
                "visible_token_estimate".into(),
                json!(self.visible_token_estimate),
            );
        }
        if self.measurement_coverage_pct != 0 {
            extra.insert(
                "measurement_coverage_pct".into(),
                json!(self.measurement_coverage_pct),
            );
        }
        let mut out = json!({
            "kind": self.kind,
            "status": self.status,
            "logical_ops": self.logical_ops,
            "physical_ops": self.physical_ops,
            "batched_ops": self.batched_ops,
            "internal_actions": self.internal_actions,
            "cache_hits": self.cache_hits,
            "cache_misses": self.cache_misses,
            "store_writes": self.store_writes,
            "wall_ms": self.wall_ms,
            "bytes_materialized": self.bytes_materialized,
        });
        if !extra.is_empty() {
            out.as_object_mut()
                .expect("contract_json object")
                .insert("extra".into(), json!(extra));
        }
        out
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRecord {
    pub id: String,
    pub op: String,
    pub status: String,
    pub logical_ops: u64,
    pub physical_ops: u64,
    pub refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeModeError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
    /// Bounded resume envelope, present only on deadline/cancellation errors.
    /// A caller that hits the wall needs to know *where* time went and what to
    /// do next without expanding a diagnostic ref first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<TimeoutEnvelope>,
}

/// Bounded, stable-shape context for a deadline or cancellation stop.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeoutEnvelope {
    /// Stable machine code, distinct from the human message.
    pub code: String,
    /// Plan phase that owned the deadline when it elapsed (the guarded step id).
    pub phase: String,
    pub elapsed_ms: u64,
    /// Deadline the plan was configured with, so the caller can size a retry.
    pub deadline_ms: u64,
    /// Whether a usable index was open when time ran out; a cold index is a
    /// different remedy (index first) than a slow plan (shrink the plan).
    pub index_state: String,
    /// Durable ref for partial work, when the run produced any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<String>,
    /// One specific next action, not generic advice.
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodeModeResponse {
    pub ack: String,
    pub execution_id: String,
    pub execution_ref: String,
    pub envelope_ref: String,
    pub result_ref: Option<String>,
    pub telemetry_ref: String,
    pub steps_ref: String,
    pub error_ref: Option<String>,
    pub visible: String,
    pub telemetry: CodeModeTelemetry,
    pub error: Option<CodeModeError>,
    /// Inline result value when small (<= max_output_bytes): refs-only
    /// envelopes force a second round-trip to recover the plan's own return
    /// (the judgment path). Large results stay behind result_ref.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

impl CodeModeResponse {
    pub fn compact_line(&self) -> String {
        if envelope_v1_enabled() {
            self.compact_legacy_line()
        } else {
            self.envelope_v2_text()
        }
    }

    pub fn compact_legacy_line(&self) -> String {
        let mut line = format!(
            "{} execution_id={} execution_ref={} telemetry_ref={} steps_ref={} visible_tokens~{} internal_actions={}",
            self.ack,
            self.execution_id,
            self.execution_ref,
            self.telemetry_ref,
            self.steps_ref,
            tokens_for_str(&self.visible),
            self.telemetry.internal_actions
        );
        if let Some(r) = &self.result_ref {
            line.push_str(" result_ref=");
            line.push_str(r);
        }
        if let Some(e) = &self.error_ref {
            line.push_str(" error_ref=");
            line.push_str(e);
        }
        line
    }

    /// Current compact CodeMode wire (protocol v2). This is the live envelope,
    /// not a deprecated leftover of v1. Renaming the method requires a protocol
    /// version bump and client migration.
    pub fn envelope_v2_text(&self) -> String {
        if let Some(error) = &self.error {
            let retry = if error.retryable {
                "retryable"
            } else {
                "final"
            };
            let message = first_chars_flat(&error.message, 80);
            // Prefer durable gz:// error/result refs so hub extractTypedRefs and
            // cross-root expand can recover after the call returns. Compact q:
            // aliases are not typed recovery refs for ZeroStack handoff.
            let recovery = self
                .error_ref
                .as_deref()
                .or(self.result_ref.as_deref())
                .unwrap_or(self.execution_ref.as_str());
            return format!("err {} {} {} t:{recovery}", error.kind, retry, message);
        }
        let raw = serde_json::to_string(&self.contract_json()).unwrap_or_default();
        let raw_tokens = tokens_for_str(&raw).max(1);
        let ops = self.telemetry.internal_actions.max(1);
        let mut line = format!("ok gz{ops} - t:{}", self.envelope_ref);
        let line_tokens = tokens_for_str(&line).max(1);
        let pct = 100usize.saturating_sub(line_tokens.saturating_mul(100).div_ceil(raw_tokens));
        line = format!("ok gz{ops} {pct}% t:{}", self.envelope_ref);
        line
    }

    pub fn contract_json(&self) -> Value {
        if envelope_v1_enabled() {
            self.contract_legacy_json()
        } else {
            self.structured_content_v2()
        }
    }

    pub fn structured_content_v2(&self) -> Value {
        // Canonical contract envelope (zerostack-mpi): `ack` C/X0, top-level
        // `execution_id`, a `refs` map with code/steps/telemetry plus result or
        // error, and inline telemetry projected to the frozen vocabulary. The
        // compact human ack stays in the text channel only.
        let safe_execution_id = safe_execution_path_component(&self.execution_id);
        let part_ref = |part: &str| format!("gz://codemode/execution/{safe_execution_id}/{part}");
        let mut refs = json!({
            "code": part_ref("code"),
            "steps": self.steps_ref,
            "telemetry": self.telemetry_ref,
        });
        if let Some(obj) = refs.as_object_mut() {
            if let Some(result_ref) = &self.result_ref {
                obj.insert("result".into(), json!(result_ref));
            }
            if let Some(error_ref) = &self.error_ref {
                obj.insert("error".into(), json!(error_ref));
            }
        }
        let mut out = json!({
            "ack": self.ack,
            "execution_id": self.execution_id,
            "refs": refs,
            "telemetry": self.telemetry.contract_json(),
        });
        if let Some(obj) = out.as_object_mut() {
            // Compatibility aliases: inline small `value` and singular `ref`
            // remain for existing consumers but never replace canonical fields.
            if let Some(result) = &self.result {
                obj.insert("value".into(), result.clone());
            }
            if let Some(result_ref) = &self.result_ref {
                obj.insert("ref".into(), json!(result_ref));
            } else if let Some(error_ref) = &self.error_ref {
                obj.insert("ref".into(), json!(error_ref));
                obj.insert("error_ref".into(), json!(error_ref));
            }
            if let Some(error) = &self.error {
                obj.insert("error".into(), json!(error));
            }
        }
        out
    }

    pub fn contract_legacy_json(&self) -> Value {
        let safe_execution_id = safe_execution_path_component(&self.execution_id);
        let part_ref = |part: &str| format!("gz://codemode/execution/{safe_execution_id}/{part}");
        let mut refs = json!({
            "code": part_ref("code"),
            "steps": self.steps_ref,
            "telemetry": self.telemetry_ref,
        });
        if let Some(obj) = refs.as_object_mut() {
            if let Some(result_ref) = &self.result_ref {
                obj.insert("result".into(), json!(result_ref));
            }
            if let Some(error_ref) = &self.error_ref {
                obj.insert("error".into(), json!(error_ref));
            }
        }
        let mut out = json!({
            "ack": self.ack,
            "execution_id": self.execution_id,
            "refs": refs,
            "telemetry": self.telemetry.contract_json(),
        });
        if let Some(obj) = out.as_object_mut() {
            if let Some(result_ref) = &self.result_ref {
                obj.insert("result_ref".into(), json!(result_ref));
            }
            if let Some(result) = &self.result {
                obj.insert("result".into(), result.clone());
            }
            if let Some(error_ref) = &self.error_ref {
                obj.insert("error_ref".into(), json!(error_ref));
            }
            if let Some(error) = &self.error {
                obj.insert("error".into(), json!(error));
            }
        }
        out
    }
}

// ── binding result ──

pub(crate) struct BindingResult {
    pub value: Value,
    pub refs: Vec<String>,
    pub bytes_materialized: usize,
}

pub(crate) fn serialize_binding_value(
    result: &BindingResult,
    step_id: &str,
) -> Result<String, CodeModeError> {
    serialize_binding_value_with(result, step_id, serde_json::to_string)
}

pub(crate) fn serialize_binding_value_with<F>(
    result: &BindingResult,
    _step_id: &str,
    serialize: F,
) -> Result<String, CodeModeError>
where
    F: FnOnce(&Value) -> Result<String, serde_json::Error>,
{
    serialize(&result.value).map_err(|e| CodeModeError {
        kind: "substrate".into(),
        message: format!("failed to serialize CodeMode operation value: {e}"),
        retryable: false,
        timeout: None,
    })
}
