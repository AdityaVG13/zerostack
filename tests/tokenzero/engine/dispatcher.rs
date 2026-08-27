//! Typed domain dispatcher identity and dependency tests (tokenzero-irx9.2).

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tokenzero_core::operation_abi::{DomainErrorKind, all_operations};
use tokenzero_engine::session_persist::{session_memory_path, with_session_root};
use tokenzero_engine::ledger::ledger_path_for_cache;
use tokenzero_engine::{
    DispatchSurface, EngineConfig, TokenZeroEngine, dispatch_cli, dispatch_codemode_method,
    dispatch_count, dispatch_mcp_tool, dispatch_operation, dispatch_raw_worker, domain_fastmcp_ops,
    is_domain_operation, last_dispatch_profile,
};

fn engine_for(root: &Path) -> TokenZeroEngine {
    let mut config = EngineConfig::for_root(root);
    config.session_dedup = false;
    config.diff_reads = false;
    config.fetch_enabled = false;
    TokenZeroEngine::new(config)
}

fn engine_with_session_dedup(root: &Path) -> TokenZeroEngine {
    let mut config = EngineConfig::for_root(root);
    config.session_dedup = true;
    config.diff_reads = false;
    config.fetch_enabled = false;
    TokenZeroEngine::new(config)
}

fn minimal_args(op: &str) -> Value {
    match op {
        "tz_read" | "read" => json!({"path": "note.txt"}),
        "tz_find" | "tz_grep" => json!({"query": "dispatcher", "path": "."}),
        "tz_recall" => json!({"query": "dispatcher"}),
        "tz_glob" => json!({"pattern": "*.txt", "path": "."}),
        "tz_tree" => json!({"path": ".", "depth": 1}),
        "tz_edit" => json!({
            "path": "note.txt",
            "edits": [{"find": "dispatcher-identity", "replace": "dispatcher-identity"}],
            "dry_run": true
        }),
        "tz_shell" => json!({"command": "true"}),
        "tz_ingest" => json!({"text": "hello-from-dispatcher"}),
        "tz_expand" => json!({"ref": "tz://deadbeef"}),
        "tz_mem" => json!({}),
        "tz_cache_pack" => json!({"scope": "agent"}),
        "tz_rewrite" => json!({"command": "echo hi"}),
        "tz_discover" => json!({}),
        "tz_report_tool_issue" => json!({
            "tool": "zero_execute",
            "summary": "dispatcher identity probe"
        }),
        "tz_batch" => json!({
            "ops": [{"tool": "tz_mem", "args": {}}]
        }),
        "tz_fetch" => json!({"url": "https://example.invalid/"}),
        _ => json!({}),
    }
}

#[test]
fn one_operation_same_dispatcher_from_all_adapters() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"dispatcher-identity").unwrap();

    let mcp = engine_for(root.path());
    let raw = engine_for(root.path());
    let cli = engine_for(root.path());
    let cm = engine_for(root.path());

    let before = dispatch_count();
    let args = json!({"path": root.path().join("note.txt").display().to_string()});

    let mcp_out = dispatch_mcp_tool(&mcp, "tz_read", &args).expect("mcp");
    let raw_out = dispatch_raw_worker(&raw, "tz_read", &args);
    let cli_out = dispatch_cli(&cli, "tz_read", &args);
    let cm_out = dispatch_codemode_method(&cm, "zero.read", &args).expect("cm");

    assert!(mcp_out.is_ok(), "mcp: {:?}", mcp_out.tool_domain_error());
    assert!(raw_out.is_ok(), "raw: {:?}", raw_out.tool_domain_error());
    assert!(cli_out.is_ok(), "cli: {:?}", cli_out.tool_domain_error());
    assert!(cm_out.is_ok(), "cm: {:?}", cm_out.tool_domain_error());

    let mcp_pulse = tokenzero_pulse::default_ledger_path(root.path());
    let pulse_text = fs::read_to_string(&mcp_pulse).unwrap_or_else(|err| {
        panic!(
            "MCP tz_read must persist Pulse accounting at {}: {err}",
            mcp_pulse.display()
        )
    });
    assert!(
        pulse_text.contains("\"event\":\"tool_call\"") || pulse_text.contains("tool_call"),
        "Pulse ledger missing tool_call: {pulse_text}"
    );
    assert!(
        mcp_out
            .tool_response
            .as_ref()
            .and_then(|response| response.accounting.as_ref())
            .is_some(),
        "MCP success without accounting would skip Pulse and still look served"
    );

    let normalize = |out: &tokenzero_engine::DispatchOutcome| {
        let resp = out.tool_response.as_ref().expect("tool response");
        (
            resp.status.clone(),
            resp.tool.clone(),
            resp.refs
                .iter()
                .map(|r| {
                    // Content-addressed refs must agree; strip only transport noise.
                    r.ref_id.clone()
                })
                .collect::<Vec<_>>(),
            resp.visible.as_ref().map(|v| v.text.clone()),
            resp.error
                .as_ref()
                .map(|e| (e.code.clone(), e.message.clone())),
        )
    };

    let m = normalize(&mcp_out);
    assert_eq!(normalize(&raw_out), m, "raw vs mcp");
    assert_eq!(normalize(&cli_out), m, "cli vs mcp");
    assert_eq!(normalize(&cm_out), m, "codemode vs mcp");

    assert!(dispatch_count() >= before + 4);
    let profile = last_dispatch_profile();
    assert!(profile.wall_ns > 0);
    // Dispatcher overhead is recorded separately from kernel work for benchmarks.
    assert!(profile.dispatcher_overhead_ns < profile.wall_ns || profile.kernel_ns == 0);
}

