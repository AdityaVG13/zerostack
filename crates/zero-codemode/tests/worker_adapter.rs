#![cfg(feature = "worker-fixture")]
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{
    CallRequest, DEFAULT_MAX_FRAME_BYTES, FrameCodecError, TIMELINE_CLOSURE_TOLERANCE_NS_V1,
    TelemetryRequestV1, WorkerTokenCountKind, WorkerTrace, encode_frame,
};
use zero_codemode::worker::{
    CancellationSignal, StaticWorkerFactory, WorkerAdapterError, WorkerClient, WorkerClientConfig,
    WorkerContext, WorkerEvent, WorkerFactory, WorkerRegistry, WorkerSpec,
};

const CONTRACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REGISTRY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REVISION: &str = "fixture-revision";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"))
}

fn factory(mode: &str) -> Arc<StaticWorkerFactory> {
    Arc::new(StaticWorkerFactory::new(binary(), REVISION, CONTRACT, REGISTRY).arg(mode))
}

fn registry(mode: &str) -> WorkerRegistry {
    let mut registry = WorkerRegistry::new();
    for engine in engines() {
        registry.register(engine, factory(mode)).unwrap();
    }
    registry
}

fn client_with_pid(mode: &str, pid_file: &std::path::Path) -> WorkerClient {
    let factory = Arc::new(
        StaticWorkerFactory::new(binary(), REVISION, CONTRACT, REGISTRY)
            .arg(mode)
            .env("ZEROSTACK_DESCENDANT_PID_FILE", pid_file.to_str().unwrap()),
    );
    let mut registry = WorkerRegistry::new();
    registry.register(EngineIdentity::FsZero, factory).unwrap();
    registry
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap()
}

fn engines() -> [EngineIdentity; 3] {
    [
        EngineIdentity::FsZero,
        EngineIdentity::GraphZero,
        EngineIdentity::TokenZero,
    ]
}

fn context(engine: EngineIdentity) -> WorkerContext {
    WorkerContext {
        engine,
        store_root: PathBuf::from(format!("/tmp/zerostack-worker-{}", engine.as_str())),
        session_id: format!("session-{}", engine.as_str()),
    }
}

fn request(id: &str, args: serde_json::Value, deadline: Option<u64>) -> CallRequest {
    CallRequest {
        request_id: id.into(),
        op: "fixture.echo".into(),
        args,
        deadline_unix_ms: deadline,
        trace: WorkerTrace {
            runtime_id: "runtime".into(),
            cell_id: "cell".into(),
            request_id: id.into(),
            trace_id: "trace".into(),
            parent_span_id: None,
            worker_revision: REVISION.into(),
            contract_digest: CONTRACT.into(),
        },
        approval_grant: None,
        telemetry_request: None,
    }
}

fn encoded_frame_bytes(frame: serde_json::Value) -> u64 {
    encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).unwrap().len() as u64
}

fn race_completion_bytes(mode: &str) -> u64 {
    let call = request("race-first", json!({}), None);
    let trace = serde_json::to_value(&call.trace).unwrap();
    let frame = if mode.starts_with("result") {
        json!({
            "kind": "result",
            "request_id": "race-first",
            "result": {
                "value": {
                    "args": call.args,
                    "store_root": "/tmp/zerostack-worker-fszero",
                    "session_id": "session-fszero"
                },
                "metadata": {
                    "effect": "read_only",
                    "approval": {"state": "not_required"},
                    "revert": {"supported": false},
                    "ownership": {
                        "engine": "fszero",
                        "session_id": "session-fszero",
                        "refs": []
                    },
                    "trace": trace
                }
            }
        })
    } else {
        json!({
            "kind": "error",
            "request_id": "race-first",
            "error": {
                "kind": "fixture",
                "message": "remote error before cancel ack",
                "retryable": false,
                "details": {"fixture_detail":"preserved"}
            },
            "trace": trace
        })
    };
    encoded_frame_bytes(frame)
}

fn race_cancel_ack_bytes() -> u64 {
    encoded_frame_bytes(json!({
        "kind": "cancel_ack",
        "request_id": "race-first",
        "cancelled": false
    }))
}

