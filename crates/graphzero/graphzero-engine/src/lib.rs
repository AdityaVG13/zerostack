//! P3.3 native snapshot query surface on GraphZero indices.
//! P4.1 blast-radius intent queries.

pub mod accounting;
pub mod blast;
pub mod codemode;
pub mod conformance;
pub mod deterministic_facts;
pub mod dispatcher;
pub mod embedded;
pub mod one_tp;
pub mod operation_abi;
pub mod oracle;
pub mod query_surface;
pub mod raw_worker_stdio;
pub mod release_gates;
pub mod rewrite_closure;
pub mod surface_bench;
pub mod surface_handshake;
pub mod task_lens;
pub mod witness_cache;
pub mod world_envelope;
pub mod zero_kernel;

pub use blast::{
    BLAST_SCHEMA_VERSION, BlastError, BlastIntentParse, BlastRadiusCapsule, EdgeProvenance,
    PlannedEdit, PlannedImpact, RetrievalEdge, RetrievalNeighborhood, RetrievalNode,
    SpeculativeBlastReport, SpeculativeBlastRequest, blast_from_json, blast_radius,
    blast_radius_with_depth, blast_to_json, impact_before_edit, parse_intent,
    retrieval_neighborhood,
};
pub use codemode::execute_plan as codemode_execute_plan;
pub use deterministic_facts::{
    FACT_KIND_ALLOWLIST, FactViolation, assert_deterministic_facts, audit_facts, audit_value,
    canonical_facts, canonical_json, debug_assert_deterministic_facts,
};
pub use dispatcher::{
    AdapterKind, CancellationToken, DispatchPhaseTimings, DispatchProfile, EngineContext, dispatch,
    dispatch_phase_timing_enabled, dispatch_profiled, private_worker_dispatch,
    take_dispatch_phase_timings,
};
pub use embedded::{EmbeddedGraphZero, SharedBlobRef, SharedGraphZeroStore};
pub use one_tp::{ONE_TP_ACK, ONE_TP_SCHEMA};
pub use operation_abi::{
    DomainError, DomainErrorKind, DomainResult, Operation, OperationArgs,
    SEMANTIC_CONTRACT_VERSION, contract_digest_hex, resolve_operation,
};
pub use oracle::{FAILURE_BUNDLE_SCHEMA_VERSION, FailureBundle, OracleBundleError, OracleMode};
pub use query_surface::{
    QuerySurface, QuerySurfaceError, QuerySurfaceRequest, QuerySurfaceResponse, QuerySurfaceRouter,
    SURFACE_NAMES,
};
pub use rewrite_closure::{
    EditSite, PropagationPolicy, REWRITE_CLOSURE_SCHEMA_VERSION, Relation, RewriteClosure,
    rewrite_closure,
};
pub use surface_handshake::{
    HandshakeAck, HandshakeRequest, Ownership, PrivateRawWorker, RAW_WORKER_VERSION,
    SEMANTIC_CONTRACT_NAME, SURFACE_MANIFEST_SCHEMA, SelectedSurface, SurfaceCapability,
    WorkerTrace, client_native_raw_worker_capability, local_capability,
    outer_router_raw_worker_capability, private_worker_dispatch_checked, validate_handshake,
};
pub use task_lens::{
    CandidatePlan, ComposedTaskLens, DependencyClosureLens, GradeBar, GradeFilterLens,
    IdentityLens, LensContext, LensError, LensOutcome, LensReceipt, LensReceiptEntry, LensScope,
    ScopeFilterLens, ScopeNode, SourceRoot, TASK_LENS_SCHEMA_VERSION, TaskContract, TaskLens,
    TaskLensReport, TaskScopeLens,
};
pub use world_envelope::{
    WORLD_ENVELOPE_VERSION, WORLD_REF_PREFIX, WorldEnvelope, WorldEnvelopeError, WorldFileEntry,
    bind_world_envelope, parse_world_envelope, parse_world_envelope_value, validate_world_ref,
};

pub use zero_kernel::ZeroStructuralEngine;