#[test]
fn differential_registry_domain_ops_raw_mcp_cli() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"dispatcher-identity").unwrap();

    let ops = domain_fastmcp_ops();
    assert!(
        ops.contains(&"tz_read") && ops.contains(&"tz_mem"),
        "expected core domain ops in list: {ops:?}"
    );

    for op in ops {
        // Skip ops that need network, pre-existing refs, or mutating dry-run edge cases.
        if matches!(op, "tz_fetch" | "tz_expand" | "tz_report_tool_issue") {
            continue;
        }
        let args = minimal_args(op);
        // Rebase path-bearing ops onto temp root.
        let args = rebase_paths(args, root.path());

        let raw_e = engine_for(root.path());
        let mcp_e = engine_for(root.path());
        let cli_e = engine_for(root.path());

        let raw = dispatch_operation(&raw_e, DispatchSurface::RawWorker, op, &args);
        let mcp = dispatch_mcp_tool(&mcp_e, op, &args).expect("mcp dispatch");
        let cli = dispatch_cli(&cli_e, op, &args);

        let norm = |o: &tokenzero_engine::DispatchOutcome| {
            (
                o.op.clone(),
                o.is_ok(),
                o.tool_response.as_ref().map(|r| r.status.clone()),
                o.tool_response
                    .as_ref()
                    .and_then(|r| r.error.as_ref())
                    .map(|e| e.code.clone()),
                o.domain_error.as_ref().map(|e| e.kind.as_str().to_string()),
            )
        };
        assert_eq!(norm(&raw), norm(&mcp), "raw vs mcp for {op}");
        assert_eq!(norm(&raw), norm(&cli), "raw vs cli for {op}");
    }
}

#[test]
fn batch_error_taxonomy_matches_cli_and_mcp() {
    let root = tempfile::tempdir().unwrap();
    let cli_engine = engine_for(root.path());
    let mcp_engine = engine_for(root.path());

    let assert_parity =
        |args: &Value, expected_kind: DomainErrorKind, expected_code: Option<&str>| {
            let cli = dispatch_cli(&cli_engine, "tz_batch", args);
            let mcp = dispatch_mcp_tool(&mcp_engine, "tz_batch", args).expect("mcp dispatch");
            assert_eq!(
                cli.tool_domain_error().map(|error| error.kind),
                Some(expected_kind),
                "cli taxonomy for {args}"
            );
            assert_eq!(
                mcp.tool_domain_error().map(|error| error.kind),
                Some(expected_kind),
                "mcp taxonomy for {args}"
            );
            assert_eq!(
                cli.tool_response
                    .as_ref()
                    .and_then(|response| response.error.as_ref())
                    .map(|error| error.code.as_str()),
                expected_code,
                "cli code for {args}"
            );
            assert_eq!(
                mcp.tool_response
                    .as_ref()
                    .and_then(|response| response.error.as_ref())
                    .map(|error| error.code.as_str()),
                expected_code,
                "mcp code for {args}"
            );
        };

    assert_parity(&json!({"ops": []}), DomainErrorKind::Validation, None);
    assert_parity(
        &json!({"ops": [{"tool": "tz_batch", "args": {"ops": []}}]}),
        DomainErrorKind::Runtime,
        Some("batch_operation_failed"),
    );
}