#[test]
fn graphzero_spec_binds_repo_env_to_handshake_root() {
    let context = context(EngineIdentity::GraphZero);
    let spec = StaticWorkerFactory::new(binary(), REVISION, CONTRACT, REGISTRY)
        .env("GRAPHZERO_REPO", "/wrong/root")
        .spec(&context)
        .unwrap();
    assert_eq!(
        spec.env.get("GRAPHZERO_REPO").map(String::as_str),
        context.store_root.to_str()
    );
}

#[test]
fn all_engines_start_dispatch_bind_ref_scheme_propagate_context_and_reap() {
    for engine in engines() {
        let context = context(engine);
        let mut client = registry("normal")
            .launch(context.clone(), WorkerClientConfig::default())
            .unwrap();
        let result = client
            .dispatch(request("echo", json!({"engine": engine.as_str()}), None))
            .unwrap();
        assert_eq!(
            result.value["store_root"],
            context.store_root.to_str().unwrap()
        );
        assert_eq!(result.value["session_id"], context.session_id);
        assert_eq!(result.value["args"]["engine"], engine.as_str());
        assert_eq!(client.accounting().requests, 1);
        client.shutdown().unwrap();
        assert!(client.is_reaped());
        assert!(client.terminal_status().unwrap().success());
    }
}

#[test]
fn registration_rejects_duplicate_and_unknown_identity() {
    let mut registry = WorkerRegistry::new();
    registry
        .register(EngineIdentity::FsZero, factory("normal"))
        .unwrap();
    assert!(matches!(
        registry.register(EngineIdentity::FsZero, factory("normal")),
        Err(WorkerAdapterError::DuplicateRegistration(
            EngineIdentity::FsZero
        ))
    ));
    assert!(matches!(
        registry.launch(
            context(EngineIdentity::GraphZero),
            WorkerClientConfig::default()
        ),
        Err(WorkerAdapterError::UnknownRegistration(
            EngineIdentity::GraphZero
        ))
    ));
}

#[test]
fn handshake_fails_closed_for_every_binding_pin_and_ref_scheme() {
    for mode in [
        "skew-engine",
        "skew-contract",
        "skew-registry",
        "skew-revision",
        "skew-ref",
    ] {
        let error = registry(mode)
            .launch(
                context(EngineIdentity::FsZero),
                WorkerClientConfig::default(),
            )
            .err()
            .expect("skew must fail");
        assert!(matches!(
            error,
            WorkerAdapterError::Protocol(FrameCodecError::InvalidContract(_))
                | WorkerAdapterError::Handshake(_)
        ));
    }
}

#[test]
fn deadline_kills_reaps_and_terminal_client_rejects_dispatch() {
    let mut client = registry("sleep")
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    let deadline = unix_ms() + 40;
    assert!(matches!(
        client.dispatch(request("slow", json!({}), Some(deadline))),
        Err(WorkerAdapterError::Deadline { .. })
    ));
    assert_terminal_and_rejects(&mut client);
}

#[test]
fn external_cancellation_interrupts_pending_call_and_reaps() {
    let mut client = registry("hold")
        .launch(
            context(EngineIdentity::GraphZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    let signal = CancellationSignal::new();
    let trigger = signal.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        trigger.cancel();
    });
    assert!(matches!(
        client.dispatch_with_cancel(request("pending", json!({}), None), &signal),
        Err(WorkerAdapterError::Cancelled { request_id }) if request_id == "pending"
    ));
    thread.join().unwrap();
    assert_terminal_and_rejects(&mut client);
}

