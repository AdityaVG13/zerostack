//! Focused fixture tests proving the canonical ZSX path runs in exactly one
//! process with no worker spawn: in-process dispatch through registered
//! fixture adapters, `zsx exec` end-to-end, and a source-level guard that
//! the exec path contains no `Command::spawn` or session socket.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tempfile::TempDir;
use zero_abi::{EffectClass, EngineIdentity};
use zsx_core::{
    DomainAdapter, SessionApprovalGrantV1, ZsxSession, ZsxSessionFailureCode, fixture,
    process_spawn_count,
};

/// Coerce a concrete fixture adapter to the registered trait object.
fn as_adapter(adapter: &Arc<fixture::FixtureAdapter>) -> Arc<dyn DomainAdapter> {
    Arc::clone(adapter) as Arc<dyn DomainAdapter>
}

/// Child processes of the current process (Linux only).
#[cfg(target_os = "linux")]
fn child_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((_, rest)) = stat.rsplit_once(") ") else {
                continue;
            };
            // After the closing paren: state(0) ppid(1) ...
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.get(1).and_then(|value| value.parse::<u32>().ok()) == Some(self_pid) {
                children.push(pid);
            }
        }
    }
    children
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn grant(
    root: &str,
    engine: EngineIdentity,
    operation: &str,
    request_id: u64,
) -> SessionApprovalGrantV1 {
    let now = now_ms();
    SessionApprovalGrantV1 {
        schema: "zerostack.session.approval_grant.v1".into(),
        grant_id: format!("grant-{}-{}", engine.as_str(), operation),
        engine,
        root: root.into(),
        generation: 1,
        request_id,
        operation: operation.into(),
        effect: EffectClass::ApprovalRequiredMutation,
        authority_digest: "a".repeat(64),
        policy_digest: "b".repeat(64),
        issued_at_unix_ms: now.saturating_sub(1),
        expires_at_unix_ms: now.saturating_add(60_000),
    }
}

fn assert_zero_result_echo(result: &Value, engine: &str, args: Value) {
    assert_eq!(result["content"]["kind"], "inline", "{result}");
    let value = &result["content"]["value"];
    assert_eq!(value["metadata"]["ownership"]["engine"], engine, "{value}");
    assert_eq!(value["value"]["args"], args, "{value}");
}

#[test]
fn in_process_dispatch_proves_one_process_and_no_worker_spawn() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().to_path_buf();
    let session_id = "fixture-one-process";
    let (fs, graph, token) = fixture::fixture_adapters(&root, session_id);
    let session = ZsxSession::builder(root.clone())
        .with_session_id(session_id)
        .fszero(as_adapter(&fs))
        .graphzero(as_adapter(&graph))
        .tokenzero(as_adapter(&token))
        .build()
        .expect("builder accepts exactly the three domain adapters");

    // One execution per surface keeps each inline result inside the visible
    // budget; the multi-surface spill path is covered by zsx_exec tests.
    let fs_result = session
        .execute(
            1,
            1,
            "return await zero.fs.compound('list', {path:'.'});",
            Duration::from_secs(30),
        )
        .expect("fszero in-process execution succeeds");
    assert_eq!(fs_result.generation, 1);
    assert_eq!(fs_result.request_id, 1);
    assert_zero_result_echo(&fs_result.value, "fszero", json!({"path": "."}));

    let graph_result = session
        .execute(
            1,
            2,
            "return await zero.graph.blast({intent:'Widget'});",
            Duration::from_secs(30),
        )
        .expect("graphzero in-process execution succeeds");
    assert_zero_result_echo(
        &graph_result.value,
        "graphzero",
        json!({"intent": "Widget"}),
    );

    let token_result = session
        .execute(
            1,
            3,
            "return await zero.token.shell('printf ok', {timeout_seconds:1});",
            Duration::from_secs(30),
        )
        .expect("tokenzero in-process execution succeeds");
    assert_zero_result_echo(
        &token_result.value,
        "tokenzero",
        json!({"command": "printf ok", "timeout_seconds": 1}),
    );

    // Every engine was served by its registered in-process adapter.
    assert_eq!(fs.calls(), 1, "fszero adapter served one dispatch");
    assert_eq!(graph.calls(), 1, "graphzero adapter served one dispatch");
    assert_eq!(token.calls(), 1, "tokenzero adapter served one dispatch");

    // No worker process was spawned anywhere in zsx-core.
    assert_eq!(process_spawn_count(), 0, "zsx-core spawned a child process");
    #[cfg(target_os = "linux")]
    {
        let children = child_pids();
        assert!(
            children.is_empty(),
            "test process has children: {children:?}"
        );
    }

    session.shutdown().expect("shutdown settles");
    assert_eq!(process_spawn_count(), 0);
}

#[test]
fn durable_attempts_do_not_collide_between_native_sessions() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().canonicalize().unwrap();

    for session_id in ["fixture-session-one", "fixture-session-two"] {
        let (fs, graph, token) = fixture::fixture_adapters(&root, session_id);
        let session = ZsxSession::builder(root.clone())
            .with_session_id(session_id)
            .fszero(as_adapter(&fs))
            .graphzero(as_adapter(&graph))
            .tokenzero(as_adapter(&token))
            .build()
            .expect("build native fixture session");
        session
            .execute(
                1,
                1,
                "return await zero.token.shell('printf ok');",
                Duration::from_secs(30),
            )
            .expect("same-root mutation journal remains unique per native session");
        session.shutdown().expect("shutdown settles");
    }
}

