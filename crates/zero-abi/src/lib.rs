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
pub mod digest;
pub mod dispatch;
pub mod raw_worker;
pub mod result;
pub mod robust_snap;
pub mod schema;
pub mod telemetry;

pub use assembly::{
    assembly_abi_contract_digest_v1, assembly_abi_contract_manifest_v1,
    validate_assembly_pre_dispatch_v1, ArtifactOwnerV1, AssemblyExpectationV1,
    AssemblyFailureCodeV1, AssemblyManifestErrorV1, AssemblyManifestV1, AssemblyPreDispatchErrorV1,
    DigestParseErrorV1, DigestV1, LinkedArtifactV1, LinkedProfileV1, PlatformIdentityV1,
    ProfileKindV1, ReceiptSchemaIdentityV1, TargetIdentityV1, VerifierIdentityV1, WorkerIdentityV1,
    ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_DOMAIN_V1, ASSEMBLY_MANIFEST_SCHEMA_VERSION,
    MAX_ASSEMBLY_ITEMS, MAX_ASSEMBLY_MANIFEST_BYTES, MAX_ASSEMBLY_STRING_BYTES,
};
pub use cache_entry::{
    CacheEntryError, CacheEntryV1, CacheKeyV1, CacheRootV1, CacheValueV1, CompletenessWitnessV1,
    OperatorIdentityV1, VerifierReceiptV1, CACHE_ENTRY_SCHEMA_V1,
};
pub use capability::{
    CapabilityMismatch, CapabilitySchema, CasLayout, FragmentBehavior, FragmentPolicy,
    HashAlgorithm, HashCapability, LayoutVersion, SharedCapability, SharedCasCapability,
};
pub use digest::{contract_digest, contract_digest_hex, sha256, sha256_hex};
pub use dispatch::{
    ApprovalGrant, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
    DispatchContractError, DispatchErrorClass, DispatchMachine, DispatchStage, EffectGrant,
    EffectPolicy, PermitGrant, PermitRequirement, RegistryEngine, SourceDiagnostic, SourceForm,
    ALL_DISPATCH_ERROR_CLASSES, CANONICAL_DISPATCH_VERSION,
};
pub use raw_worker::{
    decode_request_frame, decode_response_frame, encode_frame, raw_worker_protocol_digest_hex,
    raw_worker_protocol_manifest, validate_handshake_request, validate_request_frame,
    ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, EffectClass, EngineIdentity,
    FrameCodecError, HandshakeAck, HandshakeRequest, ProtocolLimits, RefOwnership, RevertMetadata,
    ShutdownRequest, SnapshotIdentity, WorkerBinding, WorkerCapabilities, WorkerError,
    WorkerRequestFrame, WorkerResponseFrame, WorkerResult, WorkerResultMetadata, WorkerTrace,
    DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};
pub use result::{
    ZeroResultAccessError, ZeroResultBuildError, ZeroResultV1, MAX_ACK_CHARS, MAX_PREVIEW_CHARS,
    ZERO_RESULT_V1,
};
pub use robust_snap::{
    robust_snap_contract_digest_v1, robust_snap_contract_manifest_v1,
    validate_heuristic_world_order, EvidenceDecisionTree, EvidenceLeafV1, EvidenceObservationV1,
    ProtectedEffectClassV1, ProtectedEffectSet, ProtectedEffectV1, RobustSnapCertificate,
    RobustSnapErrorV1, RobustSnapFailureCodeV1, SnapLevel, WorldFiberDescriptor,
    ROBUST_SNAP_CONTRACT_VERSION, ROBUST_SNAP_MAX_ASSUMPTIONS, ROBUST_SNAP_MAX_ASSUMPTION_BYTES,
    ROBUST_SNAP_MAX_EFFECTS, ROBUST_SNAP_MAX_EVIDENCE_DEPTH, ROBUST_SNAP_MAX_LEAVES,
    ROBUST_SNAP_MAX_WORLDS, ROBUST_SNAP_MODEL_VERSION,
};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
pub use telemetry::{TelemetryCounter, TelemetryOverflow, TelemetrySchema, ZeroTelemetryV1};