#[test]
fn direct_crash_stderr_is_complete_and_explicitly_truncated() {
    let mut config = WorkerClientConfig::default();
    config.max_stderr_bytes = 8;
    let mut client = registry("crash")
        .launch(context(EngineIdentity::TokenZero), config)
        .unwrap();
    let error = client
        .dispatch(request("crash", json!({}), None))
        .unwrap_err();
    match error {
        WorkerAdapterError::Crash {
            status: Some(status),
            stderr,
        } => {
            assert_eq!(status.code(), Some(17));
            assert!(stderr.text.len() <= 8);
            assert!(stderr.complete);
            assert!(stderr.truncated);
            assert!(stderr.observed_bytes > stderr.text.len() as u64);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(client.is_reaped());
}

#[test]
fn standalone_cancel_false_is_non_poisoning() {
    let mut client = registry("normal")
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    assert!(!client.cancel("unknown", None).unwrap());
    assert!(
        client
            .dispatch(request("after-false", json!({}), None))
            .is_ok()
    );
    client.shutdown().unwrap();
}

#[test]
fn rejected_midflight_cancel_accepts_one_correlated_result_without_resend() {
    let mut client = registry("cancel-false-result")
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    let signal = CancellationSignal::new();
    signal.cancel();
    let result = client
        .dispatch_with_cancel(request("race", json!({"ok": true}), None), &signal)
        .unwrap();
    assert_eq!(result.value["args"]["ok"], true);
    assert!(!client.is_terminal());
    client.shutdown().unwrap();
}

#[test]
fn huge_deadline_overflow_is_typed_and_reaped() {
    let mut client = registry("normal")
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    assert!(matches!(
        client.dispatch(request("huge-deadline", json!({}), Some(u64::MAX))),
        Err(WorkerAdapterError::DeadlineOverflow { .. })
    ));
    assert_terminal_and_rejects(&mut client);
}

#[test]
fn every_input_output_and_protocol_failure_reaps_and_observes_bounds() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let observed = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };

    let mut input_config = observed.clone();
    input_config.limits.max_frame_bytes = 1024;
    let mut input = registry("normal")
        .launch(context(EngineIdentity::FsZero), input_config)
        .unwrap();
    assert!(matches!(
        input.dispatch(request("huge", json!({"value": "z".repeat(2048)}), None)),
        Err(WorkerAdapterError::Bounds {
            stream: "stdin",
            ..
        })
    ));
    assert_terminal_and_rejects(&mut input);

    let mut output_config = observed.clone();
    output_config.limits.max_output_bytes = 1024;
    let mut output = registry("large-output")
        .launch(context(EngineIdentity::FsZero), output_config)
        .unwrap();
    assert!(matches!(
        output.dispatch(request("large", json!({}), None)),
        Err(WorkerAdapterError::Bounds {
            stream: "stdout",
            ..
        })
    ));
    assert_terminal_and_rejects(&mut output);

    let mut malformed = registry("malformed")
        .launch(context(EngineIdentity::FsZero), observed)
        .unwrap();
    assert!(matches!(
        malformed.dispatch(request("bad", json!({}), None)),
        Err(WorkerAdapterError::Protocol(FrameCodecError::InvalidJson(
            _
        )))
    ));
    assert_terminal_and_rejects(&mut malformed);

    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .filter(|event| event.event == WorkerEvent::BoundsError)
            .count()
            >= 2
    );
    assert!(
        events
            .iter()
            .any(|event| event.event == WorkerEvent::ProtocolError)
    );
}

#[test]
fn mismatched_result_error_and_cancel_ids_fail_closed_and_reap() {
    for mode in ["mismatch-result", "mismatch-error", "mismatch-error-trace"] {
        let mut client = registry(mode)
            .launch(
                context(EngineIdentity::FsZero),
                WorkerClientConfig::default(),
            )
            .unwrap();
        assert!(matches!(
            client.dispatch(request("expected", json!({}), None)),
            Err(WorkerAdapterError::Handshake(_) | WorkerAdapterError::Protocol(_))
        ));
        assert_terminal_and_rejects(&mut client);
    }

    let mut client = registry("mismatch-cancel")
        .launch(
            context(EngineIdentity::FsZero),
            WorkerClientConfig::default(),
        )
        .unwrap();
    assert!(matches!(
        client.cancel("expected", None),
        Err(WorkerAdapterError::Handshake(_))
    ));
    assert_terminal_and_rejects(&mut client);
}

