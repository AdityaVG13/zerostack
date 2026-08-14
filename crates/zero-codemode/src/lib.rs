#![forbid(unsafe_code)]

mod cancellation;
mod decision_gate;
mod interpreter;

mod edit_protocol;
mod host;
mod limits;
pub mod worker;
mod wrap;
pub use cancellation::CancellationSignal;

pub use decision_gate::{DECISION_REQUIRE_METHOD, DECISION_SURFACE, DecisionGate, GateResolutionV1};

pub use edit_protocol::{
    EDIT_PROTOCOL_VERSION, EditError, EditErrorClass, EditOp, EditPlan, RefKind, Side, classify_ref,
};
pub use host::{
    Connector, ConnectorCompletion, ConnectorError, DEFAULT_MAX_VISIBLE_RESULT_BYTES,
    DispatchContext, ExecutionMetrics, ExecutionOutcome, Host, HostError,
    MAX_INFLIGHT_CONNECTOR_CALLS, MAX_RESULT_SPILL_ENVELOPE_BYTES, MAX_VISIBLE_ERROR_BYTES,
    PUBLIC_RESULT_FIELDS, RESULT_SPILL_PREVIEW_BYTES, RESULT_SPILL_SCHEMA, finalize_visible_error,
    runtime_creation_count,
};
pub use limits::{HostLimits, LimitError};
pub use wrap::{PlanError, validate_plan, wrap_plan};
pub use zero_abi::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
