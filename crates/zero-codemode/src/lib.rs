#![forbid(unsafe_code)]

mod interpreter;

pub mod discovery;
mod edit_protocol;
mod host;
mod limits;
pub mod manifest;
pub mod node;
pub mod preflight;
pub mod session;
pub mod worker;
mod wrap;

pub use discovery::{
    BIN_DIR, Candidate, DATA_SUBDIR, DEV_ROOT_ENV, DEV_TARGET_SUBDIR, DISCOVERY_SCHEMA,
    DiscoveryEnv, DiscoveryError, HARNESS_BINARIES, HOME_ENV, HarnessBinary, Resolved, Source,
    candidates, is_executable_file, locate_report, resolve, resolve_all, resolve_with,
};
pub use edit_protocol::{
    EDIT_PROTOCOL_VERSION, EditError, EditErrorClass, EditOp, EditPlan, RefKind, Side, classify_ref,
};
pub use host::{
    Connector, ConnectorCompletion, ConnectorError, DEFAULT_MAX_VISIBLE_RESULT_BYTES,
    DispatchContext, Host, HostError, MAX_INFLIGHT_CONNECTOR_CALLS,
    MAX_RESULT_SPILL_ENVELOPE_BYTES, MAX_VISIBLE_ERROR_BYTES, PUBLIC_RESULT_FIELDS,
    RESULT_SPILL_PREVIEW_BYTES, RESULT_SPILL_SCHEMA, finalize_visible_error,
    runtime_creation_count,
};
pub use limits::{HostLimits, LimitError};
pub use manifest::{
    AGGREGATE_HOST_PROTOCOL, ArtifactEnv, ArtifactOutcome, EPHEMERAL_MARKERS, EPHEMERAL_REASON,
    HARNESS_ARTIFACTS, HarnessArtifact, JOURNAL_DIR, LIB_DIR, MANIFEST_SCHEMA, ManifestFacts,
    NODE_ENV, RUNTIME_MODULE_ENV, Refusal, SUBSTRATE_MODULE_ENV, StorePaths, artifact_candidates,
    is_ephemeral, is_readable_file, locate_from_process, locate_manifest, manifest_order,
    render_manifest_human, resolve_artifact, store_paths_from_process,
};
pub use node::{
    FNM_DEFAULT_ALIAS_SUBDIR, FNM_DIR_ENV, NODE_ORDER, NODE_SCHEMA, NodeCandidate, NodeEnv,
    NodeError, NodeOutcome, NodeRefusal, NodeSource, node_candidates, node_file_name, node_report,
    resolve_node, resolve_node_with,
};
pub use preflight::{
    DOCTOR_SCHEMA, DoctorCheck, DoctorReport, doctor_report, locate_complete, locate_missing,
    render_doctor_human,
};
pub use wrap::{PlanError, validate_plan, wrap_plan};
pub use zero_abi::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