#[test]
fn matching_remote_error_emits_dispatch_observation() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let config = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };
    let mut client = registry("remote-error")
        .launch(context(EngineIdentity::FsZero), config)
        .unwrap();
    assert!(matches!(
        client.dispatch(request("remote", json!({}), None)),
        Err(WorkerAdapterError::Remote { .. })
    ));
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.event == WorkerEvent::Dispatch
                && event.request_id.as_deref() == Some("remote"))
    );
    client.shutdown().unwrap();
}

#[test]
fn enabled_transport_telemetry_closes_and_preserves_worker_token_units() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let config = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };
    let mut client = registry("normal")
        .launch(context(EngineIdentity::TokenZero), config)
        .unwrap();
    let mut call = request("telemetry", json!({"payload":"domain"}), None);
    call.telemetry_request = Some(TelemetryRequestV1 {
        engine_stage_timeline: true,
        worker_token_accounting: true,
    });
    let result = client.dispatch(call).unwrap();
    assert_eq!(result.value["args"], json!({"payload":"domain"}));
    let events = events.lock().unwrap();
    let receipt = events
        .iter()
        .find(|event| {
            event.event == WorkerEvent::Dispatch && event.request_id.as_deref() == Some("telemetry")
        })
        .and_then(|event| event.settlement.as_ref())
        .expect("dispatch settlement receipt");
    let timeline = receipt.engine_timeline.as_ref().expect("engine timeline");
    let partitioned_ns = u128::from(timeline.total_ns)
        + u128::from(receipt.raw_worker_result_settlement_ns)
        + u128::from(receipt.residual_transport_ns);
    assert_eq!(
        u128::from(receipt.total_ns).abs_diff(partitioned_ns),
        u128::from(receipt.closure_error_ns)
    );
    assert!(receipt.closure_error_ns <= TIMELINE_CLOSURE_TOLERANCE_NS_V1);
    assert_eq!(timeline.total_ns, 300);
    assert_eq!(timeline.spans[0].stage, "fixture_decode");
    assert_eq!(timeline.spans[1].start_ns, 100);
    let accounting = receipt
        .worker_token_accounting
        .as_ref()
        .expect("worker token accounting");
    assert_eq!(accounting.tokenizer_id, "fixture-tokenizer-v1");
    assert_eq!(accounting.count_kind, WorkerTokenCountKind::Exact);
    assert_eq!(accounting.visible_tokens, 4);
    assert_eq!(accounting.exact_ref_tokens, Some(0));
    drop(events);
    client.shutdown().unwrap();
}

#[test]
fn remote_failure_preserves_details_trace_and_transport_receipt() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let config = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };
    let mut client = registry("remote-error")
        .launch(context(EngineIdentity::FsZero), config)
        .unwrap();
    let mut call = request("remote-telemetry", json!({}), None);
    call.telemetry_request = Some(TelemetryRequestV1 {
        engine_stage_timeline: true,
        worker_token_accounting: true,
    });
    match client.dispatch(call) {
        Err(WorkerAdapterError::Remote {
            details,
            trace,
            retryable,
            ..
        }) => {
            assert_eq!(
                details.as_deref(),
                Some(&json!({"fixture_detail":"preserved"}))
            );
            assert_eq!(trace.unwrap().request_id, "remote-telemetry");
            assert!(!retryable);
        }
        other => panic!("expected typed remote failure, got {other:?}"),
    }
    let events = events.lock().unwrap();
    let receipt = events
        .iter()
        .find(|event| event.request_id.as_deref() == Some("remote-telemetry"))
        .and_then(|event| event.settlement.as_ref())
        .expect("failure settlement receipt");
    assert!(receipt.engine_timeline.is_some());
    assert!(receipt.worker_token_accounting.is_some());
    assert!(receipt.closure_error_ns <= TIMELINE_CLOSURE_TOLERANCE_NS_V1);
    drop(events);
    client.shutdown().unwrap();
}

