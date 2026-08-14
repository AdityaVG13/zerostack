    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use zero_abi::{WorkerRequestFrame, WorkerTrace};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn worker_accounting_is_a_non_estimate_byte_upper_bound() {
        let value = json!({
            "visible":{"kind":"capsule","text":"é🙂"},
            "refs":[{"kind":"blob","ref":"tz://blob/example","bytes":9,"live":true}],
            "accounting":{
                "raw_tokens":1,
                "visible_tokens":1,
                "recovery_tokens":1,
                "billed_tokens":1,
                "cached_tokens":0
            }
        });
        let accounting = worker_token_accounting("read", &json!({"input":"é🙂"}), &value)
            .expect("upper-bound accounting");
        assert_eq!(
            accounting.count_kind,
            WorkerTokenCountKind::ConservativeUpperBound
        );
        assert_eq!(accounting.tokenizer_id, "conservative:utf8-json-bytes-v1");
        assert!(accounting.raw_tokens >= accounting.visible_tokens + 9);
        assert_eq!(accounting.recovery_tokens, 9);
        assert!(accounting.billed_tokens >= accounting.visible_tokens);
        assert_eq!(accounting.exact_ref_tokens, None);

        let malformed = json!({
            "refs":[],
            "accounting":{
                "raw_tokens":1,
                "visible_tokens":1,
                "recovery_tokens":0,
                "billed_tokens":1,
                "cached_tokens":2
            }
        });
        assert!(
            worker_token_accounting("read", &json!({}), &malformed)
                .unwrap_err()
                .contains("cached_tokens exceeds billed_tokens")
        );
        let job = worker_token_accounting(
            TOKEN_JOB_OPERATION_V1,
            &json!({"id":"job-1"}),
            &json!({"id":"job-1","status":"exited"}),
        )
        .expect("job poll upper-bound accounting");
        assert_eq!(job.count_kind, WorkerTokenCountKind::ConservativeUpperBound);
        assert_eq!(job.cached_tokens, 0);
        assert_eq!(job.recovery_tokens, 0);
        let launch = worker_token_accounting(
            "shell",
            &json!({"command":"printf ok","background":true}),
            &json!({"job":"job-1","cursor":0,"version":0}),
        )
        .expect("background launch upper-bound accounting");
        assert_eq!(
            launch.count_kind,
            WorkerTokenCountKind::ConservativeUpperBound
        );
        assert_eq!(launch.cached_tokens, 0);
        assert_eq!(launch.recovery_tokens, 0);
    }

    #[test]
    fn refs_omitting_success_accounts_zero_recovery_as_loud_estimate() {
        // Engine repos evolve independently: a successful read that omits
        // `refs` must not fail the call. Recovery accounts as zero and the
        // count kind downgrades loudly to Estimate (no proven upper bound).
        let value = json!({
            "visible":{"kind":"capsule","text":"ok"},
            "accounting":{
                "raw_tokens":1,
                "visible_tokens":1,
                "recovery_tokens":0,
                "billed_tokens":1,
                "cached_tokens":0
            }
        });
        let accounting = worker_token_accounting("read", &json!({"input":"x"}), &value)
            .expect("refs-omitting success must account, not fail");
        assert_eq!(accounting.count_kind, WorkerTokenCountKind::Estimate);
        assert_eq!(accounting.recovery_tokens, 0);
        assert!(accounting.billed_tokens >= accounting.visible_tokens);

        // Malformed refs (present but wrong shape) still fail loud.
        let malformed_refs = json!({
            "refs":"not-an-array",
            "accounting":{
                "raw_tokens":1,
                "visible_tokens":1,
                "recovery_tokens":0,
                "billed_tokens":1,
                "cached_tokens":0
            }
        });
        assert!(
            worker_token_accounting("read", &json!({}), &malformed_refs)
                .unwrap_err()
                .contains("refs must be an array")
        );
    }

    /// A `WorkerTrace` with the bare minimum for unit-testing pure helpers.
    fn test_trace() -> WorkerTrace {
        WorkerTrace {
            runtime_id: "runtime".into(),
            cell_id: "cell".into(),
            request_id: "request-1".into(),
            trace_id: "request-1".into(),
            parent_span_id: None,
            worker_revision: TOKENZERO_ENGINE_VERSION.into(),
            contract_digest: "0".repeat(64),
        }
    }

    #[test]
    fn adapter_is_send_sync_and_binding_is_tokenzero_canonical() {
        assert_send_sync::<TokenZeroAdapter>();
        let adapter = TokenZeroAdapter::new("/tmp", "session-tz").expect("adapter builds");
        assert_eq!(adapter.engine(), EngineIdentity::TokenZero);
        assert_eq!(adapter.session_id(), "session-tz");
        let binding = adapter.binding();
        assert_eq!(binding.engine, EngineIdentity::TokenZero);
        assert_eq!(binding.ref_scheme, "tz://");
        assert_eq!(binding.semantic_contract_version, SEMANTIC_CONTRACT_VERSION);
        for digest in [
            &binding.semantic_contract_digest,
            &binding.operation_registry_digest,
        ] {
            assert_eq!(digest.len(), 64);
            assert!(
                digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "digest must be lowercase hex"
            );
        }
        // Both digests bind the registry contract, exactly like the v2 worker
        // capability handshake.
        assert_eq!(
            binding.semantic_contract_digest,
            binding.operation_registry_digest
        );
        assert_eq!(binding.semantic_contract_digest, contract_digest_hex());
    }

    #[test]
    fn effect_class_matches_the_v2_worker() {
        for op in [
            "shell",
            "tz_shell",
            "zero.shell",
            "compact",
            "tz_compact",
            "zero.compact",
            "ingest",
            "tz_ingest",
            "zero.ingest",
        ] {
            assert_eq!(effect_class(op), EffectClass::Irreversible, "{op}");
        }
        for op in ["read", "find", "expand", "recall", "job"] {
            assert_eq!(effect_class(op), EffectClass::ReadOnly, "{op}");
        }
    }

    #[test]
    fn oversized_bare_expand_error_provides_a_fragment_retry() {
        let reference = format!("tz://blob/{}", "a".repeat(64));
        let request = CallRequest {
            request_id: "request-expand".into(),
            op: "expand".into(),
            args: json!({"ref":reference}),
            deadline_unix_ms: None,
            trace: test_trace(),
            approval_grant: None,
            telemetry_request: None,
        };
        let adapter = TokenZeroAdapter::new("/tmp", "session-expand").expect("adapter builds");
        let error = adapter
            .bind_outcome(
                &request,
                Ok((json!({"visible":"x".repeat(MAX_OUTPUT_BYTES)}), Vec::new())),
                Duration::ZERO,
            )
            .expect_err("oversized result must fail");
        assert_eq!(error.error.kind, "output_too_large");
        assert!(error.error.message.contains("#B0-32768"));
    }

    #[test]
    fn ref_collection_walks_values_and_keeps_only_tz_refs() {
        let value = json!({
            "visible": "tz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#B0-10 (see also)",
            "refs": ["tz://blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            "nested": {"shell": ["fz://blob/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"]},
            "job": {"tail": "content mentions tz:// but does not start with it"},
        });
        let mut refs = Vec::new();
        collect_refs(&value, &mut refs);
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|r| r.starts_with("tz://blob/aaaaaaaa")));
        assert!(refs.iter().any(|r| r.starts_with("tz://blob/bbbbbbbb")));
    }

    #[test]
    fn forbidden_mask_matches_the_v2_worker() {
        for op in [
            "plan",
            "planner",
            "js",
            "javascript",
            "mcp",
            "execute_code",
            "tz_execute_code",
            "codemode_search",
            "codemode_describe",
            "tools/call",
            "planner.plan",
            "javascript.run",
            "mcp.tools/list",
        ] {
            assert!(forbidden_operation(op), "{op}");
        }
        for op in [
            "read", "find", "shell", "ingest", "expand", "compact", "job",
        ] {
            assert!(!forbidden_operation(op), "{op}");
        }
    }

    #[test]
    fn deadline_and_cancellation_stop_before_dispatch() {
        let adapter = TokenZeroAdapter::new("/tmp", "session-tz").expect("adapter builds");
        let request = CallRequest {
            request_id: "request-1".into(),
            op: "read".into(),
            args: json!({"path": "missing.txt"}),
            deadline_unix_ms: Some(1),
            trace: test_trace(),
            approval_grant: None,
            telemetry_request: None,
        };
        let cancellation = zero_codemode::CancellationSignal::new();
        let error = adapter
            .call(AdapterCall {
                request: &request,
                cancellation: &cancellation,
            })
            .expect_err("expired deadline must fail before dispatch");
        assert_eq!(error.error.kind, "deadline");

        let request = CallRequest {
            deadline_unix_ms: None,
            ..request
        };
        cancellation.cancel();
        let error = adapter
            .call(AdapterCall {
                request: &request,
                cancellation: &cancellation,
            })
            .expect_err("cancellation must fail before dispatch");
        assert_eq!(error.error.kind, "cancelled");
    }

    /// End-to-end: register the real adapter in a `ZsxSession` (fixture
    /// adapters fill the other two slots) and run `zero.token.compact` with a
    /// payload large enough to mint an exact `tz://blob/…` ref. The adapter
    /// must publish the ref payload into the hub CAS under the session root
    /// so the connector's reachability verification passes.
    #[cfg(feature = "fixture-adapters")]
    #[test]
    fn session_compact_publishes_exact_refs_into_the_hub_cas() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let session_id = "session-tz-e2e";
        let adapter = TokenZeroAdapter::new(root.clone(), session_id).expect("adapter builds");
        let (fs, graph, _token) = crate::fixture::fixture_adapters(&root, session_id);
        let session = crate::ZsxSession::builder(&root)
            .with_session_id(session_id)
            .fszero(fs)
            .graphzero(graph)
            .tokenzero(Arc::new(adapter))
            .build()
            .expect("session builds");
        // ~60 KiB of distinct text forces the exact-ref capsule path
        // (threshold 40 KiB), minting a recoverable blob ref.
        let text: String = (0..4_000)
            .map(|index| format!("line {index}: the quick brown fox jumps over the lazy dog\n"))
            .collect();
        assert!(
            text.len() > 40 * 1024,
            "fixture must exceed the exact-ref threshold"
        );
        let source = format!(
            "return await zero.token.compact({});",
            serde_json::to_string(&text).expect("text serializes")
        );
        let result = session
            .execute(1, 1, source, Duration::from_secs(60))
            .expect("compact executes");
        let envelope = result.value;
        // The host spills oversized final results to the resolved store CAS
        // and returns a spill envelope whose `ref` carries the finalized
        // `{"value", "metadata"}` record.
        let mut finalization_ref = None;
        let finalized = if envelope.get("spilled").and_then(Value::as_bool) == Some(true) {
            let spill_ref = envelope["ref"].as_str().expect("spill ref");
            finalization_ref = Some(spill_ref.to_string());
            let resolved_store = zero_store::ResolvedStore::resolve_from_process(
                &root,
                zero_store::Engine::TokenZero,
                &[],
            );
            let spill_cas = SharedCas::open(resolved_store.cas_host());
            let parsed = ZeroRefV1::parse(spill_ref).expect("spill ref is portable v1");
            let bytes = spill_cas
                .get_verified(&parsed.hash)
                .expect("spill payload verifies");
            serde_json::from_slice::<Value>(&bytes).expect("spill payload is JSON")
        } else {
            envelope
        };
        if let Some(spill_ref) = finalization_ref {
            let expanded = session
                .execute(
                    1,
                    2,
                    format!(
                        "return await zero.token.expand({});",
                        serde_json::to_string(&spill_ref).expect("ref serializes")
                    ),
                    Duration::from_secs(60),
                )
                .expect("finalization spill expands through the same session store");
            assert_ne!(
                expanded.value["spilled"],
                json!(true),
                "expanded finalization spill: {}",
                expanded.value
            );
            assert_eq!(
                expanded.value["content"]["value"]["value"]["visible"],
                json!(serde_json::to_string(&finalized).expect("finalized result serializes"))
            );
        }
        // The finalized record is the host's `ZeroResultV1`-style envelope:
        // `content.value` holds the connector's `{"value", "metadata"}`.
        let content = &finalized["content"]["value"];
        assert_eq!(
            content["value"]["status"],
            json!("ok"),
            "finalized: {finalized}"
        );
        let refs = content["metadata"]["ownership"]["refs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(!refs.is_empty(), "compact must mint at least one ref");
        // `tz://file/…` refs are non-portable kinds: neither the adapter nor
        // the core's reachability scan publishes or retains them.
        assert!(
            refs.iter().all(|reference| {
                let reference = reference.as_str().expect("ref is a string");
                reference.starts_with("tz://blob/") || reference.starts_with("tz://file/")
            }),
            "unexpected ref kinds: {refs:?}"
        );
        // Every adapter ref must be published in the hub CAS under the
        // session root and verify against its own payload bytes.
        let cas = SharedCas::open(&root);
        for reference in &refs {
            let reference = reference.as_str().expect("ref is a string");
            if !reference.starts_with("tz://blob/") {
                continue; // non-portable kinds are engine-owned, never published
            }
            let parsed = ZeroRefV1::parse(reference).expect("portable v1 ref");
            assert_eq!(parsed.scheme, ZeroScheme::Tz);
            assert!(
                cas.contains(&parsed.hash),
                "ref {reference} must be published in the hub CAS"
            );
            let bytes = cas.get_verified(&parsed.hash).expect("CAS bytes verify");
            assert_eq!(zero_ref::content_hash_hex(&bytes), parsed.hash);
        }
        // The minted ref must also resolve through the engine's own expand
        // path (the raw payload lives in the engine recovery store). The full
        // payload exceeds the advertised 65,536-byte output cap — exactly
        // like the v2 worker — so a byte-span selector proves the positive
        // path, and the whole-blob expand proves the typed cap error.
        let blob_ref = refs
            .iter()
            .find_map(|reference| {
                let reference = reference.as_str().expect("ref is a string");
                reference
                    .starts_with("tz://blob/")
                    .then_some(reference.to_string())
            })
            .expect("compact mints a blob ref");
        let resolved = session
            .execute(
                1,
                3,
                format!(
                    "return await zero.token.expand({});",
                    serde_json::to_string(&format!("{blob_ref}#B0-800")).expect("ref serializes")
                ),
                Duration::from_secs(60),
            )
            .expect("expand executes");
        // The expand result may itself spill; the call must at least succeed.
        if resolved.value.get("spilled").and_then(Value::as_bool) != Some(true) {
            assert_eq!(
                resolved.value["content"]["value"]["value"]["status"],
                json!("ok")
            );
        }
        let oversized = session
            .execute(
                1,
                4,
                format!(
                    "return await zero.token.expand({});",
                    serde_json::to_string(&blob_ref).expect("ref serializes")
                ),
                Duration::from_secs(60),
            )
            .expect_err("whole-blob expand must exceed the output cap");
        assert!(
            oversized.to_string().contains("output_too_large"),
            "{oversized}"
        );
        session.shutdown().expect("session shuts down");
    }

    #[test]
    fn request_frame_roundtrip_keeps_trace_binding() {
        // The adapter echoes the trace verbatim; the connector rejects any
        // other binding. A valid frame that is never dispatched would hide a
        // dropped echo.
        let binding = AdapterBinding::new(
            EngineIdentity::TokenZero,
            worker_revision(),
            SEMANTIC_CONTRACT_VERSION,
            contract_digest_hex(),
            contract_digest_hex(),
            "tz://",
        )
        .expect("binding is valid");
        let request = CallRequest {
            request_id: "request-1".into(),
            op: "read".into(),
            args: json!({"path": "."}),
            deadline_unix_ms: Some(30_000),
            trace: WorkerTrace {
                runtime_id: "runtime".into(),
                cell_id: "cell".into(),
                request_id: "request-1".into(),
                trace_id: "request-1".into(),
                parent_span_id: None,
                worker_revision: binding.worker_revision.clone(),
                contract_digest: binding.semantic_contract_digest.clone(),
            },
            approval_grant: None,
            telemetry_request: None,
        };
        let frame = WorkerRequestFrame::Call {
            request: request.clone(),
        };
        zero_abi::validate_request_frame(&frame).expect("connector-shaped call frame is valid");
        let adapter = TokenZeroAdapter::new("/tmp", "session-tz").expect("adapter builds");
        let response = adapter
            .bind_outcome(
                &request,
                Ok((json!({"ok": true}), Vec::new())),
                Duration::ZERO,
            )
            .expect("small bind must succeed");
        assert_eq!(
            response.result.metadata.trace, request.trace,
            "adapter must echo request.trace verbatim"
        );
    }
