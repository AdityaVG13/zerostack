//! CodeMode host execute façade.
//!
//! Routes plans through the interpreter + connector. Spill / result
//! normalization stay here. Do not split unless EXP-012 is SEAM_CONFIRMED
//! and this file grows past the Rust soft threshold.

use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value as JsonValue;
use zero_abi::{CapabilityDescriptor, GlobalRegistration, RegistrationError, ZeroResultV1};
use zero_store::SharedCas;

use crate::{HostLimits, LimitError, PlanError};

static RUNTIME_CREATIONS: AtomicU64 = AtomicU64::new(0);

pub fn runtime_creation_count() -> u64 {
    RUNTIME_CREATIONS.load(Ordering::Relaxed) + crate::interpreter::interpreter_creation_count()
}

/// Deadline and serialization budget supplied to a connector dispatch.
///
/// Connectors must enforce this context for the complete accepted operation.
/// The host refuses to settle late completions, but connector-owned workers
/// remain responsible for stopping their underlying work at the deadline.
#[derive(Clone, Copy, Debug)]
pub struct DispatchContext {
    pub deadline: Instant,
    pub max_json_bytes: usize,
}

impl DispatchContext {
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

/// Maximum capability calls one plan may have in flight at once.
///
/// The shared completion channel has the same capacity, so connector results
/// remain bounded even while the JavaScript runtime is executing microtasks.
pub const MAX_INFLIGHT_CONNECTOR_CALLS: usize = 64;

pub(crate) struct ConnectorCompletionMessage {
    pub(crate) sequence: u64,
    pub(crate) result: Result<String, ConnectorError>,
}

/// One-shot completion authority for an accepted connector dispatch.
///
/// A connector may move this value to its own bounded dispatcher or event
/// loop, but it must complete it exactly once. The completion never enters
/// the interpreter directly; the host runtime thread receives and settles it.
pub struct ConnectorCompletion {
    sequence: u64,
    sender: mpsc::SyncSender<ConnectorCompletionMessage>,
}

impl ConnectorCompletion {
    pub(crate) fn new(sequence: u64, sender: mpsc::SyncSender<ConnectorCompletionMessage>) -> Self {
        Self { sequence, sender }
    }

    pub fn complete(self, result: Result<String, ConnectorError>) -> Result<(), ConnectorError> {
        self.sender
            .send(ConnectorCompletionMessage {
                sequence: self.sequence,
                result,
            })
            .map_err(|_| ConnectorError::new("connector completion receiver closed"))
    }
}

pub trait Connector {
    /// Accept a bounded dispatch without blocking the JavaScript runtime.
    ///
    /// Returning `Ok(())` transfers completion authority to the connector.
    /// Returning `Err` rejects the call immediately and must not complete it.
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorError {
    message: String,
}

impl ConnectorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConnectorError {}

/// Schema tag of the envelope returned in place of an oversized result.
pub const RESULT_SPILL_SCHEMA: &str = "zerostack.codemode.result_spill.v1";

/// Upper bound on the inline preview carried beside a spilled result ref.
pub const RESULT_SPILL_PREVIEW_BYTES: usize = 512;

/// Conservative byte ceiling for aggregate values shown directly to a model.
/// Receipts label bytes only; tokenizer-specific visible-token certification
/// remains a separate TokenZero boundary.
pub const DEFAULT_MAX_VISIBLE_RESULT_BYTES: usize = 1_024;

/// Hard ceiling for the serialized spill envelope itself, including its
/// exact-byte receipt. Crossing this bound fails typed instead of leaking text.
pub const MAX_RESULT_SPILL_ENVELOPE_BYTES: usize = 2_000;

/// Bound for typed error text emitted by a model-facing adapter.
pub const MAX_VISIBLE_ERROR_BYTES: usize = 1_024;

/// Honest runtime accounting for one aggregate interpreter execution.
///
/// Logical operations count capability calls. Connector dispatches count
/// accepted calls at the host boundary; a connector that fuses work below
/// that boundary must report its own engine-pass accounting separately.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ExecutionMetrics {
    pub logical_operations: u64,
    pub connector_dispatches: u64,
    pub physical_dispatches: u64,
    pub peak_inflight_connector_calls: usize,
    pub peak_retained_promises: usize,
    pub peak_estimated_promise_bytes: usize,
    pub backpressure_events: u64,
    pub instructions: u64,
    pub microtasks: usize,
    pub wall_time_ns: u64,
    pub first_saturation_cause: Option<String>,
}

/// Result plus accounting. Metrics remain available when execution fails.
#[derive(Debug)]
pub struct ExecutionOutcome {
    pub result: Result<JsonValue, HostError>,
    pub metrics: ExecutionMetrics,
}

/// Bound untrusted error text without splitting UTF-8. The typed error code
/// remains the authority; the human text is diagnostic only.
pub fn finalize_visible_error(value: &str) -> String {
    if value.len() <= MAX_VISIBLE_ERROR_BYTES {
        return value.to_owned();
    }
    const SUFFIX: &str = "... [truncated]";
    let mut end = MAX_VISIBLE_ERROR_BYTES.saturating_sub(SUFFIX.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], SUFFIX)
}

