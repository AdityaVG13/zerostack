#![forbid(unsafe_code)]

//! Engine-agnostic contracts shared by ZeroKernel and its domain engines.
//!
//! FSZero, GraphZero, and TokenZero implement typed traits. They do not expose
//! model-facing operation registries or catalogs. This crate owns the shared
//! wire invariants: canonical encoding, schema normalization, contract
//! digests, bounded requests, typed receipts, and direct ZeroKernel results.

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
pub mod etnf;
pub mod etnf_checks;
pub mod exec_dag;
pub mod exec_stream;
pub mod exec_trace;
pub mod freshness;
pub mod identity;
pub mod job;
pub mod raw_worker;
pub mod reasoning;
pub mod redaction;
pub mod result;
pub mod robust_snap;
pub mod safe_expand;
pub mod schema;
pub mod snap_effect;
pub mod speculation;
pub mod surface;
pub mod telemetry;
pub mod verdict;
pub mod work_capsule;
pub mod zbf;
pub mod zero_kernel;

pub use assembly::{
    ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_DOMAIN, ASSEMBLY_MANIFEST_SCHEMA_VERSION,
    ArtifactOwner, AssemblyExpectation, AssemblyFailureCode, AssemblyManifest,
    AssemblyManifestError, AssemblyPreDispatchError, DigestParseError, LinkedArtifact,
    LinkedProfile, MAX_ASSEMBLY_ITEMS, MAX_ASSEMBLY_MANIFEST_BYTES, MAX_ASSEMBLY_STRING_BYTES,
    PlatformIdentity, ProfileKind, ReceiptSchemaIdentity, Sha256Digest, TargetIdentity,
    VerifierIdentity, WorkerIdentity, assembly_abi_contract_digest, assembly_abi_contract_manifest,
    validate_assembly_pre_dispatch,
};
pub use cache_entry::{
    CACHE_ENTRY_SCHEMA, CacheCompletenessWitness, CacheEntry, CacheEntryError, CacheKey, CacheRoot,
    CacheValue, OperatorIdentity, VerifierReceipt,
};
pub use capability::{
    CapabilityMismatch, CapabilitySchema, CasLayout, FragmentBehavior, FragmentPolicy,
    HashAlgorithm, HashCapability, LayoutVersion, SharedCapability, SharedCasCapability,
};
pub use continuation::{
    CONTINUATION_CONTRACT_VERSION, ContinuationCompactRecord, ContinuationError,
    ContinuationHandle, ContinuationRoots, ContinuationState,
};
pub use cwir::{
    CWIR_CONTRACT_VERSION, CWIR_EDGE_DOMAIN, CWIR_EXPANSION_DOMAIN, CWIR_MAX_CANONICAL_BYTES,
    CWIR_MAX_CAPABILITIES, CWIR_MAX_EDGES, CWIR_MAX_EFFECTS, CWIR_MAX_EXPANSION_INPUT_BYTES,
    CWIR_MAX_EXPANSION_OUTPUT_BYTES, CWIR_MAX_EXPANSION_WORK_UNITS, CWIR_MAX_EXPANSIONS,
    CWIR_MAX_IDENTITY_BYTES, CWIR_MAX_NODES, CWIR_MAX_OBLIGATIONS, CWIR_MAX_REFS_PER_ITEM,
    CWIR_MODEL_VERSION, CWIR_NODE_DOMAIN, CWIR_OBLIGATION_DOMAIN, CWIR_SEMANTIC_DOMAIN,
    CWIR_TASK_DOMAIN, CausalWorkIr, CwirCoverage, CwirDeterminism, CwirEdgeKind, CwirEffectSpace,
    CwirEpistemicProduct, CwirError, CwirExpansion, CwirExpansionCost, CwirFailureCode,
    CwirFreshness, CwirHyperEdge, CwirNode, CwirNodeKind, CwirObligation, CwirObligationKind,
    CwirObligationStatus, CwirSoundness, CwirStateAnchor, CwirTaskContract,
    CwirVerificationContract, CwirVerifierClass, cwir_contract_digest, cwir_contract_manifest,
};
pub use decision::{
    ContingentPolicy, ContingentPolicyRule, DecisionError, DecisionRequired, ObservationClass,
    ObservedMatch, PolicyResolution, SemanticDecisionPoint, verdict_permits_selection,
};
pub use decision_view::{
    CompletenessGrade, DECISION_VIEW_SCHEMA_ID, DecisionView, DecisionViewBinding,
    DecisionViewError,
};
pub use digest::{contract_digest, contract_digest_hex, hex_lower_32, sha256, sha256_hex};
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
    EFFECT_IR_MAX_VERIFICATION_STEPS, EffectAdmission, EffectCapabilityBinding, EffectException,
    EffectIrError, EffectIrFailureCode, EffectPredicate, EffectProgram, EffectRollback,
    EffectTarget, EffectVerificationPlan, EffectVerificationStep, TypedEffectOperation,
    effect_ir_contract_digest, effect_ir_contract_manifest,
};
pub use etnf::{
    CheckerIdentity, ETNF_HEX_DIGEST_LEN, ETNF_MAX_EVIDENCE_ITEMS, ETNF_MAX_FALSIFIERS,
    ETNF_MAX_ID_BYTES, ETNF_MAX_STRING_BYTES, ETNF_MAX_WITNESS_FACTS, ETNF_SCHEMA_ID, EtnfError,
    EvidenceItem, ExplicitFallback, FallbackKind, Falsifier, FiniteWitness,
    ProposedAuthorityTransition, ProposedTransitionKind, ResourceLedger, RootedEvidence,
    ShadowCertificate, V7ShadowReport,
};
pub use etnf_checks::{
    BaselineSegment, CausalClosureDocument, ClosureEdge, ClosureNode, KillMetrics, SavingsCategory,
    SavingsEntry, SavingsProvenanceDocument, VCQK_CHECKER_CAUSAL_ID, VCQK_CHECKER_CHAIN_ID,
    VCQK_CHECKER_SAVINGS_ID, VCQK_CHECKER_VERSION, VCQK_CONTRACT_CAUSAL, VCQK_CONTRACT_CHAIN,
    VCQK_CONTRACT_SAVINGS, VCQK_KILL_MAX_TRACKED_COUNTEREXAMPLES, VCQK_KILL_MAX_TRACKED_ROOTS,
    VCQK_KILL_NONCONVERGENCE_MAX_ISSUES, VCQK_LEARNING_REFINEMENT_PUBLISH_AUTHORITY,
    VCQK_MAX_BASELINE_SEGMENTS, VCQK_MAX_CHAIN_LINKS, VCQK_MAX_CLOSURE_EDGES,
    VCQK_MAX_CLOSURE_NODES, VCQK_MAX_DEMANDED_OUTPUTS, VCQK_MAX_IDENTIFIER_BYTES,
    VCQK_MAX_SAVINGS_ENTRIES, VCQK_SCOPE_CAUSAL, VCQK_SCOPE_CHAIN, VCQK_SCOPE_SAVINGS,
    check_causal_closure, check_certificate_chain, check_savings_provenance,
    savings_overhead_killed,
};
pub use exec_dag::{
    ExecDag, ExecDagError, ExecNode, ExecNodeKind, MAX_EXEC_DAG_DEPENDENCIES_PER_NODE,
    MAX_EXEC_DAG_NODES,
};
pub use exec_stream::{ExecStreamEvent, StepReceipt};
pub use exec_trace::{
    ExecTrace, ExecTraceError, ExecTraceRecord, ProtectedDecisionView, TraceDivergence,
    TraceEquivalence, TraceOutcome,
};
pub use freshness::{
    CertifiedInfluenceClosure, DependencyEdge, DependencyEdgeKind, EssentialDependencyCertificate,
    EssentialDependencyWitness, FRESHNESS_CONTRACT_VERSION, FRESHNESS_MAX_EDGES,
    FRESHNESS_MAX_NODES, FRESHNESS_MAX_REPOSITORIES, FRESHNESS_MAX_WITNESSES,
    FRESHNESS_MODEL_VERSION, FreshnessDecision, FreshnessError, FreshnessFailureCode,
    FreshnessHead, FreshnessStatus, IndexedThroughCertificate, ProducerDomain, decide_freshness,
    freshness_contract_digest, freshness_contract_manifest, influence_closure,
};
pub use identity::{
    CONTRACT_VERSION, CancellationSemantics, CoverageGrade, EventLog, EventRecord, FallbackPolicy,
    HarnessContract, IdentityKernelError, MIGRATION_RECEIPT_MAX_CANONICAL_BYTES,
    MIGRATION_RECEIPT_MAX_REASON_BYTES, MessageOrdering, ObjectClass, PayloadFormationReceipt,
    ProjectSuccessorCas, ProtectedDimension, ProtectedScopeObligations, ROOT_HASH_ALGORITHM,
    ROOTED_ABI_VERSION, RootedAbiMigrationReceipt, ScopeObligation, SerializationScheme,
    SideEffectPolicy, StructuredTaskContract, SuccessorOutcome, SuccessorRecord,
    SuccessorUnchangedReason, TaskBudget, TranscriptPolicy, canonical_object_bytes,
    event_log_genesis, object_root, root_preimage, verify_object_root,
};
pub use job::{
    TOKEN_JOB_ABI_VERSION, TOKEN_JOB_DEFAULT_TAIL_BYTES, TOKEN_JOB_DEFAULT_WAIT_MS,
    TOKEN_JOB_MAX_ID_BYTES, TOKEN_JOB_MAX_TAIL_BYTES, TOKEN_JOB_MAX_WAIT_MS, TOKEN_JOB_OPERATION,
    TokenJobContractError, TokenJobPollRequest, TokenJobPollResult, TokenJobStatus,
    token_job_contract_digest, token_job_contract_manifest,
};
pub use raw_worker::{
    ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, DEFAULT_MAX_FRAME_BYTES,
    ENGINE_TIMELINE_MAX_SPANS, EffectClass, EngineIdentity, EngineStageSpan, EngineStageTimeline,
    FrameCodecError, HandshakeAck, HandshakeRequest, ProtocolLimits, RAW_WORKER_PROTOCOL_VERSION,
    RW10_FORBIDDEN_OPS, RefOwnership, RevertMetadata, ShutdownRequest, SnapshotIdentity,
    TIMELINE_CLOSURE_TOLERANCE_NS, TelemetryRequest, WORKER_ERROR_KINDS, WorkerBinding,
    WorkerCapabilities, WorkerError, WorkerRequestFrame, WorkerResponseFrame, WorkerResult,
    WorkerResultMetadata, WorkerTokenAccounting, WorkerTokenCountKind, WorkerTrace,
    canonical_worker_error_kind, decode_request_frame, decode_response_frame, encode_frame,
    is_rw10_forbidden_op, is_typed_worker_error_kind, raw_worker_protocol_digest_hex,
    raw_worker_protocol_manifest, validate_engine_stage_timeline, validate_handshake_request,
    validate_request_frame, validate_response_frame, validate_worker_token_accounting,
};
pub use reasoning::{
    NativeStatePolicy, REASONING_CONTRACT_MAX_CANONICAL_BYTES,
    REASONING_CONTRACT_MAX_EXTENSION_BYTES, REASONING_CONTRACT_MAX_EXTENSION_DEPTH,
    REASONING_CONTRACT_MAX_EXTENSION_NODES, REASONING_CONTRACT_MAX_ID_BYTES,
    REASONING_CONTRACT_MAX_STOP_SEQUENCE_BYTES, REASONING_CONTRACT_MAX_STOP_SEQUENCES,
    REASONING_CONTRACT_MAX_TOOL_PERMISSIONS, REASONING_CONTRACT_SCHEMA_VERSION,
    REASONING_CONTRACT_TEMPERATURE_PPM_MAX, REASONING_CONTRACT_TOP_P_PPM_MAX,
    REASONING_CONTRACT_VERSION, ReasoningContract, ReasoningContractError,
    ReasoningContractFailureCode, SamplingParams, StoppingPolicy, StrictReasoningAdmission,
    StrictReasoningAdmissionRecord, ToolPermission, reasoning_contract_digest,
    reasoning_contract_manifest, reasoning_contract_schema, reasoning_contract_schema_digest,
    verify_strict_no_downshift,
};
pub use redaction::DEFAULT_REDACTION_TOKEN;
pub use redaction::{EffectTrace, RedactionPolicy, Redactor, SecretsError};
pub use result::{
    MAX_ACK_CHARS, MAX_PREVIEW_CHARS, ZERO_RESULT, ZeroResult, ZeroResultAccessError,
    ZeroResultBuildError, from_step, to_wire,
};
pub use robust_snap::{
    EvidenceDecisionTree, EvidenceLeaf, EvidenceObservation, ProtectedEffect, ProtectedEffectClass,
    ProtectedEffectSet, ROBUST_SNAP_CONTRACT_VERSION, ROBUST_SNAP_MAX_ASSUMPTION_BYTES,
    ROBUST_SNAP_MAX_ASSUMPTIONS, ROBUST_SNAP_MAX_EFFECTS, ROBUST_SNAP_MAX_EVIDENCE_DEPTH,
    ROBUST_SNAP_MAX_LEAVES, ROBUST_SNAP_MAX_WORLDS, ROBUST_SNAP_MODEL_VERSION,
    RobustSnapCertificate, RobustSnapError, RobustSnapFailureCode, SnapLevel, WorldFiberDescriptor,
    robust_snap_contract_digest, robust_snap_contract_manifest, validate_heuristic_world_order,
};
pub use safe_expand::{
    CompletenessBinding, CompletenessEvidence, ExpandOutcome, ExpandPermit, LiveCompleteness,
    LiveExpandState, MAX_SAFE_EXPAND_STRING_BYTES, SAFE_EXPAND_CONTRACT_VERSION, SafeExpandError,
    SafeExpandHandle, SafeExpandIssueRequest, SafeExpandIssuer,
};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
pub use snap_effect::{
    EFFECT_RESULT_SCHEMA, EXPAND_RESULT_SCHEMA, EffectAnchorRequest, EffectChangeKind,
    EffectChangeRequest, EffectCommandRequest, EffectRequest, EffectResult, EffectTargetRequest,
    EffectTargetResult, EffectVerificationRequest, EffectVerificationResult, ExpandResult,
    SNAP_WORKSPACE_SCHEMA, SnapAccounting, SnapByteRange, SnapByteSelectionRequest,
    SnapLineSelectionRequest, SnapNewline, SnapRecovery, SnapRequest, SnapResult,
    SnapSearchRequest, SnapSelection, SnapSelectionRequest, SnapSource, SnapStructuralEvidence,
    SnapTargetRequest, SnapView, SnapViewMode, SnapViewRequest,
};
pub use speculation::{
    DEFAULT_SPECULATION_LIMIT, SPECULATION_CONTRACT, FinalizedCallProof,
    FinalizedSpeculationPlan, SpeculationAdmission, SpeculationBinding, SpeculationCandidate,
    SpeculationLedger, SpeculationPermit, SpeculationState, SpeculativeOperation,
    compile_finalized_speculation_plan,
};
pub use surface::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
pub use telemetry::{TelemetryCounter, TelemetryOverflow, TelemetrySchema, ZeroTelemetry};
pub use verdict::{Premise, SafetyVerdict, VERDICT_MAX_PREMISE_NAME_BYTES, VerdictBuildError};
pub use work_capsule::{
    CapsuleRoots, CapsuleState, GovernorDecision, GovernorInput, GovernorRegime, InterruptSchedule,
    MechanicalEvidence, MechanicalVerdict, PromotionEvidence, PromotionInputs, ScheduleAction,
    SemanticInterrupt, SemanticInterruptKind, TurnClass, TurnMetadata, TurnRecord, WorkCapsule,
    ZeroDominanceProof, choose_regime, schedule_next,
};
pub use zbf::{
    DurableProfile, DurableProfileId, ZBF_CONTAINER_FLAG, ZBF_CONTRACT_VERSION, ZBF_HEADER_LEN,
    ZBF_MAGIC, ZBF_MAX_CHILDREN, ZBF_MAX_DEPTH, ZBF_MAX_OBJECT_BYTES, ZBF_SCHEMA_MAJOR,
    ZBF_SCHEMA_MINOR, ZbfArtifactKind, ZbfError, ZbfFailureCode, ZbfHeader, ZbfObject, ZbfPayload,
    zbf_contract_digest, zbf_contract_manifest,
};
pub use zero_kernel::{
    AsgrepMode, AsgrepOptions, CancellationProbe, CertifyResult, CompressionRequest,
    CompressionResult, EngineCallContext, EngineError, EngineErrorKind, EngineInvocation,
    ExpandOptions, FileEffectKind, FileEffectReceipt, FileEffectRequest, FileEngine, FileLease,
    FileMetadata, FileReadRequest, FileSnapshot, GUEST_METHODS, HANDLE_DIGEST_BYTES, KernelBudget,
    KernelContext, KernelLedger, LookupOptions, OPERATION_TRACE_LIMIT, PARALLEL_TASK_LIMIT,
    ProjectionRequest, ProjectionResult, ReadOptions, SOURCE_BYTE_LIMIT, STATE_KEY_BYTE_LIMIT,
    STATE_KEY_LIMIT, STATE_TOTAL_BYTE_LIMIT, STATE_VALUE_BYTE_LIMIT, ShellOptions, ShellResult,
    StateEvidence, StructuralAbsence, StructuralBudget, StructuralCoverage, StructuralEngine,
    StructuralHit, StructuralQuery, StructuralResult, TokenAccounting, TokenEngine,
    ZERO_HANDLE_PREFIX, ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelError, ZeroKernelEvent,
    ZeroKernelOutcome, ZeroKernelRequest, ZeroKernelResponse, ZeroOperationStatus,
    ZeroOperationTrace, zero_kernel_response_schema,
};