fn rebase_paths(mut args: Value, root: &Path) -> Value {
    if let Some(obj) = args.as_object_mut() {
        if let Some(path) = obj.get("path").cloned() {
            match path {
                Value::String(s) if !Path::new(&s).is_absolute() => {
                    obj.insert("path".into(), json!(root.join(s).display().to_string()));
                }
                Value::Array(items) => {
                    let mapped: Vec<Value> = items
                        .into_iter()
                        .map(|item| match item {
                            Value::String(s) if !Path::new(&s).is_absolute() => {
                                json!(root.join(s).display().to_string())
                            }
                            other => other,
                        })
                        .collect();
                    obj.insert("path".into(), Value::Array(mapped));
                }
                _ => {}
            }
        }
        if let Some(Value::String(cwd)) = obj.get("cwd").cloned()
            && !Path::new(&cwd).is_absolute()
        {
            obj.insert("cwd".into(), json!(root.join(cwd).display().to_string()));
        }
    }
    args
}

#[test]
fn differential_policy_failure_agrees_across_surfaces() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"ok").unwrap();
    let _outside = root.path().join("..").join("escape-target.txt");
    // Use an absolute path outside allowed roots.
    let outside = fs::canonicalize(root.path())
        .unwrap()
        .parent()
        .unwrap()
        .join("tokenzero-dispatcher-escape.txt");
    let _ = fs::write(&outside, b"secret");

    let args = json!({"path": outside.display().to_string()});
    let raw_e = engine_for(root.path());
    let mcp_e = engine_for(root.path());
    let cm_e = engine_for(root.path());

    let raw = dispatch_raw_worker(&raw_e, "tz_read", &args);
    let mcp = dispatch_mcp_tool(&mcp_e, "tz_read", &args).unwrap();
    let cm = dispatch_codemode_method(&cm_e, "zero.read", &args).unwrap();

    for out in [&raw, &mcp, &cm] {
        assert!(!out.is_ok(), "escape should fail: {:?}", out.result);
        // Prefer typed policy/validation over success.
        let err = out.tool_domain_error().or_else(|| {
            out.tool_response.as_ref().and_then(|r| {
                r.error.as_ref().map(|e| {
                    tokenzero_core::operation_abi::DomainError::new(
                        DomainErrorKind::Policy,
                        e.message.clone(),
                    )
                })
            })
        });
        assert!(err.is_some(), "expected domain/tool error");
    }

    let code = |o: &tokenzero_engine::DispatchOutcome| {
        o.tool_response
            .as_ref()
            .and_then(|r| r.error.as_ref())
            .map(|e| e.code.clone())
    };
    assert_eq!(code(&raw), code(&mcp));
    assert_eq!(code(&raw), code(&cm));
    let _ = fs::remove_file(&outside);
}

#[test]
fn transport_control_tools_are_not_domain_ops() {
    assert!(!is_domain_operation("tz_execute_code"));
    assert!(!is_domain_operation("tz_codemode_search"));
    assert!(!is_domain_operation("codemode.limits"));
    assert!(is_domain_operation("tz_read"));
    assert!(is_domain_operation("zero.read"));

    let root = tempfile::tempdir().unwrap();
    let engine = engine_for(root.path());
    let err = dispatch_mcp_tool(&engine, "tz_execute_code", &json!({"plan": "1"}))
        .expect_err("control tool");
    assert_eq!(err.kind, DomainErrorKind::Validation);
}

#[test]
fn dispatcher_records_profile_for_benchmark_subtraction() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"profile").unwrap();
    let engine = engine_for(root.path());
    let before = dispatch_count();
    let _ = dispatch_raw_worker(
        &engine,
        "tz_read",
        &json!({"path": root.path().join("note.txt").display().to_string()}),
    );
    assert!(dispatch_count() > before);
    let p = last_dispatch_profile();
    assert_eq!(p.surface, DispatchSurface::RawWorker as u8);
    assert!(p.wall_ns >= p.kernel_ns);
}

#[test]
fn every_fastmcp_domain_op_is_dispatchable() {
    for op in all_operations() {
        if op.exposure.fastmcp_tool && is_domain_operation(op.name) {
            assert!(
                domain_fastmcp_ops().contains(&op.name),
                "missing from domain_fastmcp_ops: {}",
                op.name
            );
        }
    }
}
#[test]
fn registry_domain_ops_are_metadata_driven_not_masked() {
    // Every Canonical/LegacyAlias non-resource op must be classified domain;
    // every CodemodeControl/Resource must not. No hard-coded name denylist.
    use tokenzero_core::operation_abi::{MigrationStatus, all_operations};
    // operation_is_domain is on the engine crate-root re-export (owned by domain)
    use tokenzero_engine::operation_is_domain as eng_is_domain;
    for op in all_operations() {
        let expected = matches!(
            op.migration,
            MigrationStatus::Canonical | MigrationStatus::LegacyAlias
        ) && op.exposure.resource_uri.is_none();
        assert_eq!(
            eng_is_domain(op),
            expected,
            "classification drift for {}",
            op.name
        );
        assert_eq!(
            tokenzero_engine::is_domain_operation(op.name),
            expected,
            "name resolve drift for {}",
            op.name
        );
    }
}

