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
use zero_abi::{SurfaceContractError, SurfaceKind, SurfaceRegistration};

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

fn execute_bounded<T, D, F>(
    operation: &str,
    config: McpTransportConfig,
    limiter: Arc<Inflight>,
    dispatch: D,
    externally_cancelled: F,
) -> Result<T, McpDispatchError>
where
    T: Send + 'static,
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
    let _ = worker.join();
    outcome
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
        assert!(matches!(
            McpServerIdentity::new("", "1.0.0"),
            Err(McpTransportError::InvalidServerIdentity(_))
        ));
        assert!(matches!(
            McpServerIdentity::new("tokenzero", " 1.4.0"),
            Err(McpTransportError::InvalidServerIdentity(_))
        ));
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
            McpTransportConfig::default(),
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

        struct TestDispatcher;
        impl McpDispatcher for TestDispatcher {
            fn dispatch(
                &self,
                _tool: &str,
                arguments: Value,
                _: &McpCallContext,
            ) -> Result<Value, McpDispatchError> {
                if arguments.get("fail").and_then(Value::as_bool) == Some(true) {
                    Err(McpDispatchError::new("denied", "approval required", false)
                        .with_op("fs.read")
                        .with_data(json!({"approval_id":"a1"})))
                } else {
                    Ok(json!({"value": arguments.get("value").cloned()}))
                }
            }

            fn dispatch_output(
                &self,
                tool: &str,
                arguments: Value,
                context: &McpCallContext,
            ) -> Result<McpDispatchOutput, McpDispatchError> {
                if arguments.get("lossless").and_then(Value::as_bool) == Some(true) {
                    return Ok(McpDispatchOutput::Text(vec![
                        McpTextContent::new("ack:exact"),
                        McpTextContent::new("metadata:compact"),
                    ]));
                }
                self.dispatch(tool, arguments, context)
                    .map(McpDispatchOutput::Json)
            }
        }

        let plain_error = super::fastmcp::present_dispatch_error(
            McpDispatchError::new("denied", "approval required", false),
            McpErrorPresentation::PlainMessage,
        );
        assert_eq!(i32::from(plain_error.code), -32_000);
        assert_eq!(plain_error.message, "approval required");
        let structured_error = super::fastmcp::present_dispatch_error(
            McpDispatchError::new("denied", "approval required", false),
            McpErrorPresentation::Structured,
        );
        let structured: McpDispatchError = serde_json::from_str(&structured_error.message).unwrap();
        assert_eq!(structured.kind, "denied");

        let transport = FastMcpTransport::new(
            test_registration(SurfaceKind::Mcp),
            Arc::new(TestDispatcher),
            McpTransportConfig::default(),
        )
        .unwrap()
        .with_server_identity("tokenzero", "1.4.0")
        .unwrap()
        .with_error_presentation(McpErrorPresentation::PlainMessage)
        .with_alias_metadata(vec![McpAliasMetadata {
            canonical_id: "fs.read".into(),
            name: "read".into(),
            description: Some("Alias summary".into()),
            input_schema: json!({"type":"object","additionalProperties":true}),
            output_schema: None,
        }])
        .unwrap();
        let catalog = transport.catalog();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].name, "read_value");
        assert_eq!(catalog[1].name, "read");
        assert_eq!(catalog[0].description.as_deref(), Some("Read a value"));
        assert_eq!(catalog[0].input_schema, json!({"type":"object"}));
        assert_eq!(
            catalog[0].output_schema,
            Some(json!({
                "type": "object",
                "properties": {"value": {"type": "integer"}}
            }))
        );
        assert_eq!(catalog[1].description.as_deref(), Some("Alias summary"));
        assert_eq!(
            catalog[1].input_schema,
            json!({"type":"object","additionalProperties":true})
        );
        assert_eq!(catalog[1].output_schema, None);
        assert_eq!(
            catalog[0].annotations.as_ref().unwrap().read_only,
            Some(true)
        );

        let server = transport.build_server();
        assert_eq!(server.info().name, "tokenzero");
        assert_eq!(server.info().version, "1.4.0");
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

        let success: CallToolResult =
            serde_json::from_value(call("read_value", json!({"value":7})).result.unwrap()).unwrap();
        assert!(!success.is_error);
        assert_eq!(success.content.len(), 1);
        let Content::Text { text } = &success.content[0] else {
            panic!("FastMCP structured success must use text content");
        };
        assert_eq!(text, r#"{"value":7}"#);

        let alias_success: CallToolResult =
            serde_json::from_value(call("read", json!({"value":8})).result.unwrap()).unwrap();
        assert!(!alias_success.is_error);
        let lossless: CallToolResult =
            serde_json::from_value(call("read", json!({"lossless":true})).result.unwrap()).unwrap();
        assert_eq!(lossless.content.len(), 2);
        let Content::Text { text: primary } = &lossless.content[0] else {
            panic!("primary lossless content must remain text");
        };
        let Content::Text { text: metadata } = &lossless.content[1] else {
            panic!("secondary lossless content must remain text");
        };
        assert_eq!(primary, "ack:exact");
        assert_eq!(metadata, "metadata:compact");
        let failure: CallToolResult =
            serde_json::from_value(call("read", json!({"fail":true})).result.unwrap()).unwrap();
        assert!(failure.is_error);
        let Content::Text { text } = &failure.content[0] else {
            panic!("FastMCP plain error must use text content");
        };
        assert_eq!(text, "approval required");
    }

    #[test]
    fn configuration_preserves_engine_deadlines_and_rejects_invalid_bounds() {
        let result = execute_call(
            Arc::new(|_: &str, _: Value, context: &McpCallContext| {
                Ok(json!({"hub_deadline": context.deadline().is_some()}))
            }),
            "token.shell",
            json!({}),
            McpTransportConfig::default(),
        )
        .unwrap();
        assert_eq!(result, json!({"hub_deadline":false}));
        assert!(
            McpTransportConfig {
                tool_timeout: Duration::from_secs(3_600),
                max_inflight: 1,
            }
            .validate()
            .is_ok()
        );

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
        let reader: Arc<dyn McpResourceReader> = Arc::new(|uri: &str, context: &McpCallContext| {
            if uri == "resource://failure" {
                return Err(McpDispatchError::new(
                    "resource_failed",
                    "read failed",
                    false,
                ));
            }
            while !context.is_cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
            Err(McpDispatchError::cancelled())
        });
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
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let success = success_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap();
        assert_eq!(success.len(), 1);
        assert_eq!(success[0].uri, resource.uri);
        assert_eq!(success[0].mime_type, resource.mime_type);
        assert_eq!(success[0].text.as_deref(), Some(r#"{"answer":42}"#));

        struct ExactResourceReader;
        impl McpResourceReader for ExactResourceReader {
            fn read(&self, _: &str, _: &McpCallContext) -> Result<Value, McpDispatchError> {
                Ok(Value::Null)
            }

            fn read_output(
                &self,
                _: &str,
                _: &McpCallContext,
            ) -> Result<McpResourceOutput, McpDispatchError> {
                Ok(McpResourceOutput::Text("exact resource text".into()))
            }
        }
        let exact_handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(ExactResourceReader),
            McpTransportConfig::default(),
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let exact = exact_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap();
        assert_eq!(exact[0].text.as_deref(), Some("exact resource text"));

        let handler = super::fastmcp::RegisteredResource::new(
            &resource,
            Arc::new(|_: &str, _: &McpCallContext| {
                Err(McpDispatchError::new(
                    "resource_failed",
                    "read failed",
                    false,
                ))
            }),
            McpTransportConfig::default(),
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let error = handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
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
            McpErrorPresentation::Structured,
            Arc::new(Inflight::new(1)),
        );
        let started = Instant::now();
        let error = timeout_handler
            .read(&fastmcp_rust::McpContext::new(
                fastmcp_rust::Cx::for_testing(),
                1,
            ))
            .unwrap_err();
        assert!(error.message.contains("timeout") || error.message.contains("cancel"));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    fn test_registration(surface: SurfaceKind) -> SurfaceRegistration {
        use zero_abi::{
            ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
            EffectClass, EffectPolicy, EngineIdentity, PermitRequirement, RefOwnership,
            RegistryEngine, TelemetrySchema,
        };
        use zero_abi::{CapabilityDescriptor, DomainAdapterRegistration};

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
