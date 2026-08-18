//! Golden-freeze for WorkerResponseFrame NDJSON (zerostack-golden-response-frames-cznc).
//!
//! Locks byte-exact response bytes for HandshakeAck, Result, Error, CancelAck,
//! ShutdownAck. Uses existing public APIs only. No wall clocks, no random ids.
//! Default rows omit optional `engine_timeline` / `worker_token_accounting`
//! (absent-by-default). One extra row shows them present.

use serde_json::json;
use zero_abi::{
    decode_response_frame, encode_frame, ApprovalMetadata, ApprovalState, EffectClass,
    EngineIdentity, EngineStageSpan, EngineStageTimeline, HandshakeAck, ProtocolLimits,
    RefOwnership, RevertMetadata, WorkerBinding, WorkerCapabilities, WorkerError, WorkerResult,
    WorkerResultMetadata, WorkerResponseFrame, WorkerTrace, WorkerTokenAccounting,
    WorkerTokenCountKind, DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};

const FIXTURE_REVISION: &str = "fixture-revision";
const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

// Helper fixtures — all fixed, no now_ms / random.

fn fixture_binding() -> WorkerBinding {
    WorkerBinding {
        engine: EngineIdentity::FsZero,
        root: "/fixture/repo".to_string(),
        session_id: "session-1".to_string(),
        worker_revision: FIXTURE_REVISION.to_string(),
        semantic_contract_version: "1.0.0".to_string(),
        semantic_contract_digest: DIGEST_A.to_string(),
        operation_registry_digest: DIGEST_B.to_string(),
        ref_scheme: "ref-scheme-v1".to_string(),
    }
}

fn fixture_capabilities() -> WorkerCapabilities {
    WorkerCapabilities {
        cancellation: true,
        deadlines: true,
        approvals: true,
        revert: false,
        snapshots: false,
    }
}

fn fixture_limits() -> ProtocolLimits {
    ProtocolLimits {
        max_frame_bytes: DEFAULT_MAX_FRAME_BYTES as u64,
        max_output_bytes: 65_536,
        max_in_flight: 1,
        default_deadline_ms: 30_000,
    }
}

fn fixture_trace(request_id: &str) -> WorkerTrace {
    WorkerTrace {
        runtime_id: "runtime-1".to_string(),
        cell_id: "cell-1".to_string(),
        request_id: request_id.to_string(),
        trace_id: "trace-1".to_string(),
        parent_span_id: None,
        worker_revision: FIXTURE_REVISION.to_string(),
        contract_digest: DIGEST_A.to_string(),
    }
}

fn fixture_result_metadata(request_id: &str) -> WorkerResultMetadata {
    WorkerResultMetadata {
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
        trace: fixture_trace(request_id),
    }
}

fn fixture_handshake_ack_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::HandshakeAck {
        ack: HandshakeAck {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.to_string(),
            binding: fixture_binding(),
            capabilities: fixture_capabilities(),
            limits: fixture_limits(),
            protocol_digest: DIGEST_C.to_string(),
        },
    }
}

fn fixture_result_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::Result {
        request_id: "request-1".to_string(),
        result: WorkerResult {
            value: json!({"count":42,"data":"hello"}),
            metadata: fixture_result_metadata("request-1"),
        },
        engine_timeline: None,
        worker_token_accounting: None,
    }
}

fn fixture_result_frame_with_extras() -> WorkerResponseFrame {
    WorkerResponseFrame::Result {
        request_id: "request-1".to_string(),
        result: WorkerResult {
            value: json!({"count":42,"data":"hello"}),
            metadata: fixture_result_metadata("request-1"),
        },
        engine_timeline: Some(EngineStageTimeline {
            total_ns: 1_000,
            spans: vec![
                EngineStageSpan {
                    stage: "init".to_string(),
                    start_ns: 0,
                    duration_ns: 500,
                },
                EngineStageSpan {
                    stage: "run".to_string(),
                    start_ns: 500,
                    duration_ns: 500,
                },
            ],
        }),
        worker_token_accounting: Some(WorkerTokenAccounting {
            tokenizer_id: "test-tokenizer".to_string(),
            tokenizer_version_digest: None,
            count_kind: WorkerTokenCountKind::Exact,
            raw_tokens: 100,
            visible_tokens: 80,
            recovery_tokens: 10,
            billed_tokens: 100,
            cached_tokens: 10,
            exact_ref_tokens: None,
        }),
    }
}

