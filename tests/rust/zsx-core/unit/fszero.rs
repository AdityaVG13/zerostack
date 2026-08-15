    use super::*;
    use std::time::Instant;
    use zero_abi::WorkerTrace;

    fn test_request(deadline_unix_ms: Option<u64>) -> CallRequest {
        CallRequest {
            request_id: "reply-poll".into(),
            op: "fs.read".into(),
            args: serde_json::json!({"path":"README.md"}),
            deadline_unix_ms,
            trace: WorkerTrace {
                runtime_id: "test".into(),
                cell_id: "test".into(),
                request_id: "reply-poll".into(),
                trace_id: "reply-poll".into(),
                parent_span_id: None,
                worker_revision: "test".into(),
                contract_digest: "0".repeat(64),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn binding_uses_the_immutable_revision_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = FsZeroAdapter::new(dir.path(), "session-fszero");
        assert_eq!(adapter.engine(), EngineIdentity::FsZero);
        let binding = adapter.binding();
        assert_eq!(binding.engine, EngineIdentity::FsZero);
        assert_eq!(binding.ref_scheme, FSZERO_REF_SCHEME);
        assert_eq!(binding.semantic_contract_version, OPERATION_ABI_VERSION);
        assert_eq!(
            binding.semantic_contract_digest, binding.operation_registry_digest,
            "FSZero equates the contract digest with the operation ABI digest"
        );
        assert!(
            binding.semantic_contract_digest.len() == 64
                && binding
                    .semantic_contract_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn forbidden_operations_never_reach_the_dispatcher() {
        for op in zero_abi::RW10_FORBIDDEN_OPS {
            assert!(is_forbidden_operation(op), "{op} must be forbidden");
        }
        for op in [
            "execute_code",
            "fz_execute_code",
            "codemode_search",
            "fszero.exec",
            "tools/call",
            "planner.run",
            "js.execute",
        ] {
            assert!(is_forbidden_operation(op), "{op} must be forbidden");
        }
        for op in ["fs.read", "fs.search", "fs.expand"] {
            assert!(!is_forbidden_operation(op), "{op} must dispatch");
        }
    }

    #[test]
    fn blob_ref_conformance_mirrors_the_raw_worker() {
        let valid = "fz://blob/".to_owned() + &"a".repeat(64);
        assert!(is_conformant_blob_ref(&valid));
        let short = "fz://blob/".to_owned() + &"b".repeat(63);
        assert!(!is_conformant_blob_ref(&short));
        let upper = "fz://blob/".to_owned() + &"C".repeat(64);
        assert!(!is_conformant_blob_ref(&upper));
        assert!(is_conformant_blob_ref("fz://codemode/execution/7"));
    }

    #[test]
    fn portable_refs_are_collected_from_any_value_shape() {
        let value = serde_json::json!({
            "text": "see fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and (fz://blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb)",
            "nested": ["gz://node/symbol", {"ref": "tz://blob/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}],
        });
        let mut refs = Vec::new();
        collect_portable_refs(&value, &mut refs);
        assert_eq!(refs.len(), 4);
        assert!(
            refs.contains(
                &"fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned()
            )
        );
    }

    #[test]
    fn cancelled_reply_wait_releases_the_shared_dispatcher_promptly() {
        let (_reply_tx, reply_rx) = mpsc::sync_channel(1);
        let cancellation = CancellationSignal::new();
        let cancel_signal = cancellation.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(15));
            cancel_signal.cancel();
        });
        let started = Instant::now();
        let error = receive_call_response(&reply_rx, &cancellation, &test_request(None))
            .expect_err("cancelled wait must stop");
        cancel.join().expect("cancellation thread");
        assert_eq!(error.error.kind, "cancelled");
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn expired_reply_wait_releases_the_shared_dispatcher_promptly() {
        let (_reply_tx, reply_rx) = mpsc::sync_channel(1);
        let cancellation = CancellationSignal::new();
        let error = receive_call_response(
            &reply_rx,
            &cancellation,
            &test_request(Some(crate::connector::now_ms().saturating_sub(1))),
        )
        .expect_err("expired wait must stop");
        assert_eq!(error.error.kind, "deadline");
    }

    fn delay_request(delay_ms: u64) -> CallRequest {
        let mut request = test_request(None);
        request.op = "fs.search".into();
        request.args = serde_json::json!({"query": "x", "__delay_ms": delay_ms});
        request
    }

    #[test]
    fn drop_after_cancelled_full_channel_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = std::sync::Arc::new(FsZeroAdapter::new(dir.path(), "session-drop-bound"));
        let occupier_cancel = CancellationSignal::new();
        let queued_cancel = CancellationSignal::new();
        let occupier_request = delay_request(30_000);
        let queued_request = delay_request(30_000);

        let occupier_adapter = std::sync::Arc::clone(&adapter);
        let occupier_token = occupier_cancel.clone();
        let occupier = std::thread::spawn(move || {
            occupier_adapter.call(AdapterCall {
                request: &occupier_request,
                cancellation: &occupier_token,
            })
        });
        std::thread::sleep(Duration::from_millis(40));

        let queued_adapter = std::sync::Arc::clone(&adapter);
        let queued_token = queued_cancel.clone();
        let queued = std::thread::spawn(move || {
            queued_adapter.call(AdapterCall {
                request: &queued_request,
                cancellation: &queued_token,
            })
        });
        std::thread::sleep(Duration::from_millis(20));

        occupier_cancel.cancel();
        queued_cancel.cancel();
        let _ = occupier.join();
        let _ = queued.join();

        let started = Instant::now();
        drop(adapter);
        assert!(
            started.elapsed() < SESSION_THREAD_STOP_TIMEOUT + Duration::from_millis(500),
            "Drop must not block past the session-thread stop bound: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn cancelled_enqueue_does_not_hold_the_command_channel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = std::sync::Arc::new(FsZeroAdapter::new(dir.path(), "session-enqueue-cancel"));
        let occupier_cancel = CancellationSignal::new();
        let waiter_cancel = CancellationSignal::new();
        let occupier_request = delay_request(30_000);
        let queued_request = delay_request(30_000);
        let waiter_request = delay_request(30_000);

        let occupier_adapter = std::sync::Arc::clone(&adapter);
        let occupier_token = occupier_cancel.clone();
        let occupier = std::thread::spawn(move || {
            occupier_adapter.call(AdapterCall {
                request: &occupier_request,
                cancellation: &occupier_token,
            })
        });
        std::thread::sleep(Duration::from_millis(40));

        let queued_adapter = std::sync::Arc::clone(&adapter);
        let queued = std::thread::spawn(move || {
            queued_adapter.call(AdapterCall {
                request: &queued_request,
                cancellation: &CancellationSignal::new(),
            })
        });
        std::thread::sleep(Duration::from_millis(20));

        let waiter_adapter = std::sync::Arc::clone(&adapter);
        let waiter_token = waiter_cancel.clone();
        let waiter = std::thread::spawn(move || {
            waiter_adapter.call(AdapterCall {
                request: &waiter_request,
                cancellation: &waiter_token,
            })
        });
        std::thread::sleep(Duration::from_millis(20));
        waiter_cancel.cancel();

        let started = Instant::now();
        let waiter_error = waiter.join().expect("waiter").expect_err("cancelled enqueue");
        assert_eq!(waiter_error.error.kind, "cancelled");
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "cancelled enqueue must release promptly: {:?}",
            started.elapsed()
        );

        occupier_cancel.cancel();
        let _ = occupier.join();
        let _ = queued.join();
        drop(adapter);
    }

    #[test]
    fn durable_mkdir_fail_is_inert_and_degraded() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state_file = workspace.path().join("not-a-dir");
        std::fs::write(&state_file, b"x").expect("blocker file");
        let started = Instant::now();
        let adapter =
            FsZeroAdapter::new_with_state_root(workspace.path(), &state_file, "session-mkdir-fail");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "mkdir fail must not wait SESSION_INIT_TIMEOUT: {:?}",
            started.elapsed()
        );
        assert!(adapter.degraded(), "durable mkdir fail must set degraded");
        assert!(
            !adapter.session_is_live(),
            "mkdir fail must not start with_root"
        );
        let error = adapter
            .call(AdapterCall {
                request: &test_request(None),
                cancellation: &CancellationSignal::new(),
            })
            .expect_err("inert adapter must fail closed");
        assert_eq!(error.error.kind, "backend_unavailable");
        assert!(adapter.degraded(), "call must not clear degraded");
    }

    #[test]
    fn durable_open_fail_does_not_start_with_root() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state = tempfile::tempdir().expect("state");
        let store = state.path().join("fszero").join("store.sqlite3");
        std::fs::create_dir_all(&store).expect("blocker dir as sqlite path");
        let started = Instant::now();
        let adapter =
            FsZeroAdapter::new_with_state_root(workspace.path(), state.path(), "session-open-fail");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "durable-open fail must not wait SESSION_INIT_TIMEOUT: {:?}",
            started.elapsed()
        );
        assert!(adapter.degraded());
        assert!(
            !adapter.session_is_live(),
            "durable-open fail must not start with_root"
        );
        assert!(
            adapter.degraded(),
            "degraded must stay true after durable-open fail"
        );
    }

    #[test]
    fn explicit_in_memory_is_not_a_durable_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let adapter = FsZeroAdapter::new_in_memory(dir.path(), "session-in-memory");
        assert!(
            !adapter.degraded(),
            "explicit in-memory is not a durable-open failure"
        );
        assert!(adapter.session_is_live());
        assert_eq!(adapter.engine(), EngineIdentity::FsZero);
    }
