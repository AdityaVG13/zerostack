    use super::{
        follow_job_lifecycle, handle_jsonrpc, handle_jsonrpc_dispatching, handle_jsonrpc_request,
    };
    use crate::job_progress;
    use crate::{EngineConfig, TokenZeroEngine};
    use serde_json::Value;
    use std::panic::AssertUnwindSafe;

    fn test_engine() -> (tempfile::TempDir, TokenZeroEngine) {
        let dir = tempfile::tempdir().unwrap();
        let engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
        (dir, engine)
    }

    fn error_reason(response: &str) -> String {
        let parsed: Value = serde_json::from_str(response).unwrap();
        parsed["error"]["data"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn fractional_jsonrpc_id_is_rejected() {
        let (_dir, engine) = test_engine();
        let response = handle_jsonrpc(&engine, r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#)
            .expect("invalid id must still produce a JSON-RPC error");
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["error"]["code"], -32600, "{parsed:#}");
        assert_eq!(parsed["id"], Value::Null, "{parsed:#}");
        assert!(error_reason(&response).contains("integer"), "{response}");
    }

    #[test]
    fn integer_and_string_jsonrpc_ids_are_accepted() {
        let (_dir, engine) = test_engine();
        for request in [
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":"ping-1","method":"ping"}"#,
        ] {
            let response = handle_jsonrpc(&engine, request).unwrap();
            let parsed: Value = serde_json::from_str(&response).unwrap();
            assert!(parsed.get("result").is_some(), "{parsed:#}");
        }
    }

    #[test]
    fn follow_job_lifecycle_wait_error_emits_terminal() {
        let (_dir, engine) = test_engine();
        let session = engine.session_id().to_string();
        job_progress::remember_progress_token(&session, Some("pt-missing".into()));
        follow_job_lifecycle(&engine, "no-such-job");
        let notes = job_progress::take_notifications(&session);
        assert!(
            notes.iter().any(|note| {
                note["method"] == "notifications/progress"
                    && note["params"]["message"]
                        .as_str()
                        .is_some_and(|text| text.contains("failed"))
            }),
            "wait errors must still emit a terminal progress frame: {notes:?}"
        );
    }

    #[test]
    fn integer_progress_token_is_echoed_as_number() {
        let session = "pt-int-echo";
        job_progress::remember_progress_token_value(session, Some(serde_json::json!(7)));
        job_progress::observe(
            session,
            job_progress::JobEvent::Started {
                job_id: "job-int".into(),
            },
        );
        let notes = job_progress::take_notifications(session);
        assert!(
            notes.iter().any(|note| {
                note["method"] == "notifications/progress" && note["params"]["progressToken"] == 7
            }),
            "integer progressToken must round-trip as a JSON number: {notes:?}"
        );
    }

    #[test]
    fn fractional_progress_token_does_not_arm_progress_mode() {
        let token = job_progress::progress_token_from_params(&serde_json::json!({
            "_meta": { "progressToken": 1.5 }
        }));
        assert_eq!(
            token, None,
            "fractional progress tokens must not arm notifications/progress (same integer rule as JSON-RPC ids)"
        );
    }

    #[test]
    fn json_null_progress_token_falls_back_to_job_id() {
        let note = job_progress::plan_notification(
            job_progress::NotifyMode::Progress,
            Some(&Value::Null),
            &job_progress::JobEvent::Started {
                job_id: "job-null".into(),
            },
            false,
        )
        .expect("progress mode must still emit a frame");
        assert_eq!(
            note["params"]["progressToken"], "job-null",
            "JSON null is absence, not a literal progressToken: {note:?}"
        );
    }

    #[test]
    fn json_null_progress_token_does_not_arm_poll_only_clients() {
        let session = "pt-null-observe";
        job_progress::remember_client(session, "opencode", &serde_json::json!({}));
        job_progress::remember_progress_token_value(session, Some(Value::Null));
        job_progress::observe(
            session,
            job_progress::JobEvent::Started {
                job_id: "job-null".into(),
            },
        );
        let notes = job_progress::take_notifications(session);
        assert!(
            notes.is_empty(),
            "null token must not arm notifications/progress for poll-only clients: {notes:?}"
        );
        assert_eq!(
            job_progress::progress_token_from_params(&serde_json::json!({
                "_meta": { "progressToken": null }
            })),
            None
        );
    }

    #[test]
    fn initialize_recovers_poisoned_lifecycle_lock() {
        let (_dir, engine) = test_engine();
        let poisoned = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = engine.lifecycle.lock().unwrap();
            panic!("poison lifecycle");
        }));
        assert!(poisoned.is_err());
        assert!(engine.lifecycle.lock().is_err(), "mutex must be poisoned");

        let init = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"poison","version":"1.0.0"}}}"#,
        )
        .unwrap();
        let init: Value = serde_json::from_str(&init).unwrap();
        assert!(init.get("result").is_some(), "{init:#}");

        assert!(handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .is_none());

        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
        )
        .unwrap();
        let listed: Value = serde_json::from_str(&listed).unwrap();
        assert!(
            listed.get("result").is_some(),
            "poisoned initialize must still reach Ready: {listed:#}"
        );
    }

    #[test]
    fn classic_tools_list_meta_does_not_advertise_unfilterable_clusters() {
        let (_dir, engine) = test_engine();
        engine.mark_lifecycle_ready_for_tests();
        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&listed).unwrap();
        let clusters = parsed["result"]["_meta"]["tokenzero/toolFilter"]["availableClusters"]
            .as_array()
            .expect("tools/list must advertise availableClusters");
        let clusters: Vec<&str> = clusters.iter().filter_map(Value::as_str).collect();
        assert_eq!(clusters, ["material", "execution"]);
        let names: Vec<&str> = parsed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for exclusive in [
            "tz_execute_code",
            "tz_codemode_search",
            "tz_codemode_describe",
        ] {
            assert!(
                !names.contains(&exclusive),
                "Classic tools/list must not list {exclusive}: {names:?}"
            );
        }

        let rejected = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"tokenzero/toolCluster":"codemode"}}}"#,
        )
        .unwrap();
        let rejected: Value = serde_json::from_str(&rejected).unwrap();
        assert_eq!(rejected["error"]["data"]["kind"], "unknown_tool_cluster");
        let available = rejected["error"]["data"]["available_clusters"]
            .as_array()
            .unwrap();
        assert!(
            !available.iter().any(|cluster| cluster == "codemode"),
            "unknown_tool_cluster must not advertise the rejected cluster as available: {available:?}"
        );
    }

    #[test]
    fn classic_tools_list_does_not_advertise_undispatched_decision_views() {
        let (_dir, engine) = test_engine();
        engine.mark_lifecycle_ready_for_tests();
        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .unwrap();
        let lower = listed.to_lowercase();
        for needle in [
            "decision view",
            "decisionview",
            "reasoning-state",
            "opaque reasoning",
            "output novelty",
            "outputnovelty",
            "continuation class",
            "continuationkind",
            "decisionviewheadroom",
            "dv headroom",
            "decision_view",
            "decision-view",
            "reasoning_state",
            "output_novelty",
            "continuation_class",
            "headroom",
        ] {
            assert!(
                !lower.contains(needle),
                "Classic MCP tools/list advertises undispatched {needle:?}: {listed}"
            );
        }
    }

    #[test]
    fn classic_tools_list_does_not_advertise_missing_strict_mode() {
        let (_dir, engine) = test_engine();
        engine.mark_lifecycle_ready_for_tests();
        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        )
        .unwrap();
        let lower = listed.to_lowercase();
        for needle in ["strict mode", "strict-mode", "strict_mode", "strictmode"] {
            assert!(
                !lower.contains(needle),
                "Classic MCP tools/list advertises missing strict-mode as present ({needle:?}): {listed}"
            );
        }
    }

    #[test]
    fn advertised_prompts_capability_implements_prompts_get() {
        let (_dir, engine) = test_engine();
        let init = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"honesty","version":"1.0.0"}}}"#,
        )
        .unwrap();
        let init: Value = serde_json::from_str(&init).unwrap();
        assert!(
            init["result"]["capabilities"]["prompts"].is_object(),
            "initialize advertises prompts: {init:#}"
        );
        assert!(handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .is_none());

        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"prompts/list","params":{}}"#,
        )
        .unwrap();
        let listed: Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(
            listed["result"]["prompts"].as_array().map(Vec::len),
            Some(0),
            "{listed:#}"
        );

        let got = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"missing"}}"#,
        )
        .unwrap();
        let got: Value = serde_json::from_str(&got).unwrap();
        assert_eq!(got["error"]["code"], -32602, "{got:#}");
        assert_eq!(got["error"]["data"]["kind"], "unknown_prompt", "{got:#}");
        assert_eq!(got["error"]["data"]["provided"], "missing", "{got:#}");
        assert_eq!(
            got["error"]["data"]["available_prompts"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "{got:#}"
        );
    }

    #[test]
    fn codemode_initialize_instructions_include_report_tool_issue() {
        use tokenzero_core::McpToolSurface;
        let dir = tempfile::tempdir().unwrap();
        let mut engine = TokenZeroEngine::new(EngineConfig::for_root(dir.path()));
        engine.config.tool_surface = McpToolSurface::CodeMode;
        let init = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"honesty","version":"1.0.0"}}}"#,
        )
        .unwrap();
        let init: Value = serde_json::from_str(&init).unwrap();
        let instructions = init["result"]["instructions"].as_str().unwrap_or("");
        assert!(
            instructions.contains("tz_report_tool_issue"),
            "CodeMode initialize must not claim a three-tool catalog: {instructions}"
        );
        assert!(
            !instructions.contains("exactly tz_execute_code"),
            "CodeMode initialize must not use exclusive 'exactly' language that omits report: {instructions}"
        );

        let discovered = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":4,"method":"server/discover","params":{}}"#,
        )
        .unwrap();
        let discovered: Value = serde_json::from_str(&discovered).unwrap();
        let discover_instructions = discovered["result"]["instructions"].as_str().unwrap_or("");
        assert!(
            discover_instructions.contains("tz_report_tool_issue"),
            "CodeMode server/discover must list the advertised report tool: {discover_instructions}"
        );

        assert!(handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .is_none());
        let listed = handle_jsonrpc(
            &engine,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        )
        .unwrap();
        let listed: Value = serde_json::from_str(&listed).unwrap();
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(
            names.contains(&"tz_report_tool_issue"),
            "CodeMode tools/list must include the advertised report tool: {names:?}"
        );
    }

    fn handle_jsonrpc_with_induced_panic(engine: &TokenZeroEngine, line: &str) -> Option<String> {
        handle_jsonrpc_dispatching(engine, line, |engine, item| {
            if item.get("method").and_then(Value::as_str) == Some("tokenzero/internal/test-panic") {
                panic!("test-induced tool panic");
            }
            handle_jsonrpc_request(engine, item)
        })
    }

    #[test]
    fn single_request_panic_returns_internal_error_without_unwinding() {
        let (_dir, engine) = test_engine();
        let caught = std::panic::catch_unwind(AssertUnwindSafe(|| {
            handle_jsonrpc_with_induced_panic(
                &engine,
                r#"{"jsonrpc":"2.0","id":9,"method":"tokenzero/internal/test-panic","params":{}}"#,
            )
        }));
        let response = caught
            .expect("single-request handler panic must not unwind the JSON-RPC adapter")
            .expect("panicking request with id must still emit a JSON-RPC error");
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["id"], 9, "{parsed:#}");
        assert_eq!(parsed["error"]["code"], -32603, "{parsed:#}");
        assert_eq!(parsed["error"]["data"]["error_type"], "INTERNAL", "{parsed:#}");
        assert!(
            parsed["error"]["data"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("test-induced tool panic")),
            "{parsed:#}"
        );
    }

    #[test]
    fn panicking_notification_stays_suppressed() {
        let (_dir, engine) = test_engine();
        let caught = std::panic::catch_unwind(AssertUnwindSafe(|| {
            handle_jsonrpc_with_induced_panic(
                &engine,
                r#"{"jsonrpc":"2.0","method":"tokenzero/internal/test-panic","params":{}}"#,
            )
        }));
        let response = caught
            .expect("notification handler panic must not unwind the JSON-RPC adapter");
        assert!(
            response.is_none(),
            "JSON-RPC notifications must not grow a response after a handler panic: {response:?}"
        );
    }