#[test]
fn telemetry_is_request_scoped_and_does_not_leak_to_default_calls() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let config = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };
    let mut client = registry("normal")
        .launch(context(EngineIdentity::GraphZero), config)
        .unwrap();
    let mut enabled = request("enabled", json!({}), None);
    enabled.telemetry_request = Some(TelemetryRequestV1 {
        engine_stage_timeline: true,
        worker_token_accounting: true,
    });
    client.dispatch(enabled).unwrap();
    client
        .dispatch(request("disabled", json!({}), None))
        .unwrap();
    let events = events.lock().unwrap();
    let receipt = |id: &str| {
        events
            .iter()
            .find(|event| event.request_id.as_deref() == Some(id))
            .and_then(|event| event.settlement.as_ref())
            .unwrap()
    };
    assert!(receipt("enabled").engine_timeline.is_some());
    assert!(receipt("enabled").worker_token_accounting.is_some());
    assert!(receipt("disabled").engine_timeline.is_none());
    assert!(receipt("disabled").worker_token_accounting.is_none());
    drop(events);
    client.shutdown().unwrap();
}

#[test]
fn concurrent_clients_do_not_cross_contaminate_transport_telemetry() {
    let handles = (0..4)
        .map(|index| {
            std::thread::spawn(move || {
                let events = Arc::new(Mutex::new(Vec::new()));
                let sink = events.clone();
                let config = WorkerClientConfig {
                    observer: Some(Arc::new(move |event| {
                        sink.lock().unwrap().push(event.clone())
                    })),
                    ..WorkerClientConfig::default()
                };
                let mut client = registry("normal")
                    .launch(context(EngineIdentity::FsZero), config)
                    .unwrap();
                let id = format!("concurrent-{index}");
                let enabled = index % 2 == 0;
                let mut call = request(&id, json!({}), None);
                if enabled {
                    call.telemetry_request = Some(TelemetryRequestV1 {
                        engine_stage_timeline: true,
                        worker_token_accounting: true,
                    });
                }
                client.dispatch(call).unwrap();
                let receipt = events
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|event| event.request_id.as_deref() == Some(id.as_str()))
                    .and_then(|event| event.settlement.clone())
                    .unwrap();
                client.shutdown().unwrap();
                (enabled, receipt)
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        let (enabled, receipt) = handle.join().unwrap();
        assert_eq!(receipt.engine_timeline.is_some(), enabled);
        assert_eq!(receipt.worker_token_accounting.is_some(), enabled);
    }
}

#[test]
fn missing_or_unsolicited_transport_telemetry_fails_closed() {
    for mode in [
        "omit-telemetry",
        "unsolicited-telemetry",
        "unclosable-telemetry",
    ] {
        let mut client = registry(mode)
            .launch(
                context(EngineIdentity::FsZero),
                WorkerClientConfig::default(),
            )
            .unwrap();
        let mut call = request("telemetry-skew", json!({}), None);
        if mode != "unsolicited-telemetry" {
            call.telemetry_request = Some(TelemetryRequestV1 {
                engine_stage_timeline: true,
                worker_token_accounting: true,
            });
        }
        assert!(matches!(
            client.dispatch(call),
            Err(WorkerAdapterError::Handshake(_))
        ));
        assert_terminal_and_rejects(&mut client);
    }
}

#[cfg(unix)]
#[test]
fn inherited_pipe_reports_incomplete_then_shutdown_kills_descendant() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let mut client = client_with_pid("inherited-open", &pid_file);
    let pid = read_descendant_pid(&pid_file);
    let capture = client.stderr_capture();
    assert!(!capture.complete);
    assert!(!capture.truncated);
    client.shutdown().unwrap();
    assert_descendant_gone(pid);
}

#[cfg(unix)]
#[test]
fn deadline_cancel_and_crash_kill_fixture_descendants() {
    for mode in ["tree-deadline", "tree-cancel", "tree-crash"] {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("pid");
        let mut client = client_with_pid(mode, &pid_file);
        match mode {
            "tree-deadline" => {
                let deadline = unix_ms() + 50;
                assert!(matches!(
                    client.dispatch(request("tree", json!({}), Some(deadline))),
                    Err(WorkerAdapterError::Deadline { .. })
                ));
            }
            "tree-cancel" => {
                let signal = CancellationSignal::new();
                let trigger = signal.clone();
                let thread = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(30));
                    trigger.cancel();
                });
                assert!(matches!(
                    client.dispatch_with_cancel(request("tree", json!({}), None), &signal),
                    Err(WorkerAdapterError::Cancelled { .. })
                ));
                thread.join().unwrap();
            }
            "tree-crash" => {
                assert!(matches!(
                    client.dispatch(request("tree", json!({}), None)),
                    Err(WorkerAdapterError::Crash { .. })
                ));
            }
            _ => unreachable!(),
        }
        let pid = read_descendant_pid(&pid_file);
        assert!(client.is_reaped());
        assert_descendant_gone(pid);
    }
}