/// The complete public capability-result surface. Domain values live under
/// `content.value`; omitted exact values use `content.ref` instead.
pub const PUBLIC_RESULT_FIELDS: &[&str] = &["ack", "content"];

#[derive(Clone, Debug)]
pub struct Host {
    pub(crate) limits: HostLimits,
    pub(crate) registration: GlobalRegistration,
    pub(crate) spill_root: Option<PathBuf>,
    pub(crate) max_visible_result_bytes: usize,
}

impl Host {
    pub fn new(limits: HostLimits, registration: GlobalRegistration) -> Result<Self, HostError> {
        limits.validate().map_err(HostError::Limits)?;
        registration.validate().map_err(HostError::Registration)?;
        Ok(Self {
            max_visible_result_bytes: limits.max_json_bytes,
            limits,
            registration,
            spill_root: None,
        })
    }

    /// Publish results larger than `max_json_bytes` into the content-addressed
    /// store rooted at `cas_root` and return a ref plus a bounded preview,
    /// instead of failing with [HostError::ResultTooLarge].
    pub fn with_result_spill(mut self, cas_root: impl Into<PathBuf>) -> Self {
        self.spill_root = Some(cas_root.into());
        self
    }

    /// Set the finalized result byte budget independently from connector frame
    /// bounds. A zero budget is rejected loudly.
    pub fn with_visible_result_budget(mut self, max_bytes: usize) -> Result<Self, HostError> {
        if max_bytes == 0 {
            return Err(HostError::Limits(LimitError::Zero(
                "max_visible_result_bytes",
            )));
        }
        self.max_visible_result_bytes = max_bytes;
        Ok(self)
    }

    pub fn limits(&self) -> HostLimits {
        self.limits
    }

    pub fn registration(&self) -> &GlobalRegistration {
        &self.registration
    }

    pub fn execute(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
    ) -> Result<JsonValue, HostError> {
        self.execute_with_cancel(plan, connector, Arc::new(AtomicBool::new(false)))
    }

    /// Execute one plan and retain aggregate scheduling/resource telemetry.
    pub fn execute_measured(&self, plan: &str, connector: Rc<dyn Connector>) -> ExecutionOutcome {
        self.execute_measured_with_cancel_timeout_context(
            plan,
            connector,
            Arc::new(AtomicBool::new(false)),
            self.limits.wall_timeout,
            0,
            0,
        )
    }

    pub fn execute_measured_with_cancel_timeout_context(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
        generation: u64,
        request_id: u64,
    ) -> ExecutionOutcome {
        crate::interpreter::execute_measured(
            self,
            plan,
            connector,
            cancelled,
            timeout.min(self.limits.wall_timeout),
            generation,
            request_id,
        )
    }

