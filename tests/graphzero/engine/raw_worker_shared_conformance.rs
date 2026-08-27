//! Pinned ZeroStack raw-worker-v2 parser/serializer parity for GraphZero.

use graphzero_engine::dispatcher::{AdapterKind, EngineContext};
use graphzero_engine::surface_handshake::raw_worker;
use serde_json::{Value, json};
use zero_abi::{EngineIdentity, WorkerRequestFrame, WorkerResponseFrame};

fn trace(request_id: &str, worker_revision: &str, contract_digest: &str) -> Value {
    json!({
        "runtime_id":"runtime-1",
        "cell_id":"cell-1",
        "request_id":request_id,
        "trace_id":"trace-1",
        "worker_revision":worker_revision,
        "contract_digest":contract_digest,
    })
}

#[test]
fn graph_adapter_uses_shared_request_parser_and_serializer() {
    let digest = "a".repeat(64);
    let call_trace = trace("request-1", "revision-1", &digest);
    let requests = [
        json!({
            "kind":"handshake",
            "request":{
                "protocol_version":zero_abi::RAW_WORKER_PROTOCOL_VERSION,
                "root":"/repo",
                "session_id":"session-1",
                "expected_engine":"graphzero",
                "expected_worker_revision":"revision-1",
                "expected_contract_digest":digest,
                "expected_registry_digest":"b".repeat(64),
            }
        }),
        json!({
            "kind":"call",
            "request":{
                "request_id":"request-1",
                "op":"orient",
                "args":{"query":"Widget"},
                "deadline_unix_ms":4_102_444_800_000_u64,
                "trace":call_trace,
                "approval_grant":{
                    "grant_id":"grant-1",
                    "engine":"graphzero",
                    "root":"/repo",
                    "session_id":"session-1",
                    "request_id":"request-1",
                    "operation":"orient",
                    "effect":"read_only",
                    "authority_digest":"c".repeat(64),
                    "policy_digest":"d".repeat(64),
                    "issued_at_unix_ms":1,
                    "expires_at_unix_ms":4_102_444_800_000_u64,
                },
                "telemetry_request":{
                    "engine_stage_timeline":false,
                    "worker_token_accounting":false,
                }
            }
        }),
        json!({"kind":"cancel","request":{"request_id":"request-1","reason":"done"}}),
        json!({"kind":"shutdown","request":{"reason":"fixture complete"}}),
    ];

    for request in requests {
        let bytes = serde_json::to_vec(&request).unwrap();
        let shared = zero_abi::decode_request_frame(&bytes, zero_abi::DEFAULT_MAX_FRAME_BYTES)
            .expect("shared parser");
        let adapter = raw_worker::decode_request_frame(&bytes, raw_worker::DEFAULT_MAX_FRAME_BYTES)
            .expect("GraphZero adapter parser");
        assert_eq!(adapter, shared);
        assert_eq!(
            raw_worker::encode_frame(&adapter, raw_worker::DEFAULT_MAX_FRAME_BYTES).unwrap(),
            zero_abi::encode_frame(&shared, zero_abi::DEFAULT_MAX_FRAME_BYTES).unwrap()
        );
    }

    for invalid in [
        br#"{"kind":"cancel","request":{"request_id":""}}"#.as_slice(),
        br#"{"kind":"shutdown","request":{"reason":"ok"},"extra":true}"#.as_slice(),
    ] {
        let shared = zero_abi::decode_request_frame(invalid, zero_abi::DEFAULT_MAX_FRAME_BYTES)
            .expect_err("shared parser must reject mutant");
        let adapter = raw_worker::decode_request_frame(invalid, raw_worker::DEFAULT_MAX_FRAME_BYTES)
            .expect_err("adapter must reject mutant");
        assert_eq!(adapter.kind(), shared.kind());
        assert_eq!(adapter.to_string(), shared.to_string());
    }
}

