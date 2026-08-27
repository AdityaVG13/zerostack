//! FSZero-specific tests plus the shared ZeroStack test contract.

use zero_abi::{DEFAULT_MAX_FRAME_BYTES, FrameCodecError, WorkerResponseFrame};

/// Decode non-empty NDJSON responses through the canonical raw-worker codec.
///
/// The hub `zero-testkit` crate that used to provide this helper was
/// liquidated from the hub (ZeroStack a38fc4a); the codec itself lives in
/// hub `zero-abi` and remains shared.
pub fn decode_worker_transcript(bytes: &[u8]) -> Result<Vec<WorkerResponseFrame>, FrameCodecError> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| zero_abi::decode_response_frame(line, DEFAULT_MAX_FRAME_BYTES))
        .collect()
}
