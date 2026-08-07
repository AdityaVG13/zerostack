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
pub mod cwir;
pub mod digest;
pub mod dispatch;
pub mod effect;
pub mod freshness;
pub mod job;
pub mod raw_worker;
pub mod reasoning;
pub mod result;
pub mod robust_snap;
pub mod schema;
pub mod telemetry;
pub mod zbf;

pub use assembly::{
    ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_DOMAIN_V1, ASSEMBLY_MANIFEST_SCHEMA_VERSION,
    ArtifactOwnerV1, AssemblyExpectationV1, AssemblyFailureCodeV1, AssemblyManifestErrorV1,
    AssemblyManifestV1, AssemblyPreDispatchErrorV1, DigestParseErrorV1, DigestV1, LinkedArtifactV1,
    LinkedProfileV1, MAX_ASSEMBLY_ITEMS, MAX_ASSEMBLY_MANIFEST_BYTES, MAX_ASSEMBLY_STRING_BYTES,
    PlatformIdentityV1, ProfileKindV1, ReceiptSchemaIdentityV1, TargetIdentityV1,
    VerifierIdentityV1, WorkerIdentityV1, assembly_abi_contract_digest_v1,
    assembly_abi_contract_manifest_v1, validate_assembly_pre_dispatch_v1,
};
pub use cache_entry::{
    CACHE_ENTRY_SCHEMA_V1, CacheEntryError, CacheEntryV1, CacheKeyV1, CacheRootV1, CacheValueV1,
    CompletenessWitnessV1, OperatorIdentityV1, VerifierReceiptV1,
};
pub use capability::{
    CapabilityMismatch, CapabilitySchema, CasLayout, FragmentBehavior, FragmentPolicy,
    HashAlgorithm, HashCapability, LayoutVersion, SharedCapability, SharedCasCapability,
};
pub use cwir::{
    CWIR_CONTRACT_VERSION_V1, CWIR_EDGE_DOMAIN_V1, CWIR_EXPANSION_DOMAIN_V1,
    CWIR_MAX_CANONICAL_BYTES_V1, CWIR_MAX_CAPABILITIES_V1, CWIR_MAX_EDGES_V1, CWIR_MAX_EFFECTS_V1,
    CWIR_MAX_EXPANSION_INPUT_BYTES_V1, CWIR_MAX_EXPANSION_OUTPUT_BYTES_V1,
    CWIR_MAX_EXPANSION_WORK_UNITS_V1, CWIR_MAX_EXPANSIONS_V1, CWIR_MAX_IDENTITY_BYTES_V1,
    CWIR_MAX_NODES_V1, CWIR_MAX_OBLIGATIONS_V1, CWIR_MAX_REFS_PER_ITEM_V1, CWIR_MODEL_VERSION_V1,
    CWIR_NODE_DOMAIN_V1, CWIR_OBLIGATION_DOMAIN_V1, CWIR_SEMANTIC_DOMAIN_V1, CWIR_TASK_DOMAIN_V1,
    CausalWorkIrV1, CwirCoverageV1, CwirDeterminismV1, CwirEdgeKindV1, CwirEffectSpaceV1,
    CwirEpistemicProductV1, CwirErrorV1, CwirExpansionCostV1, CwirExpansionV1, CwirFailureCodeV1,
    CwirFreshnessV1, CwirHyperEdgeV1, CwirNodeKindV1, CwirNodeV1, CwirObligationKindV1,
    CwirObligationStatusV1, CwirObligationV1, CwirSoundnessV1, CwirStateAnchorV1,
    CwirTaskContractV1, CwirVerificationContractV1, CwirVerifierClassV1, cwir_contract_digest_v1,
    cwir_contract_manifest_v1,
};
pub use digest::{contract_digest, contract_digest_hex, sha256, sha256_hex};
pub use dispatch::{
    ALL_DISPATCH_ERROR_CLASSES, ApprovalGrant, ApprovalRequirement, CANONICAL_DISPATCH_VERSION,
    CanonicalOperation, CanonicalRegistry, DispatchContractError, DispatchErrorClass,
    DispatchMachine, DispatchStage, EffectGrant, EffectPolicy, PermitGrant, PermitRequirement,
    RegistryEngine, SourceDiagnostic, SourceForm,
};
pub use effect::{
    EFFECT_IR_ACTION_DOMAIN_V1, EFFECT_IR_CONTRACT_VERSION_V1, EFFECT_IR_MAX_CANONICAL_BYTES_V1,
    EFFECT_IR_MAX_CAPABILITIES_V1, EFFECT_IR_MAX_EXCEPTIONS_V1, EFFECT_IR_MAX_INTENTS_V1,
    EFFECT_IR_MAX_LITERAL_BYTES_V1, EFFECT_IR_MAX_OPERATIONS_V1, EFFECT_IR_MAX_PRECONDITIONS_V1,
    EFFECT_IR_MAX_REFS_PER_OPERATION_V1, EFFECT_IR_MAX_STRING_BYTES_V1, EFFECT_IR_MAX_TARGETS_V1,
    EFFECT_IR_MAX_VERIFICATION_STEPS_V1, EffectAdmissionV1, EffectCapabilityBindingV1,
    EffectExceptionV1, EffectIrErrorV1, EffectIrFailureCodeV1, EffectPredicateV1, EffectProgramV1,
    EffectRollbackV1, EffectTargetV1, EffectVerificationPlanV1, EffectVerificationStepV1,
    TypedEffectOperationV1, effect_ir_contract_digest_v1, effect_ir_contract_manifest_v1,
};
pub use freshness::{
    CertifiedInfluenceClosure, DependencyEdgeKindV1, DependencyEdgeV1,
    EssentialDependencyCertificate, EssentialDependencyWitnessV1, FRESHNESS_CONTRACT_VERSION,
    FRESHNESS_MAX_EDGES, FRESHNESS_MAX_NODES, FRESHNESS_MAX_REPOSITORIES, FRESHNESS_MAX_WITNESSES,
    FRESHNESS_MODEL_VERSION, FreshnessDecisionV1, FreshnessErrorV1, FreshnessFailureCodeV1,
    FreshnessHeadV1, FreshnessStatusV1, IndexedThroughCertificate, ProducerDomainV1,
    decide_freshness_v1, freshness_contract_digest_v1, freshness_contract_manifest_v1,
    influence_closure_v1,
};
pub use job::{
    TOKEN_JOB_ABI_VERSION_V1, TOKEN_JOB_DEFAULT_TAIL_BYTES_V1, TOKEN_JOB_DEFAULT_WAIT_MS_V1,
    TOKEN_JOB_MAX_ID_BYTES_V1, TOKEN_JOB_MAX_TAIL_BYTES_V1, TOKEN_JOB_MAX_WAIT_MS_V1,
    TOKEN_JOB_OPERATION_V1, TokenJobContractError, TokenJobPollRequestV1, TokenJobPollResultV1,
    TokenJobStatusV1, token_job_contract_digest_v1, token_job_contract_manifest_v1,
};
pub use raw_worker::{
    ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, DEFAULT_MAX_FRAME_BYTES,
    ENGINE_TIMELINE_MAX_SPANS_V1, EffectClass, EngineIdentity, EngineStageSpanV1,
    EngineStageTimelineV1, FrameCodecError, HandshakeAck, HandshakeRequest, ProtocolLimits,
    RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata, ShutdownRequest, SnapshotIdentity,
    TIMELINE_CLOSURE_TOLERANCE_NS_V1, TelemetryRequestV1, WorkerBinding, WorkerCapabilities,
    WorkerError, WorkerRequestFrame, WorkerResponseFrame, WorkerResult, WorkerResultMetadata,
    WorkerTokenAccountingV1, WorkerTokenCountKind, WorkerTrace, decode_request_frame,
    decode_response_frame, encode_frame, raw_worker_protocol_digest_hex,
    raw_worker_protocol_manifest, validate_engine_stage_timeline_v1, validate_handshake_request,
    validate_request_frame, validate_response_frame, validate_worker_token_accounting_v1,
};
pub use reasoning::{
    NativeStatePolicyV1, REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1,
    REASONING_CONTRACT_MAX_EXTENSION_BYTES_V1, REASONING_CONTRACT_MAX_EXTENSION_DEPTH_V1,
    REASONING_CONTRACT_MAX_EXTENSION_NODES_V1, REASONING_CONTRACT_MAX_ID_BYTES_V1,
    REASONING_CONTRACT_SCHEMA_VERSION_V1, REASONING_CONTRACT_VERSION_V1, ReasoningContractErrorV1,
    ReasoningContractFailureCodeV1, ReasoningContractV1, StrictReasoningAdmissionRecordV1,
    StrictReasoningAdmissionV1, reasoning_contract_digest_v1, reasoning_contract_manifest_v1,
    reasoning_contract_schema_digest_v1, reasoning_contract_schema_v1,
    verify_strict_no_downshift_v1,
};
pub use result::{
    MAX_ACK_CHARS, MAX_PREVIEW_CHARS, ZERO_RESULT_V1, ZeroResultAccessError, ZeroResultBuildError,
    ZeroResultV1,
};
pub use robust_snap::{
    EvidenceDecisionTree, EvidenceLeafV1, EvidenceObservationV1, ProtectedEffectClassV1,
    ProtectedEffectSet, ProtectedEffectV1, ROBUST_SNAP_CONTRACT_VERSION,
    ROBUST_SNAP_MAX_ASSUMPTION_BYTES, ROBUST_SNAP_MAX_ASSUMPTIONS, ROBUST_SNAP_MAX_EFFECTS,
    ROBUST_SNAP_MAX_EVIDENCE_DEPTH, ROBUST_SNAP_MAX_LEAVES, ROBUST_SNAP_MAX_WORLDS,
    ROBUST_SNAP_MODEL_VERSION, RobustSnapCertificate, RobustSnapErrorV1, RobustSnapFailureCodeV1,
    SnapLevel, WorldFiberDescriptor, robust_snap_contract_digest_v1,
    robust_snap_contract_manifest_v1, validate_heuristic_world_order,
};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
pub use telemetry::{TelemetryCounter, TelemetryOverflow, TelemetrySchema, ZeroTelemetryV1};
pub use zbf::{
    DurableProfileIdV1, DurableProfileV1, ZBF_CONTAINER_FLAG_V1, ZBF_CONTRACT_VERSION_V1,
    ZBF_HEADER_LEN_V1, ZBF_MAGIC_V1, ZBF_MAX_CHILDREN_V1, ZBF_MAX_DEPTH_V1,
    ZBF_MAX_OBJECT_BYTES_V1, ZBF_SCHEMA_MAJOR_V1, ZBF_SCHEMA_MINOR_V1, ZbfArtifactKindV1,
    ZbfErrorV1, ZbfFailureCodeV1, ZbfHeaderV1, ZbfObjectV1, ZbfPayloadV1, zbf_contract_digest_v1,
    zbf_contract_manifest_v1,
};
