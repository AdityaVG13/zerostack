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

use crate::{SurfaceContractError, SurfaceKind, SurfaceRegistration};
#[cfg(feature = "fastmcp")]
use zero_abi::CanonicalResource;

/// Default maximum wall time for one compatibility tool call.
pub const DEFAULT_MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// Hard upper bound for one compatibility tool call.
pub const MAX_MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(300);
/// Default number of callbacks that may run at once.
pub const DEFAULT_MCP_MAX_INFLIGHT: usize = 16;
/// Hard upper bound for concurrent compatibility callbacks.
pub const MAX_MCP_MAX_INFLIGHT: usize = 256;
const CANCELLATION_POLL: Duration = Duration::from_millis(10);

/// Configuration shared by every dynamically registered tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McpTransportConfig {
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

impl McpTransportConfig {
    pub fn validate(self) -> Result<Self, McpTransportError> {
        if self.tool_timeout.is_zero() {
            return Err(McpTransportError::InvalidConfig(
                "tool_timeout must be greater than zero".into(),
            ));
        }
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
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl McpCallContext {
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.deadline
    }

    pub fn check(&self) -> Result<(), McpDispatchError> {
        if self.is_cancelled() {
            Err(McpDispatchError::cancelled())
        } else {
            Ok(())
        }
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
}

/// Engine-owned resource payload seam. The transport owns only bounded
/// execution and FastMCP serialization; URI meaning stays with the engine.
pub trait McpResourceReader: Send + Sync {
    fn read(&self, uri: &str, context: &McpCallContext) -> Result<Value, McpDispatchError>;
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
            Self::Surface(error) => write!(formatter, "invalid MCP surface registration: {error}"),
            Self::WrongSurface(surface) => write!(
                formatter,
                "MCP compatibility transport requires mcp surface, got {}",
                surface.as_str()
            ),
            Self::MissingResourceReader => {
                write!(formatter, "MCP resources require an engine-owned resource reader")
            },
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
    let permit = limiter.acquire(tool)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let context = McpCallContext {
        deadline: Instant::now() + config.tool_timeout,
        cancelled: Arc::clone(&cancelled),
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    let deadline = context.deadline;
    let tool_name = tool.to_owned();
    let _worker = thread::Builder::new()
        .name("zerostack-mcp-tool".into())
        .spawn(move || {
            let _permit = permit;
            let result = dispatcher.dispatch(&tool_name, arguments, &context);
            let _ = sender.send(result);
        })
        .map_err(|error| {
            McpDispatchError::new(
                "dispatch_failed",
                format!("failed to start MCP domain callback: {error}"),
                false,
            )
            .with_op(tool)
        })?;

    let started = Instant::now();
    loop {
        if externally_cancelled() {
            cancelled.store(true, Ordering::Release);
            return Err(McpDispatchError::cancelled().with_op(tool));
        }
        let elapsed = started.elapsed();
        if elapsed >= config.tool_timeout {
            cancelled.store(true, Ordering::Release);
            return Err(McpDispatchError::timeout(tool, config.tool_timeout));
        }
        let wait = (config.tool_timeout - elapsed).min(CANCELLATION_POLL);
        match receiver.recv_timeout(wait) {
            Ok(Err(error)) if error.kind == "cancelled" && Instant::now() >= deadline => {
                return Err(McpDispatchError::timeout(tool, config.tool_timeout));
            }
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::Release);
                return Err(McpDispatchError::disconnected(tool));
            }
        }
    }
}

#[cfg(feature = "fastmcp")]
mod fastmcp {
    use super::*;
    use fastmcp_rust::prelude::{
        Content, McpContext, McpError, Resource, ResourceContent, Server, Tool,
    };
    use fastmcp_rust::ResourceHandler;
    use fastmcp_rust::{ToolAnnotations, ToolHandler};
    use zero_abi::{CanonicalOperation, EffectClass};

    struct RegisteredTool {
        definition: Tool,
        dispatcher: Arc<dyn McpDispatcher>,
        config: McpTransportConfig,
        limiter: Arc<Inflight>,
    }

    struct ResourceDispatcher {
        reader: Arc<dyn McpResourceReader>,
        uri: String,
    }