#[test]
fn every_registry_domain_op_is_kernel_dispatchable() {
    use tokenzero_engine::all_domain_operations;
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"dispatcher-identity").unwrap();
    let engine = engine_for(root.path());
    for op in all_domain_operations() {
        // Minimal args; kernel may return tool-level errors but must not be TransportOnly.
        let args = minimal_args(op.name);
        let args = rebase_paths(args, root.path());
        let outcome = dispatch_raw_worker(&engine, op.name, &args);
        if let Some(err) = &outcome.domain_error {
            assert!(
                !err.message.contains("transport-control only"),
                "domain op {} rejected as transport-only: {}",
                op.name,
                err.message
            );
        }
    }
}

#[test]
fn non_domain_cli_commands_are_not_registry_domain_ops() {
    // Administration / audit CLI commands must not be classified as domain ops.
    let admin = [
        "doctor",
        "install",
        "clients",
        "pulse",
        "session_ledger",
        "bench",
        "quote",
        "hook",
        "mcp",
        "codemode",
    ];
    for name in admin {
        assert!(
            !is_domain_operation(name),
            "admin CLI name {name} must not resolve as domain op"
        );
    }
    // Domain ops that intentionally have no first-class CLI verb stay domain.
    assert!(is_domain_operation("tz_batch"));
    assert!(is_domain_operation("tz_report_tool_issue"));
}

/// A shell deadline spelled in milliseconds must actually bound the command.
///
/// Regression for tokenzero-gpa0: `timeout_ms` was not among the keys the shell
/// dispatcher consulted, so it was accepted and discarded. The command then ran
/// to completion under the default 60s timeout and reported success. Measured
/// through the live router before the fix, `{ timeout_ms: 1000 }` on an 8s
/// command returned after 8048ms with status `ok`.
///
/// This asserts on elapsed wall time rather than the response shape: the bug
/// was that the command KEPT RUNNING, which only a clock can observe.
#[test]
fn shell_timeout_ms_actually_bounds_the_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = engine_for(dir.path());

    let started = std::time::Instant::now();
    let _ = dispatch_codemode_method(
        &engine,
        "zero.shell",
        &json!({"command": "sleep 10", "timeout_ms": 1500}),
    );
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(6),
        "timeout_ms was ignored: a 10s command under a 1500ms deadline took {elapsed:?}"
    );
}

/// The two spellings must not disagree. Equivalent requests in different units
/// producing different behavior is how the millisecond path stayed broken while
/// the seconds path looked fine.
#[test]
fn shell_timeout_units_agree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = engine_for(dir.path());

    let ms_started = std::time::Instant::now();
    let _ = dispatch_codemode_method(
        &engine,
        "zero.shell",
        &json!({"command": "sleep 10", "timeout_ms": 2000}),
    );
    let ms_elapsed = ms_started.elapsed();

    let secs_started = std::time::Instant::now();
    let _ = dispatch_codemode_method(
        &engine,
        "zero.shell",
        &json!({"command": "sleep 10", "timeout_seconds": 2}),
    );
    let secs_elapsed = secs_started.elapsed();

    let delta = ms_elapsed.abs_diff(secs_elapsed);
    assert!(
        delta < std::time::Duration::from_secs(2),
        "timeout_ms ({ms_elapsed:?}) and timeout_seconds ({secs_elapsed:?}) disagree"
    );
}

/// Session memory persist is fail-closed on the MCP product path: a served
/// read must write session-memory.json, and a file-as-root must not return ok.
#[test]
fn mcp_read_session_persist_fail_closed_and_soak_lite() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("note.txt"), b"session-persist-product").unwrap();
    let args = json!({"path": dir.path().join("note.txt").display().to_string()});
    let session_root = dir.path().join("session-root");
    fs::create_dir_all(&session_root).unwrap();

    let ok = with_session_root(&session_root, || {
        let engine = engine_with_session_dedup(dir.path());
        dispatch_mcp_tool(&engine, "tz_read", &args).expect("mcp")
    });
    assert!(
        ok.is_ok(),
        "writable session root: {:?}",
        ok.tool_domain_error()
    );
    let memory_path = with_session_root(&session_root, || {
        session_memory_path(&EngineConfig::for_root(dir.path()).cache_path)
    });
    assert!(
        memory_path.is_file(),
        "MCP tz_read with session_dedup must persist session-memory.json at {}",
        memory_path.display()
    );

    let blocker = dir.path().join("blocked-root");
    fs::write(&blocker, b"not-a-directory").unwrap();
    let failed = with_session_root(&blocker, || {
        let engine = engine_with_session_dedup(dir.path());
        dispatch_mcp_tool(&engine, "tz_read", &args).expect("mcp dispatch typed")
    });
    assert!(
        !failed.is_ok(),
        "file-as session root must fail closed, not drop persist_inner: {:?}",
        failed.result
    );
    let code = failed
        .tool_response
        .as_ref()
        .and_then(|response| response.error.as_ref())
        .map(|error| error.code.as_str());
    assert_eq!(
        code,
        Some("session_persist_failed"),
        "persist failure must be typed, not an ok envelope: {:?}",
        failed.tool_response
    );
}