#[cfg(unix)]
#[test]
fn lifecycle_reap_p95_is_below_one_second() {
    const RUNS: usize = 20;
    let mut completed = Vec::with_capacity(RUNS);
    let mut cancelled = Vec::with_capacity(RUNS);
    let mut timed_out = Vec::with_capacity(RUNS);
    let mut crashed = Vec::with_capacity(RUNS);
    let mut session_closed = Vec::with_capacity(RUNS);

    for iteration in 0..RUNS {
        let mut client = registry("normal")
            .launch(
                context(EngineIdentity::FsZero),
                WorkerClientConfig::default(),
            )
            .unwrap();
        assert!(
            client
                .dispatch(request(&format!("complete-{iteration}"), json!({}), None,))
                .is_ok()
        );
        let started = Instant::now();
        client.shutdown().unwrap();
        assert!(client.is_reaped());
        completed.push(started.elapsed());

        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("cancelled.pid");
        let mut client = client_with_pid("tree-cancel", &pid_file);
        let signal = CancellationSignal::new();
        let trigger = signal.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            trigger.cancel();
        });
        let started = Instant::now();
        assert!(matches!(
            client.dispatch_with_cancel(
                request(&format!("cancel-{iteration}"), json!({}), None),
                &signal,
            ),
            Err(WorkerAdapterError::Cancelled { .. })
        ));
        thread.join().unwrap();
        let descendant = read_descendant_pid(&pid_file);
        assert_descendant_gone(descendant);
        cancelled.push(started.elapsed());

        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("timeout.pid");
        let mut client = client_with_pid("tree-deadline", &pid_file);
        let started = Instant::now();
        assert!(matches!(
            client.dispatch(request(
                &format!("timeout-{iteration}"),
                json!({}),
                Some(unix_ms() + 50),
            )),
            Err(WorkerAdapterError::Deadline { .. })
        ));
        let descendant = read_descendant_pid(&pid_file);
        assert_descendant_gone(descendant);
        timed_out.push(started.elapsed());

        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("crash.pid");
        let mut client = client_with_pid("tree-crash", &pid_file);
        let started = Instant::now();
        assert!(matches!(
            client.dispatch(request(&format!("crash-{iteration}"), json!({}), None,)),
            Err(WorkerAdapterError::Crash { .. })
        ));
        let descendant = read_descendant_pid(&pid_file);
        assert_descendant_gone(descendant);
        crashed.push(started.elapsed());

        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("session.pid");
        let client = client_with_pid("inherited-open", &pid_file);
        let descendant = read_descendant_pid(&pid_file);
        let started = Instant::now();
        drop(client);
        assert_descendant_gone(descendant);
        session_closed.push(started.elapsed());
    }

    assert_p95_below_one_second("completed", &mut completed);
    assert_p95_below_one_second("cancelled", &mut cancelled);
    assert_p95_below_one_second("timed_out", &mut timed_out);
    assert_p95_below_one_second("crashed", &mut crashed);
    assert_p95_below_one_second("session_closed", &mut session_closed);
}