#[test]
fn approvals_reachability_and_bounded_dispatch_survive_in_process() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let session_id = "fixture-approvals";
    let (fs, graph, token) = fixture::fixture_adapters(&root, session_id);
    let session = ZsxSession::builder(root.clone())
        .with_session_id(session_id)
        .fszero(as_adapter(&fs))
        .graphzero(as_adapter(&graph))
        .tokenzero(as_adapter(&token))
        .build()
        .expect("builder accepts the three domain adapters");

    // A CAS-backed ref owned by GraphZero, published before the call so the
    // connector can verify and retain it.
    let reference = fixture::publish_fixture_blob(&root, EngineIdentity::GraphZero, b"reachable");
    let approval_plan = "const r = await zero.graph.blast({intent:'x', __approval_fixture:true}); return r.content.value.metadata.approval.state;";
    let refs_plan = format!(
        "const r = await zero.graph.blast({{intent:'x', __reachability_ref_fixture:'{reference}'}}); return r.content.value.metadata.ownership.refs;"
    );

    let approval = grant(
        root.to_str().unwrap(),
        EngineIdentity::GraphZero,
        "blast",
        7,
    );
    let result = session
        .execute_with_approvals(1, 7, approval_plan, Duration::from_secs(30), vec![approval])
        .expect("approval-granted in-process execution succeeds");
    assert_eq!(result.value, json!("granted"), "{result:?}");

    // The connector verified the CAS-backed ref and retained it for GC.
    let result = session
        .execute(1, 8, refs_plan, Duration::from_secs(30))
        .expect("reachability ref survives in-process dispatch");
    assert_eq!(result.value, json!([reference]), "{result:?}");

    // The same grant id cannot replay in a later request.
    let replay = grant(
        root.to_str().unwrap(),
        EngineIdentity::GraphZero,
        "blast",
        9,
    );
    let error = session
        .execute_with_approvals(1, 9, approval_plan, Duration::from_secs(30), vec![replay])
        .expect_err("approval replay must fail closed");
    assert_eq!(error.code, ZsxSessionFailureCode::ApprovalReplay, "{error}");

    // A grantless approval-required operation fails closed.
    let error = session
        .execute(1, 10, approval_plan, Duration::from_secs(30))
        .expect_err("missing approval must fail closed");
    assert!(
        error.to_string().contains("approval") || error.to_string().contains("Required"),
        "{error}"
    );

    session.shutdown().expect("shutdown settles");
}

#[test]
fn deadlines_and_cancellation_are_enforced_in_process() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().to_path_buf();
    let session_id = "fixture-cancellation";
    let (fs, graph, token) = fixture::fixture_adapters(&root, session_id);
    let session = ZsxSession::builder(root)
        .with_session_id(session_id)
        .fszero(as_adapter(&fs))
        .graphzero(as_adapter(&graph))
        .tokenzero(as_adapter(&token))
        .build()
        .expect("builder accepts the three domain adapters");

    // The fixture adapter sleeps cooperatively; a short wall timeout must
    // surface as a deadline error instead of hanging.
    let slow = "return await zero.graph.blast({intent:'slow', __fixture_delay_ms: 5000});";
    let error = session
        .execute(1, 1, slow, Duration::from_millis(50))
        .expect_err("fixture delay must exceed the wall deadline");
    assert!(
        error.to_string().contains("deadline") || error.to_string().contains("expired"),
        "{error}"
    );

    // Cancellation interrupts an in-flight adapter call.
    let cancellation = session.cancellation();
    let handle = std::thread::spawn(move || {
        session
            .execute(1, 2, slow, Duration::from_secs(30))
            .map(|_| ())
            .err()
            .map(|error| error.to_string())
    });
    std::thread::sleep(Duration::from_millis(100));
    cancellation.cancel();
    let error = handle
        .join()
        .expect("cancelled execution thread settles")
        .expect("cancelled execution must fail");
    assert!(
        error.contains("cancelled")
            || error.contains("deadline")
            || error.contains("cancel")
            || error.contains("stale"),
        "{error}"
    );
}

#[test]
fn zsx_exec_runs_the_embedded_core_in_one_process() {
    let directory = TempDir::new().unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_zsx"))
        .args(["exec", "-C", directory.path().to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("zsx binary spawns");
    #[cfg(not(windows))]
    let plan = b"return await zero.token.shell('printf zsx');".as_slice();
    #[cfg(windows)]
    let plan = b"return await zero.token.shell({command:'cmd',args:['cmd','/d','/s','/c','set /p =zsx<nul']});"
        .as_slice();
    child.stdin.take().unwrap().write_all(plan).unwrap();
    let output = child.wait_with_output().expect("zsx exec completes");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["protocol"], "zerostack.zsx.v1", "{response}");
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["generation"], 1, "{response}");
    let value = &response["result"]["content"]["value"];
    assert_eq!(
        value["metadata"]["ownership"]["engine"], "tokenzero",
        "{response}"
    );
    assert_eq!(value["value"]["status"], "ok", "{response}");
    assert_eq!(value["value"]["visible"], "zsx", "{response}");
}

#[test]
fn zsx_exec_source_has_no_process_spawn_or_session_socket_path() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let banned: &[&str] = &[
        "Command::spawn",
        "std::process::Command",
        "UnixStream",
        "zerostack-session",
        "ZEROSTACK_SESSION_SOCKET",
        "ZEROSTACK_SESSION_TOKEN",
    ];
    for name in ["src/main.rs", "src/exec.rs"] {
        let path = manifest.join(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for token in banned {
            assert!(
                !source.contains(token),
                "{} must not contain {token:?}",
                path.display()
            );
        }
    }
}
