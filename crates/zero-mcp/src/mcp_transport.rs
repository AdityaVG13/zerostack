//! Hub-owned FastMCP compatibility transport.
//!
//! Engine adapters provide a validated [`SurfaceRegistration`] and a callback
//! that owns operation semantics. This module owns tool registration, bounded
//! callback execution, cancellation hooks, and FastMCP stdio lifecycle. It
//! does not import an engine crate or execute CodeMode plans.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(feature = "fastmcp")]
use zero_abi::CanonicalResource;
use zero_abi::{
    SurfaceContractError, SurfaceKind, SurfaceRegistration, ZeroKernelResponse, canonical_json,
};

pub const ZERO_CARRIER_TOOL_NAME: &str = "zero";
pub const ZERO_CARRIER_PLAN_BYTE_LIMIT: usize = 64 * 1024;
pub const ZERO_CARRIER_MESSAGE_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroCarrierSampling {
    SameModel,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ZeroCarrierCapabilities {
    pub cancellation: bool,
    pub progress: bool,
    pub sampling: ZeroCarrierSampling,
    pub maximum_inbound_bytes: u64,
    pub maximum_outbound_bytes: u64,
    pub native_package_digest: String,
}

impl ZeroCarrierCapabilities {
    pub fn validate(&self) -> Result<(), McpTransportError> {
        if self.maximum_inbound_bytes == 0
            || self.maximum_outbound_bytes == 0
            || self.maximum_inbound_bytes > ZERO_CARRIER_MESSAGE_BYTE_LIMIT
            || self.maximum_outbound_bytes > ZERO_CARRIER_MESSAGE_BYTE_LIMIT
        {
            return Err(McpTransportError::InvalidCarrier(
                "carrier message bounds must be finite and within policy".into(),
            ));
        }
        if self.native_package_digest.len() != 64
            || !self
                .native_package_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(McpTransportError::InvalidCarrier(
                "native package digest must be 64 hexadecimal characters".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroCarrierRequest {
    pub plan: String,
}

pub fn zero_carrier_catalog() -> Value {
    serde_json::json!([{
        "name": ZERO_CARRIER_TOOL_NAME,
        "description": "Run one bounded JavaScript or TypeScript cell with direct z.* methods.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "plan": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": ZERO_CARRIER_PLAN_BYTE_LIMIT,
                }
            },
            "required": ["plan"]
        }
    }])
}

pub fn decode_zero_carrier_request(
    arguments: Value,
    capabilities: &ZeroCarrierCapabilities,
) -> Result<ZeroCarrierRequest, McpTransportError> {
    capabilities.validate()?;
    let encoded = serde_json::to_vec(&arguments)
        .map_err(|error| McpTransportError::InvalidCarrier(error.to_string()))?;
    if encoded.len() as u64 > capabilities.maximum_inbound_bytes {
        return Err(McpTransportError::InvalidCarrier(
            "carrier request exceeds the negotiated inbound bound".into(),
        ));
    }
    let request: ZeroCarrierRequest = serde_json::from_value(arguments)
        .map_err(|error| McpTransportError::InvalidCarrier(error.to_string()))?;
    let bytes = request.plan.as_bytes().len();
    if bytes == 0 || bytes > ZERO_CARRIER_PLAN_BYTE_LIMIT {
        return Err(McpTransportError::InvalidCarrier(format!(
            "carrier plan must contain 1..={ZERO_CARRIER_PLAN_BYTE_LIMIT} UTF-8 bytes"
        )));
    }
    Ok(request)
}

pub fn render_zero_carrier_response(
    response: &ZeroKernelResponse,
    capabilities: &ZeroCarrierCapabilities,
) -> Result<String, McpTransportError> {
    capabilities.validate()?;
    response
        .validate()
        .map_err(|error| McpTransportError::InvalidCarrier(error.to_string()))?;
    let value = serde_json::to_value(response)
        .map_err(|error| McpTransportError::InvalidCarrier(error.to_string()))?;
    let rendered = canonical_json(&value);
    if rendered.len() as u64 > capabilities.maximum_outbound_bytes {
        return Err(McpTransportError::InvalidCarrier(
            "canonical ZeroKernel response exceeds the negotiated outbound bound".into(),
        ));
    }
    Ok(rendered)
}

/// Default compatibility behavior: no hub-imposed outer deadline. Domain
/// operations retain their own declared deadlines and remain cancellable.
pub const DEFAULT_MCP_TOOL_TIMEOUT: Duration = Duration::ZERO;
/// Hard upper bound for an explicitly configured finite outer deadline.
pub const MAX_MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(3_600);
/// Default number of callbacks that may run at once.
pub const DEFAULT_MCP_MAX_INFLIGHT: usize = 16;
/// Hard upper bound for concurrent compatibility callbacks.
pub const MAX_MCP_MAX_INFLIGHT: usize = 256;
const CANCELLATION_POLL: Duration = Duration::from_millis(10);
/// How long a cancelled/timeout handler waits for a late worker result
/// before attaching a still-running receipt and detaching. Must stay far
/// below any product tool timeout so the handler thread is never unbounded.
const LATE_RESULT_BOUND: Duration = Duration::from_millis(100);

/// Configuration shared by every dynamically registered tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McpTransportConfig {
    /// Finite hub outer deadline. Zero delegates deadline policy to the engine.
    pub tool_timeout: Duration,
    pub max_inflight: usize,
}

impl Default for McpTransportConfig {
    fn default() -> Self {
        Self {
            tool_timeout: DEFAULT_MCP_TOOL_TIMEOUT,
            max_inflight: DEFAULT_MCP_MAX_INFLIGHT,
        }
    }
}

/// Alias-specific MCP catalog metadata. The alias name must already be
/// declared by the canonical operation; this only preserves its visible face.
#[derive(Clone, Debug, PartialEq)]
pub struct McpAliasMetadata {
    pub canonical_id: String,
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
}

/// Engine-owned MCP initialize identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerIdentity {
    pub name: String,
    pub version: String,
}

