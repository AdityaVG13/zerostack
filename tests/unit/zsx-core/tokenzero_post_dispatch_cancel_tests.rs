    use super::*;
    use crate::adapter::{AdapterCall, DomainAdapter};
    use zero_abi::WorkerTrace;
    use zero_codemode::CancellationSignal;

    fn empty_trace() -> WorkerTrace {
        WorkerTrace {
            runtime_id: String::new(),
            cell_id: String::new(),
            request_id: String::new(),
            trace_id: String::new(),
            parent_span_id: None,
            worker_revision: String::new(),
            contract_digest: String::new(),
        }
    }

    fn request(op: &str, args: Value) -> CallRequest {
        CallRequest {
            request_id: "rd6o".into(),
            op: op.into(),
            args,
            deadline_unix_ms: None,
            trace: empty_trace(),
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn post_dispatch_cancel_keeps_committed_ok() {
        let root = std::env::temp_dir().join(format!("zerostack-rd6o-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let fixture = root.join("committed.txt");
        std::fs::write(&fixture, b"committed-ok").expect("fixture");
        let adapter = TokenZeroAdapter::new(&root, "rd6o").expect("adapter");

        let path = fixture.to_string_lossy().into_owned();
        let pre = request("read", serde_json::json!({ "path": path.clone() }));
        let cancelled = CancellationSignal::new();
        cancelled.cancel();
        let err = adapter
            .call(AdapterCall {
                request: &pre,
                cancellation: &cancelled,
            })
            .expect_err("pre-dispatch cancel");
        assert_eq!(err.error.kind, "cancelled");
        assert!(
            err.error.message.contains("before dispatch"),
            "{}",
            err.error.message
        );

        let late = CancellationSignal::new();
        let req = request("read", serde_json::json!({ "path": path }));
        let outcome = adapter.dispatch(&req);
        late.cancel();
        assert!(late.is_cancelled());
        assert!(outcome.is_ok(), "read fixture should commit Ok: {outcome:?}");
        // bind_outcome is the post-dispatch path and must not consult cancel.
        let _ = late.is_cancelled();
        let bound = adapter.bind_outcome(&req, outcome, Duration::from_millis(1));
        assert!(
            bound.is_ok(),
            "committed Ok must bind after post-dispatch cancel: {bound:?}"
        );
    }

    #[test]
    fn find_does_not_fail_on_unresolvable_tz_blob_in_content() {
        let root = std::env::temp_dir().join(format!("zerostack-6jpf-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp root");
        let phantom = "tz://blob/8bb4f40ade21d5e1f6c3dd5d955f2b46c06059863bdc0b497820481d86d03f60";
        let fixture = root.join("hits.txt");
        std::fs::write(&fixture, format!("cancel timeout {phantom}\n")).expect("fixture");
        let adapter = TokenZeroAdapter::new(&root, "6jpf").expect("adapter");
        let req = request(
            "find",
            serde_json::json!({
                "query": "cancel",
                "path": fixture.to_string_lossy(),
            }),
        );
        let outcome = adapter
            .call(AdapterCall {
                request: &req,
                cancellation: &CancellationSignal::new(),
            })
            .expect("find must return hits, not an unresolvable tz://blob error");
        let rendered = serde_json::to_string(&outcome.result.value).unwrap_or_default();
        assert!(
            !rendered.contains("not resolvable from the engine recovery store"),
            "{rendered}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

