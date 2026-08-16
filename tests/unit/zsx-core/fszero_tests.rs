    use super::*;
    use serde_json::json;
    use zero_abi::WorkerTrace;

    fn empty_request() -> CallRequest {
        CallRequest {
            request_id: "t".into(),
            op: "fs.search".into(),
            args: json!({}),
            deadline_unix_ms: None,
            trace: WorkerTrace {
                runtime_id: String::new(),
                cell_id: String::new(),
                request_id: String::new(),
                trace_id: String::new(),
                parent_span_id: None,
                worker_revision: String::new(),
                contract_digest: String::new(),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn harvested_foreign_blob_is_not_retained() {
        let mut session = FSZeroSession::new();
        let gz_blob = format!("gz://blob/{}", "a".repeat(64));
        let tz_blob = format!("tz://blob/{}", "b".repeat(64));
        let fz_blob = session.recovery.put_content_ref(b"owned");
        let result = DomainResult::success(
            "fs.search",
            Some("R1".into()),
            Some(json!({
                "detail": format!(
                    "see {gz_blob} and gz://node/symbol and {tz_blob} and {fz_blob}"
                )
            })),
            Vec::new(),
            false,
        );
        let refs = collect_and_conform_refs(&session, &empty_request(), &result)
            .expect("foreign-scheme harvest must stay Ok");
        assert!(
            refs.iter()
                .all(|reference| !reference.starts_with("gz://blob/")),
            "harvested gz://blob must not land on ownership.refs: {refs:?}"
        );
        assert!(
            refs.iter()
                .all(|reference| !reference.starts_with("tz://blob/")),
            "harvested tz://blob must not land on ownership.refs: {refs:?}"
        );
        assert!(
            refs.iter()
                .all(|reference| !reference.contains("gz://node")),
            "gz://node harvest stays dropped: {refs:?}"
        );
        assert!(
            refs.iter().any(|reference| reference == &fz_blob),
            "expandable fz://blob must still be kept: {refs:?}"
        );
        let again = collect_and_conform_refs(&session, &empty_request(), &result)
            .expect("idempotent");
        assert_eq!(refs, again);
    }

    #[test]
    fn sitting_reply_wins_over_cancel() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let request = empty_request();
        let cancel = CancellationSignal::new();
        cancel.cancel();
        let sent = adapter_error("internal", "sitting-reply", &request);
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(15));
            let _ = tx.send(Err(sent));
        });
        let err = receive_call_response(&rx, &cancel, &request)
            .expect_err("sitting reply is an adapter error");
        assert_eq!(err.error.kind, "internal", "{}", err.error.message);
        assert!(
            err.error.message.contains("sitting-reply"),
            "{}",
            err.error.message
        );
        worker.join().expect("reply sender");
    }

    fn success_result(value: Option<Value>, refs: Vec<String>) -> DomainResult {
        DomainResult::success("fs.read", Some("R1".into()), value, refs, false)
    }

    #[test]
    fn enrich_utf8_from_cloned_recovery_key_after_result_move() {
        let mut session = FSZeroSession::new();
        let key = session.recovery.put_content_ref(b"hello-utf8");
        let mut result = success_result(
            Some(json!({"prior_field": 1})),
            vec!["fz://blob/dead".into()],
        );
        let inline = Some(json!({"kept": true}));
        let recovery_key = Some(key.clone());
        let moved = result.clone();
        let _ = moved;
        enrich_recovery_payload(
            &session,
            recovery_key.as_deref(),
            &mut result,
            &[key.clone()],
        );
        let value = result.value.expect("enriched value");
        assert_eq!(value["prior_field"], 1);
        assert_eq!(value["payload_utf8"], "hello-utf8");
        assert_eq!(value["ref"], key);
        assert!(value.get("payload_hex").is_none());
        assert_eq!(inline.unwrap()["kept"], true);
    }

    #[test]
    fn enrich_non_utf8_uses_payload_hex() {
        let mut session = FSZeroSession::new();
        let key = session.recovery.put_content_ref(&[0xff, 0x00, 0xfe]);
        let mut result = success_result(None, Vec::new());
        enrich_recovery_payload(&session, Some(&key), &mut result, &[key.clone()]);
        let value = result.value.expect("hex payload");
        assert_eq!(value["payload_hex"], "ff00fe");
        assert_eq!(value["bytes_len"], 3);
        assert!(value.get("payload_utf8").is_none());
        assert_eq!(value["ref"], key);
    }

    #[test]
    fn enrich_single_batch_recovers_the_snap_payload() {
        let mut session = FSZeroSession::new();
        let source_ref = session.recovery.put_content_ref(
            b"HIT src/lib.rs#L7-L9 kind=literal\n| 8: unique_needle();",
        );
        let batch = serde_json::to_vec(&json!([{
            "operation": "fs.search",
            "source_ref": source_ref,
            "payload_len": 58
        }]))
        .unwrap();
        let batch_ref = session.recovery.put_content_ref(&batch);
        let mut result = success_result(Some(json!({"count": 1})), vec![batch_ref.clone()]);
        enrich_recovery_payload(&session, Some(&batch_ref), &mut result, &[batch_ref.clone()]);
        let value = result.value.expect("single batch enrichment");
        assert!(
            value["payload_utf8"]
                .as_str()
                .unwrap()
                .starts_with("HIT src/lib.rs#L7-L9"),
            "{value}"
        );
        assert_eq!(value["source_ref"], source_ref);
        assert!(
            value["batch_payload_utf8"]
                .as_str()
                .unwrap()
                .contains("source_ref")
        );
        assert_eq!(value["ref"], batch_ref);
    }

    #[test]
    fn enrich_falls_back_to_first_result_ref_when_key_missing() {
        let mut session = FSZeroSession::new();
        let key = session.recovery.put_content_ref(b"from-ref");
        let mut result = success_result(Some(json!({})), vec![key.clone()]);
        enrich_recovery_payload(&session, None, &mut result, &[key.clone()]);
        let value = result.value.expect("fallback");
        assert_eq!(value["payload_utf8"], "from-ref");
        assert_eq!(value["ref"], key);
    }

    #[test]
    fn enrich_missing_key_leaves_value_and_refs_intact() {
        let session = FSZeroSession::new();
        let mut result = success_result(
            Some(json!({"kept": "yes"})),
            vec!["fz://blob/missing".into()],
        );
        enrich_recovery_payload(
            &session,
            Some("missing-key"),
            &mut result,
            &["fz://blob/missing".into()],
        );
        assert_eq!(result.value.unwrap()["kept"], "yes");
        assert_eq!(result.refs, vec!["fz://blob/missing"]);
    }

    #[test]
    fn adapter_read_enriches_utf8_recovery_after_result_move() {
        let root = std::env::temp_dir().join(format!("zerostack-qadr-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let fixture = root.join("note.txt");
        std::fs::write(&fixture, b"qadr-payload").expect("fixture");
        let adapter = FsZeroAdapter::new_in_memory(&root, "qadr");
        assert!(!adapter.degraded(), "in-memory adapter must be live");
        let request = CallRequest {
            request_id: "qadr".into(),
            op: "fs.read".into(),
            args: json!({ "path": "note.txt" }),
            deadline_unix_ms: None,
            trace: empty_request().trace,
            approval_grant: None,
            telemetry_request: None,
        };
        let outcome = adapter
            .call(AdapterCall {
                request: &request,
                cancellation: &CancellationSignal::new(),
            })
            .expect("fs.read through real adapter");
        let value = &outcome.result.value;
        let rendered = serde_json::to_string(value).unwrap_or_default();
        assert!(
            value.get("payload_utf8").and_then(Value::as_str) == Some("qadr-payload")
                || rendered.contains("qadr-payload"),
            "adapter path must enrich or return the file body: {rendered}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn domain_error_kind_table_is_rw5_closed() {
        let rows = [
            ("invalid_argument", "validation"),
            ("permission_denied", "policy"),
            ("incompatible_contract", "policy"),
            ("cancelled", "cancelled"),
            ("deadline", "deadline_exceeded"),
            ("deadline_exceeded", "deadline_exceeded"),
            ("busy", "timeout"),
            ("not-a-real-class", "internal"),
        ];
        for (class, want) in rows {
            assert_eq!(domain_error_kind(class), want, "class={class}");
        }
        assert!(is_forbidden_operation("planner"));
        assert!(is_forbidden_operation("js.execute"));
    }

    #[test]
    fn late_ok_aborts_journaled_mutations_not_reads() {
        assert!(domain_failure_aborts_plan("fs.write", false));
        assert!(domain_failure_aborts_plan("fs.edit", false));
        assert!(domain_failure_aborts_plan("fs.transact", false));
        assert!(domain_failure_aborts_plan("fs.read", true));
        assert!(!domain_failure_aborts_plan("fs.read", false));
        assert!(!domain_failure_aborts_plan("fs.search", false));
        assert!(!domain_failure_aborts_plan("fs.list", false));
    }

    fn adapter_call(
        adapter: &FsZeroAdapter,
        op: &str,
        args: Value,
    ) -> Result<AdapterResponse, AdapterError> {
        let request = CallRequest {
            request_id: format!("late-ok-{op}"),
            op: op.into(),
            args,
            deadline_unix_ms: None,
            trace: empty_request().trace,
            approval_grant: None,
            telemetry_request: None,
        };
        adapter.call(AdapterCall {
            request: &request,
            cancellation: &CancellationSignal::new(),
        })
    }

    #[test]
    fn failed_write_aborts_instead_of_late_ok() {
        let root = std::env::temp_dir().join(format!(
            "zerostack-late-ok-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let adapter = FsZeroAdapter::new_in_memory(&root, "late-ok-write");
        assert!(!adapter.degraded(), "in-memory adapter must be live");
        let err = adapter_call(&adapter, "fs.write", json!({}))
            .expect_err("failed fs.write must abort the plan");
        assert!(
            !err.error.message.contains("sitting-reply"),
            "must be the domain failure, not a channel drop: {}",
            err.error.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_read_stays_late_ok() {
        let root = std::env::temp_dir().join(format!(
            "zerostack-late-ok-read-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let adapter = FsZeroAdapter::new_in_memory(&root, "late-ok-read");
        assert!(!adapter.degraded(), "in-memory adapter must be live");
        let outcome = adapter_call(
            &adapter,
            "fs.read",
            json!({ "path": "does-not-exist.txt" }),
        )
        .expect("missing sibling read must stay Ok");
        let value = &outcome.result.value;
        let ok = value.get("ok").and_then(Value::as_bool);
        let error_text = value.get("error_text").and_then(Value::as_str);
        assert_eq!(ok, Some(false), "salvage must carry DomainResult.ok=false: {value}");
        assert!(
            error_text.is_some() || value.get("error").is_some(),
            "salvage must keep the typed error: {value}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