/// MCP/CodeMode/CLI share dispatch: a served accounting block must land in
/// ledger.jsonl. A directory occupying that path must fail the envelope.
#[test]
fn mcp_read_writes_response_ledger_and_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"ledger-product").unwrap();
    let args = json!({"path": root.path().join("note.txt").display().to_string()});
    let engine = engine_for(root.path());
    let out = dispatch_mcp_tool(&engine, "tz_read", &args).expect("mcp");
    assert!(out.is_ok(), "mcp read: {:?}", out.tool_domain_error());
    let ledger_path = ledger_path_for_cache(&engine.config.cache_path);
    let pulse_path = tokenzero_pulse::default_ledger_path(root.path());
    let text = fs::read_to_string(&ledger_path).unwrap_or_else(|err| {
        panic!(
            "MCP tz_read must persist response ledger at {}: {err}",
            ledger_path.display()
        )
    });
    assert!(
        text.contains("\"tool\"") && (text.contains("read") || text.contains("tz_read")),
        "response ledger missing tool record: {text}"
    );
    assert_ne!(
        ledger_path, pulse_path,
        "response ledger.jsonl is not the Pulse JSONL"
    );

    let blocked = tempfile::tempdir().unwrap();
    fs::write(blocked.path().join("note.txt"), b"ledger-blocked").unwrap();
    let mut config = EngineConfig::for_root(blocked.path());
    config.session_dedup = false;
    config.diff_reads = false;
    config.fetch_enabled = false;
    let blocked_ledger = ledger_path_for_cache(&config.cache_path);
    if let Some(parent) = blocked_ledger.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::create_dir_all(&blocked_ledger).unwrap();
    let engine = TokenZeroEngine::new(config);
    let failed = dispatch_mcp_tool(
        &engine,
        "tz_read",
        &json!({"path": blocked.path().join("note.txt").display().to_string()}),
    )
    .expect("mcp dispatch typed");
    assert!(
        !failed.is_ok(),
        "directory-as ledger.jsonl must fail closed: {:?}",
        failed.result
    );
    let message = failed
        .domain_error
        .as_ref()
        .map(|err| err.message.as_str())
        .unwrap_or("");
    assert!(
        message.contains("response ledger"),
        "ledger persist failure must be typed, not an ok envelope: {:?}",
        failed.domain_error
    );
}

/// Edit persist-after-write is envelope-fail, not ok+diagnostic. The file
/// stays applied so clients must not retry the hunks.
#[test]
fn mcp_edit_session_persist_after_write_fails_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let note = dir.path().join("note.txt");
    fs::write(&note, b"hello-edit").unwrap();
    let args = json!({
        "path": note.display().to_string(),
        "edits": [{"find": "hello-edit", "replace": "hello-applied"}],
    });
    let blocker = dir.path().join("blocked-root");
    fs::write(&blocker, b"not-a-directory").unwrap();
    let failed = with_session_root(&blocker, || {
        let engine = engine_with_session_dedup(dir.path());
        dispatch_mcp_tool(&engine, "tz_edit", &args).expect("mcp dispatch typed")
    });
    assert!(
        !failed.is_ok(),
        "edit persist-after-write must fail the envelope: {:?}",
        failed.result
    );
    let error = failed
        .tool_response
        .as_ref()
        .and_then(|response| response.error.as_ref());
    assert_eq!(
        error.map(|err| err.code.as_str()),
        Some("session_persist_failed"),
        "persist failure must be typed, not an ok envelope: {:?}",
        failed.tool_response
    );
    assert!(
        error
            .map(|err| err.message.contains("do not retry"))
            .unwrap_or(false),
        "error must say the file landed: {:?}",
        error
    );
    let body = fs::read_to_string(&note).unwrap();
    assert_eq!(
        body, "hello-applied",
        "file must stay applied; do not reverse the write"
    );
}