    impl McpDispatcher for ResourceDispatcher {
        fn dispatch(
            &self,
            _tool: &str,
            _arguments: Value,
            context: &McpCallContext,
        ) -> Result<Value, McpDispatchError> {
            self.reader.read(&self.uri, context)
        }
    }

    pub(super) struct RegisteredResource {
        definition: Resource,
        reader: Arc<dyn McpResourceReader>,
        config: McpTransportConfig,
        limiter: Arc<Inflight>,
    }

    impl RegisteredResource {
        pub(super) fn new(
            resource: &CanonicalResource,
            reader: Arc<dyn McpResourceReader>,
            config: McpTransportConfig,
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
            let result = execute_call_with_limiter(
                Arc::new(ResourceDispatcher {
                    reader: Arc::clone(&self.reader),
                    uri: uri.clone(),
                }),
                &uri,
                Value::Null,
                self.config,
                Arc::clone(&self.limiter),
                || context.is_cancelled(),
            );
            context
                .checkpoint()
                .map_err(|_| McpError::request_cancelled())?;
            match result {
                Ok(value) => Ok(vec![ResourceContent {
                    uri,
                    mime_type: self.definition.mime_type.clone(),
                    text: Some(serde_json::to_string(&value).expect("JSON values are serializable")),
                    blob: None,
                }]),
                Err(error) => Err(McpError::tool_error(error.wire_text())),
            }
        }
    }

