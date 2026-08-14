    use super::*;

    fn trace() -> WorkerTrace {
        WorkerTrace {
            runtime_id: "runtime-1".into(),
            cell_id: "cell-1".into(),
            request_id: "request-1".into(),
            trace_id: "trace-1".into(),
            parent_span_id: None,
            worker_revision: "abc123".into(),
            contract_digest: "d".repeat(64),
        }
    }

    #[test]
    fn engine_identity_and_call_frame_bytes_are_golden() {
        let identities = [
            (EngineIdentity::FsZero, "fszero", ["fs_zero", "fs"]),
            (
                EngineIdentity::GraphZero,
                "graphzero",
                ["graph_zero", "graph"],
            ),
            (
                EngineIdentity::TokenZero,
                "tokenzero",
                ["token_zero", "token"],
            ),
        ];
        for (identity, canonical, aliases) in identities {
            assert_eq!(
                serde_json::to_string(&identity).unwrap(),
                format!("\"{canonical}\"")
            );
            for alias in aliases {
                let decoded: EngineIdentity =
                    serde_json::from_str(&format!("\"{alias}\"")).unwrap();
                assert_eq!(decoded, identity);
                assert_eq!(
                    serde_json::to_string(&decoded).unwrap(),
                    format!("\"{canonical}\"")
                );
            }
        }
        for invalid in ["fz", "FSZero", "fs-zero", ""] {
            assert!(serde_json::from_str::<EngineIdentity>(&format!("\"{invalid}\"")).is_err());
        }

        let call = WorkerRequestFrame::Call {
            request: CallRequest {
                request_id: "request-1".into(),
                op: "read".into(),
                args: json!({"path": "README.md"}),
                deadline_unix_ms: Some(100),
                trace: trace(),
                approval_grant: None,
                telemetry_request: None,
            },
        };
        let encoded = encode_frame(&call, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            std::str::from_utf8(&encoded).unwrap(),
            concat!(
                r#"{"kind":"call","request":{"request_id":"request-1","op":"read","args":{"path":"README.md"},"deadline_unix_ms":100,"trace":{"runtime_id":"runtime-1","cell_id":"cell-1","request_id":"request-1","trace_id":"trace-1","worker_revision":"abc123","contract_digest":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}}"#,
                "\n"
            )
        );
    }

    #[test]
    fn call_and_cancel_round_trip_through_bounded_ndjson() {
        let call = WorkerRequestFrame::Call {
            request: CallRequest {
                request_id: "request-1".into(),
                op: "read".into(),
                args: json!({"path": "README.md"}),
                deadline_unix_ms: Some(100),
                trace: trace(),
                approval_grant: None,
                telemetry_request: None,
            },
        };
        let encoded = encode_frame(&call, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            call
        );

        let cancel = WorkerRequestFrame::Cancel {
            request: CancelRequest {
                request_id: "request-1".into(),
                reason: Some("cell terminated".into()),
            },
        };
        let encoded = encode_frame(&cancel, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            cancel
        );
    }

    fn call_frame_bytes(op: &str, deadline: Option<u64>) -> Vec<u8> {
        encode_frame(
            &WorkerRequestFrame::Call {
                request: CallRequest {
                    request_id: "request-1".into(),
                    op: op.into(),
                    args: json!({}),
                    deadline_unix_ms: deadline,
                    trace: trace(),
                    approval_grant: None,
                    telemetry_request: None,
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap()
    }

    #[test]
    fn decode_request_frame_rejects_schema_violations() {
        for bytes in [
            call_frame_bytes("", None),
            call_frame_bytes("read", Some(0)),
        ] {
            assert!(matches!(
                decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES),
                Err(FrameCodecError::InvalidContract(_))
            ));
        }

        let missing_args = br#"{"kind":"call","request":{"request_id":"r","op":"read","trace":{"runtime_id":"r","cell_id":"c","request_id":"r","trace_id":"t","worker_revision":"w","contract_digest":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}}"#;
        assert!(matches!(
            decode_request_frame(missing_args, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));

        let bad_digest = encode_frame(
            &WorkerRequestFrame::Handshake {
                request: HandshakeRequest {
                    protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
                    root: "/repo".into(),
                    session_id: "session-1".into(),
                    expected_engine: EngineIdentity::FsZero,
                    expected_worker_revision: None,
                    expected_contract_digest: "NOTHEX".into(),
                    expected_registry_digest: None,
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(
            decode_request_frame(&bad_digest, DEFAULT_MAX_FRAME_BYTES)
                .unwrap_err()
                .kind(),
            "contract_mismatch"
        );

        let empty_reason = encode_frame(
            &WorkerRequestFrame::Shutdown {
                request: ShutdownRequest {
                    reason: String::new(),
                },
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        assert!(matches!(
            decode_request_frame(&empty_reason, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidContract(_))
        ));
    }

    #[test]
    fn abi_hardening_call_trace_request_id_binding() {
        let matching = call_frame_bytes("read", None);
        assert!(decode_request_frame(&matching, DEFAULT_MAX_FRAME_BYTES).is_ok());

        let mut mismatched: Value = serde_json::from_slice(&matching).unwrap();
        mismatched["request"]["trace"]["request_id"] = json!("request-2");
        let bytes = serde_json::to_vec(&mismatched).unwrap();
        let message = decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES)
            .unwrap_err()
            .to_string();
        assert_eq!(
            message,
            "call.trace.request_id mismatch: expected=request-1 actual=request-2"
        );
    }

    #[test]
    fn abi_hardening_protocol_version_mismatch_reports_expected_canonical_version() {
        let request = HandshakeRequest {
            protocol_version: "zerostack.raw_worker.v1".into(),
            root: "/repo".into(),
            session_id: "session-1".into(),
            expected_engine: EngineIdentity::FsZero,
            expected_worker_revision: None,
            expected_contract_digest: "d".repeat(64),
            expected_registry_digest: None,
        };
        let message = validate_request_frame(&WorkerRequestFrame::Handshake { request })
            .unwrap_err()
            .to_string();
        assert_eq!(
            message,
            "protocol_version mismatch: expected=zerostack.raw_worker.v2 actual=zerostack.raw_worker.v1"
        );
    }

    #[test]
    fn frame_size_boundary_is_inclusive_at_max() {
        let at_max = vec![b'x'; DEFAULT_MAX_FRAME_BYTES];
        assert!(matches!(
            decode_request_frame(&at_max, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));

        let over_max = vec![b'x'; DEFAULT_MAX_FRAME_BYTES + 1];
        assert_eq!(
            decode_request_frame(&over_max, DEFAULT_MAX_FRAME_BYTES).unwrap_err(),
            FrameCodecError::TooLarge {
                actual: DEFAULT_MAX_FRAME_BYTES + 1,
                maximum: DEFAULT_MAX_FRAME_BYTES,
            }
        );
        assert!(matches!(
            decode_response_frame(&over_max, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::TooLarge { .. })
        ));
    }

    #[test]
    fn decode_response_frame_round_trips_and_is_size_bounded() {
        let ack = WorkerResponseFrame::CancelAck {
            request_id: "request-1".into(),
            cancelled: true,
        };
        let encoded = encode_frame(&ack, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            decode_response_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            ack
        );
        assert!(matches!(
            decode_response_frame(&encoded, 8),
            Err(FrameCodecError::TooLarge { .. })
        ));
        assert!(matches!(
            decode_response_frame(b"\n", DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::Empty)
        ));
    }

    #[test]
    fn shutdown_ack_accepts_only_the_canonical_empty_payload() {
        assert_eq!(
            decode_response_frame(br#"{"kind":"shutdown_ack"}"#, DEFAULT_MAX_FRAME_BYTES).unwrap(),
            WorkerResponseFrame::ShutdownAck
        );
        let error = decode_response_frame(
            br#"{"kind":"shutdown_ack","extra":true}"#,
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap_err();
        assert_eq!(error.kind(), "invalid_frame");
        assert!(error.to_string().contains("unknown field `extra`"));
    }

    #[test]
    fn optional_transport_telemetry_is_absent_by_default_and_round_trips_when_enabled() {
        let mut call = CallRequest {
            request_id: "request-1".into(),
            op: "read".into(),
            args: json!({"path":"README.md"}),
            deadline_unix_ms: Some(100),
            trace: trace(),
            approval_grant: None,
            telemetry_request: None,
        };
        let disabled = encode_frame(
            &WorkerRequestFrame::Call {
                request: call.clone(),
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        let disabled_text = std::str::from_utf8(&disabled).unwrap();
        assert!(!disabled_text.contains("telemetry_request"));
        assert!(!disabled_text.contains("engine_timeline"));
        assert!(!disabled_text.contains("worker_token_accounting"));

        call.telemetry_request = Some(TelemetryRequestV1 {
            engine_stage_timeline: true,
            worker_token_accounting: true,
        });
        let enabled = encode_frame(
            &WorkerRequestFrame::Call {
                request: call.clone(),
            },
            DEFAULT_MAX_FRAME_BYTES,
        )
        .unwrap();
        let decoded = decode_request_frame(&enabled, DEFAULT_MAX_FRAME_BYTES).unwrap();
        assert_eq!(decoded, WorkerRequestFrame::Call { request: call });
        let enabled_json: Value = serde_json::from_slice(trim_frame(&enabled)).unwrap();
        assert_eq!(enabled_json["request"]["args"], json!({"path":"README.md"}));
        assert_eq!(
            enabled_json["request"]["telemetry_request"],
            json!({"engine_stage_timeline":true,"worker_token_accounting":true})
        );
    }

    #[test]
    fn engine_timeline_validation_enforces_order_disjointness_and_closure() {
        let valid = EngineStageTimelineV1 {
            total_ns: 300,
            spans: vec![
                EngineStageSpanV1 {
                    stage: "decode".into(),
                    start_ns: 0,
                    duration_ns: 100,
                },
                EngineStageSpanV1 {
                    stage: "execute".into(),
                    start_ns: 100,
                    duration_ns: 200,
                },
            ],
        };
        validate_engine_stage_timeline_v1(&valid).unwrap();
        let mut overlap = valid.clone();
        overlap.spans[1].start_ns = 99;
        assert!(matches!(
            validate_engine_stage_timeline_v1(&overlap),
            Err(FrameCodecError::InvalidContract(message)) if message.contains("overlaps")
        ));
        let mut open = valid.clone();
        open.total_ns = 1_000_000;
        assert!(matches!(
            validate_engine_stage_timeline_v1(&open),
            Err(FrameCodecError::InvalidContract(message)) if message.contains("does not close")
        ));
        let mut overflow = valid;
        overflow.spans[1].start_ns = u64::MAX;
        assert!(matches!(
            validate_engine_stage_timeline_v1(&overflow),
            Err(FrameCodecError::InvalidContract(message)) if message.contains("overflow")
        ));
    }

    #[test]
    fn worker_token_accounting_is_typed_and_never_inferred_from_bytes() {
        let exact = WorkerTokenAccountingV1 {
            tokenizer_version_digest: Some(
                "3278763c4d4dd11356d55cabfadb66db6de8260c8e300d681690efb8b1298f04".into(),
            ),
            tokenizer_id: "tokenizer-v1".into(),
            count_kind: WorkerTokenCountKind::Exact,
            raw_tokens: 100,
            visible_tokens: 40,
            recovery_tokens: 80,
            billed_tokens: 120,
            cached_tokens: 20,
            exact_ref_tokens: None,
        };
        validate_worker_token_accounting_v1(&exact).unwrap();
        let mut bad_cache = exact.clone();
        bad_cache.cached_tokens = 121;
        assert!(validate_worker_token_accounting_v1(&bad_cache).is_err());
        let mut estimator = exact.clone();
        estimator.tokenizer_id = "estimator:fixture".into();
        assert!(validate_worker_token_accounting_v1(&estimator).is_err());
        estimator.count_kind = WorkerTokenCountKind::Estimate;
        validate_worker_token_accounting_v1(&estimator).unwrap();
        let mut empty = exact;
        empty.tokenizer_id = " ".into();
        assert!(validate_worker_token_accounting_v1(&empty).is_err());
    }

    #[test]
    fn protocol_manifest_covers_type_level_binding_surface() {
        let manifest = raw_worker_protocol_manifest();
        let binding = manifest["binding"].as_array().unwrap();
        for field in ["semantic_contract_version", "ref_scheme"] {
            assert!(binding.iter().any(|value| value == field), "{field}");
        }
        assert!(
            manifest["trace"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "parent_span_id")
        );
        for (section, field) in [
            ("call", "telemetry_request"),
            ("result_frame", "engine_timeline"),
            ("result_frame", "worker_token_accounting"),
            ("error_frame", "engine_timeline"),
            ("error_frame", "worker_token_accounting"),
        ] {
            assert!(
                manifest[section]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == field),
                "{section}.{field}"
            );
        }
        assert_eq!(
            manifest["linked_contracts"]["assembly_abi_contract_digest"],
            assembly_abi_contract_digest_v1().to_hex()
        );
        assert_eq!(
            manifest["linked_contracts"]["assembly_manifest_schema_version"],
            ASSEMBLY_MANIFEST_SCHEMA_VERSION
        );
        assert_eq!(
            manifest["linked_contracts"]["robust_snap_contract_digest"],
            robust_snap_contract_digest_v1().to_hex()
        );
    }

    #[test]
    fn oversized_and_unknown_frames_fail_closed() {
        let oversized = vec![b'x'; 33];
        assert!(matches!(
            decode_request_frame(&oversized, 32),
            Err(FrameCodecError::TooLarge { .. })
        ));
        let unknown = br#"{"kind":"call","request":{"request_id":"r","op":"x","args":{},"trace":{"runtime_id":"r","cell_id":"c","request_id":"r","trace_id":"t","worker_revision":"w","contract_digest":"d"}},"ambient_node":true}"#;
        assert!(matches!(
            decode_request_frame(unknown, DEFAULT_MAX_FRAME_BYTES),
            Err(FrameCodecError::InvalidJson(_))
        ));
    }

    #[test]
    fn handshake_rejects_skew_and_wrong_binding() {
        let binding = WorkerBinding {
            engine: EngineIdentity::FsZero,
            root: "/repo".into(),
            session_id: "session-1".into(),
            worker_revision: "abc123".into(),
            semantic_contract_version: "1".into(),
            semantic_contract_digest: "a".repeat(64),
            operation_registry_digest: "b".repeat(64),
            ref_scheme: "fz".into(),
        };
        let mut request = HandshakeRequest {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
            root: binding.root.clone(),
            session_id: binding.session_id.clone(),
            expected_engine: binding.engine,
            expected_worker_revision: Some(binding.worker_revision.clone()),
            expected_contract_digest: binding.semantic_contract_digest.clone(),
            expected_registry_digest: Some(binding.operation_registry_digest.clone()),
        };
        validate_handshake_request(&request, &binding).unwrap();
        request.session_id = "other-session".into();
        assert_eq!(
            validate_handshake_request(&request, &binding)
                .unwrap_err()
                .kind(),
            "contract_mismatch"
        );
    }

    #[test]
    fn deadline_and_protocol_digest_are_deterministic() {
        let call = CallRequest {
            request_id: "request-1".into(),
            op: "read".into(),
            args: Value::Null,
            deadline_unix_ms: Some(100),
            trace: trace(),
            approval_grant: None,
            telemetry_request: None,
        };
        assert!(!call.deadline_expired(99));
        assert!(call.deadline_expired(100));
        let digest = raw_worker_protocol_digest_hex();
        assert_eq!(digest.len(), 64);
        assert_eq!(
            digest,
            "e2daca4d95cbd2780f2e10b30b823e9398747bfe15e38ca0810f634a387aeace"
        );
        assert_eq!(digest, raw_worker_protocol_digest_hex());
    }

    #[test]
    fn manifest_field_inventories_are_explicitly_order_stable() {
        let manifest = raw_worker_protocol_manifest();
        for key in [
            "binding",
            "handshake_request",
            "capabilities",
            "limits",
            "call",
            "telemetry_request",
            "engine_stage_span",
            "engine_stage_timeline",
            "worker_token_accounting",
            "result_frame",
            "error_frame",
            "trace",
            "result_metadata",
        ] {
            let fields = manifest[key]
                .as_array()
                .unwrap_or_else(|| panic!("{key} field inventory"));
            assert!(
                fields
                    .windows(2)
                    .all(|pair| pair[0].as_str() <= pair[1].as_str()),
                "{key} field inventory is not sorted: {fields:?}"
            );
        }
    }
