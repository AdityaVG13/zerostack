#![forbid(unsafe_code)]
//!
//! Canonical generic CodeMode host authority. The receive audit on
//! 2026-08-17 classified `FSZero/crates/fs-zero/src/codemode/{js,host,limits}`
//! as host concerns already implemented here by [`Host`], the restricted
//! interpreter, and the shared limits table. FSZero's connector, runtime,
//! transaction, recipes, world parsing, and `fz://` result adaptation remain
//! engine/store concerns. Its MCP transport loop is already owned by
//! `zero-mcp`; only FSZero store methods remain engine-local.
//!
//! `GraphZero/crates/graphzero-engine/src/codemode` was audited on the same
//! date: generic execution, limits, scheduling, and response bounding map to
//! this host; query steps, snapshot state, `gz://query` envelopes, and index
//! evidence remain GraphZero domain authority.

mod cancellation;
mod decision_gate;
mod interpreter;

mod edit_protocol;
mod host;
mod limits;
pub mod worker;
mod wrap;
pub use cancellation::CancellationSignal;

pub use decision_gate::{
    DECISION_REQUIRE_METHOD, DECISION_SURFACE, DecisionGate, GateResolution, GateRuleUsage,
    GateUsageReport,
};

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
pub use limits::{
    CODEMODE_WALL_MS_ENVS, HostLimits, LimitError, MAX_WALL_MS, OUTPUT_WALL_ARRANGEMENTS,
    OutputWallArrangement, effective_max_wall_ms,
};
pub use wrap::{PlanError, validate_plan, wrap_plan};
pub use zero_abi::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
