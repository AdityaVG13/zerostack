//! Unstructured-byte crash oracle for raw-worker frames (zerostack-raw-worker-unstructured-bytes-ci-c4j6).
//!
//! Feeds random `&[u8]` with size guards into both decoders. Must never panic.
//! Keeps the typed Call proptest as the structure-aware oracle; this file is only
//! the untrusted-bytes boundary check. No fuzz crate, no harness.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use serde_json::json;
use zero_abi::{
    decode_request_frame, decode_response_frame, encode_frame, ApprovalMetadata, ApprovalState,
    EffectClass, EngineIdentity, HandshakeRequest, RefOwnership, RevertMetadata,
    WorkerRequestFrame, WorkerResponseFrame, WorkerResult, WorkerResultMetadata, WorkerTrace,
    DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 256 },
        failure_persistence: if cfg!(miri) {
            None
        } else {
            Some(Box::new(FileFailurePersistence::WithSource(
                "proptest-regressions",
            )))
        },
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn unstructured_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        // DEFAULT_MAX guard + smaller selected max (64 and 8) per bead.
        let _ = decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_response_frame(&bytes, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_request_frame(&bytes, 64);
        let _ = decode_response_frame(&bytes, 64);
        let _ = decode_request_frame(&bytes, 8);
        let _ = decode_response_frame(&bytes, 8);
    }

    #[test]
    fn unstructured_bytes_with_larger_limit_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let capped = if bytes.len() > DEFAULT_MAX_FRAME_BYTES {
            &bytes[..DEFAULT_MAX_FRAME_BYTES]
        } else {
            &bytes[..]
        };
        let _ = decode_request_frame(capped, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_response_frame(capped, DEFAULT_MAX_FRAME_BYTES);
    }
}

#[test]
fn truncated_frame_does_not_panic() {
    // Build a valid Result frame, truncate, and ensure decode returns error not panic.
    let frame = valid_result_frame();
    let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).expect("encode valid result");
    // Truncated variants: 0, 1, half, minus one
    for len in [0usize, 1, encoded.len() / 2, encoded.len().saturating_sub(1)] {
        let truncated = &encoded[..len.min(encoded.len())];
        let _ = decode_response_frame(truncated, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_response_frame(truncated, 64);
        let _ = decode_request_frame(truncated, DEFAULT_MAX_FRAME_BYTES);
    }
    // Also truncated request frame
    let req = WorkerRequestFrame::Handshake {
        request: HandshakeRequest {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.to_string(),
            root: "/fixture/repo".to_string(),
            session_id: "session-1".to_string(),
            expected_engine: EngineIdentity::FsZero,
            expected_worker_revision: None,
            expected_contract_digest:
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            expected_registry_digest: None,
        },
    };
    let req_encoded = encode_frame(&req, DEFAULT_MAX_FRAME_BYTES).expect("encode handshake");
    let truncated = &req_encoded[..req_encoded.len() / 2];
    let _ = decode_request_frame(truncated, DEFAULT_MAX_FRAME_BYTES);
}

#[test]
fn valid_result_fixture_round_trips() {
    let frame = valid_result_frame();
    let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).expect("encode");
    let decoded = decode_response_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).expect("decode");
    assert_eq!(decoded, frame);
    // Also decode via fixture bytes (not mocked, real encode output)
    let fixture_bytes = encoded.clone();
    let decoded2 = decode_response_frame(&fixture_bytes, DEFAULT_MAX_FRAME_BYTES).expect("fixture decode");
    assert_eq!(decoded2, frame);
}

fn valid_result_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::Result {
        request_id: "request-1".to_string(),
        result: WorkerResult {
            value: json!({"count":42,"data":"hello"}),
            metadata: WorkerResultMetadata {
                effect: EffectClass::ReadOnly,
                approval: ApprovalMetadata {
                    state: ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
                revert: RevertMetadata {
                    supported: false,
                    journal_id: None,
                    rollback_op: None,
                },
                ownership: RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: "session-1".to_string(),
                    refs: vec![],
                    snapshot: None,
                },
                trace: WorkerTrace {
                    runtime_id: "runtime-1".to_string(),
                    cell_id: "cell-1".to_string(),
                    request_id: "request-1".to_string(),
                    trace_id: "trace-1".to_string(),
                    parent_span_id: None,
                    worker_revision: "fixture-revision".to_string(),
                    contract_digest:
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                },
            },
        },
        engine_timeline: None,
        worker_token_accounting: None,
    }
}