fn fixture_error_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::Error {
        request_id: Some("request-1".to_string()),
        error: WorkerError::new("internal", "something failed", false).unwrap(),
        trace: Some(fixture_trace("request-1")),
        engine_timeline: None,
        worker_token_accounting: None,
    }
}

fn fixture_cancel_ack_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::CancelAck {
        request_id: "request-1".to_string(),
        cancelled: true,
    }
}

fn fixture_shutdown_ack_frame() -> WorkerResponseFrame {
    WorkerResponseFrame::ShutdownAck
}

// Golden bytes — byte-exact, canonical (definition-order struct fields, BTreeMap-sorted Value keys).
// Generated from encode_frame and frozen. A serde rename or default-field leak breaks these.

const HANDSHAKE_ACK_GOLDEN: &str = concat!(
    r#"{"kind":"handshake_ack","ack":{"protocol_version":"zerostack.raw_worker","binding":{"engine":"fszero","root":"/fixture/repo","session_id":"session-1","worker_revision":"fixture-revision","semantic_contract_version":"1.0.0","semantic_contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operation_registry_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","ref_scheme":"ref-scheme-v1"},"capabilities":{"cancellation":true,"deadlines":true,"approvals":true,"revert":false,"snapshots":false},"limits":{"max_frame_bytes":1048576,"max_output_bytes":65536,"max_in_flight":1,"default_deadline_ms":30000},"protocol_digest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}}"#,
    "\n"
);

const RESULT_GOLDEN: &str = concat!(
    r#"{"kind":"result","request_id":"request-1","result":{"value":{"count":42,"data":"hello"},"metadata":{"effect":"read_only","approval":{"state":"not_required"},"revert":{"supported":false},"ownership":{"engine":"fszero","session_id":"session-1","refs":[]},"trace":{"runtime_id":"runtime-1","cell_id":"cell-1","request_id":"request-1","trace_id":"trace-1","worker_revision":"fixture-revision","contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}}"#,
    "\n"
);

const RESULT_WITH_EXTRAS_GOLDEN: &str = concat!(
    r#"{"kind":"result","request_id":"request-1","result":{"value":{"count":42,"data":"hello"},"metadata":{"effect":"read_only","approval":{"state":"not_required"},"revert":{"supported":false},"ownership":{"engine":"fszero","session_id":"session-1","refs":[]},"trace":{"runtime_id":"runtime-1","cell_id":"cell-1","request_id":"request-1","trace_id":"trace-1","worker_revision":"fixture-revision","contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}},"engine_timeline":{"total_ns":1000,"spans":[{"stage":"init","start_ns":0,"duration_ns":500},{"stage":"run","start_ns":500,"duration_ns":500}]},"worker_token_accounting":{"tokenizer_id":"test-tokenizer","count_kind":"exact","raw_tokens":100,"visible_tokens":80,"recovery_tokens":10,"billed_tokens":100,"cached_tokens":10}}"#,
    "\n"
);

const ERROR_GOLDEN: &str = concat!(
    r#"{"kind":"error","request_id":"request-1","error":{"kind":"internal","message":"something failed","retryable":false},"trace":{"runtime_id":"runtime-1","cell_id":"cell-1","request_id":"request-1","trace_id":"trace-1","worker_revision":"fixture-revision","contract_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
    "\n"
);