impl McpServerIdentity {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, McpTransportError> {
        let identity = Self {
            name: name.into(),
            version: version.into(),
        };
        for (label, value) in [
            ("server name", identity.name.as_str()),
            ("server version", identity.version.as_str()),
        ] {
            if value.is_empty() || value.trim() != value {
                return Err(McpTransportError::InvalidServerIdentity(format!(
                    "{label} must be non-empty and trimmed"
                )));
            }
        }
        Ok(identity)
    }
}

/// Model-visible MCP error text policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpErrorPresentation {
    #[default]
    Structured,
    PlainMessage,
}

impl McpTransportConfig {
    pub fn validate(self) -> Result<Self, McpTransportError> {
        if self.tool_timeout > MAX_MCP_TOOL_TIMEOUT {
            return Err(McpTransportError::InvalidConfig(format!(
                "tool_timeout must not exceed {}s",
                MAX_MCP_TOOL_TIMEOUT.as_secs()
            )));
        }
        if self.max_inflight == 0 {
            return Err(McpTransportError::InvalidConfig(
                "max_inflight must be greater than zero".into(),
            ));
        }
        if self.max_inflight > MAX_MCP_MAX_INFLIGHT {
            return Err(McpTransportError::InvalidConfig(format!(
                "max_inflight must not exceed {MAX_MCP_MAX_INFLIGHT}"
            )));
        }
        Ok(self)
    }
}

/// Validate the selected install-time surface for a compatibility transport.
pub fn validate_mcp_registration(
    registration: &SurfaceRegistration,
) -> Result<(), McpTransportError> {
    registration.validate()?;
    if registration.surface != SurfaceKind::Mcp {
        return Err(McpTransportError::WrongSurface(registration.surface));
    }
    Ok(())
}

/// Cooperative cancellation and deadline information for one domain callback.
#[derive(Clone, Debug)]
pub struct McpCallContext {
    deadline: Option<Instant>,
    cancelled: Arc<AtomicBool>,
}

impl McpCallContext {
    /// Finite hub deadline, or `None` when the engine owns deadline policy.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn check(&self) -> Result<(), McpDispatchError> {
        if self.is_cancelled() {
            Err(McpDispatchError::cancelled())
        } else {
            Ok(())
        }
    }
}

/// Lossless model-visible text returned by an engine MCP adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpTextContent {
    pub text: String,
}

impl McpTextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Engine callback output. `Json` preserves the original compatibility path;
/// `Text` preserves exact, ordered MCP content entries without reserialization.
#[derive(Clone, Debug, PartialEq)]
pub enum McpDispatchOutput {
    Json(Value),
    Text(Vec<McpTextContent>),
}

