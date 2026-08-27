//! Bounded interpreter host used by ZeroKernel.

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value as JsonValue;
use zero_abi::{CapabilityDescriptor, GlobalRegistration, RegistrationError, ZeroOperationTrace};

use crate::{HostLimits, LimitError, PlanError};

pub fn runtime_creation_count() -> u64 {
    crate::interpreter::interpreter_creation_count()
}

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

pub const MAX_INFLIGHT_CONNECTOR_CALLS: usize = 64;

pub(crate) struct ConnectorCompletionMessage {
    pub(crate) sequence: u64,
    pub(crate) result: Result<String, ConnectorError>,
}

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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConnectorError {}

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

#[derive(Debug)]
pub struct ExecutionOutcome {
    pub result: Result<JsonValue, HostError>,
    pub metrics: ExecutionMetrics,
    pub operations: Vec<ZeroOperationTrace>,
    pub operations_truncated: bool,
}

#[derive(Clone, Debug)]
pub struct Host {
    pub(crate) limits: HostLimits,
    pub(crate) registration: GlobalRegistration,
    pub(crate) guest: Option<Arc<crate::guest::GuestSurface>>,
}

impl Host {
    pub fn new_zero_kernel(
        limits: HostLimits,
        registration: GlobalRegistration,
    ) -> Result<Self, HostError> {
        limits.validate().map_err(HostError::Limits)?;
        if registration.root != "z"
            || registration
                .capabilities
                .iter()
                .any(|capability| capability.surface != "z")
        {
            return Err(HostError::Data(
                "ZeroKernel accepts only direct z methods".into(),
            ));
        }
        registration.validate().map_err(HostError::Registration)?;
        Ok(Self {
            limits,
            registration,
            guest: None,
        })
    }

    pub fn with_guest_surface(mut self, guest: Arc<crate::guest::GuestSurface>) -> Self {
        self.guest = Some(guest);
        self
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

    pub fn execute_measured(&self, plan: &str, connector: Rc<dyn Connector>) -> ExecutionOutcome {
        self.execute_measured_with_cancel_timeout(
            plan,
            connector,
            Arc::new(AtomicBool::new(false)),
            self.limits.wall_timeout,
        )
    }

    pub fn execute_measured_with_cancel_timeout(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> ExecutionOutcome {
        crate::interpreter::execute_measured(
            self,
            plan,
            connector,
            cancelled,
            timeout.min(self.limits.wall_timeout),
        )
    }

    pub fn execute_with_cancel(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<JsonValue, HostError> {
        self.execute_with_cancel_timeout(plan, connector, cancelled, self.limits.wall_timeout)
    }

    pub fn execute_with_cancel_timeout(
        &self,
        plan: &str,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<JsonValue, HostError> {
        crate::interpreter::execute(
            self,
            plan,
            connector,
            cancelled,
            timeout.min(self.limits.wall_timeout),
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
    Runtime(String),
    MethodNotFound(String),
    SurfaceNotFound(String),
    Connector(String),
    Json(String),
    ResultTooLarge { actual: usize, maximum: usize },
    MemoryLimit { requested: usize, maximum: usize },
    MicrotaskLimit,
    CallBudgetExceeded { made: u64, maximum: u64 },
    DeadlineExceeded,
    FuelExhausted,
    Cancelled,
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limits(error) => write!(formatter, "invalid limits: {error}"),
            Self::Registration(error) => write!(formatter, "invalid registration: {error}"),
            Self::Plan(error) => write!(formatter, "invalid plan: {error}"),
            Self::Parse(message) => write!(formatter, "parse error: {message}"),
            Self::UnsupportedSyntax(message) => write!(formatter, "unsupported syntax: {message}"),
            Self::Data(message) => write!(formatter, "data error: {message}"),
            Self::Execution(message) => write!(formatter, "execution error: {message}"),
            Self::Runtime(message) => write!(formatter, "runtime error: {message}"),
            Self::MethodNotFound(message) | Self::SurfaceNotFound(message) => {
                write!(formatter, "JavaScript exception: {message}")
            }
            Self::Connector(message) => write!(formatter, "connector error: {message}"),
            Self::Json(message) => write!(formatter, "JSON error: {message}"),
            Self::ResultTooLarge { actual, maximum } => {
                write!(formatter, "result is {actual} bytes; maximum is {maximum}")
            }
            Self::MemoryLimit { requested, maximum } => write!(
                formatter,
                "memory budget exceeded: requested {requested} bytes; maximum is {maximum}"
            ),
            Self::MicrotaskLimit => formatter.write_str("microtask ceiling exceeded"),
            Self::CallBudgetExceeded { made, maximum } => write!(
                formatter,
                "host-call budget exceeded: made {made} calls; maximum is {maximum}"
            ),
            Self::DeadlineExceeded => formatter.write_str("wall-clock deadline exceeded"),
            Self::FuelExhausted => formatter.write_str("instruction budget exhausted"),
            Self::Cancelled => formatter.write_str("execution cancelled"),
        }
    }
}

impl std::error::Error for HostError {}