    impl RegisteredTool {
        fn new(
            operation: &CanonicalOperation,
            dispatcher: Arc<dyn McpDispatcher>,
            config: McpTransportConfig,
            limiter: Arc<Inflight>,
        ) -> Self {
            let read_only = operation.effect_policy.effect_class == EffectClass::ReadOnly;
            Self {
                definition: Tool {
                    name: operation
                        .mcp_tool_name
                        .clone()
                        .unwrap_or_else(|| operation.canonical_id.clone()),
                    description: Some(if operation.description.is_empty() {
                        format!("{} operation", operation.canonical_id)
                    } else {
                        operation.description.clone()
                    }),
                    input_schema: operation.args_schema.clone(),
                    output_schema: operation.output_schema.clone(),
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
            let result = execute_call_with_limiter(
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
                Ok(value) => Ok(vec![Content::text(
                    serde_json::to_string(&value).expect("JSON values are serializable"),
                )]),
                Err(error) => Err(McpError::tool_error(error.wire_text())),
            }
        }
    }

    /// A validated dynamic FastMCP server over one engine registration.
    pub struct FastMcpTransport {
        registration: SurfaceRegistration,
        dispatcher: Arc<dyn McpDispatcher>,
        resource_reader: Option<Arc<dyn McpResourceReader>>,
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
            Ok(Self {
                registration,
                dispatcher,
                resource_reader: None,
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
            Ok(Self {
                registration,
                dispatcher,
                resource_reader: Some(resource_reader),
                config,
            })
        }

        pub fn registration(&self) -> &SurfaceRegistration {
            &self.registration
        }

        /// Return the dynamic tool catalog derived from the canonical registry.
        pub fn catalog(&self) -> Vec<Tool> {
            let limiter = Arc::new(Inflight::new(self.config.max_inflight));
            self.registration
                .adapter
                .registry
                .operations
                .iter()
                .map(|operation| {
                    let canonical = RegisteredTool::new(
                        operation,
                        Arc::clone(&self.dispatcher),
                        self.config,
                        Arc::clone(&limiter),
                    )
                    .definition;
                    let aliases = operation
                        .aliases
                        .iter()
                        .map(|alias| Tool {
                            name: alias.clone(),
                            description: canonical.description.clone(),
                            input_schema: canonical.input_schema.clone(),
                            output_schema: canonical.output_schema.clone(),
                            icon: None,
                            version: None,
                            tags: Vec::new(),
                            annotations: canonical.annotations.clone(),
                        })
                        .collect::<Vec<_>>();
                    std::iter::once(canonical)
                        .chain(aliases)
                        .collect::<Vec<_>>()
                })
                .flatten()
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
            let mut builder =
                Server::new(self.registration.root.clone(), env!("CARGO_PKG_VERSION"))
                    .request_timeout(0);
            for operation in &self.registration.adapter.registry.operations {
                builder = builder.tool(RegisteredTool::new(
                    operation,
                    Arc::clone(&self.dispatcher),
                    self.config,
                    Arc::clone(&limiter),
                ));
                for alias in &operation.aliases {
                    let mut alias_operation = operation.clone();
                    alias_operation.canonical_id = alias.clone();
                    alias_operation.mcp_tool_name = Some(alias.clone());
                    alias_operation.aliases.clear();
                    builder = builder.tool(RegisteredTool::new(
                        &alias_operation,
                        Arc::clone(&self.dispatcher),
                        self.config,
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
                        Arc::clone(&limiter),
                    ));
                }
            }
            builder
                .instructions(self.registration.instructions.clone().unwrap_or_else(|| {
                    "ZeroStack engine MCP compatibility transport".into()
                }))
                .build()
        }

        /// Run the validated server on FastMCP's stdio transport.
        pub fn run_stdio(self) -> ! {
            self.build_server().run_stdio()
        }
    }
}

#[cfg(feature = "fastmcp")]
pub use fastmcp::FastMcpTransport;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_registration_requires_the_mcp_surface() {
        let registration = test_registration(SurfaceKind::CodeMode);
        assert!(matches!(
            validate_mcp_registration(&registration),
            Err(McpTransportError::WrongSurface(SurfaceKind::CodeMode))
        ));
        assert!(validate_mcp_registration(&test_registration(SurfaceKind::Mcp)).is_ok());
    }

    #[test]
    fn structured_success_and_error_text_are_lossless() {
        let value = json!({"ack":"ok", "content":{"kind":"inline", "value":{"n":1}}});
        let callback_value = value.clone();
        let success = execute_call(
            Arc::new(move |_: &str, _: Value, _: &McpCallContext| Ok(callback_value.clone())),
            "fs.read",
            json!({}),
            McpTransportConfig::default(),
        )
        .unwrap();
        assert_eq!(success, value);

        let error = McpDispatchError::new("denied", "approval required", false)
            .with_op("fs.read")
            .with_data(json!({"approval_id":"a1"}));
        let round_trip: McpDispatchError = serde_json::from_str(&error.wire_text()).unwrap();
        assert_eq!(round_trip, error);
    }

    #[test]
    fn callback_timeout_sets_cancellation_and_returns_bounded_error() {
        let observed = Arc::new(AtomicBool::new(false));
        let callback_observed = Arc::clone(&observed);
        let result = execute_call(
            Arc::new(move |_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                callback_observed.store(true, Ordering::Release);
                Err(McpDispatchError::cancelled())
            }),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "timeout");
        for _ in 0..50 {
            if observed.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("callback did not observe timeout cancellation");
    }

    #[test]
    fn external_cancellation_is_reported_without_waiting_for_timeout() {
        let started = Instant::now();
        let result = execute_call_with_cancel(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(McpDispatchError::cancelled())
            }),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: Duration::from_secs(1),
                max_inflight: 1,
            },
            move || started.elapsed() >= Duration::from_millis(25),
        );
        assert_eq!(result.unwrap_err().kind, "cancelled");
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[cfg(feature = "fastmcp")]
    #[test]
    fn fastmcp_catalog_and_tools_call_preserve_structured_results() {
        use fastmcp_rust::{
            CallToolParams, CallToolResult, ClientCapabilities, ClientInfo, Content, Cx,
            JsonRpcRequest, NotificationSender, PendingRequests, RequestSender, ServerCapabilities,
            ServerInfo, Session,
        };

        let transport = FastMcpTransport::new(
            test_registration(SurfaceKind::Mcp),
            {
                let invoked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
                let seen = Arc::clone(&invoked);
                Arc::new(move |tool: &str, arguments: Value, _: &McpCallContext| {
                    seen.lock().unwrap().push(tool.to_owned());
                    if arguments.get("fail").and_then(Value::as_bool) == Some(true) {
                        Err(McpDispatchError::new("denied", "approval required", false)
                            .with_op("fs.read")
                            .with_data(json!({"approval_id":"a1"})))
                    } else {
                        Ok(json!({"value": arguments.get("value").cloned()}))
                    }
                })
            },
            McpTransportConfig::default(),
        )
        .unwrap();
        let catalog = transport.catalog();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].name, "read_value");
        assert_eq!(catalog[1].name, "read");
        assert_eq!(catalog[0].description.as_deref(), Some("Read a value"));
        assert_eq!(catalog[0].input_schema, json!({"type":"object"}));
        assert_eq!(catalog[0].output_schema, Some(json!({
            "type": "object",
            "properties": {"value": {"type": "integer"}}
        })));
        assert_eq!(
            catalog[0].annotations.as_ref().unwrap().read_only,
            Some(true)
        );