impl From<Value> for McpDispatchOutput {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

/// Engine-owned resource output. Text and blob carriers remain byte-visible;
/// JSON keeps the compatibility behavior used by existing adapters.
#[derive(Clone, Debug, PartialEq)]
pub enum McpResourceOutput {
    Json(Value),
    Text(String),
    Blob(String),
}

impl From<Value> for McpResourceOutput {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

/// Engine callback used by the generic transport.
pub trait McpDispatcher: Send + Sync {
    fn dispatch(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<Value, McpDispatchError>;

    fn dispatch_output(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<McpDispatchOutput, McpDispatchError> {
        self.dispatch(tool, arguments, context)
            .map(McpDispatchOutput::Json)
    }
}

/// Engine-owned resource payload seam. The transport owns only bounded
/// execution and FastMCP serialization; URI meaning stays with the engine.
pub trait McpResourceReader: Send + Sync {
    fn read(&self, uri: &str, context: &McpCallContext) -> Result<Value, McpDispatchError>;

    fn read_output(
        &self,
        uri: &str,
        context: &McpCallContext,
    ) -> Result<McpResourceOutput, McpDispatchError> {
        self.read(uri, context).map(McpResourceOutput::Json)
    }
}

impl<F> McpResourceReader for F
where
    F: Fn(&str, &McpCallContext) -> Result<Value, McpDispatchError> + Send + Sync,
{
    fn read(&self, uri: &str, context: &McpCallContext) -> Result<Value, McpDispatchError> {
        self(uri, context)
    }
}

impl<F> McpDispatcher for F
where
    F: Fn(&str, Value, &McpCallContext) -> Result<Value, McpDispatchError> + Send + Sync,
{
    fn dispatch(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<Value, McpDispatchError> {
        self(tool, arguments, context)
    }
}

pub trait ZeroCarrierExecutor: Send + Sync {
    fn execute(
        &self,
        plan: &str,
        context: &McpCallContext,
    ) -> Result<ZeroKernelResponse, McpDispatchError>;
}

impl<F> ZeroCarrierExecutor for F
where
    F: Fn(&str, &McpCallContext) -> Result<ZeroKernelResponse, McpDispatchError> + Send + Sync,
{
    fn execute(
        &self,
        plan: &str,
        context: &McpCallContext,
    ) -> Result<ZeroKernelResponse, McpDispatchError> {
        self(plan, context)
    }
}

pub struct ZeroCarrierDispatcher {
    executor: Arc<dyn ZeroCarrierExecutor>,
    capabilities: ZeroCarrierCapabilities,
}

impl ZeroCarrierDispatcher {
    pub fn new(
        executor: Arc<dyn ZeroCarrierExecutor>,
        capabilities: ZeroCarrierCapabilities,
    ) -> Result<Self, McpTransportError> {
        capabilities.validate()?;
        Ok(Self {
            executor,
            capabilities,
        })
    }

    fn execute(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<String, McpDispatchError> {
        if tool != ZERO_CARRIER_TOOL_NAME {
            return Err(McpDispatchError::new(
                "unknown_tool",
                format!("ZeroKernel carrier exposes only {ZERO_CARRIER_TOOL_NAME:?}"),
                false,
            )
            .with_op(tool));
        }
        context.check()?;
        let request =
            decode_zero_carrier_request(arguments, &self.capabilities).map_err(|error| {
                McpDispatchError::new("invalid_request", error.to_string(), false).with_op(tool)
            })?;
        let response = self.executor.execute(&request.plan, context)?;
        context.check()?;
        render_zero_carrier_response(&response, &self.capabilities).map_err(|error| {
            McpDispatchError::new("invalid_response", error.to_string(), false).with_op(tool)
        })
    }
}

impl McpDispatcher for ZeroCarrierDispatcher {
    fn dispatch(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<Value, McpDispatchError> {
        let rendered = self.execute(tool, arguments, context)?;
        serde_json::from_str(&rendered).map_err(|error| {
            McpDispatchError::new("invalid_response", error.to_string(), false).with_op(tool)
        })
    }

    fn dispatch_output(
        &self,
        tool: &str,
        arguments: Value,
        context: &McpCallContext,
    ) -> Result<McpDispatchOutput, McpDispatchError> {
        self.execute(tool, arguments, context)
            .map(|text| McpDispatchOutput::Text(vec![McpTextContent::new(text)]))
    }
}

/// Structured domain failure preserved in the MCP tool-result text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpDispatchError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl McpDispatchError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            retryable,
            op: None,
            data: None,
        }
    }

    pub fn with_op(mut self, op: impl Into<String>) -> Self {
        self.op = Some(op.into());
        self
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn cancelled() -> Self {
        Self::new("cancelled", "MCP tool call cancelled", true)
    }

    fn timeout(tool: &str, timeout: Duration) -> Self {
        Self::new(
            "timeout",
            format!("MCP tool {tool:?} exceeded {}ms", timeout.as_millis()),
            true,
        )
        .with_op(tool)
    }

    fn disconnected(tool: &str) -> Self {
        Self::new(
            "dispatch_failed",
            "MCP domain dispatcher disconnected before completing",
            false,
        )
        .with_op(tool)
    }

    fn capacity(tool: &str) -> Self {
        Self::new(
            "busy",
            "MCP compatibility callback capacity is exhausted",
            true,
        )
        .with_op(tool)
    }

    pub fn wire_text(&self) -> String {
        serde_json::to_string(self).expect("McpDispatchError is serializable")
    }
}

impl fmt::Display for McpDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.wire_text())
    }
}

impl std::error::Error for McpDispatchError {}

#[derive(Debug)]
pub enum McpTransportError {
    InvalidConfig(String),
    InvalidAliasMetadata(String),
    InvalidServerIdentity(String),
    InvalidCarrier(String),
    Surface(SurfaceContractError),
    WrongSurface(SurfaceKind),
    MissingResourceReader,
}

impl fmt::Display for McpTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => {
                write!(formatter, "invalid MCP transport config: {message}")
            }
            Self::InvalidAliasMetadata(message) => {
                write!(formatter, "invalid MCP alias metadata: {message}")
            }
            Self::InvalidServerIdentity(message) => {
                write!(formatter, "invalid MCP server identity: {message}")
            }
            Self::InvalidCarrier(message) => {
                write!(formatter, "invalid ZeroKernel carrier: {message}")
            }
            Self::Surface(error) => write!(formatter, "invalid MCP surface registration: {error}"),
            Self::WrongSurface(surface) => write!(
                formatter,
                "MCP compatibility transport requires mcp surface, got {}",
                surface.as_str()
            ),
            Self::MissingResourceReader => {
                write!(
                    formatter,
                    "MCP resources require an engine-owned resource reader"
                )
            }
        }
    }
}

impl std::error::Error for McpTransportError {}

impl From<SurfaceContractError> for McpTransportError {
    fn from(error: SurfaceContractError) -> Self {
        Self::Surface(error)
    }
}

