#![cfg(all(unix, feature = "quickjs"))]
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use zero_codemode::session::SessionExecutor;
use zerostack_machine_permit::session_owner::ProcessIdentity;
fn start(owner: ProcessIdentity) -> (TempDir, std::process::Child, String, String, u64) {
    start_configured(owner, |_, _| {})
}

fn start_configured<F>(
    owner: ProcessIdentity,
    configure: F,
) -> (TempDir, std::process::Child, String, String, u64)
where
    F: FnOnce(&mut Command, &std::path::Path),
{
    let d = TempDir::new().unwrap();
    let runtime = d.path().join("runtime");
    let token = "a".repeat(64);
    let shutdown_token = "b".repeat(64);
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-session"));
    command
        .args([
            "serve",
            "--root",
            d.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &owner.encode(),
        ])
        .env("ZEROSTACK_SESSION_TOKEN", &token)
        .env("ZEROSTACK_SESSION_SHUTDOWN_TOKEN", &shutdown_token)
        .env("ZEROSTACK_SESSION_ROOT", d.path())
        .env("ZEROSTACK_TEST_MODE", "1")
        .env(
            "ZERO_FSZERO_RAW_BIN",
            env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"),
        )
        .env(
            "ZERO_GRAPHZERO_RAW_BIN",
            env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"),
        )
        .env(
            "ZERO_TOKENZERO_RAW_BIN",
            env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    configure(&mut command, d.path());
    let mut c = command.spawn().unwrap();
    let mut ready = String::new();
    let n = BufReader::new(c.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if n == 0 {
        let mut stderr = String::new();
        c.stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        panic!(
            "session exited before ready: status={:?} stderr={stderr:?}",
            c.try_wait().unwrap()
        );
    }
    let ready_json: Value = serde_json::from_str(&ready)
        .unwrap_or_else(|e| panic!("invalid ready JSON {e}: {ready:?}"));
    assert_eq!(ready_json["type"], "ready");
    assert_eq!(ready_json["protocol"], "zerostack-session/v1");
    let generation = ready_json["generation"]
        .as_u64()
        .filter(|g| *g != 0)
        .expect("nonzero generation");
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(runtime.join("session.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    (d, c, token, shutdown_token, generation)
}
fn send(s: &mut UnixStream, v: Value) {
    serde_json::to_writer(&mut *s, &v).unwrap();
    s.write_all(b"\n").unwrap();
    s.flush().unwrap()
}
fn read(r: &mut BufReader<UnixStream>) -> Value {
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    serde_json::from_str(&s).unwrap()
}
fn read_rejection(r: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut s = String::new();
    r.read_line(&mut s).unwrap();
    if s.is_empty() {
        None
    } else {
        Some(serde_json::from_str(&s).unwrap())
    }
}
#[test]
fn authenticated_cross_surface_and_rejections() {
    let (d, mut c, t, shutdown_token, _generation) = start(ProcessIdentity::current().unwrap());
    let sock = d.path().join("runtime/session.sock");
    let mut bad = UnixStream::connect(&sock).unwrap();
    let mut br = BufReader::new(bad.try_clone().unwrap());
    send(
        &mut bad,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":"wrong"}),
    );
    let rejected = read_rejection(&mut br);
    if let Some(response) = rejected {
        assert_eq!(response["ok"], false);
    }
    let mut s = UnixStream::connect(&sock).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    send(
        &mut s,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":t}),
    );
    let hello = read(&mut r);
    assert_eq!(hello["ok"], true);
    let generation = hello["generation"].as_u64().unwrap();
    let source="const a=await zero.fs.compound('read',{x:1});const b=await zero.token.shell('echo fixture');return {a,b};";
    send(
        &mut s,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":source}),
    );
    let out = read(&mut r);
    assert_eq!(out["result"]["a"]["value"]["args"]["x"], 1);
    assert_eq!(
        out["result"]["b"]["value"]["args"]["command"],
        "echo fixture"
    );
    assert_eq!(
        out["result"]["a"]["metadata"]["ownership"]["session_id"],
        out["result"]["b"]["metadata"]["ownership"]["session_id"]
    );
    send(
        &mut s,
        json!({"type":"execute","id":2,"generation":generation,"root":"/","source":"return 1"}),
    );
    assert_eq!(read(&mut r)["ok"], false);
    send(
        &mut s,
        json!({"type":"shutdown","id":3,"token":shutdown_token}),
    );
    let _ = read(&mut r);
    c.wait().unwrap();
    assert!(!sock.exists())
}
#[test]
fn all_public_methods_preserve_arguments_and_ref_owners() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let mut stream = UnixStream::connect(&socket).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":token}),
    );
    let generation = read(&mut reader)["generation"].as_u64().unwrap();
    let source = r#"
        const fsPlan = await zero.fs.plan('map widget entrypoint files');
        const fsStructural = await zero.fs.structural('callers', 'Widget');
        const fsCompound = await zero.fs.compound('read', {path:'src/lib.rs'});
        const fsReadMany = await zero.fs.read_many(['a.rs'], {max_bytes:32});
        const fsListMany = await zero.fs.list_many([{path:'src'}]);
        const fsSearchMany = await zero.fs.search_many(['Widget']);
        const fsAstMany = await zero.fs.ast_search_many([{pattern:'fn $F()'}]);
        const graphBlast = await zero.graph.blast('Widget', {depth:2});
        const graphQuery = await zero.graph.query('symbol', 'Widget');
        const graphOrient = await zero.graph.orient('context', 'Widget');
        const graphRecall = await zero.graph.recall('Widget');
        const graphVerify = await zero.graph.verify('Widget', 'no_remaining_callers');
        const graphSnap = await zero.graph.snap('Widget', 64);
        const graphReserve = await zero.graph.reserve('acquire', {key:'Widget'});
        const graphIndex = await zero.graph.index();
        const graphRemember = await zero.graph.remember({text:'Widget fact'});
        const tokenCompact = await zero.token.compact({widget:true});
        const fsExpand = await zero.token.expand('fz://blob/0000000000000000000000000000000000000000000000000000000000000000');
        const graphExpand = await zero.token.expand('gz://blob/0000000000000000000000000000000000000000000000000000000000000000');
        const tokenExpand = await zero.token.expand('tz://blob/0000000000000000000000000000000000000000000000000000000000000000');
        const tokenFind = await zero.token.find('Widget', 'src');
        const tokenShell = await zero.token.shell('printf ok', {timeout_seconds:1});
        return {fsPlan,fsStructural,fsCompound,fsReadMany,fsListMany,fsSearchMany,fsAstMany,graphBlast,graphQuery,graphOrient,graphRecall,graphVerify,graphSnap,graphReserve,graphIndex,graphRemember,tokenCompact,fsExpand,graphExpand,tokenExpand,tokenFind,tokenShell};
    "#;
    send(
        &mut stream,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":source}),
    );
    let result = read(&mut reader);
    assert_eq!(result["ok"], true, "{result}");
    let result = &result["result"];
    assert_eq!(result["fsPlan"]["value"]["args"]["queries"][0], "widget");
    assert_eq!(result["fsReadMany"]["value"]["args"]["paths"][0], "a.rs");
    assert_eq!(
        result["fsSearchMany"]["value"]["args"]["queries"][0],
        "Widget"
    );
    assert_eq!(result["graphRecall"]["value"]["args"]["query"], "Widget");
    assert_eq!(
        result["tokenShell"]["value"]["args"]["command"],
        "printf ok"
    );
    assert_eq!(
        result["fsStructural"]["value"]["args"]["query"],
        "callers:Widget"
    );
    assert_eq!(result["graphBlast"]["value"]["args"]["depth"], 2);
    assert_eq!(result["graphQuery"]["value"]["args"]["surface"], "symbol");
    assert_eq!(
        result["fsExpand"]["metadata"]["ownership"]["engine"],
        "fszero"
    );
    assert_eq!(
        result["graphExpand"]["metadata"]["ownership"]["engine"],
        "graphzero"
    );
    assert_eq!(
        result["tokenExpand"]["metadata"]["ownership"]["engine"],
        "tokenzero"
    );
    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn owner_sigkill_removes_socket_under_one_second() {
    let mut owner = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let (d, mut session, _, _, _) = start(ProcessIdentity::capture(owner.id()).unwrap());
    let sock = d.path().join("runtime/session.sock");
    owner.kill().unwrap();
    owner.wait().unwrap();
    let started = Instant::now();
    while sock.exists() && started.elapsed() < Duration::from_secs(1) {
        thread::sleep(Duration::from_millis(10))
    }
    assert!(!sock.exists());
    assert!(started.elapsed() < Duration::from_secs(1));
    session.wait().unwrap();
}

#[test]
fn owner_sigkill_reaps_worker_descendants_before_socket_cleanup() {
    let mut owner = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let (d, mut session, _, _, _) = start_configured(
        ProcessIdentity::capture(owner.id()).unwrap(),
        |command, root| {
            command
                .env("ZEROSTACK_DESCENDANT_PID_FILE", root.join("descendant.pid"))
                .env("ZEROSTACK_FSZERO_RAW_ARGS", "spawn-descendant-normal");
        },
    );
    let socket = d.path().join("runtime/session.sock");
    let descendant_pid = std::fs::read_to_string(d.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let descendant = ProcessIdentity::capture(descendant_pid).unwrap();
    owner.kill().unwrap();
    owner.wait().unwrap();
    let started = Instant::now();
    while (socket.exists() || descendant.is_live().unwrap_or(false))
        && started.elapsed() < Duration::from_secs(1)
    {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!descendant.is_live().unwrap_or(false));
    assert!(!socket.exists());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(session.wait().unwrap().success());
}

#[test]
fn terminal_cancellation_rejects_queued_execution() {
    let d = TempDir::new().unwrap();
    std::env::set_var("ZEROSTACK_SESSION_ROOT", d.path());
    std::env::set_var("ZEROSTACK_TEST_MODE", "1");
    for engine in ["FSZERO", "GRAPHZERO", "TOKENZERO"] {
        std::env::set_var(
            format!("ZERO_{engine}_RAW_BIN"),
            env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"),
        );
    }
    let exec = SessionExecutor::new().unwrap();
    assert_eq!(
        exec.execute("return 1", Duration::from_secs(1)).unwrap(),
        json!(1)
    );
    exec.cancellation().cancel();
    let started = Instant::now();
    let error = exec
        .execute("for (;;) {}", Duration::from_secs(30))
        .unwrap_err();
    assert_eq!(error.to_string(), "connector error: session cancelled");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn request_timeout_interrupts_active_plan() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let mut stream = UnixStream::connect(&socket).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":token}),
    );
    let generation = read(&mut reader)["generation"].as_u64().unwrap();
    let started = Instant::now();
    send(
        &mut stream,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":"for (;;) {}","timeout_ms":10}),
    );
    let response = read(&mut reader);
    assert_eq!(response["ok"], false);
    assert!(started.elapsed() < Duration::from_secs(1));
    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
    assert!(!socket.exists());
}

#[test]
fn owner_sigkill_interrupts_active_plan_under_one_second() {
    let mut owner = Command::new("sleep").arg("30").spawn().unwrap();
    let (d, mut session, token, _, _) = start(ProcessIdentity::capture(owner.id()).unwrap());
    let socket = d.path().join("runtime/session.sock");
    let mut stream = UnixStream::connect(&socket).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":token}),
    );
    let generation = read(&mut reader)["generation"].as_u64().unwrap();
    send(
        &mut stream,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":"for (;;) {}","timeout_ms":30000}),
    );
    thread::sleep(Duration::from_millis(20));
    owner.kill().unwrap();
    owner.wait().unwrap();
    let started = Instant::now();
    while socket.exists() && started.elapsed() < Duration::from_secs(1) {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!socket.exists());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(session.wait().unwrap().success());
}

#[test]
#[ignore = "requires verified FSZero, GraphZero, and TokenZero raw-worker artifacts"]
fn real_workers_execute_one_cross_surface_plan() {
    let d = TempDir::new().unwrap();
    std::fs::write(d.path().join("fixture.txt"), "real worker fixture\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_zsx"));
    command
        .args(["exec", "-C", d.path().to_str().unwrap()])
        .env_remove("ZEROSTACK_SESSION_SOCKET")
        .env_remove("ZEROSTACK_SESSION_TOKEN")
        .env_remove("ZEROSTACK_TEST_MODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"
            const fs = await zero.fs.compound('read', {path:'fixture.txt'});
            const graph = await zero.graph.index();
            const token = await zero.token.compact('real worker fixture');
            return {fs,graph,token};
            "#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["result"]["fs"]["metadata"]["ownership"]["engine"],
        "fszero"
    );
    assert_eq!(
        response["result"]["graph"]["metadata"]["ownership"]["engine"],
        "graphzero"
    );
    assert_eq!(
        response["result"]["token"]["metadata"]["ownership"]["engine"],
        "tokenzero"
    );
}

#[test]
fn zsx_fallback_executes_and_rejects_partial_inherited_endpoint() {
    let d = TempDir::new().unwrap();
    let worker = env!("CARGO_BIN_EXE_zero-codemode-worker-fixture");
    let mut command = Command::new(env!("CARGO_BIN_EXE_zsx"));
    command
        .args(["exec", "-C", d.path().to_str().unwrap()])
        .env_remove("ZEROSTACK_SESSION_SOCKET")
        .env_remove("ZEROSTACK_SESSION_TOKEN")
        .env("ZEROSTACK_TEST_MODE", "1")
        .env("ZERO_FSZERO_RAW_BIN", worker)
        .env("ZERO_GRAPHZERO_RAW_BIN", worker)
        .env("ZERO_TOKENZERO_RAW_BIN", worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"return await zero.token.shell('printf zsx');")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["value"]["args"]["command"], "printf zsx");

    let output = Command::new(env!("CARGO_BIN_EXE_zsx"))
        .args(["exec", "-C", d.path().to_str().unwrap()])
        .env("ZEROSTACK_SESSION_SOCKET", d.path().join("missing.sock"))
        .env_remove("ZEROSTACK_SESSION_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"], "incomplete inherited session endpoint");
}

