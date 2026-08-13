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
        for op in [
            "execute_code",
            "fz_execute_code",
            "codemode_search",
            "fszero.exec",
            "tools/call",
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