struct Inflight {
    active: AtomicUsize,
    maximum: usize,
}

impl Inflight {
    fn new(maximum: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            maximum,
        }
    }

    fn acquire(self: &Arc<Self>, tool: &str) -> Result<InflightGuard, McpDispatchError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.maximum).then_some(active + 1)
            })
            .map(|_| InflightGuard(Arc::clone(self)))
            .map_err(|_| McpDispatchError::capacity(tool))
    }
}

struct InflightGuard(Arc<Inflight>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Release);
    }
}

/// Execute one callback with bounded wall time and cooperative cancellation.
pub fn execute_call(
    dispatcher: Arc<dyn McpDispatcher>,
    tool: &str,
    arguments: Value,
    config: McpTransportConfig,
) -> Result<Value, McpDispatchError> {
    execute_call_with_cancel(dispatcher, tool, arguments, config, || false)
}

/// Execute one callback while polling an external cancellation hook.
pub fn execute_call_with_cancel<F>(
    dispatcher: Arc<dyn McpDispatcher>,
    tool: &str,
    arguments: Value,
    config: McpTransportConfig,
    externally_cancelled: F,
) -> Result<Value, McpDispatchError>
where
    F: Fn() -> bool,
{
    let config = config.validate().map_err(|error| {
        McpDispatchError::new("invalid_config", error.to_string(), false).with_op(tool)
    })?;
    execute_call_with_limiter(
        dispatcher,
        tool,
        arguments,
        config,
        Arc::new(Inflight::new(config.max_inflight)),
        externally_cancelled,
    )
}

fn execute_call_with_limiter<F>(
    dispatcher: Arc<dyn McpDispatcher>,
    tool: &str,
    arguments: Value,
    config: McpTransportConfig,
    limiter: Arc<Inflight>,
    externally_cancelled: F,
) -> Result<Value, McpDispatchError>
where
    F: Fn() -> bool,
{
    let tool_name = tool.to_owned();
    execute_bounded(
        tool,
        config,
        limiter,
        move |context| dispatcher.dispatch(&tool_name, arguments, context),
        externally_cancelled,
    )
}

#[cfg(feature = "fastmcp")]
fn execute_output_with_limiter<F>(
    dispatcher: Arc<dyn McpDispatcher>,
    tool: &str,
    arguments: Value,
    config: McpTransportConfig,
    limiter: Arc<Inflight>,
    externally_cancelled: F,
) -> Result<McpDispatchOutput, McpDispatchError>
where
    F: Fn() -> bool,
{
    let tool_name = tool.to_owned();
    execute_bounded(
        tool,
        config,
        limiter,
        move |context| dispatcher.dispatch_output(&tool_name, arguments, context),
        externally_cancelled,
    )
}

#[cfg(feature = "fastmcp")]
fn execute_resource_with_limiter<F>(
    reader: Arc<dyn McpResourceReader>,
    uri: &str,
    config: McpTransportConfig,
    limiter: Arc<Inflight>,
    externally_cancelled: F,
) -> Result<McpResourceOutput, McpDispatchError>
where
    F: Fn() -> bool,
{
    let owned_uri = uri.to_owned();
    execute_bounded(
        uri,
        config,
        limiter,
        move |context| reader.read_output(&owned_uri, context),
        externally_cancelled,
    )
}

trait LateOkPayload {
    fn late_ok_data(self) -> Value;
}

impl LateOkPayload for Value {
    fn late_ok_data(self) -> Value {
        serde_json::json!({ "result": self })
    }
}

impl LateOkPayload for McpDispatchOutput {
    fn late_ok_data(self) -> Value {
        match self {
            Self::Json(value) => serde_json::json!({ "result": value }),
            Self::Text(parts) => {
                let texts: Vec<String> = parts.into_iter().map(|part| part.text).collect();
                serde_json::json!({ "result": { "text": texts } })
            }
        }
    }
}

impl LateOkPayload for McpResourceOutput {
    fn late_ok_data(self) -> Value {
        match self {
            Self::Json(value) => serde_json::json!({ "result": value }),
            Self::Text(text) => serde_json::json!({ "result": { "text": text } }),
            Self::Blob(blob) => serde_json::json!({ "result": { "blob": blob } }),
        }
    }
}

fn commit_race_error(operation: &str, data: Value) -> McpDispatchError {
    McpDispatchError::new(
        "commit_race",
        "MCP tool completed after cancel/timeout; result is not a clean success",
        false,
    )
    .with_op(operation)
    .with_data(data)
}

