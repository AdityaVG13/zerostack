#![forbid(unsafe_code)]

pub mod discovery;
mod edit_protocol;
mod host;
mod limits;
mod wrap;

pub use discovery::{
    candidates, is_executable_file, locate_report, resolve, resolve_all, resolve_with, Candidate,
    DiscoveryEnv, DiscoveryError, HarnessBinary, Resolved, Source, BIN_DIR, DATA_SUBDIR,
    DEV_ROOT_ENV, DEV_TARGET_SUBDIR, DISCOVERY_SCHEMA, HARNESS_BINARIES, HOME_ENV,
};
pub use edit_protocol::{
    classify_ref, EditError, EditErrorClass, EditOp, EditPlan, RefKind, Side, EDIT_PROTOCOL_VERSION,
};
pub use host::{
    runtime_creation_count, CapabilityDescriptor, Connector, ConnectorError, DispatchContext,
    GlobalRegistration, Host, HostError, RegistrationError, RESULT_SPILL_PREVIEW_BYTES,
    RESULT_SPILL_SCHEMA,
};
pub use limits::{HostLimits, LimitError};
pub use wrap::{validate_plan, wrap_plan, PlanError};
