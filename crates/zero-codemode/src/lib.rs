#![forbid(unsafe_code)]

pub mod discovery;
mod edit_protocol;
mod host;
mod limits;
pub mod manifest;
pub mod node;
pub mod session;
pub mod worker;
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
    GlobalRegistration, Host, HostError, RegistrationError, CANONICAL_REF_ALIASES,
    CANONICAL_RESULT_FIELDS, CANONICAL_TEXT_ALIASES, RESULT_SPILL_PREVIEW_BYTES,
    RESULT_SPILL_SCHEMA,
};
pub use limits::{HostLimits, LimitError};
pub use manifest::{
    artifact_candidates, is_ephemeral, is_readable_file, locate_manifest, manifest_order,
    resolve_artifact, ArtifactEnv, ArtifactOutcome, HarnessArtifact, ManifestFacts, Refusal,
    StorePaths, EPHEMERAL_MARKERS, EPHEMERAL_REASON, HARNESS_ARTIFACTS, JOURNAL_DIR, LIB_DIR,
    MANIFEST_SCHEMA, NODE_ENV, RUNTIME_MODULE_ENV, SUBSTRATE_MODULE_ENV,
};
pub use node::{
    node_candidates, node_file_name, node_report, resolve_node, resolve_node_with, NodeCandidate,
    NodeEnv, NodeError, NodeOutcome, NodeRefusal, NodeSource, FNM_DEFAULT_ALIAS_SUBDIR,
    FNM_DIR_ENV, NODE_ORDER, NODE_SCHEMA,
};
pub use wrap::{validate_plan, wrap_plan, PlanError};