const CANCEL_ACK_GOLDEN: &str = r#"{"kind":"cancel_ack","request_id":"request-1","cancelled":true}"#;
const CANCEL_ACK_GOLDEN_NL: &str = concat!(r#"{"kind":"cancel_ack","request_id":"request-1","cancelled":true}"#, "\n");

const SHUTDOWN_ACK_GOLDEN: &str = r#"{"kind":"shutdown_ack"}"#;
const SHUTDOWN_ACK_GOLDEN_NL: &str = concat!(r#"{"kind":"shutdown_ack"}"#, "\n");

fn assert_golden(frame: &WorkerResponseFrame, expected: &str) {
    let encoded = encode_frame(frame, DEFAULT_MAX_FRAME_BYTES).expect("encode golden");
    let encoded_str = String::from_utf8(encoded.clone()).expect("utf8");
    // Exact byte match (including trailing newline)
    assert_eq!(encoded_str, expected, "golden mismatch for {:?}", frame);
    // Round-trip
    let decoded = decode_response_frame(encoded_str.as_bytes(), DEFAULT_MAX_FRAME_BYTES).expect("decode golden");
    assert_eq!(&decoded, frame);
    // Also decode the expected literal directly
    let decoded2 = decode_response_frame(expected.as_bytes(), DEFAULT_MAX_FRAME_BYTES).expect("decode literal");
    assert_eq!(&decoded2, frame);
}

#[test]
fn handshake_ack_is_golden() {
    let frame = fixture_handshake_ack_frame();
    assert_golden(&frame, HANDSHAKE_ACK_GOLDEN);
    // Also check that a flipped bit fails
    let tampered = HANDSHAKE_ACK_GOLDEN.replace("fszero", "graphzero");
    let res = decode_response_frame(tampered.as_bytes(), DEFAULT_MAX_FRAME_BYTES).expect("tampered still parses");
    assert_ne!(res, frame);
}

#[test]
fn result_is_golden_and_omits_optional_fields() {
    let frame = fixture_result_frame();
    assert_golden(&frame, RESULT_GOLDEN);
    let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap();
    let s = String::from_utf8(encoded).unwrap();
    assert!(!s.contains("engine_timeline"), "default Result must omit engine_timeline");
    assert!(!s.contains("worker_token_accounting"), "default Result must omit worker_token_accounting");
}

#[test]
fn result_with_extras_is_golden_and_includes_optional_fields() {
    let frame = fixture_result_frame_with_extras();
    assert_golden(&frame, RESULT_WITH_EXTRAS_GOLDEN);
    let s = String::from_utf8(encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap()).unwrap();
    assert!(s.contains("engine_timeline"));
    assert!(s.contains("worker_token_accounting"));
}

#[test]
fn error_is_golden_and_omits_optional_fields_by_default() {
    let frame = fixture_error_frame();
    assert_golden(&frame, ERROR_GOLDEN);
    let s = String::from_utf8(encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap()).unwrap();
    assert!(!s.contains("engine_timeline"));
    assert!(!s.contains("worker_token_accounting"));
    // error.kind closed set: unknown kind must be rejected
    let bad = r#"{"kind":"error","error":{"kind":"potato","message":"x","retryable":false}}"#;
    assert!(decode_response_frame(bad.as_bytes(), DEFAULT_MAX_FRAME_BYTES).is_err());
}

#[test]
fn cancel_ack_is_golden() {
    let frame = fixture_cancel_ack_frame();
    assert_golden(&frame, CANCEL_ACK_GOLDEN_NL);
    // Round-trip via CancelAck
    let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap();
    assert_eq!(String::from_utf8(encoded).unwrap(), CANCEL_ACK_GOLDEN_NL);
    let flipped = CANCEL_ACK_GOLDEN.replace("true", "false");
    let decoded = decode_response_frame(flipped.as_bytes(), DEFAULT_MAX_FRAME_BYTES).unwrap();
    assert_ne!(decoded, frame);
}

#[test]
fn shutdown_ack_is_golden_and_rejects_unknown_fields() {
    let frame = fixture_shutdown_ack_frame();
    assert_golden(&frame, SHUTDOWN_ACK_GOLDEN_NL);
    // Serde's internally tagged unit variant would silently accept extra fields;
    // reject_shutdown_ack_unknown_fields must fail.
    let extra = r#"{"kind":"shutdown_ack","extra":1}"#;
    let res = decode_response_frame(extra.as_bytes(), DEFAULT_MAX_FRAME_BYTES);
    assert!(res.is_err(), "shutdown_ack with extra field must be rejected, got {:?}", res);
    // Also empty and TooLarge still handled (smoke)
    assert!(decode_response_frame(b"", DEFAULT_MAX_FRAME_BYTES).is_err());
    let too_large = vec![b'a'; DEFAULT_MAX_FRAME_BYTES + 1];
    assert!(matches!(
        decode_response_frame(&too_large, DEFAULT_MAX_FRAME_BYTES),
        Err(zero_abi::FrameCodecError::TooLarge { .. })
    ));
}
