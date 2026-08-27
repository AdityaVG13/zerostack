//! FSZero binding to the hub-owned raw-worker v2 contract.
//!
//! ZeroStack owns every wire type, validator, frame codec, manifest, and
//! protocol digest. FSZero has no parallel protocol or digest authority.

pub use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, CancelRequest, DEFAULT_MAX_FRAME_BYTES,
    EffectClass, EngineIdentity, FrameCodecError, HandshakeAck, HandshakeRequest, ProtocolLimits,
    RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata, ShutdownRequest, SnapshotIdentity,
    WorkerBinding, WorkerCapabilities, WorkerError, WorkerRequestFrame, WorkerResponseFrame,
    WorkerResult, WorkerResultMetadata, WorkerTrace, decode_request_frame, decode_response_frame,
    encode_frame, raw_worker_protocol_digest_hex, raw_worker_protocol_manifest,
    validate_handshake_request,
};

#[cfg(test)]
#[path = "../../../../tests/fszero/unit/fszero-core/raw_worker_protocol_tests.rs"]
mod tests;
