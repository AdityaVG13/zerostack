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
    BIN_DIR, Candidate, DATA_SUBDIR, DEV_ROOT_ENV, DEV_TARGET_SUBDIR, DISCOVERY_SCHEMA,
    DiscoveryEnv, DiscoveryError, HARNESS_BINARIES, HOME_ENV, HarnessBinary, Resolved, Source,
    candidates, is_executable_file, locate_report, resolve, resolve_all, resolve_with,
};
pub use edit_protocol::{
    EDIT_PROTOCOL_VERSION, EditError, EditErrorClass, EditOp, EditPlan, RefKind, Side, classify_ref,
};
pub use host::{
    CANONICAL_REF_ALIASES, CANONICAL_RESULT_FIELDS, CANONICAL_TEXT_ALIASES, CapabilityDescriptor,
    Connector, ConnectorError, DEFAULT_MAX_VISIBLE_RESULT_BYTES, DispatchContext,
    GlobalRegistration, Host, HostError, MAX_RESULT_SPILL_ENVELOPE_BYTES, MAX_VISIBLE_ERROR_BYTES,
    RESULT_SPILL_PREVIEW_BYTES, RESULT_SPILL_SCHEMA, RegistrationError, finalize_visible_error,
    runtime_creation_count,
};
pub use limits::{HostLimits, LimitError};
pub use manifest::{
    ArtifactEnv, ArtifactOutcome, EPHEMERAL_MARKERS, EPHEMERAL_REASON, HARNESS_ARTIFACTS,
    HarnessArtifact, JOURNAL_DIR, LIB_DIR, MANIFEST_SCHEMA, ManifestFacts, NODE_ENV,
    RUNTIME_MODULE_ENV, Refusal, SUBSTRATE_MODULE_ENV, StorePaths, artifact_candidates,
    is_ephemeral, is_readable_file, locate_manifest, manifest_order, resolve_artifact,
};
pub use node::{
    FNM_DEFAULT_ALIAS_SUBDIR, FNM_DIR_ENV, NODE_ORDER, NODE_SCHEMA, NodeCandidate, NodeEnv,
    NodeError, NodeOutcome, NodeRefusal, NodeSource, node_candidates, node_file_name, node_report,
    resolve_node, resolve_node_with,
};
pub use wrap::{PlanError, validate_plan, wrap_plan};
