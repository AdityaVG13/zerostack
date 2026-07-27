use proptest::prelude::*;
use serde_json::{json, Map, Value};
use zero_abi::{
    decode_request_frame, encode_frame, raw_worker_protocol_digest_hex, CallRequest,
    WorkerRequestFrame, WorkerTrace, DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION,
};
use zerostack_codemode_conformance::schema::{validate_against_schema, SchemaName};

fn fixture_frames() -> Vec<Value> {
    serde_json::from_str(include_str!("../fixtures/raw_worker_v2_frames.json"))
        .expect("raw-worker v2 fixtures parse")
}

#[test]
fn golden_frames_match_schema_and_shared_rust_types() {
    for frame in fixture_frames() {
        validate_against_schema(SchemaName::RawWorkerV2, &frame)
            .expect("fixture matches canonical schema");
        let bytes = serde_json::to_vec(&frame).unwrap();
        let decoded = decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES)
            .expect("fixture matches canonical Rust frame");
        let encoded = encode_frame(&decoded, DEFAULT_MAX_FRAME_BYTES).unwrap();
        let round_trip: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(round_trip, frame);
    }
    assert_eq!(RAW_WORKER_PROTOCOL_VERSION, "zerostack.raw_worker.v2");
    assert_eq!(raw_worker_protocol_digest_hex().len(), 64);
}

proptest! {
    #[test]
    fn arbitrary_typed_calls_round_trip_without_contract_drift(
        op in "[a-z][a-z0-9_.]{0,63}",
        fields in prop::collection::btree_map("[a-z]{1,12}", any::<i64>(), 0..16),
        deadline in prop::option::of(1_u64..4_102_444_800_000_u64),
    ) {
        let args = Value::Object(fields.into_iter().map(|(key, value)| (key, json!(value))).collect::<Map<_, _>>());
        let trace = WorkerTrace {
            runtime_id: "runtime-fuzz".into(),
            cell_id: "cell-fuzz".into(),
            request_id: "request-fuzz".into(),
            trace_id: "trace-fuzz".into(),
            parent_span_id: None,
            worker_revision: "revision-fuzz".into(),
            contract_digest: "c".repeat(64),
        };
        let frame = WorkerRequestFrame::Call {
            request: CallRequest {
                request_id: "request-fuzz".into(),
                op,
                args,
                deadline_unix_ms: deadline,
                trace,
            },
        };
        let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap();
        let value: Value = serde_json::from_slice(&encoded).unwrap();
        validate_against_schema(SchemaName::RawWorkerV2, &value).unwrap();
        prop_assert_eq!(decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(), frame);
    }
}