    pub fn execute_with_cancel(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<JsonValue, HostError> {
        self.execute_with_cancel_timeout_context(
            plan,
            connector,
            cancelled,
            self.limits.wall_timeout,
            0,
            0,
        )
    }

    pub fn execute_with_cancel_timeout(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<JsonValue, HostError> {
        self.execute_with_cancel_timeout_context(plan, connector, cancelled, timeout, 0, 0)
    }

    pub fn execute_with_cancel_timeout_context(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
        generation: u64,
        request_id: u64,
    ) -> Result<JsonValue, HostError> {
        crate::interpreter::execute(
            self,
            plan,
            connector,
            cancelled,
            timeout.min(self.limits.wall_timeout),
            generation,
            request_id,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostError {
    Limits(LimitError),
    Registration(RegistrationError),
    Plan(PlanError),
    Parse(String),
    UnsupportedSyntax(String),
    Data(String),
    Execution(String),
    VerdictRejected(String),
    Runtime(String),
    JavaScript(String),
    MethodNotFound(String),
    SurfaceNotFound(String),
    Connector(String),
    Json(String),
    ResultTooLarge { actual: usize, maximum: usize },
    ResultSpill(String),
    MemoryLimit { requested: usize, maximum: usize },
    MicrotaskLimit,
    DeadlineExceeded,
    FuelExhausted,
    Cancelled,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limits(error) => write!(f, "invalid limits: {error}"),
            Self::Registration(error) => write!(f, "invalid registration: {error}"),
            Self::Plan(error) => write!(f, "invalid plan: {error}"),
            Self::Parse(message) => write!(f, "parse error: {message}"),
            Self::UnsupportedSyntax(message) => write!(f, "unsupported syntax: {message}"),
            Self::Data(message) => write!(f, "data error: {message}"),
            Self::Execution(message) => write!(f, "execution error: {message}"),
            Self::VerdictRejected(message) => write!(f, "verdict rejected: {message}"),
            Self::Runtime(message) => write!(f, "runtime error: {message}"),
            Self::JavaScript(message)
            | Self::MethodNotFound(message)
            | Self::SurfaceNotFound(message) => write!(f, "JavaScript exception: {message}"),
            Self::Connector(message) => write!(f, "connector error: {message}"),
            Self::Json(message) => write!(f, "JSON error: {message}"),
            Self::ResultTooLarge { actual, maximum } => {
                write!(f, "result is {actual} bytes; maximum is {maximum}")
            }
            Self::ResultSpill(message) => write!(f, "result spill failed: {message}"),
            Self::MemoryLimit { requested, maximum } => {
                write!(
                    f,
                    "memory budget exceeded: requested {requested} bytes; maximum is {maximum}"
                )
            }
            Self::MicrotaskLimit => f.write_str("microtask ceiling exceeded"),
            Self::DeadlineExceeded => f.write_str("wall-clock deadline exceeded"),
            Self::FuelExhausted => f.write_str("instruction budget exhausted"),
            Self::Cancelled => f.write_str("execution cancelled"),
        }
    }
}

impl std::error::Error for HostError {}

fn public_result_ack(value: &JsonValue) -> String {
    [
        value.pointer("/value/tool_response/ack"),
        value.pointer("/value/ack"),
        value.get("ack"),
    ]
    .into_iter()
    .flatten()
    .filter_map(JsonValue::as_str)
    .find(|ack| (1..=zero_abi::MAX_ACK_CHARS).contains(&ack.chars().count()))
    .unwrap_or("ok")
    .to_owned()
}

fn declared_zero_result(value: &JsonValue) -> Result<Option<ZeroResultV1>, HostError> {
    let declared = (value.get("ack").is_some() && value.get("content").is_some())
        || value.pointer("/content/kind").is_some();
    if !declared {
        return Ok(None);
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|error| HostError::Json(format!("invalid declared zero-result/v1: {error}")))
}

fn explicit_result_reference(
    value: &JsonValue,
) -> Result<Option<(&str, Option<String>)>, HostError> {
    let candidates = [
        (
            value.pointer("/value/tool_response/visible/kind"),
            value.pointer("/value/tool_response/visible/ref"),
            value.pointer("/value/tool_response/visible/preview"),
        ),
        (
            value.pointer("/value/content/kind"),
            value.pointer("/value/content/ref"),
            value.pointer("/value/content/preview"),
        ),
    ];
    for (kind, reference, preview) in candidates {
        if kind.and_then(JsonValue::as_str) != Some("ref") {
            continue;
        }
        let reference = reference.and_then(JsonValue::as_str).ok_or_else(|| {
            HostError::Json("explicit ref result requires a string ref".to_owned())
        })?;
        let preview = preview
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    HostError::Json("explicit ref result preview must be a string".to_owned())
                })
            })
            .transpose()?;
        return Ok(Some((reference, preview)));
    }
    Ok(None)
}