#[test]
fn worker_sha256_mismatch_fails_closed_without_stale_socket() {
    let d = TempDir::new().unwrap();
    let runtime = d.path().join("runtime");
    let worker = env!("CARGO_BIN_EXE_zero-codemode-worker-fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_zerostack-session"))
        .args([
            "serve",
            "--root",
            d.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &ProcessIdentity::current().unwrap().encode(),
        ])
        .env("ZEROSTACK_SESSION_TOKEN", "a".repeat(64))
        .env("ZEROSTACK_SESSION_SHUTDOWN_TOKEN", "b".repeat(64))
        .env("ZEROSTACK_TEST_MODE", "1")
        .env("ZERO_FSZERO_RAW_BIN", worker)
        .env("ZERO_GRAPHZERO_RAW_BIN", worker)
        .env("ZERO_TOKENZERO_RAW_BIN", worker)
        .env("ZEROSTACK_FSZERO_WORKER_SHA256", "0".repeat(64))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("raw worker SHA-256 mismatch"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!runtime.join("session.sock").exists());
    assert!(!runtime.exists());
}

#[test]
fn malformed_and_oversize_frames_are_rejected() {
    let (d, mut c, _, _, _) = start(ProcessIdentity::current().unwrap());
    let sock = d.path().join("runtime/session.sock");
    let mut s = UnixStream::connect(&sock).unwrap();
    s.write_all(b"not-json\n").unwrap();
    drop(s);
    let mut s = UnixStream::connect(&sock).unwrap();
    let _ = s.write_all(&vec![b'x'; zero_codemode::session::MAX_SESSION_FRAME + 2]);
    drop(s);
    c.kill().unwrap();
    let _ = c.wait();
}