#[test]
fn injected_spin_and_cancellation_loss_hit_hard_deadlines() {
    const RUNS: usize = 20;
    for mode in ["spin", "ignore-cancel"] {
        let mut samples = Vec::with_capacity(RUNS);
        for iteration in 0..RUNS {
            let mut client = registry(mode)
                .launch(
                    context(EngineIdentity::TokenZero),
                    WorkerClientConfig::default(),
                )
                .unwrap();
            let request = request(
                &format!("hard-{mode}-{iteration}"),
                json!({}),
                Some(unix_ms() + 75),
            );
            let started = Instant::now();
            let outcome = if mode == "ignore-cancel" {
                let signal = CancellationSignal::new();
                signal.cancel();
                client.dispatch_with_cancel(request, &signal)
            } else {
                client.dispatch(request)
            };
            assert!(matches!(outcome, Err(WorkerAdapterError::Deadline { .. })));
            assert!(client.is_reaped());
            samples.push(started.elapsed());
        }
        assert_p95_below_one_second(mode, &mut samples);
    }
}

#[test]
fn result_and_error_before_false_cancel_ack_are_correlated_and_reusable() {
    for mode in ["result-first-cancel-false", "error-first-cancel-false"] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let config = WorkerClientConfig {
            observer: Some(Arc::new(move |event| {
                sink.lock().unwrap().push(event.clone())
            })),
            ..WorkerClientConfig::default()
        };
        let mut client = registry(mode)
            .launch(context(EngineIdentity::FsZero), config)
            .unwrap();
        let signal = CancellationSignal::new();
        signal.cancel();
        let outcome = client.dispatch_with_cancel(request("race-first", json!({}), None), &signal);
        if mode.starts_with("result") {
            assert!(outcome.is_ok());
        } else {
            assert!(matches!(outcome, Err(WorkerAdapterError::Remote { .. })));
        }
        let dispatch = events
            .lock()
            .unwrap()
            .iter()
            .find(|event| {
                event.event == WorkerEvent::Dispatch
                    && event.request_id.as_deref() == Some("race-first")
            })
            .cloned()
            .expect("race dispatch observation");
        assert_eq!(dispatch.output_bytes, race_completion_bytes(mode));
        assert_ne!(dispatch.output_bytes, race_cancel_ack_bytes());
        assert!(
            client
                .dispatch(request("reuse", json!({"reuse": true}), None))
                .is_ok()
        );
        client.shutdown().unwrap();
    }
}

