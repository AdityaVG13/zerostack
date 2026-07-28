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

pub mod digest;
pub mod dispatch;
pub mod raw_worker;
pub mod schema;

pub use digest::{contract_digest, contract_digest_hex, sha256, sha256_hex};
pub use dispatch::{
    ApprovalGrant, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
    DispatchContractError, DispatchErrorClass, DispatchMachine, DispatchStage, EffectGrant,
    EffectPolicy, PermitGrant, PermitRequirement, RegistryEngine, SourceDiagnostic, SourceForm,
    ALL_DISPATCH_ERROR_CLASSES, CANONICAL_DISPATCH_VERSION,
};
pub use raw_worker::{
    decode_request_frame, encode_frame, raw_worker_protocol_digest_hex,
    raw_worker_protocol_manifest, validate_handshake_request, ApprovalMetadata, ApprovalState,
    CallRequest, CancelRequest, EffectClass, FrameCodecError, HandshakeAck, HandshakeRequest,
    ProtocolLimits, RefOwnership, RevertMetadata, ShutdownRequest, SnapshotIdentity, WorkerBinding,
    WorkerCapabilities, WorkerError, WorkerRequestFrame, WorkerResponseFrame, WorkerResult,
    WorkerResultMetadata, WorkerTrace, DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};
pub use schema::{
    canonical_json, canonical_schema_json, normalize_schema, schema_diff, schema_fingerprint_hex,
    schema_property_keys, schema_required_keys, schemas_structurally_equal,
};
