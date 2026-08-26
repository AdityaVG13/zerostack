//! Unstructured-byte crash oracle for raw-worker frames (zerostack-raw-worker-unstructured-bytes-ci-c4j6).
//! Inspection 2026-08-17: decode_*_frame are panic-free (no unwrap/expect on untrusted
//! bytes, checked arithmetic, bounded JSON parse); no production fix required.
//!
//! Feeds random `&[u8]` with size guards into both decoders. Must never panic.
//! Keeps the typed Call proptest as the structure-aware oracle; this file is only
//! the untrusted-bytes boundary check. No fuzz crate, no harness.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use serde_json::json;
use zero_abi::{
    ApprovalMetadata, ApprovalState, DEFAULT_MAX_FRAME_BYTES, EffectClass, EngineIdentity,
    HandshakeRequest, RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata,
    WorkerRequestFrame, WorkerResponseFrame, WorkerResult, WorkerResultMetadata, WorkerTrace,
    decode_request_frame, decode_response_frame, encode_frame,
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
    fn unstructured_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        // Single merged no-panic property covering small and default limits (was two duplicate tests).
        let _ = decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_response_frame(&bytes, DEFAULT_MAX_FRAME_BYTES);
        let _ = decode_request_frame(&bytes, 64);
        let _ = decode_response_frame(&bytes, 64);
        let _ = decode_request_frame(&bytes, 8);
        let _ = decode_response_frame(&bytes, 8);
    }
}

#[test]
fn truncated_json_is_rejected_but_terminal_newline_is_optional() {
    let frame = valid_result_frame();
    let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).expect("encode valid result");
    assert_eq!(encoded.last(), Some(&b'\n'));
    let response_json_len = encoded.len() - 1;
    for len in 0..response_json_len {
        let truncated = &encoded[..len];
        assert!(
            decode_response_frame(truncated, DEFAULT_MAX_FRAME_BYTES).is_err(),
            "response JSON truncated at {len} bytes must be rejected"
        );
        assert!(
            decode_request_frame(truncated, DEFAULT_MAX_FRAME_BYTES).is_err(),
            "truncated response parsed as request at {len} bytes"
        );
    }
    assert!(
        decode_response_frame(&encoded[..response_json_len], DEFAULT_MAX_FRAME_BYTES).is_ok(),
        "the terminal newline is framing convenience, not part of JSON"
    );
    let req = WorkerRequestFrame::Handshake {
        request: HandshakeRequest {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.to_string(),
            root: "/fixture".into(),
            session_id: "session-1".into(),
            expected_engine: EngineIdentity::FsZero,
            expected_worker_revision: Some("fixture-revision".into()),
            expected_contract_digest:
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            expected_registry_digest: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            ),
        },
    };
    let req_encoded = encode_frame(&req, DEFAULT_MAX_FRAME_BYTES).expect("encode handshake");
    assert_eq!(req_encoded.last(), Some(&b'\n'));
    let request_json_len = req_encoded.len() - 1;
    for len in 0..request_json_len {
        assert!(
            decode_request_frame(&req_encoded[..len], DEFAULT_MAX_FRAME_BYTES).is_err(),
            "request JSON truncated at {len} bytes must be rejected"
        );
    }
    assert!(
        decode_request_frame(&req_encoded[..request_json_len], DEFAULT_MAX_FRAME_BYTES).is_ok(),
        "the terminal newline is optional"
    );
}

#[test]
fn valid_result_fixture_round_trips() {
    // Fixed pinned wire bytes (independent fixture) for a default Result frame.
    // This is RESULT_GOLDEN without relying on encode-then-decode of same impl.
    const PINNED_RESULT_JSON: &str = r#"{"kind":"result","request_id":"request-1","result":{"value":{"count":42,"data":"hello"},"metadata":{"effect":"read_only","approval":{"state":"not_required"},"revert":{"supported":false},"ownership":{"engine":"fszero","session_id":"session-1","refs":[]},"trace":{"runtime_id":"runtime-1","cell_id":"cell-1","request_id":"request-1","trace_id":"trace-1","worker_revision":"fixture-revision","contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}}"#;
    let pinned_bytes = format!("{PINNED_RESULT_JSON}\n");
    let decoded = decode_response_frame(pinned_bytes.as_bytes(), DEFAULT_MAX_FRAME_BYTES)
        .expect("decode pinned fixture");
    let expected = valid_result_frame();
    assert_eq!(
        decoded, expected,
        "pinned wire bytes must decode to expected frame"
    );
    // Encoding the expected frame must reproduce the pinned bytes exactly (canonical wire compat).
    let reencoded = encode_frame(&expected, DEFAULT_MAX_FRAME_BYTES).expect("encode");
    assert_eq!(String::from_utf8(reencoded).unwrap(), pinned_bytes);
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
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_string(),
                },
            },
        },
        engine_timeline: None,
        worker_token_accounting: None,
    }
}