#[test]
fn graph_adapter_uses_shared_response_parser_and_serializer() {
    let digest = "a".repeat(64);
    let result_trace = trace("request-1", "revision-1", &digest);
    let timeline = json!({
        "total_ns":300,
        "spans":[
            {"stage":"decode","start_ns":0,"duration_ns":100},
            {"stage":"dispatch","start_ns":100,"duration_ns":200}
        ]
    });
    let accounting = json!({
        "tokenizer_id":"fixture-tokenizer-v1",
        "count_kind":"exact",
        "raw_tokens":8,
        "visible_tokens":4,
        "recovery_tokens":0,
        "billed_tokens":8,
        "cached_tokens":2,
        "exact_ref_tokens":0
    });
    let responses = [
        json!({
            "kind":"handshake_ack",
            "ack":{
                "protocol_version":zero_abi::RAW_WORKER_PROTOCOL_VERSION,
                "binding":{
                    "engine":"graphzero",
                    "root":"/repo",
                    "session_id":"session-1",
                    "worker_revision":"revision-1",
                    "semantic_contract_version":"graphzero.raw.v1",
                    "semantic_contract_digest":digest,
                    "operation_registry_digest":"b".repeat(64),
                    "ref_scheme":"gz://"
                },
                "capabilities":{
                    "cancellation":false,
                    "deadlines":true,
                    "approvals":false,
                    "revert":false,
                    "snapshots":false
                },
                "limits":{
                    "max_frame_bytes":1_048_576,
                    "max_output_bytes":65_536,
                    "max_in_flight":1,
                    "default_deadline_ms":30_000
                },
                "protocol_digest":zero_abi::raw_worker_protocol_digest_hex()
            }
        }),
        json!({
            "kind":"result",
            "request_id":"request-1",
            "result":{
                "value":{"status":"ok"},
                "metadata":{
                    "effect":"read_only",
                    "approval":{"state":"not_required"},
                    "revert":{"supported":false},
                    "ownership":{
                        "engine":"graphzero",
                        "session_id":"session-1",
                        "refs":[format!("gz://blob/{}", "0".repeat(64))]
                    },
                    "trace":result_trace
                }
            },
            "engine_timeline":timeline,
            "worker_token_accounting":accounting
        }),
        json!({
            "kind":"error",
            "request_id":"request-1",
            "error":{"kind":"validation","message":"invalid fixture","retryable":false},
            "trace":trace("request-1", "revision-1", &"a".repeat(64))
        }),
        json!({"kind":"cancel_ack","request_id":"request-1","cancelled":false}),
        json!({"kind":"shutdown_ack"}),
    ];

    for response in responses {
        let bytes = serde_json::to_vec(&response).unwrap();
        let shared = zero_abi::decode_response_frame(&bytes, zero_abi::DEFAULT_MAX_FRAME_BYTES)
            .expect("shared response parser");
        let adapter = raw_worker::decode_response_frame(&bytes, raw_worker::DEFAULT_MAX_FRAME_BYTES)
            .expect("GraphZero adapter response parser");
        assert_eq!(adapter, shared);
        assert_eq!(
            raw_worker::encode_frame(&adapter, raw_worker::DEFAULT_MAX_FRAME_BYTES).unwrap(),
            zero_abi::encode_frame(&shared, zero_abi::DEFAULT_MAX_FRAME_BYTES).unwrap()
        );
    }

    for invalid in [
        br#"{"kind":"error","error":{"kind":"","message":"bad","retryable":false}}"#.as_slice(),
        br#"{"kind":"cancel_ack","request_id":"request-1","cancelled":false,"extra":true}"#
            .as_slice(),
        br#"{"kind":"shutdown_ack","extra":true}"#.as_slice(),
    ] {
        let shared = zero_abi::decode_response_frame(invalid, zero_abi::DEFAULT_MAX_FRAME_BYTES)
            .expect_err("shared response parser must reject mutant");
        let adapter = raw_worker::decode_response_frame(invalid, raw_worker::DEFAULT_MAX_FRAME_BYTES)
            .expect_err("adapter response parser must reject mutant");
        assert_eq!(adapter.kind(), shared.kind());
        assert_eq!(adapter.to_string(), shared.to_string());
    }
}

