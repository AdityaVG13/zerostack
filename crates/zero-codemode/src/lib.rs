#![forbid(unsafe_code)]

mod edit_protocol;
mod host;
mod limits;
mod wrap;

pub use edit_protocol::{
    classify_ref, EditError, EditErrorClass, EditOp, EditPlan, RefKind, Side, EDIT_PROTOCOL_VERSION,
};
pub use host::{
    runtime_creation_count, CapabilityDescriptor, Connector, ConnectorError, DispatchContext,
    GlobalRegistration, Host, HostError, RegistrationError,
};
pub use limits::{HostLimits, LimitError};
pub use wrap::{validate_plan, wrap_plan, PlanError};