fn execute_bounded<T, D, F>(
    operation: &str,
    config: McpTransportConfig,
    limiter: Arc<Inflight>,
    dispatch: D,
    externally_cancelled: F,
) -> Result<T, McpDispatchError>
where
    T: LateOkPayload + Send + 'static,
    D: FnOnce(&McpCallContext) -> Result<T, McpDispatchError> + Send + 'static,
    F: Fn() -> bool,
{
    let permit = limiter.acquire(operation)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = (!config.tool_timeout.is_zero()).then(|| Instant::now() + config.tool_timeout);
    let context = McpCallContext {
        deadline,
        cancelled: Arc::clone(&cancelled),
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("zerostack-mcp-tool".into())
        .spawn(move || {
            let _permit = permit;
            let result = dispatch(&context);
            let _ = sender.send(result);
        })
        .map_err(|error| {
            McpDispatchError::new(
                "dispatch_failed",
                format!("failed to start MCP domain callback: {error}"),
                false,
            )
            .with_op(operation)
        })?;

    let started = Instant::now();
    let outcome = loop {
        if externally_cancelled() {
            cancelled.store(true, Ordering::Release);
            break Err(McpDispatchError::cancelled().with_op(operation));
        }
        let elapsed = started.elapsed();
        if !config.tool_timeout.is_zero() && elapsed >= config.tool_timeout {
            cancelled.store(true, Ordering::Release);
            break Err(McpDispatchError::timeout(operation, config.tool_timeout));
        }
        let wait = if config.tool_timeout.is_zero() {
            CANCELLATION_POLL
        } else {
            (config.tool_timeout - elapsed).min(CANCELLATION_POLL)
        };
        match receiver.recv_timeout(wait) {
            Ok(Err(error))
                if error.kind == "cancelled"
                    && deadline.is_some_and(|deadline| Instant::now() >= deadline) =>
            {
                break Err(McpDispatchError::timeout(operation, config.tool_timeout));
            }
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::Release);
                break Err(McpDispatchError::disconnected(operation));
            }
        }
    };
    match outcome {
        Ok(value) => {
            let _ = worker.join();
            Ok(value)
        }
        Err(error) if error.kind == "cancelled" || error.kind == "timeout" => {
            let late = match receiver.recv_timeout(LATE_RESULT_BOUND) {
                Ok(late) => Some(late),
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => None,
            };
            match late {
                Some(Ok(value)) => {
                    let _ = worker.join();
                    Err(commit_race_error(operation, value.late_ok_data()))
                }
                Some(Err(late_error)) => {
                    let _ = worker.join();
                    if matches!(late_error.kind.as_str(), "cancelled" | "timeout") {
                        Err(error)
                    } else {
                        Err(late_error)
                    }
                }
                None => {
                    drop(worker);
                    Err(error.with_data(serde_json::json!({ "still_running": true })))
                }
            }
        }
        Err(error) => {
            let _ = worker.join();
            Err(error)
        }
    }
}

#[cfg(feature = "fastmcp")]
mod fastmcp {
    use super::*;
    use fastmcp_rust::ResourceHandler;
    use fastmcp_rust::prelude::{
        Content, McpContext, McpError, Resource, ResourceContent, Server, Tool,
    };
    use fastmcp_rust::{ToolAnnotations, ToolHandler};
    use zero_abi::{CanonicalOperation, EffectClass};

    struct RegisteredTool {
        definition: Tool,
        dispatcher: Arc<dyn McpDispatcher>,
        config: McpTransportConfig,
        error_presentation: McpErrorPresentation,
        limiter: Arc<Inflight>,
    }

    struct RegisteredZeroCarrierTool {
        definition: Tool,
        dispatcher: Arc<ZeroCarrierDispatcher>,
        config: McpTransportConfig,
        limiter: Arc<Inflight>,
    }

    impl RegisteredZeroCarrierTool {
        fn new(
            dispatcher: Arc<ZeroCarrierDispatcher>,
            config: McpTransportConfig,
            limiter: Arc<Inflight>,
        ) -> Self {
            let catalog = zero_carrier_catalog();
            let entry = &catalog.as_array().expect("carrier catalog is an array")[0];
            Self {
                definition: Tool {
                    name: ZERO_CARRIER_TOOL_NAME.into(),
                    description: entry["description"].as_str().map(str::to_owned),
                    input_schema: entry["inputSchema"].clone(),
                    output_schema: None,
                    icon: None,
                    version: None,
                    tags: Vec::new(),
                    annotations: Some(ToolAnnotations {
                        destructive: Some(true),
                        idempotent: None,
                        read_only: Some(false),
                        open_world_hint: Some(false),
                    }),
                },
                dispatcher,
                config,
                limiter,
            }
        }
    }

    impl ToolHandler for RegisteredZeroCarrierTool {
        fn definition(&self) -> Tool {
            self.definition.clone()
        }

        fn call(
            &self,
            context: &McpContext,
            arguments: Value,
        ) -> fastmcp_rust::McpResult<Vec<Content>> {
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            let dispatcher: Arc<dyn McpDispatcher> = self.dispatcher.clone();
            let result = execute_output_with_limiter(
                dispatcher,
                ZERO_CARRIER_TOOL_NAME,
                arguments,
                self.config,
                Arc::clone(&self.limiter),
                || context.is_cancelled(),
            );
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            match result {
                Ok(McpDispatchOutput::Text(items)) => Ok(items
                    .into_iter()
                    .map(|item| Content::text(item.text))
                    .collect()),
                Ok(McpDispatchOutput::Json(value)) => {
                    Ok(vec![Content::text(canonical_json(&value))])
                }
                Err(error) => Err(present_dispatch_error(
                    error,
                    McpErrorPresentation::Structured,
                )),
            }
        }
    }

    pub struct FastMcpZeroCarrier {
        dispatcher: Arc<ZeroCarrierDispatcher>,
        config: McpTransportConfig,
        server_identity: McpServerIdentity,
    }