        let server = transport.build_server();
        let mut session = Session::new(
            ServerInfo {
                name: "zerostack".into(),
                version: "test".into(),
            },
            ServerCapabilities::default(),
        );
        session.initialize(
            ClientInfo {
                name: "test-client".into(),
                version: "test".into(),
            },
            ClientCapabilities::default(),
            "2025-06-18".into(),
        );
        let notification_sender: NotificationSender = Arc::new(|_| {});
        let request_sender =
            RequestSender::new(Arc::new(PendingRequests::new()), Arc::new(|_| Ok(())));
        let mut call = |name: &str, arguments: Value| {
            let request = JsonRpcRequest::new(
                "tools/call",
                Some(
                    serde_json::to_value(CallToolParams {
                        name: name.into(),
                        arguments: Some(arguments),
                        meta: None,
                    })
                    .unwrap(),
                ),
                1,
            );
            server
                .dispatch_request(
                    &Cx::for_testing(),
                    &mut session,
                    request,
                    &notification_sender,
                    &request_sender,
                )
                .unwrap()
        };

        let success: CallToolResult = serde_json::from_value(
            call("read_value", json!({"value":7})).result.unwrap(),
        )
        .unwrap();
        assert!(!success.is_error);
        assert_eq!(success.content.len(), 1);
        let Content::Text { text } = &success.content[0] else {
            panic!("FastMCP structured success must use text content");
        };
        assert_eq!(text, r#"{"value":7}"#);

        let alias_success: CallToolResult =
            serde_json::from_value(call("read", json!({"value":8})).result.unwrap()).unwrap();
        assert!(!alias_success.is_error);
        let failure: CallToolResult =
            serde_json::from_value(call("read", json!({"fail":true})).result.unwrap()).unwrap();
        assert!(failure.is_error);
        let Content::Text { text } = &failure.content[0] else {
            panic!("FastMCP structured error must use text content");
        };
        let error: McpDispatchError = serde_json::from_str(text).unwrap();
        assert_eq!(error.kind, "denied");
        assert_eq!(error.data, Some(json!({"approval_id":"a1"})));
    }

    #[test]
    fn invalid_configuration_fails_before_starting_callback() {
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&callback_calls);
        let result = execute_call(
            Arc::new(move |_: &str, _: Value, _: &McpCallContext| {
                callback_count.fetch_add(1, Ordering::Relaxed);
                Ok(json!(null))
            }),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: Duration::ZERO,
                max_inflight: 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");
        assert_eq!(callback_calls.load(Ordering::Relaxed), 0);

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: DEFAULT_MCP_TOOL_TIMEOUT,
                max_inflight: 0,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: DEFAULT_MCP_TOOL_TIMEOUT,
                max_inflight: MAX_MCP_MAX_INFLIGHT + 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");

        let result = execute_call(
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            "fs.read",
            json!({}),
            McpTransportConfig {
                tool_timeout: MAX_MCP_TOOL_TIMEOUT + Duration::from_secs(1),
                max_inflight: 1,
            },
        );
        assert_eq!(result.unwrap_err().kind, "invalid_config");
    }

