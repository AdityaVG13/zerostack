use proptest::prelude::*;
use serde_json::{Map, Value, json};
use zero_abi::{
    CallRequest, DEFAULT_MAX_FRAME_BYTES, RAW_WORKER_PROTOCOL_VERSION, WorkerRequestFrame,
    WorkerTrace, decode_request_frame, encode_frame, raw_worker_protocol_digest_hex,
};
use zerostack_shared_tests::schema::{SchemaName, validate_against_schema};

fn fixture_frames() -> Vec<Value> {
    serde_json::from_str(include_str!("../../fixtures/raw_worker_v2_frames.json"))
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
                approval_grant: None,
                telemetry_request: None,
            },
        };
        let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap();
        let value: Value = serde_json::from_slice(&encoded).unwrap();
        validate_against_schema(SchemaName::RawWorkerV2, &value).unwrap();
        prop_assert_eq!(decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(), frame);
    }
}

fn approval_call(engine: &str) -> CallRequest {
    serde_json::from_value(json!({
        "request_id": "req-approval",
        "op": "fs.write",
        "args": {},
        "trace": {
            "runtime_id": "runtime", "cell_id": "cell", "request_id": "req-approval",
            "trace_id": "trace", "worker_revision": "rev",
            "contract_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        },
        "approval_grant": {
            "grant_id": "grant-1", "engine": engine, "root": "/workspace",
            "session_id": "session", "request_id": "req-approval", "operation": "fs.write",
            "effect": "approval_required_mutation",
            "authority_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "policy_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "issued_at_unix_ms": 100, "expires_at_unix_ms": 200
        }
    }))
    .unwrap()
}

#[test]
fn engine_identities_are_closed_and_canonical() {
    for canonical in ["fszero", "graphzero", "tokenzero"] {
        let request = approval_call(canonical);
        assert_eq!(
            serde_json::to_value(request).unwrap()["approval_grant"]["engine"],
            canonical
        );
    }
    assert!(
        serde_json::from_value::<CallRequest>(
            serde_json::to_value(approval_call("fszero")).unwrap()
        )
        .is_ok()
    );
    let mut unknown = serde_json::to_value(approval_call("fszero")).unwrap();
    unknown["approval_grant"]["engine"] = json!("futurezero");
    assert!(serde_json::from_value::<CallRequest>(unknown).is_err());
}

#[test]
fn approval_grant_binding_expiry_effect_digest_and_replay_are_enforced() {
    use std::collections::BTreeSet;
    let request = approval_call("fszero");
    let grant = request.approval_grant.as_ref().unwrap();
    let engine = grant.engine;
    let effect = grant.effect;
    let mut consumed = BTreeSet::new();
    assert!(
        request
            .validate_approval_grant(engine, "/workspace", "session", effect, 150, &mut consumed)
            .is_ok()
    );
    assert_eq!(
        format!(
            "{:?}",
            request
                .validate_approval_grant(
                    engine,
                    "/workspace",
                    "session",
                    effect,
                    150,
                    &mut consumed
                )
                .unwrap_err()
        ),
        "Replayed"
    );

    let mut missing = serde_json::to_value(&request).unwrap();
    missing.as_object_mut().unwrap().remove("approval_grant");
    let missing: CallRequest = serde_json::from_value(missing).unwrap();
    assert_eq!(
        format!(
            "{:?}",
            missing
                .validate_approval_grant(
                    engine,
                    "/workspace",
                    "session",
                    effect,
                    150,
                    &mut BTreeSet::new()
                )
                .unwrap_err()
        ),
        "Missing"
    );
    assert_eq!(
        format!(
            "{:?}",
            request
                .validate_approval_grant(
                    engine,
                    "/other",
                    "session",
                    effect,
                    150,
                    &mut BTreeSet::new()
                )
                .unwrap_err()
        ),
        "BindingMismatch"
    );
    assert_eq!(
        format!(
            "{:?}",
            request
                .validate_approval_grant(
                    engine,
                    "/workspace",
                    "session",
                    effect,
                    200,
                    &mut BTreeSet::new()
                )
                .unwrap_err()
        ),
        "Expired"
    );

    for (field, value, expected) in [
        ("effect", json!("read_only"), "WrongEffect"),
        ("policy_digest", json!("ABC"), "Malformed"),
    ] {
        let mut value_request = serde_json::to_value(&request).unwrap();
        value_request["approval_grant"][field] = value;
        let parsed: CallRequest = serde_json::from_value(value_request).unwrap();
        assert_eq!(
            format!(
                "{:?}",
                parsed
                    .validate_approval_grant(
                        engine,
                        "/workspace",
                        "session",
                        effect,
                        150,
                        &mut BTreeSet::new()
                    )
                    .unwrap_err()
            ),
            expected
        );
    }
}