/// Normalize the transport-owned worker result before JavaScript can observe it.
/// Raw WorkerResponseFrame metadata stays inside inline content; a producer may
/// request ref content only through an explicit typed `kind: "ref"` value.
pub(crate) fn normalize_public_result(encoded: &str) -> Result<String, HostError> {
    let value: JsonValue = serde_json::from_str(encoded)
        .map_err(|error| HostError::Json(format!("connector returned invalid JSON: {error}")))?;
    if let Some(result) = declared_zero_result(&value)? {
        return serde_json::to_string(&result).map_err(|error| HostError::Json(error.to_string()));
    }
    if let Some(result) = value
        .get("value")
        .map(declared_zero_result)
        .transpose()?
        .flatten()
        .filter(|result| result.kind() == "ref")
    {
        return serde_json::to_string(&result).map_err(|error| HostError::Json(error.to_string()));
    }
    let ack = public_result_ack(&value);
    let result = match explicit_result_reference(&value)? {
        Some((reference, preview)) => ZeroResultV1::reference(&ack, reference, preview)
            .map_err(|error| HostError::Json(format!("invalid explicit ref result: {error}")))?,
        None => ZeroResultV1::inline(ack, value)
            .expect("validated fallback ack always constructs zero-result/v1"),
    };
    serde_json::to_string(&result).map_err(|error| HostError::Json(error.to_string()))
}

fn is_canonical_spill_ref(reference: &str) -> bool {
    let Some(digest) = reference.strip_prefix("tz://blob/") else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Explicit expansion is the one intentional escape from the default visible
/// result budget. Keep the authorization narrow: one direct aggregate call,
/// one canonical spill ref, and no surrounding plan work.
pub(crate) fn directly_expands_one_spill_ref(plan: &str) -> bool {
    let trimmed = plan.trim();
    let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed).trim();
    let argument = [
        "return await zero.token.expand(",
        "return zero.token.expand(",
    ]
    .into_iter()
    .find_map(|prefix| trimmed.strip_prefix(prefix))
    .and_then(|tail| tail.strip_suffix(')'))
    .map(str::trim);
    let Some(argument) = argument else {
        return false;
    };
    let reference = if argument.starts_with('"') {
        serde_json::from_str::<String>(argument).ok()
    } else {
        argument
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .filter(|value| !value.contains('\\') && !value.contains('\''))
            .map(str::to_owned)
    };
    reference.as_deref().is_some_and(is_canonical_spill_ref)
}

pub(crate) fn is_terminal_exact_token_expansion(value: &JsonValue) -> bool {
    let inline = serde_json::from_value::<ZeroResultV1>(value.clone())
        .ok()
        .and_then(|result| result.inline_value().ok().cloned());
    let value = inline.as_ref().unwrap_or(value);
    let Some(root) = value.as_object().filter(|root| root.len() == 2) else {
        return false;
    };
    let Some(metadata) = root.get("metadata") else {
        return false;
    };
    if metadata
        .pointer("/ownership/engine")
        .and_then(JsonValue::as_str)
        != Some("tokenzero")
    {
        return false;
    }
    let Some(result) = root.get("value") else {
        return false;
    };
    let visible = result.get("visible").and_then(JsonValue::as_str);
    visible.is_some()
        && result.get("op").and_then(JsonValue::as_str) == Some("tz_expand")
        && result.get("status").and_then(JsonValue::as_str) == Some("ok")
        && result.get("mode").and_then(JsonValue::as_str) == Some("exact")
        && result
            .pointer("/tool_response/tool")
            .and_then(JsonValue::as_str)
            == Some("expand")
        && result
            .pointer("/tool_response/recovery/do_not_recompact")
            .and_then(JsonValue::as_bool)
            == Some(true)
        && result
            .pointer("/tool_response/recovery/exact_bytes")
            .and_then(JsonValue::as_bool)
            == Some(true)
        && result
            .pointer("/tool_response/recovery/terminal")
            .and_then(JsonValue::as_bool)
            == Some(true)
        && result
            .pointer("/tool_response/visible/text")
            .and_then(JsonValue::as_str)
            == visible
}