    #[cfg(feature = "fastmcp")]
    #[test]
    fn fastmcp_resource_callback_failure_and_timeout_are_bounded() {
        use fastmcp_rust::ResourceHandler;
        use zero_abi::CanonicalResource;

        let resource = CanonicalResource {
            uri: "resource://fixture".into(),
            name: "Fixture".into(),
            description: "fixture resource".into(),
            mime_type: Some("application/json".into()),
        };
        let mut registration = test_registration(SurfaceKind::Mcp);
        registration.adapter.registry.resources = vec![resource.clone()];
        let reader: Arc<dyn McpResourceReader> = Arc::new(
            |uri: &str, context: &McpCallContext| {
                if uri == "resource://failure" {
                    return Err(McpDispatchError::new("resource_failed", "read failed", false));
                }
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(McpDispatchError::cancelled())
            },
        );
        let missing_reader = FastMcpTransport::new(
            registration.clone(),
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            McpTransportConfig::default(),
        );
        assert!(matches!(
            missing_reader,
            Err(McpTransportError::MissingResourceReader)
        ));
        let transport = FastMcpTransport::with_resources(
            registration,
            Arc::new(|_: &str, _: Value, _: &McpCallContext| Ok(json!(null))),
            reader,
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
        )
        .unwrap();
        assert_eq!(transport.resource_catalog()[0].uri, resource.uri);
        let success_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, _: &McpCallContext| Ok(json!({"answer": 42}))),
            McpTransportConfig::default(),
            Arc::new(Inflight::new(1)),
        );
        let success = success_handler
            .read(&fastmcp_rust::McpContext::new(fastmcp_rust::Cx::for_testing(), 1))
            .unwrap();
        assert_eq!(success.len(), 1);
        assert_eq!(success[0].uri, resource.uri);
        assert_eq!(success[0].mime_type, resource.mime_type);
        assert_eq!(success[0].text.as_deref(), Some(r#"{"answer":42}"#));
        let handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, _: &McpCallContext| {
                Err(McpDispatchError::new("resource_failed", "read failed", false))
            }),
            McpTransportConfig::default(),
            Arc::new(Inflight::new(1)),
        );
        let error = handler
            .read(&fastmcp_rust::McpContext::new(fastmcp_rust::Cx::for_testing(), 1))
            .unwrap_err();
        assert!(error.message.contains("resource_failed"));

        let timeout_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, context: &McpCallContext| {
                while !context.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(McpDispatchError::cancelled())
            }),
            McpTransportConfig {
                tool_timeout: Duration::from_millis(25),
                max_inflight: 1,
            },
            Arc::new(Inflight::new(1)),
        );
        let started = Instant::now();
        let error = timeout_handler
            .read(&fastmcp_rust::McpContext::new(fastmcp_rust::Cx::for_testing(), 1))
            .unwrap_err();
        assert!(error.message.contains("timeout") || error.message.contains("cancel"));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    fn test_registration(surface: SurfaceKind) -> SurfaceRegistration {
        use crate::{CapabilityDescriptor, DomainAdapterRegistration};
        use zero_abi::{
            ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
            EffectClass, EffectPolicy, EngineIdentity, PermitRequirement, RefOwnership,
            RegistryEngine, TelemetrySchema,
        };

        SurfaceRegistration::new(
            surface,
            "zero",
            DomainAdapterRegistration {
                engine: EngineIdentity::FsZero,
                registry: CanonicalRegistry {
                    version: zero_abi::CANONICAL_DISPATCH_VERSION.into(),
                    engine: RegistryEngine::FsZero,
                    operations: vec![CanonicalOperation {
                        canonical_id: "fs.read".into(),
                        description: "Read a value".into(),
                        aliases: vec!["read".into()],
                        args_schema: json!({"type":"object"}),
                        output_schema: Some(json!({
                            "type": "object",
                            "properties": {"value": {"type": "integer"}}
                        })),
                        mcp_tool_name: Some("read_value".into()),
                        effect_policy: EffectPolicy {
                            effect_class: EffectClass::ReadOnly,
                            permit: PermitRequirement::NotRequired,
                            approval: ApprovalRequirement::NotRequired,
                        },
                        errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
                    }],
                    resources: vec![],
                },
                ref_ownership: RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: "session".into(),
                    refs: vec!["fz://ref".into()],
                    snapshot: None,
                },
                telemetry_schema: TelemetrySchema::V1,
                capabilities: vec![CapabilityDescriptor::new("fs", "read")],
            },
        )
    }
}
