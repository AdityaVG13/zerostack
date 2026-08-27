#![forbid(unsafe_code)]

//! Restricted interpreter used by ZeroKernel.
//!
//! This crate owns JavaScript evaluation, finite limits, promise scheduling,
//! and the direct host-call seam. It exposes no engine namespace, command catalog, transport adapter,
//! or compatibility runtime.

pub mod guest;
mod host;
mod interpreter;
mod limits;
mod wrap;

pub use guest::{GuestContext, GuestSurface};
pub use host::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, ExecutionMetrics,
    ExecutionOutcome, Host, HostError, MAX_INFLIGHT_CONNECTOR_CALLS, runtime_creation_count,
};
pub use limits::{HostLimits, LimitError};
pub use wrap::{PlanError, validate_plan};
pub use zero_abi::{CapabilityDescriptor, GlobalRegistration, RegistrationError};