    impl FastMcpZeroCarrier {
        pub fn new(
            executor: Arc<dyn ZeroCarrierExecutor>,
            capabilities: ZeroCarrierCapabilities,
            config: McpTransportConfig,
        ) -> Result<Self, McpTransportError> {
            let config = config.validate()?;
            let dispatcher = Arc::new(ZeroCarrierDispatcher::new(executor, capabilities)?);
            Ok(Self {
                dispatcher,
                config,
                server_identity: McpServerIdentity::new("zero", env!("CARGO_PKG_VERSION"))?,
            })
        }

        pub fn with_server_identity(
            mut self,
            name: impl Into<String>,
            version: impl Into<String>,
        ) -> Result<Self, McpTransportError> {
            self.server_identity = McpServerIdentity::new(name, version)?;
            Ok(self)
        }

        pub fn catalog(&self) -> Vec<Tool> {
            vec![
                RegisteredZeroCarrierTool::new(
                    Arc::clone(&self.dispatcher),
                    self.config,
                    Arc::new(Inflight::new(self.config.max_inflight)),
                )
                .definition,
            ]
        }

        pub fn build_server(&self) -> Server {
            Server::new(
                self.server_identity.name.clone(),
                self.server_identity.version.clone(),
            )
            .request_timeout(0)
            .tool(RegisteredZeroCarrierTool::new(
                Arc::clone(&self.dispatcher),
                self.config,
                Arc::new(Inflight::new(self.config.max_inflight)),
            ))
            .instructions("One canonical zero tool. The runtime is ZeroKernel.")
            .build()
        }

        pub fn run_stdio(self) -> ! {
            self.build_server().run_stdio()
        }
    }

    pub(super) fn present_dispatch_error(
        error: McpDispatchError,
        presentation: McpErrorPresentation,
    ) -> McpError {
        match presentation {
            McpErrorPresentation::Structured => McpError::tool_error(error.wire_text()),
            McpErrorPresentation::PlainMessage => McpError::tool_error(error.message),
        }
    }

    pub(super) struct RegisteredResource {
        definition: Resource,
        reader: Arc<dyn McpResourceReader>,
        config: McpTransportConfig,
        error_presentation: McpErrorPresentation,
        limiter: Arc<Inflight>,
    }

    impl RegisteredResource {
        pub(super) fn new(
            resource: &CanonicalResource,
            reader: Arc<dyn McpResourceReader>,
            config: McpTransportConfig,
            error_presentation: McpErrorPresentation,
            limiter: Arc<Inflight>,
        ) -> Self {
            Self {
                definition: Resource {
                    uri: resource.uri.clone(),
                    name: resource.name.clone(),
                    description: (!resource.description.is_empty())
                        .then(|| resource.description.clone()),
                    mime_type: resource.mime_type.clone(),
                    icon: None,
                    version: None,
                    tags: Vec::new(),
                },
                reader,
                config,
                error_presentation,
                limiter,
            }
        }
    }

    impl ResourceHandler for RegisteredResource {
        fn definition(&self) -> Resource {
            self.definition.clone()
        }

        fn read(&self, context: &McpContext) -> fastmcp_rust::McpResult<Vec<ResourceContent>> {
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            let uri = self.definition.uri.clone();
            let result = execute_resource_with_limiter(
                Arc::clone(&self.reader),
                &uri,
                self.config,
                Arc::clone(&self.limiter),
                || context.is_cancelled(),
            );
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            match result {
                Ok(output) => {
                    let (text, blob) = match output {
                        McpResourceOutput::Json(value) => (
                            Some(
                                serde_json::to_string(&value)
                                    .expect("JSON values are serializable"),
                            ),
                            None,
                        ),
                        McpResourceOutput::Text(text) => (Some(text), None),
                        McpResourceOutput::Blob(blob) => (None, Some(blob)),
                    };
                    Ok(vec![ResourceContent {
                        uri,
                        mime_type: self.definition.mime_type.clone(),
                        text,
                        blob,
                    }])
                }
                Err(error) => Err(present_dispatch_error(error, self.error_presentation)),
            }
        }
    }

    impl RegisteredTool {
        fn new(
            operation: &CanonicalOperation,
            dispatcher: Arc<dyn McpDispatcher>,
            config: McpTransportConfig,
            error_presentation: McpErrorPresentation,
            limiter: Arc<Inflight>,
        ) -> Self {
            let name = operation
                .mcp_tool_name
                .clone()
                .unwrap_or_else(|| operation.canonical_id.clone());
            let description = Some(if operation.description.is_empty() {
                format!("{} operation", operation.canonical_id)
            } else {
                operation.description.clone()
            });
            Self::with_definition(
                operation,
                name,
                description,
                operation.args_schema.clone(),
                operation.output_schema.clone(),
                dispatcher,
                config,
                error_presentation,
                limiter,
            )
        }

