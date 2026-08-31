//! Hub-owned MCP carrier for the single ZeroKernel `zero` tool. The
//! carrier owns bounded callback execution, cancellation hooks, and stdio
//! lifecycle. Domain engines remain behind the in-process ZeroKernel dispatcher.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use zero_abi::{ZeroKernelResponse, canonical_json, zero_kernel_response_schema};

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
        "description": "Run one bounded JavaScript or TypeScript cell with the complete ZeroKernel surface: z.read, z.find, z.edit, z.apply, z.run, and z.state. Keep dependent work in one cell and use Promise.all for independent calls. z.find is workspace-root confined and has no files mode; use z.read for directory listings and exact bytes. No other z.* methods exist.",
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
        },
        "outputSchema": zero_kernel_response_schema()
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

/// Configuration shared by ZeroKernel carrier calls.
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

/// ZeroKernel MCP initialize identity.
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

/// Engine callback output. `Json` serializes one value; `Text` preserves exact,
/// ordered MCP content entries.
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
    InvalidServerIdentity(String),
    InvalidCarrier(String),
}

impl fmt::Display for McpTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => {
                write!(formatter, "invalid MCP transport config: {message}")
            }
            Self::InvalidServerIdentity(message) => {
                write!(formatter, "invalid MCP server identity: {message}")
            }
            Self::InvalidCarrier(message) => {
                write!(formatter, "invalid ZeroKernel carrier: {message}")
            }
        }
    }
}

impl std::error::Error for McpTransportError {}

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
    use fastmcp_rust::prelude::{Content, McpContext, McpError, Server, Tool};
    use fastmcp_rust::{ToolAnnotations, ToolHandler};

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
                    output_schema: Some(entry["outputSchema"].clone()),
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
                Err(error) => Err(present_dispatch_error(error)),
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
            .instructions("ZeroKernel exposes one zero tool whose cell has exactly z.read, z.find, z.edit, z.apply, z.run, and z.state.")
            .build()
        }

        pub fn run_stdio(self) -> ! {
            self.build_server().run_stdio()
        }
    }

    pub(super) fn present_dispatch_error(error: McpDispatchError) -> McpError {
        McpError::tool_error(error.wire_text())
    }
}

#[cfg(feature = "fastmcp")]
pub use fastmcp::FastMcpZeroCarrier;