#[test]
fn bounded_writer_handles_nonreading_workers_without_orphans() {
    let temp = tempfile::tempdir().unwrap();
    let startup_pid = temp.path().join("startup-pid");
    let startup_factory = Arc::new(
        StaticWorkerFactory::new(binary(), REVISION, CONTRACT, REGISTRY)
            .arg("stop-reading-startup")
            .env(
                "ZEROSTACK_DESCENDANT_PID_FILE",
                startup_pid.to_str().unwrap(),
            ),
    );
    let mut startup_registry = WorkerRegistry::new();
    startup_registry
        .register(EngineIdentity::FsZero, startup_factory)
        .unwrap();
    let mut short = WorkerClientConfig::default();
    short.handshake_timeout = Duration::from_millis(80);
    short.shutdown_timeout = Duration::from_millis(80);
    short.limits.default_deadline_ms = 80;
    let started = std::time::Instant::now();
    assert!(
        startup_registry
            .launch(context(EngineIdentity::FsZero), short.clone())
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    let pid = read_descendant_pid(&startup_pid);
    assert_descendant_gone(pid);

    for operation in ["dispatch", "cancel", "shutdown", "drop"] {
        let pid_file = temp.path().join(format!("{operation}-pid"));
        let factory = Arc::new(
            StaticWorkerFactory::new(binary(), REVISION, CONTRACT, REGISTRY)
                .arg("stop-reading-after-handshake")
                .env("ZEROSTACK_DESCENDANT_PID_FILE", pid_file.to_str().unwrap()),
        );
        let mut worker_registry = WorkerRegistry::new();
        worker_registry
            .register(EngineIdentity::FsZero, factory)
            .unwrap();
        let mut client = worker_registry
            .launch(context(EngineIdentity::FsZero), short.clone())
            .unwrap();
        let pid = read_descendant_pid(&pid_file);
        let started = std::time::Instant::now();
        match operation {
            "dispatch" => {
                let huge = "x".repeat(900_000);
                assert!(matches!(
                    client.dispatch(request("blocked", json!({"huge": huge}), None)),
                    Err(WorkerAdapterError::WriterTimeout | WorkerAdapterError::WriterBusy)
                ));
                assert!(client.is_reaped());
            }
            "cancel" => assert!(client.cancel("blocked", None).is_err()),
            "shutdown" => assert!(client.shutdown().is_err()),
            "drop" => drop(client),
            _ => unreachable!(),
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_descendant_gone(pid);
    }
}

#[test]
fn invalid_specs_fail_before_spawn() {
    let base = WorkerSpec {
        engine: EngineIdentity::FsZero,
        program: binary(),
        args: vec!["normal".into()],
        env: BTreeMap::new(),
        store_root: PathBuf::from("/tmp/store"),
        session_id: "session".into(),
        expected_worker_revision: REVISION.into(),
        expected_contract_digest: CONTRACT.into(),
        expected_registry_digest: REGISTRY.into(),
    };
    for invalid in [
        WorkerSpec {
            store_root: PathBuf::new(),
            ..base.clone()
        },
        WorkerSpec {
            session_id: String::new(),
            ..base.clone()
        },
        WorkerSpec {
            expected_worker_revision: String::new(),
            ..base.clone()
        },
        WorkerSpec {
            expected_contract_digest: String::new(),
            ..base.clone()
        },
        WorkerSpec {
            expected_registry_digest: String::new(),
            ..base.clone()
        },
    ] {
        assert!(matches!(
            WorkerClient::spawn(invalid, WorkerClientConfig::default()),
            Err(WorkerAdapterError::Configuration(_))
        ));
    }

    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let invalid = WorkerSpec {
            store_root: PathBuf::from(OsString::from_vec(vec![0xff])),
            ..base
        };
        assert!(matches!(
            WorkerClient::spawn(invalid, WorkerClientConfig::default()),
            Err(WorkerAdapterError::Configuration(_))
        ));
    }
}

#[test]
fn observations_and_saturating_accounting_cover_lifecycle() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let config = WorkerClientConfig {
        observer: Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone())
        })),
        ..WorkerClientConfig::default()
    };
    let mut client = registry("normal")
        .launch(context(EngineIdentity::FsZero), config)
        .unwrap();
    client
        .dispatch(request("observed", json!({}), None))
        .unwrap();
    client.shutdown().unwrap();
    let accounting = client.accounting();
    assert!(accounting.input_bytes > 0 && accounting.output_bytes > 0);
    let events = events.lock().unwrap();
    for expected in [
        WorkerEvent::Started,
        WorkerEvent::Handshake,
        WorkerEvent::Dispatch,
        WorkerEvent::Shutdown,
    ] {
        assert!(events.iter().any(|event| event.event == expected));
    }
    assert!(events.iter().any(|event| event.input_bytes > 0));
    assert!(events.iter().any(|event| event.output_bytes > 0));
}

fn assert_p95_below_one_second(label: &str, samples: &mut [Duration]) {
    assert!(!samples.is_empty(), "{label} must have samples");
    samples.sort_unstable();
    let rank = (samples.len() * 95).div_ceil(100);
    let p95 = samples[rank - 1];
    eprintln!("{label} lifecycle p95={p95:?} samples={}", samples.len());
    assert!(
        p95 < Duration::from_secs(1),
        "{label} lifecycle p95 {p95:?} exceeded one second"
    );
}

fn read_descendant_pid(path: &std::path::Path) -> i32 {
    for _ in 0..100 {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("descendant pid was not published");
}

fn assert_descendant_gone(pid: i32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if !descendant_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !descendant_exists(pid),
        "descendant process {pid} survived process-tree teardown"
    );
}

fn descendant_exists(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("execute kill -0 descendant probe")
        .success()
}

fn assert_terminal_and_rejects(client: &mut WorkerClient) {
    assert!(client.is_reaped());
    assert!(client.terminal_status().is_some());
    assert!(matches!(
        client.dispatch(request("after-terminal", json!({}), None)),
        Err(WorkerAdapterError::Configuration(_))
    ));
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}