        fn new_alias(
            operation: &CanonicalOperation,
            alias: &str,
            metadata: Option<&McpAliasMetadata>,
            dispatcher: Arc<dyn McpDispatcher>,
            config: McpTransportConfig,
            error_presentation: McpErrorPresentation,
            limiter: Arc<Inflight>,
        ) -> Self {
            let canonical_description = Some(if operation.description.is_empty() {
                format!("{} operation", operation.canonical_id)
            } else {
                operation.description.clone()
            });
            Self::with_definition(
                operation,
                alias.to_owned(),
                metadata
                    .map(|entry| entry.description.clone())
                    .unwrap_or(canonical_description),
                metadata
                    .map(|entry| entry.input_schema.clone())
                    .unwrap_or_else(|| operation.args_schema.clone()),
                metadata
                    .map(|entry| entry.output_schema.clone())
                    .unwrap_or_else(|| operation.output_schema.clone()),
                dispatcher,
                config,
                error_presentation,
                limiter,
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn with_definition(
            operation: &CanonicalOperation,
            name: String,
            description: Option<String>,
            input_schema: Value,
            output_schema: Option<Value>,
            dispatcher: Arc<dyn McpDispatcher>,
            config: McpTransportConfig,
            error_presentation: McpErrorPresentation,
            limiter: Arc<Inflight>,
        ) -> Self {
            let read_only = operation.effect_policy.effect_class == EffectClass::ReadOnly;
            Self {
                definition: Tool {
                    name,
                    description,
                    input_schema,
                    output_schema,
                    icon: None,
                    version: None,
                    tags: Vec::new(),
                    annotations: Some(ToolAnnotations {
                        destructive: Some(!read_only),
                        idempotent: None,
                        read_only: Some(read_only),
                        open_world_hint: Some(false),
                    }),
                },
                dispatcher,
                config,
                error_presentation,
                limiter,
            }
        }
    }

    impl ToolHandler for RegisteredTool {
        fn definition(&self) -> Tool {
            self.definition.clone()
        }

        fn call(
            &self,
            context: &McpContext,
            arguments: Value,
        ) -> fastmcp_rust::McpResult<Vec<Content>> {
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            let result = execute_output_with_limiter(
                Arc::clone(&self.dispatcher),
                &self.definition.name,
                arguments,
                self.config,
                Arc::clone(&self.limiter),
                || context.is_cancelled(),
            );
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            match result {
                Ok(McpDispatchOutput::Json(value)) => Ok(vec![Content::text(
                    serde_json::to_string(&value).expect("JSON values are serializable"),
                )]),
                Ok(McpDispatchOutput::Text(items)) => Ok(items
                    .into_iter()
                    .map(|item| Content::text(item.text))
                    .collect()),
                Err(error) => Err(present_dispatch_error(error, self.error_presentation)),
            }
        }
    }

    /// A validated dynamic FastMCP server over one engine registration.
    pub struct FastMcpTransport {
        registration: SurfaceRegistration,
        dispatcher: Arc<dyn McpDispatcher>,
        resource_reader: Option<Arc<dyn McpResourceReader>>,
        alias_metadata: Vec<McpAliasMetadata>,
        server_identity: McpServerIdentity,
        error_presentation: McpErrorPresentation,
        config: McpTransportConfig,
    }

    impl FastMcpTransport {
        pub fn new(
            registration: SurfaceRegistration,
            dispatcher: Arc<dyn McpDispatcher>,
            config: McpTransportConfig,
        ) -> Result<Self, McpTransportError> {
            validate_mcp_registration(&registration)?;
            let config = config.validate()?;
            if !registration.adapter.registry.resources.is_empty() {
                return Err(McpTransportError::MissingResourceReader);
            }
            let server_identity = McpServerIdentity {
                name: registration.root.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
            };
            Ok(Self {
                registration,
                dispatcher,
                resource_reader: None,
                alias_metadata: Vec::new(),
                server_identity,
                error_presentation: McpErrorPresentation::default(),
                config,
            })
        }

        /// Construct a transport with an engine-owned resource reader.
        pub fn with_resources(
            registration: SurfaceRegistration,
            dispatcher: Arc<dyn McpDispatcher>,
            resource_reader: Arc<dyn McpResourceReader>,
            config: McpTransportConfig,
        ) -> Result<Self, McpTransportError> {
            validate_mcp_registration(&registration)?;
            let config = config.validate()?;
            let server_identity = McpServerIdentity {
                name: registration.root.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
            };
            Ok(Self {
                registration,
                dispatcher,
                resource_reader: Some(resource_reader),
                alias_metadata: Vec::new(),
                server_identity,
                error_presentation: McpErrorPresentation::default(),
                config,
            })
        }

        pub fn registration(&self) -> &SurfaceRegistration {
            &self.registration
        }

        pub fn with_server_identity(
            mut self,
            name: impl Into<String>,
            version: impl Into<String>,
        ) -> Result<Self, McpTransportError> {
            self.server_identity = McpServerIdentity::new(name, version)?;
            Ok(self)
        }

        pub fn with_error_presentation(mut self, presentation: McpErrorPresentation) -> Self {
            self.error_presentation = presentation;
            self
        }

        /// Add lossless alias-specific catalog metadata. Every entry must bind
        /// an alias already declared by its canonical operation.
        pub fn with_alias_metadata(
            mut self,
            alias_metadata: Vec<McpAliasMetadata>,
        ) -> Result<Self, McpTransportError> {
            for (index, entry) in alias_metadata.iter().enumerate() {
                if entry.name.is_empty() || entry.name.trim() != entry.name {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "entry {index} has invalid alias name {:?}",
                        entry.name
                    )));
                }
                if !entry.input_schema.is_object() || entry.input_schema.get("type").is_none() {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "alias {:?} input schema must be an object with type",
                        entry.name
                    )));
                }
                if entry
                    .output_schema
                    .as_ref()
                    .is_some_and(|schema| !schema.is_object())
                {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "alias {:?} output schema must be an object",
                        entry.name
                    )));
                }
                let Some(operation) = self
                    .registration
                    .adapter
                    .registry
                    .operations
                    .iter()
                    .find(|operation| operation.canonical_id == entry.canonical_id)
                else {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "unknown canonical operation {:?}",
                        entry.canonical_id
                    )));
                };
                if !operation.aliases.iter().any(|alias| alias == &entry.name) {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "{:?} is not an alias of {:?}",
                        entry.name, entry.canonical_id
                    )));
                }
                if alias_metadata[..index].iter().any(|prior| {
                    prior.canonical_id == entry.canonical_id && prior.name == entry.name
                }) {
                    return Err(McpTransportError::InvalidAliasMetadata(format!(
                        "duplicate alias metadata for {:?}",
                        entry.name
                    )));
                }
            }
            self.alias_metadata = alias_metadata;
            Ok(self)
        }

        fn alias_metadata(
            &self,
            operation: &CanonicalOperation,
            alias: &str,
        ) -> Option<&McpAliasMetadata> {
            self.alias_metadata
                .iter()
                .find(|entry| entry.canonical_id == operation.canonical_id && entry.name == alias)
        }

        /// Return the dynamic tool catalog derived from the canonical registry.
        pub fn catalog(&self) -> Vec<Tool> {
            let limiter = Arc::new(Inflight::new(self.config.max_inflight));
            self.registration
                .adapter
                .registry
                .operations
                .iter()
                .flat_map(|operation| {
                    let canonical = RegisteredTool::new(
                        operation,
                        Arc::clone(&self.dispatcher),
                        self.config,
                        self.error_presentation,
                        Arc::clone(&limiter),
                    )
                    .definition;
                    let aliases = operation
                        .aliases
                        .iter()
                        .map(|alias| {
                            let metadata = self.alias_metadata(operation, alias);
                            Tool {
                                name: alias.clone(),
                                description: metadata
                                    .map(|entry| entry.description.clone())
                                    .unwrap_or_else(|| canonical.description.clone()),
                                input_schema: metadata
                                    .map(|entry| entry.input_schema.clone())
                                    .unwrap_or_else(|| canonical.input_schema.clone()),
                                output_schema: metadata
                                    .map(|entry| entry.output_schema.clone())
                                    .unwrap_or_else(|| canonical.output_schema.clone()),
                                icon: None,
                                version: None,
                                tags: Vec::new(),
                                annotations: canonical.annotations.clone(),
                            }
                        })
                        .collect::<Vec<_>>();
                    std::iter::once(canonical)
                        .chain(aliases)
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        /// Return the dynamic resource catalog. Resource payloads are never
        /// synthesized by the hub and are available only with a reader.
        pub fn resource_catalog(&self) -> Vec<Resource> {
            self.registration
                .adapter
                .registry
                .resources
                .iter()
                .map(|resource| Resource {
                    uri: resource.uri.clone(),
                    name: resource.name.clone(),
                    description: (!resource.description.is_empty())
                        .then(|| resource.description.clone()),
                    mime_type: resource.mime_type.clone(),
                    icon: None,
                    version: None,
                    tags: Vec::new(),
                })
                .collect()
        }

        pub fn build_server(&self) -> Server {
            let limiter = Arc::new(Inflight::new(self.config.max_inflight));
            let mut builder = Server::new(
                self.server_identity.name.clone(),
                self.server_identity.version.clone(),
            )
            .request_timeout(0);
            for operation in &self.registration.adapter.registry.operations {
                builder = builder.tool(RegisteredTool::new(
                    operation,
                    Arc::clone(&self.dispatcher),
                    self.config,
                    self.error_presentation,
                    Arc::clone(&limiter),
                ));
                for alias in &operation.aliases {
                    builder = builder.tool(RegisteredTool::new_alias(
                        operation,
                        alias,
                        self.alias_metadata(operation, alias),
                        Arc::clone(&self.dispatcher),
                        self.config,
                        self.error_presentation,
                        Arc::clone(&limiter),
                    ));
                }
            }
            if let Some(reader) = &self.resource_reader {
                for resource in &self.registration.adapter.registry.resources {
                    builder = builder.resource(RegisteredResource::new(
                        resource,
                        Arc::clone(reader),
                        self.config,
                        self.error_presentation,
                        Arc::clone(&limiter),
                    ));
                }
            }
            builder
                .instructions(
                    self.registration
                        .instructions
                        .clone()
                        .unwrap_or_else(|| "ZeroStack engine MCP compatibility transport".into()),
                )
                .build()
        }

        /// Run the validated server on FastMCP's stdio transport.
        pub fn run_stdio(self) -> ! {
            self.build_server().run_stdio()
        }
    }
}

#[cfg(feature = "fastmcp")]
pub use fastmcp::{FastMcpTransport, FastMcpZeroCarrier};
