#![forbid(unsafe_code)]

//! Engine-agnostic operation ABI contract machinery shared by ZeroStack engines.
//!
//! Each engine (TokenZero, FSZero, GraphZero) keeps its own operation
//! registry, enums, and catalogs. This crate owns the parts that must never
//! drift between engines:
//!
//! - canonical JSON encoding with deterministic key order
//! - JSON Schema normalization and structural comparison
//! - schema fingerprints and the contract digest hash
//!
//! Engines wrap these primitives with their own registry types and parity
//! assertions, so adopting this crate changes no digests and no behavior.

pub mod assembly;
pub mod cache_entry;
pub mod capability;
pub mod continuation;
pub mod cwir;
pub mod decision;
pub mod decision_view;
pub mod digest;
pub mod dispatch;
pub mod effect;
pub mod exec_dag;
pub mod exec_stream;
pub mod exec_trace;
pub mod freshness;
pub mod identity;
pub mod job;
pub mod raw_worker;
pub mod reasoning;
pub mod result;
pub mod redaction;
pub mod robust_snap;
pub mod schema;
pub mod surface;
pub mod telemetry;
pub mod verdict;
pub mod zbf;
pub mod zerokernel;
pub mod zero_execute;

pub use assembly::{
    ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_DOMAIN, ASSEMBLY_MANIFEST_SCHEMA_VERSION,
    ArtifactOwner, AssemblyExpectation, AssemblyFailureCode, AssemblyManifestError,
    AssemblyManifest, AssemblyPreDispatchError, DigestParseError, Sha256Digest, LinkedArtifact,
    LinkedProfile, MAX_ASSEMBLY_ITEMS, MAX_ASSEMBLY_MANIFEST_BYTES, MAX_ASSEMBLY_STRING_BYTES,
    PlatformIdentity, ProfileKind, ReceiptSchemaIdentity, TargetIdentity,
    VerifierIdentity, WorkerIdentity, assembly_abi_contract_digest,
    assembly_abi_contract_manifest, validate_assembly_pre_dispatch,
};
pub use cache_entry::{
    CACHE_ENTRY_SCHEMA, CacheEntryError, CacheEntry, CacheKey, CacheRoot, CacheValue,
    CacheCompletenessWitness, OperatorIdentity, VerifierReceipt,
};
pub use capability::{
    CapabilityMismatch, CapabilitySchema, CasLayout, FragmentBehavior, FragmentPolicy,
    HashAlgorithm, HashCapability, LayoutVersion, SharedCapability, SharedCasCapability,
};
pub use cwir::{
    CWIR_CONTRACT_VERSION, CWIR_EDGE_DOMAIN, CWIR_EXPANSION_DOMAIN,
    CWIR_MAX_CANONICAL_BYTES, CWIR_MAX_CAPABILITIES, CWIR_MAX_EDGES, CWIR_MAX_EFFECTS,
    CWIR_MAX_EXPANSION_INPUT_BYTES, CWIR_MAX_EXPANSION_OUTPUT_BYTES,
    CWIR_MAX_EXPANSION_WORK_UNITS, CWIR_MAX_EXPANSIONS, CWIR_MAX_IDENTITY_BYTES,
    CWIR_MAX_NODES, CWIR_MAX_OBLIGATIONS, CWIR_MAX_REFS_PER_ITEM, CWIR_MODEL_VERSION,
    CWIR_NODE_DOMAIN, CWIR_OBLIGATION_DOMAIN, CWIR_SEMANTIC_DOMAIN, CWIR_TASK_DOMAIN,
    CausalWorkIr, CwirCoverage, CwirDeterminism, CwirEdgeKind, CwirEffectSpace,
    CwirEpistemicProduct, CwirError, CwirExpansionCost, CwirExpansion, CwirFailureCode,
    CwirFreshness, CwirHyperEdge, CwirNodeKind, CwirNode, CwirObligationKind,
    CwirObligationStatus, CwirObligation, CwirSoundness, CwirStateAnchor,
    CwirTaskContract, CwirVerificationContract, CwirVerifierClass, cwir_contract_digest,
    cwir_contract_manifest,
};
pub use decision::{
    ContingentPolicyRule, ContingentPolicy, DecisionError, DecisionRequired,
    ObservationClass, ObservedMatch, PolicyResolution, SemanticDecisionPoint,
    verdict_permits_selection,
};
pub use decision_view::{
    CompletenessGrade, DecisionViewBinding, DecisionViewError, DecisionView,
    DECISION_VIEW_SCHEMA_ID,
};
pub use continuation::{
    ContinuationCompactRecord, ContinuationError, ContinuationHandle, ContinuationRoots,
    CONTINUATION_CONTRACT_VERSION,
};
pub use digest::{contract_digest, contract_digest_hex, sha256, sha256_hex};
pub use identity::{
    CancellationSemantics, CONTRACT_VERSION, CoverageGrade, EventLog, EventRecord,
    FallbackPolicy, HarnessContract, MessageOrdering,
    IdentityKernelError, MIGRATION_RECEIPT_MAX_CANONICAL_BYTES, MIGRATION_RECEIPT_MAX_REASON_BYTES,
    ObjectClass, PayloadFormationReceipt, ProjectSuccessorCas,
    ProtectedDimension, ProtectedScopeObligations, ROOTED_ABI_VERSION, ROOT_HASH_ALGORITHM,
    RootedAbiMigrationReceipt, ScopeObligation, SerializationScheme, SideEffectPolicy,
    StructuredTaskContract,
    SuccessorOutcome,
    SuccessorRecord, SuccessorUnchangedReason, TaskBudget, TranscriptPolicy,
    canonical_object_bytes,
    event_log_genesis, object_root, root_preimage, verify_object_root,
};
pub use dispatch::{
    ALL_DISPATCH_ERROR_CLASSES, ApprovalGrant, ApprovalRequirement, CANONICAL_DISPATCH_VERSION,
    CanonicalOperation, CanonicalRegistry, CanonicalResource, DispatchContractError,
    DispatchErrorClass, DispatchMachine, DispatchStage, EffectGrant, EffectPolicy, PermitGrant,
    PermitRequirement, RegistryEngine, SourceDiagnostic, SourceForm,
};
pub use effect::{
    EFFECT_IR_ACTION_DOMAIN, EFFECT_IR_CONTRACT_VERSION, EFFECT_IR_MAX_CANONICAL_BYTES,
    EFFECT_IR_MAX_CAPABILITIES, EFFECT_IR_MAX_EXCEPTIONS, EFFECT_IR_MAX_INTENTS,
    EFFECT_IR_MAX_LITERAL_BYTES, EFFECT_IR_MAX_OPERATIONS, EFFECT_IR_MAX_PRECONDITIONS,
    EFFECT_IR_MAX_REFS_PER_OPERATION, EFFECT_IR_MAX_STRING_BYTES, EFFECT_IR_MAX_TARGETS,
    EFFECT_IR_MAX_VERIFICATION_STEPS, EffectAdmission, EffectCapabilityBinding,
    EffectException, EffectIrError, EffectIrFailureCode, EffectPredicate, EffectProgram,
    EffectRollback, EffectTarget, EffectVerificationPlan, EffectVerificationStep,
    TypedEffectOperation, effect_ir_contract_digest, effect_ir_contract_manifest,
};
pub use exec_dag::{
    MAX_EXEC_DAG_DEPENDENCIES_PER_NODE, MAX_EXEC_DAG_NODES, ExecDagError, ExecDag,
    ExecNodeKind, ExecNode,
};
pub use exec_stream::{ExecStreamEvent, StepReceipt};
pub use exec_trace::{
    ExecTraceError, ExecTraceRecord, ExecTrace, ProtectedDecisionView,
    TraceDivergence, TraceEquivalence, TraceOutcome,
};
pub use freshness::{
    CertifiedInfluenceClosure, DependencyEdgeKind, DependencyEdge,
    EssentialDependencyCertificate, EssentialDependencyWitness, FRESHNESS_CONTRACT_VERSION,
    FRESHNESS_MAX_EDGES, FRESHNESS_MAX_NODES, FRESHNESS_MAX_REPOSITORIES, FRESHNESS_MAX_WITNESSES,
    FRESHNESS_MODEL_VERSION, FreshnessDecision, FreshnessError, FreshnessFailureCode,
    FreshnessHead, FreshnessStatus, IndexedThroughCertificate, ProducerDomain,
    decide_freshness, freshness_contract_digest, freshness_contract_manifest,
    influence_closure,
};
pub use job::{
    TOKEN_JOB_ABI_VERSION, TOKEN_JOB_DEFAULT_TAIL_BYTES, TOKEN_JOB_DEFAULT_WAIT_MS,
    TOKEN_JOB_MAX_ID_BYTES, TOKEN_JOB_MAX_TAIL_BYTES, TOKEN_JOB_MAX_WAIT_MS,
    TOKEN_JOB_OPERATION, TokenJobContractError, TokenJobPollRequest, TokenJobPollResult,
    TokenJobStatus, token_job_contract_digest, token_job_contract_manifest,
};
pub use raw_worker::{
    ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, DEFAULT_MAX_FRAME_BYTES,
    ENGINE_TIMELINE_MAX_SPANS, EffectClass, EngineIdentity, EngineStageSpan,
    EngineStageTimeline, FrameCodecError, HandshakeAck, HandshakeRequest, ProtocolLimits,
    RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata, ShutdownRequest, SnapshotIdentity,
    TIMELINE_CLOSURE_TOLERANCE_NS, TelemetryRequest, WORKER_ERROR_KINDS, WorkerBinding,
    WorkerCapabilities, WorkerError, WorkerRequestFrame, WorkerResponseFrame, WorkerResult,
    WorkerResultMetadata,
    WorkerTokenAccounting, WorkerTokenCountKind, WorkerTrace, decode_request_frame,
    canonical_worker_error_kind, is_rw10_forbidden_op, is_typed_worker_error_kind,
    RW10_FORBIDDEN_OPS,
    decode_response_frame, encode_frame, raw_worker_protocol_digest_hex,
    raw_worker_protocol_manifest, validate_engine_stage_timeline, validate_handshake_request,
    validate_request_frame, validate_response_frame, validate_worker_token_accounting,
};
pub use reasoning::{
    NativeStatePolicy, REASONING_CONTRACT_MAX_CANONICAL_BYTES,
    REASONING_CONTRACT_MAX_EXTENSION_BYTES, REASONING_CONTRACT_MAX_EXTENSION_DEPTH,
    REASONING_CONTRACT_MAX_EXTENSION_NODES, REASONING_CONTRACT_MAX_ID_BYTES,
    REASONING_CONTRACT_MAX_STOP_SEQUENCES, REASONING_CONTRACT_MAX_STOP_SEQUENCE_BYTES,
    REASONING_CONTRACT_MAX_TOOL_PERMISSIONS, REASONING_CONTRACT_SCHEMA_VERSION,
    REASONING_CONTRACT_TEMPERATURE_PPM_MAX, REASONING_CONTRACT_TOP_P_PPM_MAX,
    REASONING_CONTRACT_VERSION, ReasoningContractError,
    ReasoningContractFailureCode, ReasoningContract, SamplingParams, StoppingPolicy,
    StrictReasoningAdmissionRecord, StrictReasoningAdmission, ToolPermission,
    reasoning_contract_digest, reasoning_contract_manifest,
    reasoning_contract_schema_digest, reasoning_contract_schema,
    verify_strict_no_downshift,
};
pub use result::{
    MAX_ACK_CHARS, MAX_PREVIEW_CHARS, ZERO_RESULT, ZeroResultAccessError, ZeroResultBuildError,
    ZeroResult, zero_result_from_engine_step, zero_result_to_wire,
};
pub use redaction::{EffectTrace, RedactionPolicy, Redactor, SecretsError};
pub use redaction::DEFAULT_REDACTION_TOKEN;
pub use robust_snap::{
    EvidenceDecisionTree, EvidenceLeaf, EvidenceObservation, ProtectedEffectClass,
    ProtectedEffectSet, ProtectedEffect, ROBUST_SNAP_CONTRACT_VERSION,
    ROBUST_SNAP_MAX_ASSUMPTION_BYTES, ROBUST_SNAP_MAX_ASSUMPTIONS, ROBUST_SNAP_MAX_EFFECTS,
    ROBUST_SNAP_MAX_EVIDENCE_DEPTH, ROBUST_SNAP_MAX_LEAVES, ROBUST_SNAP_MAX_WORLDS,
    ROBUST_SNAP_MODEL_VERSION, RobustSnapCertificate, RobustSnapError, RobustSnapFailureCode,
    SnapLevel, WorldFiberDescriptor, robust_snap_contract_digest,
    robust_snap_contract_manifest, validate_heuristic_world_order,
};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
pub use surface::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
pub use telemetry::{TelemetryCounter, TelemetryOverflow, TelemetrySchema, ZeroTelemetry};
pub use verdict::{
    Premise, SafetyVerdict, VerdictBuildError, VERDICT_MAX_PREMISE_NAME_BYTES,
};
pub use zero_execute::{
    AuditEventRange, ContinuationState, ZeroExecuteError, ZeroExecuteFields,
    ZeroExecuteKind, ZeroExecuteResult, ZERO_EXECUTE_ABI_VERSION,
};
pub use zerokernel::{
    ExactHandles, FiniteBudget, PreflightReport, KernelResourceLedger, ReturnKind,
    ReturnPolicy, RootBindings, RootEvidence, RootSnapshot, ZerokernelError,
    ZerokernelExecuteRequest, ZerokernelExecuteResponse, ZerokernelResultKind,
    ZEROKERNEL_ABI_VERSION, MAX_CALLS, MAX_CPU_MS,
    MAX_MEMORY_BYTES, MAX_KERNEL_PREVIEW_CHARS, MAX_WALL_MS,
};
pub use zbf::{
    DurableProfileId, DurableProfile, ZBF_CONTAINER_FLAG, ZBF_CONTRACT_VERSION,
    ZBF_HEADER_LEN, ZBF_MAGIC, ZBF_MAX_CHILDREN, ZBF_MAX_DEPTH,
    ZBF_MAX_OBJECT_BYTES, ZBF_SCHEMA_MAJOR, ZBF_SCHEMA_MINOR, ZbfArtifactKind,
    ZbfError, ZbfFailureCode, ZbfHeader, ZbfObject, ZbfPayload, zbf_contract_digest,
    zbf_contract_manifest,
};