/// Publish an oversized encoded result into the CAS and describe it with a ref
/// plus a bounded preview, so a large final value degrades to a fetchable
/// reference instead of a hard framing error.
pub(crate) fn spill_result(cas_root: &Path, encoded: &str) -> Result<JsonValue, HostError> {
    let cas = SharedCas::open_labeled(cas_root, "codemode-result-spill");
    let hash = cas
        .put(encoded.as_bytes())
        .map_err(|error| HostError::ResultSpill(error.to_string()))?;
    let reference = format!("tz://blob/{hash}");
    // Real head-of-content preview: a spill receipt that hides everything
    // behind "[exact result omitted]" makes small-but-spilled results
    // unusable without a second expand round trip.
    let mut preview_end = encoded.len().min(RESULT_SPILL_PREVIEW_BYTES);
    while preview_end > 0 && !encoded.is_char_boundary(preview_end) {
        preview_end -= 1;
    }
    // JSON escaping can expand up to 6x; shrink until the escaped form stays
    // inside twice the raw preview budget so the envelope cap always holds.
    while preview_end > 0 {
        let escaped_len = serde_json::to_string(&encoded[..preview_end])
            .map(|escaped| escaped.len())
            .unwrap_or(usize::MAX);
        if escaped_len <= RESULT_SPILL_PREVIEW_BYTES * 2 {
            break;
        }
        preview_end /= 2;
        while preview_end > 0 && !encoded.is_char_boundary(preview_end) {
            preview_end -= 1;
        }
    }
    let preview_truncated = preview_end < encoded.len();
    let preview = &encoded[..preview_end];
    debug_assert!(preview.len() <= RESULT_SPILL_PREVIEW_BYTES);
    let raw_bytes = encoded.len();
    let mut envelope = serde_json::json!({
        "schema": RESULT_SPILL_SCHEMA,
        "spilled": true,
        "ref": reference,
        "sha256": hash,
        "bytes": raw_bytes,
        "preview": preview,
        "previewBytes": preview.len(),
        "previewTruncated": preview_truncated,
        "receipt": {
            "schema": "zerostack.codemode.result_finalization_receipt.v1",
            "rawResultJsonBytes": raw_bytes,
            "inlineResultBytes": 0,
            "omittedBehindExactRefBytes": raw_bytes,
            "typedFailureBytes": 0,
            "finalizedValueJsonBytes": 0,
            "visibleTokenCount": JsonValue::Null,
            "visibleTokenCountStatus": "requires_tokenzero_certification",
            "savingsBytes": 0,
            "integrity": "sha256-cas",
        },
    });
    for _ in 0..16 {
        let visible_bytes = serde_json::to_vec(&envelope)
            .map_err(|error| HostError::ResultSpill(error.to_string()))?
            .len();
        let savings_bytes = raw_bytes.saturating_sub(visible_bytes);
        let receipt = envelope
            .get_mut("receipt")
            .and_then(JsonValue::as_object_mut)
            .ok_or_else(|| HostError::ResultSpill("missing finalization receipt".into()))?;
        let prior_visible = receipt
            .get("finalizedValueJsonBytes")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let prior_savings = receipt
            .get("savingsBytes")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        if prior_visible == visible_bytes && prior_savings == savings_bytes {
            if visible_bytes > MAX_RESULT_SPILL_ENVELOPE_BYTES {
                return Err(HostError::ResultSpill(format!(
                    "finalized spill envelope is {visible_bytes} bytes; maximum is {MAX_RESULT_SPILL_ENVELOPE_BYTES}"
                )));
            }
            return Ok(envelope);
        }
        receipt.insert("finalizedValueJsonBytes".into(), visible_bytes.into());
        receipt.insert("savingsBytes".into(), savings_bytes.into());
    }
    Err(HostError::ResultSpill(
        "finalized spill receipt length did not converge".into(),
    ))
}