#[test]
fn live_adapter_emits_hub_decodable_request_gated_timelines() {
    let temp = tempfile::tempdir().expect("timeline root");
    let repo = temp.path().join("repo");
    let store = temp.path().join("store");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    let ctx = EngineContext::for_paths(repo, store, AdapterKind::PrivateWorker);
    let mut worker = raw_worker::RawWorker::new("/repo", "session-1");
    let binding = worker.binding().clone();
    let handshake = WorkerRequestFrame::Handshake {
        request: zero_abi::HandshakeRequest {
            protocol_version: zero_abi::RAW_WORKER_PROTOCOL_VERSION.into(),
            root: binding.root.clone(),
            session_id: binding.session_id.clone(),
            expected_engine: EngineIdentity::GraphZero,
            expected_worker_revision: Some(binding.worker_revision.clone()),
            expected_contract_digest: binding.semantic_contract_digest.clone(),
            expected_registry_digest: Some(binding.operation_registry_digest.clone()),
        },
    };
    let response = worker.handle_line(
        &ctx,
        &zero_abi::encode_frame(&handshake, zero_abi::DEFAULT_MAX_FRAME_BYTES).unwrap(),
    );
    let ack = zero_abi::decode_response_frame(&response, zero_abi::DEFAULT_MAX_FRAME_BYTES)
        .expect("hub decodes GraphZero handshake");
    match ack {
        WorkerResponseFrame::HandshakeAck { ack } => {
            assert_eq!(ack.binding.engine, EngineIdentity::GraphZero);
            assert_eq!(ack.binding.ref_scheme, "gz://");
            assert_eq!(
                ack.protocol_digest,
                zero_abi::raw_worker_protocol_digest_hex()
            );
            assert_eq!(
                serde_json::to_value(&ack.capabilities)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .len(),
                5
            );
        }
        other => panic!("expected handshake_ack, got {other:?}"),
    }

    let call = json!({
        "kind":"call",
        "request":{
            "request_id":"request-1",
            "op":"not_a_real_op",
            "args":{},
            "trace":trace(
                "request-1",
                &binding.worker_revision,
                &binding.semantic_contract_digest
            ),
            "approval_grant":null,
            "telemetry_request":{
                "engine_stage_timeline":false,
                "worker_token_accounting":true
            }
        }
    });
    let response = worker.handle_line(&ctx, &serde_json::to_vec(&call).unwrap());
    match zero_abi::decode_response_frame(&response, zero_abi::DEFAULT_MAX_FRAME_BYTES)
        .expect("hub decodes GraphZero error")
    {
        WorkerResponseFrame::Error {
            request_id,
            error,
            engine_timeline,
            worker_token_accounting,
            ..
        } => {
            assert_eq!(request_id.as_deref(), Some("request-1"));
            assert_eq!(error.kind, "validation");
            assert!(engine_timeline.is_none());
            assert!(worker_token_accounting.is_none());
        }
        other => panic!("expected typed untimed error, got {other:?}"),
    }

    let timed_error = json!({
        "kind":"call",
        "request":{
            "request_id":"timed-error",
            "op":"not_a_real_op",
            "args":{},
            "trace":trace(
                "timed-error",
                &binding.worker_revision,
                &binding.semantic_contract_digest
            ),
            "telemetry_request":{
                "engine_stage_timeline":true,
                "worker_token_accounting":true
            }
        }
    });
    let response = worker.handle_line(&ctx, &serde_json::to_vec(&timed_error).unwrap());
    match zero_abi::decode_response_frame(&response, zero_abi::DEFAULT_MAX_FRAME_BYTES)
        .expect("hub decodes timed GraphZero error")
    {
        WorkerResponseFrame::Error {
            engine_timeline: Some(timeline),
            worker_token_accounting,
            ..
        } => {
            zero_abi::validate_engine_stage_timeline(&timeline).unwrap();
            assert_eq!(timeline.spans[0].stage, "graphzero.raw_worker_call");
            assert!(worker_token_accounting.is_none());
        }
        other => panic!("expected typed timed error, got {other:?}"),
    }

    let timed_success = json!({
        "kind":"call",
        "request":{
            "request_id":"timed-success",
            "op":"ctx_ref",
            "args":{"value":{"proof":true}},
            "trace":trace(
                "timed-success",
                &binding.worker_revision,
                &binding.semantic_contract_digest
            ),
            "telemetry_request":{
                "engine_stage_timeline":true,
                "worker_token_accounting":true
            }
        }
    });
    let response = worker.handle_line(&ctx, &serde_json::to_vec(&timed_success).unwrap());
    match zero_abi::decode_response_frame(&response, zero_abi::DEFAULT_MAX_FRAME_BYTES)
        .expect("hub decodes timed GraphZero result")
    {
        WorkerResponseFrame::Result {
            engine_timeline: Some(timeline),
            worker_token_accounting,
            ..
        } => {
            zero_abi::validate_engine_stage_timeline(&timeline).unwrap();
            assert_eq!(timeline.spans[0].stage, "graphzero.raw_worker_call");
            assert!(worker_token_accounting.is_none());
        }
        other => panic!("expected typed timed result, got {other:?}"),
    }
}
