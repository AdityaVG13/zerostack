#![cfg(all(unix, feature = "worker-fixture"))]
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
#[cfg(feature = "worker-fixture")]
use zero_codemode::session::SessionExecutor;
use zero_store::{ensure_layout, Engine, ResolvedStore, SharedCas};
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
fn exact_result(root: &std::path::Path, response: &Value) -> Value {
    let result = &response["result"];
    if result["spilled"] != true {
        return result.clone();
    }
    let sha = result["sha256"].as_str().expect("spill sha256");
    let resolved = ResolvedStore::resolve_from_process(root, Engine::TokenZero, &[]);
    ensure_layout(&resolved).unwrap();
    let stored = SharedCas::open(resolved.cas_host())
        .get_verified(sha)
        .unwrap();
    serde_json::from_slice(&stored).unwrap()
}

fn inline_public(result: &Value) -> &Value {
    assert_eq!(result["content"]["kind"], "inline", "{result}");
    &result["content"]["value"]
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

fn connect_authenticated(
    socket: &std::path::Path,
    token: &str,
) -> (UnixStream, BufReader<UnixStream>, u64) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":token}),
    );
    let hello = read(&mut reader);
    assert_eq!(hello["ok"], true, "{hello}");
    let generation = hello["generation"].as_u64().unwrap();
    (stream, reader, generation)
}

#[test]
fn repeated_zsx_calls_use_distinct_request_ids() {
    let (d, mut session, token, shutdown_token, generation) =
        start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let mut ids = std::collections::HashSet::new();
    for expected in [1_u64, 2] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zsx"));
        command
            .args(["exec", "-C", d.path().to_str().unwrap()])
            .env("ZEROSTACK_SESSION_SOCKET", &socket)
            .env("ZEROSTACK_SESSION_TOKEN", &token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        write!(child.stdin.take().unwrap(), "return {expected};").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["generation"], generation, "{response}");
        assert!(ids.insert(response["id"].as_u64().unwrap()), "{response}");
    }
    let (mut stream, mut reader, _) = connect_authenticated(&socket, &token);
    send(
        &mut stream,
        json!({"type":"shutdown","id":u64::MAX,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
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
    send(
        &mut s,
        json!({"type":"status","id":900,"generation":generation}),
    );
    let status = read(&mut r);
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(
        status["result"]["schema"],
        "zerostack.session.aggregate_resource_receipt.v1"
    );
    assert_eq!(status["result"]["workers"].as_array().unwrap().len(), 3);
    let active_sum: u64 = status["result"]["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["active_tree_rss_bytes"].as_u64().unwrap())
        .sum();
    assert!(active_sum <= status["result"]["active_tree_rss_bytes"].as_u64().unwrap());
    let source = r#"const a=await zero.fs.compound('read',{x:1});
        const b=await zero.token.shell('echo fixture');
        const c=await zero.token.read('Cargo.toml',{raw:true,fresh:true});
        return {a,b,c};"#;
    send(
        &mut s,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":source}),
    );
    let out = read(&mut r);
    let exact = exact_result(d.path(), &out);
    for name in ["a", "b", "c"] {
        serde_json::from_value::<zero_abi::ZeroResultV1>(exact[name].clone())
            .unwrap_or_else(|error| panic!("{name} did not emit zero-result/v1: {error}"));
    }
    assert_eq!(exact["a"]["content"]["value"]["value"]["args"]["x"], 1);
    assert_eq!(
        exact["b"]["content"]["value"]["value"]["args"]["command"],
        "echo fixture"
    );
    assert_eq!(
        exact["c"]["content"]["value"]["value"]["args"]["path"],
        "Cargo.toml"
    );
    assert_eq!(exact["c"]["content"]["value"]["value"]["args"]["raw"], true);
    assert_eq!(
        exact["c"]["content"]["value"]["value"]["args"]["fresh"],
        true
    );
    assert_eq!(
        exact["a"]["content"]["value"]["metadata"]["ownership"]["session_id"],
        exact["c"]["content"]["value"]["metadata"]["ownership"]["session_id"]
    );
    let invalid_shell = r#"try {
        await zero.token.shell('touch must-not-run',{raw:true});
        return {executed:true};
    } catch (error) {
        return {name:error.name,message:String(error)};
    }"#;
    send(
        &mut s,
        json!({"type":"execute","id":2,"generation":generation,"root":d.path(),"source":invalid_shell}),
    );
    let invalid_shell = exact_result(d.path(), &read(&mut r));
    assert_eq!(invalid_shell["name"], "TypeError");
    assert!(invalid_shell["message"]
        .as_str()
        .unwrap()
        .contains("unknown option 'raw'"));
    assert!(invalid_shell["message"]
        .as_str()
        .unwrap()
        .contains(r#"mode: "exact""#));
    send(
        &mut s,
        json!({"type":"execute","id":3,"generation":generation,"root":"/","source":"return 1"}),
    );
    let root_error = read(&mut r);
    assert_eq!(root_error["ok"], false);
    assert_eq!(root_error["code"], "authorized_root_mismatch");
    let detail = root_error["error"].as_str().unwrap();
    assert!(detail.contains(&format!(
        "authorized root {:?}",
        d.path().canonicalize().unwrap()
    )));
    assert!(detail.contains("requested root \"/\""));
    assert!(detail.contains("zerostack-session serve --root \"/\""));
    assert!(detail.contains("send the same canonical root"));
    send(
        &mut s,
        json!({"type":"shutdown","id":4,"token":shutdown_token}),
    );
    let _ = read(&mut r);
    c.wait().unwrap();
    assert!(!sock.exists())
}

#[test]
fn approval_grant_reaches_one_exact_worker_call_and_cannot_replay_or_leak() {
    let (directory, mut session, token, shutdown_token, _) =
        start(ProcessIdentity::current().unwrap());
    let socket = directory.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    let root = directory.path().canonicalize().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let grant = json!({
        "schema":"zerostack.session.approval_grant.v1",
        "grant_id":"grant-exact-1",
        "engine":"fszero",
        "root":root,
        "generation":generation,
        "request_id":11,
        "operation":"fs.write",
        "effect":"approval_required_mutation",
        "authority_digest":"a".repeat(64),
        "policy_digest":"b".repeat(64),
        "issued_at_unix_ms":now.saturating_sub(1),
        "expires_at_unix_ms":now.saturating_add(60_000),
    });
    let source = "return await zero.fs.compound('write',{__approval_fixture:true});";
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":11,
            "generation":generation,
            "root":directory.path(),
            "source":source,
            "approval_grants":[grant.clone()],
        }),
    );
    let response = read(&mut reader);
    assert_eq!(response["ok"], true, "{response}");
    let exact = exact_result(directory.path(), &response);
    let forwarded = &exact["content"]["value"]["value"]["approval_grant"];
    assert_eq!(forwarded["grant_id"], "grant-exact-1");
    assert_eq!(forwarded["engine"], "fszero");
    assert_eq!(forwarded["root"], root.to_string_lossy().as_ref());
    assert_eq!(forwarded["operation"], "fs.write");
    assert_eq!(forwarded["effect"], "approval_required_mutation");
    assert!(forwarded["request_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));

    let mut replay = grant;
    replay["request_id"] = json!(12);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":12,
            "generation":generation,
            "root":directory.path(),
            "source":source,
            "approval_grants":[replay],
        }),
    );
    let replayed = read(&mut reader);
    assert_eq!(replayed["ok"], false, "{replayed}");
    assert_eq!(replayed["code"], "approval_replay");

    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":13,
            "generation":generation,
            "root":directory.path(),
            "source":source,
        }),
    );
    let unapproved = read(&mut reader);
    assert_eq!(unapproved["ok"], false, "{unapproved}");
    assert_eq!(unapproved["code"], "backend_execution");
    assert!(unapproved["error"]
        .as_str()
        .unwrap()
        .contains("worker approval required or denied"));

    send(
        &mut stream,
        json!({"type":"shutdown","id":14,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    session.wait().unwrap();
}
#[test]
fn aggregate_promise_all_uses_one_bounded_parallel_wave() {
    let (directory, mut session, token, shutdown_token, _) =
        start_configured(ProcessIdentity::current().unwrap(), |command, _| {
            command.env("ZEROSTACK_TOKENZERO_RAW_ARGS", "sleep");
        });
    let socket = directory.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    // Leave enough time for the elapsed assertion to report accidental
    // serialization instead of surfacing SO_RCVTIMEO as an opaque WouldBlock.
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let started = Instant::now();
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":1,
            "generation":generation,
            "root":directory.path(),
            "source":r#"const calls = Array.from({length: 3}, (_, sequence) => {
                const ref = `tz://blob/${String(sequence).padStart(64, '0')}`;
                return zero.token.expand(ref).then(value => value.content.value.value.args.ref);
            });
                return await Promise.all(calls);"#,
        }),
    );
    let response = read(&mut reader);
    let elapsed = started.elapsed();
    assert_eq!(response["ok"], true, "{response}");
    let exact = exact_result(directory.path(), &response);
    let expected = Value::Array(
        (0..3)
            .map(|sequence| Value::String(format!("tz://blob/{sequence:064}")))
            .collect(),
    );
    assert_eq!(exact, expected);
    assert!(
        elapsed < Duration::from_secs(4),
        "three two-second calls did not finish in one bounded parallel wave: {elapsed:?}"
    );
    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
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
    let response = read(&mut reader);
    assert_eq!(response["ok"], true, "{response}");
    let result = exact_result(d.path(), &response);
    for name in [
        "fsPlan",
        "fsStructural",
        "fsCompound",
        "fsReadMany",
        "fsListMany",
        "fsSearchMany",
        "fsAstMany",
        "graphBlast",
        "graphQuery",
        "graphOrient",
        "graphRecall",
        "graphVerify",
        "graphSnap",
        "graphReserve",
        "graphIndex",
        "graphRemember",
        "tokenCompact",
        "fsExpand",
        "graphExpand",
        "tokenExpand",
        "tokenFind",
        "tokenShell",
    ] {
        serde_json::from_value::<zero_abi::ZeroResultV1>(result[name].clone())
            .unwrap_or_else(|error| panic!("{name} did not emit zero-result/v1: {error}"));
    }
    assert_eq!(
        inline_public(&result["fsPlan"])["value"]["args"]["queries"][0],
        "widget"
    );
    assert_eq!(
        inline_public(&result["fsReadMany"])["value"]["args"]["paths"][0],
        "a.rs"
    );
    assert_eq!(
        inline_public(&result["fsSearchMany"])["value"]["args"]["queries"][0],
        "Widget"
    );
    assert_eq!(
        inline_public(&result["graphRecall"])["value"]["args"]["query"],
        "Widget"
    );
    assert_eq!(
        inline_public(&result["tokenShell"])["value"]["args"]["command"],
        "printf ok"
    );
    assert_eq!(
        inline_public(&result["fsStructural"])["value"]["args"]["query"],
        "callers:Widget"
    );
    assert_eq!(
        inline_public(&result["graphBlast"])["value"]["args"]["depth"],
        2
    );
    assert_eq!(
        inline_public(&result["graphQuery"])["value"]["args"]["surface"],
        "symbol"
    );
    assert_eq!(
        inline_public(&result["fsExpand"])["metadata"]["ownership"]["engine"],
        "fszero"
    );
    assert_eq!(
        inline_public(&result["graphExpand"])["metadata"]["ownership"]["engine"],
        "graphzero"
    );
    assert_eq!(
        inline_public(&result["tokenExpand"])["metadata"]["ownership"]["engine"],
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
fn opaque_handles_cross_fszero_graphzero_tokenzero_without_translating_bytes() {
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
        const fs = await zero.fs.compound('read', {
            __opaque_chain_fixture:true,
            payload_hex:'00ff807f0a0d225ce298830041'
        });
        const graph = await zero.graph.remember({
            __opaque_chain_fixture:true,
            source_ref:fs.content.value.value.ref
        });
        const token = await zero.token.compact({
            __opaque_chain_fixture:true,
            source_ref:graph.content.value.value.ref
        });
        const expanded = await zero.token.expand(token.content.value.value.ref);
        return {fs,graph,token,expanded};
    "#;
    send(
        &mut stream,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":source}),
    );
    let response = read(&mut reader);
    assert_eq!(response["ok"], true, "{response}");
    let result = exact_result(d.path(), &response);

    let fs_ref = "fz://blob/bcd20edba0525325b8fdfcfb22adaaf4196def23850e610e79e19722f354ea05";
    let graph_ref = "gz://blob/e883e99307a4c9d4e5ceb783a46e8bb4653e87d424f225d3906f01632fc7f189";
    let token_ref = "tz://blob/2f3c9f0c1d762e6bb7f1090ee708f8dff09d772e10a6568e1c7b7a2f607b97f6";
    assert_eq!(inline_public(&result["fs"])["value"]["ref"], fs_ref);
    assert_eq!(
        inline_public(&result["graph"])["value"],
        json!({"ref":graph_ref})
    );
    assert_eq!(
        inline_public(&result["token"])["value"],
        json!({"ref":token_ref})
    );
    assert_eq!(
        inline_public(&result["fs"])["metadata"]["ownership"]["refs"],
        json!([fs_ref])
    );
    assert_eq!(
        inline_public(&result["graph"])["metadata"]["ownership"]["refs"],
        json!([graph_ref])
    );
    assert_eq!(
        inline_public(&result["token"])["metadata"]["ownership"]["refs"],
        json!([token_ref])
    );
    assert!(!result["graph"].to_string().contains("fz://"));
    assert!(!result["token"].to_string().contains("gz://"));
    assert_eq!(
        inline_public(&result["expanded"])["value"],
        json!({
            "payload_hex":"00ff807f0a0d225ce298830041",
            "sha256":"bcd20edba0525325b8fdfcfb22adaaf4196def23850e610e79e19722f354ea05",
            "length":13
        })
    );
    assert_eq!(
        inline_public(&result["expanded"])["metadata"]["ownership"]["engine"],
        "tokenzero"
    );

    let fixture = d.path().join(".zerostack-opaque-chain-fixture");
    assert_eq!(
        std::fs::read(
            fixture
                .join("bytes")
                .join("bcd20edba0525325b8fdfcfb22adaaf4196def23850e610e79e19722f354ea05")
        )
        .unwrap(),
        [0, 255, 128, 127, 10, 13, 34, 92, 226, 152, 131, 0, 65]
    );
    assert_eq!(
        std::fs::read(
            fixture
                .join("graph")
                .join("e883e99307a4c9d4e5ceb783a46e8bb4653e87d424f225d3906f01632fc7f189.ref")
        )
        .unwrap(),
        fs_ref.as_bytes()
    );
    assert_eq!(
        std::fs::read(
            fixture
                .join("token")
                .join("2f3c9f0c1d762e6bb7f1090ee708f8dff09d772e10a6568e1c7b7a2f607b97f6.ref")
        )
        .unwrap(),
        graph_ref.as_bytes()
    );

    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn replacement_cancels_inflight_and_suppresses_stale_result() {
    let (d, mut session, token, shutdown_token, initial_generation) =
        start_configured(ProcessIdentity::current().unwrap(), |command, _| {
            command.env("ZEROSTACK_TOKENZERO_RAW_ARGS", "hold");
        });
    let socket = d.path().join("runtime/session.sock");
    let (mut old_stream, mut old_reader, old_generation) = connect_authenticated(&socket, &token);
    assert_eq!(old_generation, initial_generation);
    send(
        &mut old_stream,
        json!({
            "type":"execute",
            "id":41,
            "generation":old_generation,
            "root":d.path(),
            "source":"return await zero.token.shell('hold');",
            "timeout_ms":30000
        }),
    );
    thread::sleep(Duration::from_millis(20));

    let (mut control, mut control_reader, control_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(control_generation, old_generation);
    send(
        &mut control,
        json!({
            "type":"replace",
            "id":42,
            "generation":old_generation,
            "token":shutdown_token,
            "reason":"before_switch"
        }),
    );
    let replaced = read(&mut control_reader);
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(replaced["result"]["previous_generation"], old_generation);
    assert_eq!(replaced["generation"], old_generation);
    assert_eq!(replaced["result"]["reauthentication_required"], true);
    let new_generation = replaced["result"]["generation"].as_u64().unwrap();
    assert_eq!(new_generation, old_generation + 1);

    let (mut current_stream, mut current_reader, authenticated_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(authenticated_generation, new_generation);
    send(
        &mut current_stream,
        json!({
            "type":"execute",
            "id":44,
            "generation":new_generation,
            "root":d.path(),
            "source":"return 44;"
        }),
    );
    let stale_settlement = read(&mut old_reader);
    assert_eq!(stale_settlement["ok"], false, "{stale_settlement}");
    assert_eq!(stale_settlement["id"], 41);
    assert_eq!(stale_settlement["code"], "stale_generation");

    let (mut stale_stream, mut stale_reader, observed_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(observed_generation, new_generation);
    send(
        &mut stale_stream,
        json!({
            "type":"execute",
            "id":43,
            "generation":old_generation,
            "root":d.path(),
            "source":"return 'must-not-run';"
        }),
    );
    let stale_admission = read(&mut stale_reader);
    assert_eq!(stale_admission["code"], "stale_generation");

    let current = read(&mut current_reader);
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["result"], 44);
    send(
        &mut current_stream,
        json!({"type":"shutdown","id":45,"token":shutdown_token}),
    );
    assert_eq!(read(&mut current_reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn overlapping_replacements_have_one_linearized_winner() {
    let (d, mut session, token, shutdown_token, generation) =
        start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut first, mut first_reader, first_generation) = connect_authenticated(&socket, &token);
    let (mut second, mut second_reader, second_generation) = connect_authenticated(&socket, &token);
    assert_eq!(first_generation, generation);
    assert_eq!(second_generation, generation);
    send(
        &mut first,
        json!({
            "type":"replace","id":300,"generation":generation,
            "token":shutdown_token,"reason":"before_fork"
        }),
    );
    send(
        &mut second,
        json!({
            "type":"replace","id":301,"generation":generation,
            "token":shutdown_token,"reason":"worker_revision_change"
        }),
    );
    let first_result = read(&mut first_reader);
    let second_result = read(&mut second_reader);
    assert_ne!(first_result["ok"], second_result["ok"]);
    let loser = if first_result["ok"] == false {
        &first_result
    } else {
        &second_result
    };
    assert!(
        loser["code"] == "stale_generation"
            || loser["code"] == "replacement_in_progress"
            || loser["code"] == "reauthentication_required",
        "{loser}"
    );
    let next_generation = if first_result["ok"] == true {
        first_result["result"]["generation"].as_u64().unwrap()
    } else {
        second_result["result"]["generation"].as_u64().unwrap()
    };
    let (mut current, mut current_reader, observed_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(observed_generation, next_generation);
    send(
        &mut current,
        json!({"type":"shutdown","id":302,"token":shutdown_token}),
    );
    assert_eq!(read(&mut current_reader)["generation"], next_generation);
    assert!(session.wait().unwrap().success());
}

#[test]
fn concurrent_client_bound_returns_typed_retry_guidance() {
    let (d, mut session, token, shutdown_token, generation) =
        start_configured(ProcessIdentity::current().unwrap(), |command, _| {
            command.env("ZEROSTACK_TOKENZERO_RAW_ARGS", "hold");
        });
    let socket = d.path().join("runtime/session.sock");
    let mut blocked = Vec::new();
    for id in 100..107 {
        let (mut stream, reader, observed_generation) = connect_authenticated(&socket, &token);
        assert_eq!(observed_generation, generation);
        send(
            &mut stream,
            json!({
                "type":"execute","id":id,"generation":generation,"root":d.path(),
                "source":"return await zero.token.shell('hold');","timeout_ms":30000
            }),
        );
        blocked.push((stream, reader));
    }
    let (mut control, mut control_reader, control_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(control_generation, generation);

    let overflow = UnixStream::connect(&socket).unwrap();
    overflow
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut overflow_reader = BufReader::new(overflow);
    let rejected = read(&mut overflow_reader);
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert_eq!(rejected["code"], "backpressure");
    assert_eq!(rejected["retry_after_ms"], 1);

    send(
        &mut control,
        json!({
            "type":"replace","id":200,"generation":generation,
            "token":shutdown_token,"reason":"manual"
        }),
    );
    let replaced = read(&mut control_reader);
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(replaced["generation"], generation);
    let next_generation = replaced["result"]["generation"].as_u64().unwrap();
    for (_, mut reader) in blocked {
        let settlement = read(&mut reader);
        assert_eq!(settlement["ok"], false, "{settlement}");
        assert_eq!(settlement["code"], "stale_generation");
    }
    let (mut current, mut current_reader, observed_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(observed_generation, next_generation);
    send(
        &mut current,
        json!({"type":"shutdown","id":201,"token":shutdown_token}),
    );
    let stopped = read(&mut current_reader);
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert_eq!(stopped["generation"], next_generation);
    assert!(session.wait().unwrap().success());
}

#[test]
fn request_ids_are_global_per_generation() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut first, mut first_reader, generation) = connect_authenticated(&socket, &token);
    let (mut second, mut second_reader, second_generation) = connect_authenticated(&socket, &token);
    assert_eq!(second_generation, generation);
    send(
        &mut first,
        json!({
            "type":"execute","id":77,"generation":generation,
            "root":d.path(),"source":"return 1;"
        }),
    );
    assert_eq!(read(&mut first_reader)["result"], 1);
    send(
        &mut second,
        json!({
            "type":"execute","id":77,"generation":generation,
            "root":d.path(),"source":"return 2;"
        }),
    );
    let duplicate = read(&mut second_reader);
    assert_eq!(duplicate["ok"], false, "{duplicate}");
    assert_eq!(duplicate["code"], "duplicate_request_id");
    send(
        &mut second,
        json!({"type":"shutdown","id":78,"token":shutdown_token}),
    );
    assert_eq!(read(&mut second_reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn ten_thousand_calls_settle_once_without_generation_drift() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    for id in 1..=10_000_u64 {
        send(
            &mut stream,
            json!({
                "type":"execute","id":id,"generation":generation,
                "root":d.path(),"source":"return 1;"
            }),
        );
        let settled = read(&mut reader);
        assert_eq!(settled["ok"], true, "request {id}: {settled}");
        assert_eq!(settled["id"], id);
        assert_eq!(settled["generation"], generation);
        assert_eq!(settled["result"], 1);
    }
    send(
        &mut stream,
        json!({"type":"shutdown","id":10_001,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
#[ignore = "native timing gate; run explicitly on an idle release-gate host"]
fn warm_session_entry_latency_meets_p50_and_p95_gates() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    for id in 1..=100_u64 {
        send(
            &mut stream,
            json!({
                "type":"execute","id":id,"generation":generation,
                "root":d.path(),"source":"return 1;","timeout_ms":1000
            }),
        );
        assert_eq!(read(&mut reader)["ok"], true);
    }

    let mut samples_us = Vec::with_capacity(1_000);
    for id in 101..=1_100_u64 {
        let started = Instant::now();
        send(
            &mut stream,
            json!({
                "type":"execute","id":id,"generation":generation,
                "root":d.path(),"source":"return 1;","timeout_ms":1000
            }),
        );
        let settled = read(&mut reader);
        samples_us.push(started.elapsed().as_micros());
        assert_eq!(settled["ok"], true, "request {id}: {settled}");
    }
    samples_us.sort_unstable();
    let p50_us = samples_us[499];
    let p95_us = samples_us[949];
    println!("warm_session_latency p50_us={p50_us} p95_us={p95_us}");
    assert!(p50_us <= 1_000, "warm p50 {p50_us}us exceeds 1000us");
    assert!(p95_us <= 2_000, "warm p95 {p95_us}us exceeds 2000us");

    send(
        &mut stream,
        json!({"type":"shutdown","id":1_101,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn interpreter_globals_do_not_cross_model_visible_plans() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    send(
        &mut stream,
        json!({
            "type":"execute","id":90,"generation":generation,"root":d.path(),
            "source":"globalThis.__zerostack_leak_probe=7;return __zerostack_leak_probe;"
        }),
    );
    assert_eq!(read(&mut reader)["result"], 7);
    send(
        &mut stream,
        json!({
            "type":"execute","id":91,"generation":generation,"root":d.path(),
            "source":"return typeof globalThis.__zerostack_leak_probe;"
        }),
    );
    assert_eq!(read(&mut reader)["result"], "undefined");
    send(
        &mut stream,
        json!({"type":"shutdown","id":92,"token":shutdown_token}),
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

#[cfg(feature = "worker-fixture")]
#[test]
fn terminal_cancellation_rejects_queued_execution() {
    let d = TempDir::new().unwrap();
    let exec = SessionExecutor::new_with_worker_fixture(
        d.path().to_path_buf(),
        "test-terminal-cancel".into(),
        env!("CARGO_BIN_EXE_zero-codemode-worker-fixture").into(),
    )
    .unwrap();
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
    let exact = exact_result(d.path(), &response);
    assert_eq!(
        inline_public(&exact["fs"])["metadata"]["ownership"]["engine"],
        "fszero"
    );
    assert_eq!(
        inline_public(&exact["graph"])["metadata"]["ownership"]["engine"],
        "graphzero"
    );
    assert_eq!(
        inline_public(&exact["token"])["metadata"]["ownership"]["engine"],
        "tokenzero"
    );
}

// ── Native lifecycle/resource evidence (q6am gate) ──────────────────────
//
// Thresholds are named Q6AM_* and use only integer units (ms, us, ppm,
// basis points, counts) so no float nondeterminism can enter the gate.
const Q6AM_CANONICAL_SOAK_SECONDS: u64 = 1800;
const Q6AM_SETTLE_SECONDS: u64 = 60;
const Q6AM_STRESS_RSS_GROWTH_BYTES: u64 = 5 * 1024 * 1024;
const Q6AM_IDLE_RSS_TARGET_BYTES: u64 = 96 * 1024 * 1024;
const Q6AM_IDLE_RSS_HARD_CAP_BYTES: u64 = 128 * 1024 * 1024;
const Q6AM_ACTIVE_RSS_CAP_BYTES: u64 = 256 * 1024 * 1024;
const Q6AM_IDLE_RSS_DRIFT_BYTES: u64 = 5 * 1024 * 1024;
const Q6AM_STABLE_COUNT_DRIFT: u64 = 0;
const Q6AM_IDLE_CPU_AVG_BP: u64 = 10; // strictly less than 0.1%
const Q6AM_IDLE_CPU_P99_BP: u64 = 100; // strictly less than 1%
const Q6AM_WARM_ENTRY_P50_US: u64 = 1_000;
const Q6AM_WARM_ENTRY_P95_US: u64 = 2_000;
const Q6AM_SHUTDOWN_P95_MS: u64 = 250;
const Q6AM_CRASH_REAP_P95_MS: u64 = 1_000;
const Q6AM_COLD_START_TARGET_MS: u64 = 3_000;
const Q6AM_COLD_START_HARD_CAP_MS: u64 = 5_000;
const Q6AM_ZSX_ADDED_P95_US: u64 = 1_000;

fn sha256_hex_file(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("read {} for SHA-256: {error}", path.display()));
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// "12.3" -> 1230 basis points (12.3%); integer-only parsing, no floats.
fn parse_cpu_percent_bp(raw: &str) -> u64 {
    let (whole, frac) = match raw.trim().split_once('.') {
        Some((whole, frac)) => (whole, frac.chars().next().unwrap_or('0')),
        None => (raw.trim(), '0'),
    };
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().unwrap_or(0)
    };
    let frac: u64 = frac.to_digit(10).unwrap_or(0) as u64;
    whole
        .saturating_mul(100)
        .saturating_add(frac.saturating_mul(10))
}

/// `MM:SS.frac` or `HH:MM:SS.frac` cumulative CPU time in microseconds.
fn parse_cpu_time_micros(raw: &str) -> u64 {
    let mut parts = raw.trim().split(':').collect::<Vec<_>>();
    let seconds = parts.pop().unwrap_or("0");
    let (whole_seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let whole_seconds = whole_seconds.parse::<u64>().unwrap_or(0);
    let mut fraction_micros = fraction.bytes().take(6).fold(0_u64, |value, byte| {
        let digit = if byte.is_ascii_digit() {
            u64::from(byte - b'0')
        } else {
            0
        };
        value.saturating_mul(10).saturating_add(digit)
    });
    for _ in fraction.len().min(6)..6 {
        fraction_micros = fraction_micros.saturating_mul(10);
    }
    let whole_minutes = match parts.as_slice() {
        [minutes] => minutes.parse::<u64>().unwrap_or(0),
        [hours, minutes] => hours
            .parse::<u64>()
            .unwrap_or(0)
            .saturating_mul(60)
            .saturating_add(minutes.parse::<u64>().unwrap_or(0)),
        _ => 0,
    };
    whole_minutes
        .saturating_mul(60_000_000)
        .saturating_add(whole_seconds.saturating_mul(1_000_000))
        .saturating_add(fraction_micros)
}

#[test]
fn cpu_time_parser_preserves_subsecond_precision() {
    assert_eq!(parse_cpu_time_micros("0:00.46"), 460_000);
    assert_eq!(parse_cpu_time_micros("12:34.5"), 754_500_000);
    assert_eq!(parse_cpu_time_micros("01:02:03.004005"), 3_723_004_005);
}

fn lsof_available(pid: u32) -> bool {
    Command::new("lsof")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .is_ok_and(|output| output.status.success())
}

fn count_fds(pid: u32, method: &str) -> Option<u64> {
    match method {
        "lsof" => {
            let output = Command::new("lsof")
                .arg("-p")
                .arg(pid.to_string())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            Some(
                text.lines()
                    .filter(|line| !line.starts_with("COMMAND"))
                    .count() as u64,
            )
        }
        "procfs" => {
            let fd_dir = std::path::Path::new("/proc")
                .join(pid.to_string())
                .join("fd");
            Some(std::fs::read_dir(fd_dir).ok()?.count() as u64)
        }
        _ => None,
    }
}

fn fd_evidence_method(pid: u32) -> &'static str {
    if lsof_available(pid) {
        "lsof"
    } else if std::path::Path::new("/proc")
        .join(pid.to_string())
        .join("fd")
        .is_dir()
    {
        "procfs"
    } else {
        "unavailable"
    }
}

fn native_thread_count(pid: u32) -> Option<u64> {
    if cfg!(target_os = "macos") {
        let output = Command::new("/bin/ps")
            .args(["-M", "-p", &pid.to_string(), "-o", "pid="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        return Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                .saturating_sub(1) as u64,
        );
    }
    None
}

#[derive(Clone)]
struct TreeSample {
    wall_unix_ms: u64,
    rss_bytes: u64,
    threads: u64,
    processes: u64,
    cpu_micros: u64,
    cpu_percent_bp: u64,
    fds: Option<u64>,
    fd_evidence: &'static str,
    pids: Vec<u32>,
}

/// One native evidence snapshot of the whole sidecar process tree (sidecar
/// plus every descendant reachable through ppid), via ps(1) plus lsof(1)
/// with a Linux /proc fallback for FD counts.
fn sample_tree(sidecar_pid: u32) -> TreeSample {
    let (all_processes, process_fields, field_count) = if cfg!(target_os = "macos") {
        ("-A", "pid=,ppid=,rss=,%cpu=,time=", 5)
    } else {
        ("-e", "pid=,ppid=,rss=,pcpu=,time=,nlwp=", 6)
    };
    let output = Command::new("/bin/ps")
        .args([all_processes, "-o", process_fields])
        .output()
        .unwrap_or_else(|error| panic!("native lifecycle evidence requires ps(1): {error}"));
    assert!(
        output.status.success(),
        "ps(1) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut rows: Vec<(u32, u32, u64, u64, u64, u64)> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != field_count {
            continue;
        }
        let (Ok(pid), Ok(ppid), Ok(rss_kb)) = (
            fields[0].parse::<u32>(),
            fields[1].parse::<u32>(),
            fields[2].parse::<u64>(),
        ) else {
            continue;
        };
        let nlwp = if cfg!(target_os = "macos") {
            0
        } else {
            let Ok(value) = fields[5].parse::<u64>() else {
                continue;
            };
            value
        };
        rows.push((
            pid,
            ppid,
            rss_kb,
            parse_cpu_percent_bp(fields[3]),
            parse_cpu_time_micros(fields[4]),
            nlwp,
        ));
    }
    let mut tree: std::collections::HashSet<u32> = std::collections::HashSet::from([sidecar_pid]);
    loop {
        let before = tree.len();
        for &(pid, ppid, _, _, _, _) in &rows {
            if tree.contains(&ppid) {
                tree.insert(pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    let evidence = fd_evidence_method(sidecar_pid);
    let mut sample = TreeSample {
        wall_unix_ms: now_unix_ms(),
        rss_bytes: 0,
        threads: 0,
        processes: 0,
        cpu_micros: 0,
        cpu_percent_bp: 0,
        fds: Some(0),
        fd_evidence: evidence,
        pids: Vec::new(),
    };
    for &(pid, _, rss_kb, cpu_bp, cpu_secs, nlwp) in &rows {
        if !tree.contains(&pid) {
            continue;
        }
        sample.processes += 1;
        sample.rss_bytes = sample.rss_bytes.saturating_add(rss_kb.saturating_mul(1024));
        sample.threads = sample
            .threads
            .saturating_add(native_thread_count(pid).unwrap_or(nlwp));
        sample.cpu_micros = sample.cpu_micros.saturating_add(cpu_secs);
        sample.cpu_percent_bp = sample.cpu_percent_bp.saturating_add(cpu_bp);
        sample.pids.push(pid);
        if let Some(fds) = count_fds(pid, evidence) {
            sample.fds = sample.fds.map(|total| total.saturating_add(fds));
        } else {
            sample.fds = None;
        }
    }
    sample.pids.sort_unstable();
    sample
}

fn percentile(values: &[u64], numerator: usize) -> u64 {
    assert!(!values.is_empty());
    values[((values.len() * numerator).div_ceil(100)).saturating_sub(1)]
}

fn threshold_at_most(name: &str, unit: &str, limit: u64, observed: u64) -> Value {
    json!({
        "name": name,
        "unit": unit,
        "comparison": "at_most",
        "limit": limit,
        "observed": observed,
        "pass": observed <= limit,
    })
}

fn threshold_less_than(name: &str, unit: &str, limit: u64, observed: u64) -> Value {
    json!({
        "name": name,
        "unit": unit,
        "comparison": "less_than",
        "limit": limit,
        "observed": observed,
        "pass": observed < limit,
    })
}

fn threshold_true(name: &str, observed: bool) -> Value {
    json!({
        "name": name,
        "unit": "bool",
        "comparison": "required_true",
        "observed": observed,
        "pass": observed,
    })
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn valid_git_head(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Native lifecycle/resource evidence gate. Runs real FSZero/GraphZero/
/// TokenZero workers, a warm cross-surface plan, 10,000 mixed calls, full
/// sidecar process-tree sampling via ps(1)/lsof(1), a configurable soak,
/// and writes canonical JSON to ZEROSTACK_LIFECYCLE_RECEIPT.
///
/// Environment contract (all read from the parent environment):
/// - ZERO_FSZERO_RAW_BIN, ZERO_GRAPHZERO_RAW_BIN, ZERO_TOKENZERO_RAW_BIN
///   (required): verified real raw-worker binaries. ZEROSTACK_TEST_MODE is
///   cleared so the workers run in normal (non-fixture) mode.
/// - ZEROSTACK_LIFECYCLE_RECEIPT (required): path of the JSON receipt.
/// - ZEROSTACK_LIFECYCLE_SOAK_SECONDS (optional, default 1800): soak length
///   in seconds. Exactly 1800 is canonical; any other value is a clearly
///   labeled noncanonical developer short run whose receipt can never pass.
/// - ZEROSTACK_SOURCE_HEAD and ZEROSTACK_HUB_HEAD (required): the exact hub
///   evidence-subject commit. ZERO_{FSZERO,GRAPHZERO,TOKENZERO}_SOURCE_HEAD
///   (required): exact engine source commits bound to the worker artifacts.
///
/// Requires native ps(1); lsof(1) is preferred for FD evidence with a Linux
/// /proc fallback when absent. The receipt says pass only when the soak is
/// canonical and every q6am threshold passes.
#[test]
#[ignore = "native lifecycle/resource gate; requires real ZERO_*_RAW_BIN workers, ps(1)/lsof(1), a canonical 1800s soak, and writes canonical JSON to ZEROSTACK_LIFECYCLE_RECEIPT; run explicitly on an idle release-gate host"]
fn native_lifecycle_resource_evidence_q6am_receipt() {
    let fszero = std::env::var("ZERO_FSZERO_RAW_BIN")
        .expect("ZERO_FSZERO_RAW_BIN must name a verified real FSZero raw worker");
    let graphzero = std::env::var("ZERO_GRAPHZERO_RAW_BIN")
        .expect("ZERO_GRAPHZERO_RAW_BIN must name a verified real GraphZero raw worker");
    let tokenzero = std::env::var("ZERO_TOKENZERO_RAW_BIN")
        .expect("ZERO_TOKENZERO_RAW_BIN must name a verified real TokenZero raw worker");
    let receipt_path = std::env::var("ZEROSTACK_LIFECYCLE_RECEIPT")
        .expect("ZEROSTACK_LIFECYCLE_RECEIPT must name the canonical JSON receipt path");
    let soak_seconds: u64 = std::env::var("ZEROSTACK_LIFECYCLE_SOAK_SECONDS")
        .map(|value| {
            value.parse().unwrap_or_else(|error| {
                panic!("ZEROSTACK_LIFECYCLE_SOAK_SECONDS={value:?}: {error}")
            })
        })
        .unwrap_or(Q6AM_CANONICAL_SOAK_SECONDS);
    assert!(
        soak_seconds >= 1,
        "ZEROSTACK_LIFECYCLE_SOAK_SECONDS must be >= 1"
    );
    let canonical = soak_seconds == Q6AM_CANONICAL_SOAK_SECONDS;
    let source_head = std::env::var("ZEROSTACK_SOURCE_HEAD")
        .expect("ZEROSTACK_SOURCE_HEAD must bind the evidence-subject commit");
    let hub_head = std::env::var("ZEROSTACK_HUB_HEAD")
        .expect("ZEROSTACK_HUB_HEAD must bind the evidence-subject commit");
    let fszero_head = std::env::var("ZERO_FSZERO_SOURCE_HEAD")
        .expect("ZERO_FSZERO_SOURCE_HEAD must bind the real worker source");
    let graphzero_head = std::env::var("ZERO_GRAPHZERO_SOURCE_HEAD")
        .expect("ZERO_GRAPHZERO_SOURCE_HEAD must bind the real worker source");
    let repository_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("zero-codemode must remain under the workspace root");
    let repository_root_text = repository_root
        .to_str()
        .expect("workspace root must be UTF-8");
    let repository_head = command_stdout("git", &["-C", repository_root_text, "rev-parse", "HEAD"]);
    let repository_status = command_stdout(
        "git",
        &[
            "-C",
            repository_root_text,
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
    );
    let repository_clean = repository_status.as_deref() == Some("");
    let tokenzero_head = std::env::var("ZERO_TOKENZERO_SOURCE_HEAD")
        .expect("ZERO_TOKENZERO_SOURCE_HEAD must bind the real worker source");

    let sidecar_sha = sha256_hex_file(std::path::Path::new(env!(
        "CARGO_BIN_EXE_zerostack-session"
    )));
    let fszero_sha = sha256_hex_file(std::path::Path::new(&fszero));
    let graphzero_sha = sha256_hex_file(std::path::Path::new(&graphzero));
    let tokenzero_sha = sha256_hex_file(std::path::Path::new(&tokenzero));

    let cold_start_started = Instant::now();
    let (d, mut session, token, shutdown_token, generation) =
        start_configured(ProcessIdentity::current().unwrap(), |command, _| {
            command
                .env("ZERO_FSZERO_RAW_BIN", &fszero)
                .env("ZERO_GRAPHZERO_RAW_BIN", &graphzero)
                .env("ZERO_TOKENZERO_RAW_BIN", &tokenzero)
                .env_remove("ZEROSTACK_TEST_MODE");
        });
    let cold_start_ms = cold_start_started.elapsed().as_millis() as u64;
    std::fs::write(d.path().join("fixture.txt"), "lifecycle fixture\n").unwrap();
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, observed_generation) = connect_authenticated(&socket, &token);
    assert_eq!(observed_generation, generation);
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();

    send(
        &mut stream,
        json!({"type":"status","id":0,"generation":generation}),
    );
    let resource_status = read(&mut reader);
    assert_eq!(resource_status["ok"], true, "{resource_status}");
    assert_eq!(
        resource_status["result"]["schema"],
        "zerostack.session.aggregate_resource_receipt.v1"
    );
    let hard_tree_memory_enforced =
        resource_status["result"]["workers"]
            .as_array()
            .is_some_and(|workers| {
                workers.len() == 3
                    && workers
                        .iter()
                        .all(|worker| worker["hard_tree_memory_enforced"] == true)
            });
    let zsx_plan = d.path().join("zsx-latency.js");
    std::fs::write(&zsx_plan, "return 1;").unwrap();
    let mut zsx_us = Vec::with_capacity(100);
    for probe in 0..100_u64 {
        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_zsx"))
            .args([
                "exec",
                "-C",
                d.path().to_str().unwrap(),
                "--file",
                zsx_plan.to_str().unwrap(),
            ])
            .env("ZEROSTACK_SESSION_SOCKET", &socket)
            .env("ZEROSTACK_SESSION_TOKEN", &token)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "zsx probe {probe}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["ok"],
            true,
            "zsx probe {probe}"
        );
        zsx_us.push(started.elapsed().as_micros() as u64);
    }
    zsx_us.sort_unstable();
    // Cold then warm cross-surface plan (real FSZero + GraphZero + TokenZero).
    let cross_surface = r#"const fs = await zero.fs.compound('read', {path:'fixture.txt'});
        const graph = await zero.graph.index();
        const token = await zero.token.compact('lifecycle fixture');
        return {fs,graph,token};"#;
    let cold_started = Instant::now();
    send(
        &mut stream,
        json!({"type":"execute","id":1,"generation":generation,"root":d.path(),"source":cross_surface,"timeout_ms":30000}),
    );
    let cold_response = read(&mut reader);
    assert_eq!(cold_response["ok"], true, "{cold_response}");
    let cold_plan_ms = cold_started.elapsed().as_millis() as u64;
    let warm_started = Instant::now();
    send(
        &mut stream,
        json!({"type":"execute","id":2,"generation":generation,"root":d.path(),"source":cross_surface,"timeout_ms":30000}),
    );
    let warm_response = read(&mut reader);
    assert_eq!(warm_response["ok"], true, "{warm_response}");
    let warm_plan_ms = warm_started.elapsed().as_millis() as u64;

    // Baseline tree sample, then 10,000 mixed calls in batches of 2,000 with
    // a full ps/lsof tree sample after every batch (stress growth evidence).
    let baseline = sample_tree(session.id());
    let stress_started = Instant::now();
    let mut stress_samples = Vec::new();
    let mut generation_drift = 0_u64;
    let mut next_id = 3_u64;
    for _batch in 1..=5_u64 {
        for _offset in 0..2_000_u64 {
            let id = next_id;
            next_id += 1;
            let source = match id % 6 {
                0 | 1 => "return await zero.fs.compound('read', {path:'fixture.txt'});".to_string(),
                2 | 3 => "return await zero.graph.index();".to_string(),
                _ => format!("return await zero.token.compact('mix-{id}');"),
            };
            send(
                &mut stream,
                json!({"type":"execute","id":id,"generation":generation,"root":d.path(),"source":source,"timeout_ms":30000}),
            );
            let settled = read(&mut reader);
            assert_eq!(settled["ok"], true, "stress call {id}: {settled}");
            if settled["generation"] != generation {
                generation_drift += 1;
            }
            assert_eq!(
                settled["generation"], generation,
                "generation drift at stress call {id}: {settled}"
            );
        }
        stress_samples.push(sample_tree(session.id()));
    }
    let stress_elapsed_seconds = stress_started.elapsed().as_secs();
    let post_stress = stress_samples.last().cloned().unwrap();
    let stress_growth_rss_bytes = post_stress.rss_bytes.saturating_sub(baseline.rss_bytes);
    let active_peak_rss_bytes = stress_samples
        .iter()
        .map(|sample| sample.rss_bytes)
        .max()
        .unwrap_or(post_stress.rss_bytes);
    let stress_growth_fds = match (baseline.fds, post_stress.fds) {
        (Some(base), Some(later)) => Some(later.saturating_sub(base)),
        _ => None,
    };
    let stress_growth_threads = post_stress.threads.saturating_sub(baseline.threads);
    let stress_growth_processes = post_stress.processes.saturating_sub(baseline.processes);

    // The acceptance window begins only after a bounded settle interval. A
    // noncanonical developer run uses one second so it stays quick, but can
    // never produce pass=true.
    thread::sleep(Duration::from_secs(if canonical {
        Q6AM_SETTLE_SECONDS
    } else {
        1
    }));
    let idle_baseline = sample_tree(session.id());
    let soak_deadline = Instant::now() + Duration::from_secs(soak_seconds);
    let sample_interval = Duration::from_secs((soak_seconds / 30).clamp(1, 60));
    let mut soak_samples = Vec::new();
    loop {
        let now = Instant::now();
        if now >= soak_deadline {
            break;
        }
        let wait = sample_interval.min(soak_deadline.saturating_duration_since(now));
        thread::sleep(wait);
        soak_samples.push(sample_tree(session.id()));
    }
    let idle_last = soak_samples.last().unwrap_or(&idle_baseline);
    let soak_process_drift = soak_samples
        .iter()
        .map(|sample| sample.processes.abs_diff(idle_baseline.processes))
        .max()
        .unwrap_or(0);
    let soak_thread_drift = soak_samples
        .iter()
        .map(|sample| sample.threads.abs_diff(idle_baseline.threads))
        .max()
        .unwrap_or(0);
    let soak_fd_drift = match idle_baseline.fds {
        Some(base) => soak_samples
            .iter()
            .map(|sample| sample.fds.map(|fds| fds.abs_diff(base)))
            .collect::<Option<Vec<_>>>()
            .and_then(|values| values.into_iter().max()),
        None => None,
    };
    let idle_min_rss_bytes = std::iter::once(idle_baseline.rss_bytes)
        .chain(soak_samples.iter().map(|sample| sample.rss_bytes))
        .min()
        .unwrap();
    let idle_max_rss_bytes = std::iter::once(idle_baseline.rss_bytes)
        .chain(soak_samples.iter().map(|sample| sample.rss_bytes))
        .max()
        .unwrap();
    let idle_rss_drift_bytes = idle_max_rss_bytes.saturating_sub(idle_min_rss_bytes);
    let idle_cpu_delta_micros = idle_last
        .cpu_micros
        .saturating_sub(idle_baseline.cpu_micros);
    let idle_cpu_avg_bp =
        idle_cpu_delta_micros.saturating_mul(10_000) / soak_seconds.saturating_mul(1_000_000);
    let mut idle_cpu_samples_bp = soak_samples
        .iter()
        .map(|sample| sample.cpu_percent_bp)
        .collect::<Vec<_>>();
    idle_cpu_samples_bp.sort_unstable();
    let idle_cpu_p99_bp = if idle_cpu_samples_bp.is_empty() {
        idle_baseline.cpu_percent_bp
    } else {
        percentile(&idle_cpu_samples_bp, 99)
    };

    // Warm entry distribution after the soak. The existing zsx process-overhead
    // gate remains separate; this measures the sidecar protocol itself.
    let mut idle_us = Vec::with_capacity(100);
    for _probe in 0..100_u64 {
        let id = next_id;
        next_id += 1;
        let started = Instant::now();
        send(
            &mut stream,
            json!({"type":"execute","id":id,"generation":generation,"root":d.path(),"source":"return 1;","timeout_ms":5000}),
        );
        let settled = read(&mut reader);
        assert_eq!(settled["ok"], true, "idle probe {id}: {settled}");
        assert_eq!(settled["generation"], generation, "{settled}");
        idle_us.push(started.elapsed().as_micros() as u64);
    }
    idle_us.sort_unstable();
    let warm_entry_p50_us = percentile(&idle_us, 50);
    let warm_entry_p95_us = percentile(&idle_us, 95);
    let final_identities = soak_samples
        .last()
        .map(|sample| &sample.pids)
        .unwrap_or(&baseline.pids)
        .iter()
        .filter_map(|pid| ProcessIdentity::capture(*pid).ok())
        .collect::<Vec<_>>();

    // Shutdown latency and runtime cleanup.

    let zsx_p95_us = percentile(&zsx_us, 95);
    let zsx_added_p95_us = zsx_p95_us.saturating_sub(warm_entry_p95_us);
    let shutdown_started = Instant::now();
    send(
        &mut stream,
        json!({"type":"shutdown","id":next_id,"token":shutdown_token}),
    );
    let stopped = read(&mut reader);
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert!(session.wait().unwrap().success());
    let shutdown_ms = shutdown_started.elapsed().as_millis() as u64;
    let socket_gone = !socket.exists();
    let runtime_gone = !d.path().join("runtime").exists();
    let tree_empty = final_identities
        .iter()
        .all(|identity| !identity.is_live().unwrap_or(false));
    let runtime_cleanup = socket_gone && runtime_gone && tree_empty;

    // Twenty native trials establish p95 teardown instead of treating one
    // successful sample as a distribution. Each trial prewarms the same three
    // real workers under a fresh session-owned runtime.
    let mut normal_teardown_ms = vec![shutdown_ms];
    for trial in 1..20_u64 {
        let (trial_dir, mut trial_session, trial_token, trial_shutdown_token, _) =
            start_configured(ProcessIdentity::current().unwrap(), |command, _| {
                command
                    .env("ZERO_FSZERO_RAW_BIN", &fszero)
                    .env("ZERO_GRAPHZERO_RAW_BIN", &graphzero)
                    .env("ZERO_TOKENZERO_RAW_BIN", &tokenzero)
                    .env_remove("ZEROSTACK_TEST_MODE");
            });
        let trial_socket = trial_dir.path().join("runtime/session.sock");
        let (mut trial_stream, mut trial_reader, _) =
            connect_authenticated(&trial_socket, &trial_token);
        let trial_identities = sample_tree(trial_session.id())
            .pids
            .into_iter()
            .filter_map(|pid| ProcessIdentity::capture(pid).ok())
            .collect::<Vec<_>>();
        let started = Instant::now();
        send(
            &mut trial_stream,
            json!({"type":"shutdown","id":20_000 + trial,"token":trial_shutdown_token}),
        );
        assert_eq!(read(&mut trial_reader)["ok"], true);
        assert!(trial_session.wait().unwrap().success());
        normal_teardown_ms.push(started.elapsed().as_millis() as u64);
        assert!(!trial_socket.exists());
        assert!(!trial_dir.path().join("runtime").exists());
        assert!(
            trial_identities
                .iter()
                .all(|identity| !identity.is_live().unwrap_or(false)),
            "normal teardown left a captured process alive"
        );
    }
    normal_teardown_ms.sort_unstable();
    let normal_teardown_p95_ms = percentile(&normal_teardown_ms, 95);

    let mut crash_reap_ms = Vec::with_capacity(20);
    for _trial in 0..20_u64 {
        let mut owner = Command::new("sleep").arg("30").spawn().unwrap();
        let owner_identity = ProcessIdentity::capture(owner.id()).unwrap();
        let (trial_dir, mut trial_session, _, _, _) =
            start_configured(owner_identity, |command, _| {
                command
                    .env("ZERO_FSZERO_RAW_BIN", &fszero)
                    .env("ZERO_GRAPHZERO_RAW_BIN", &graphzero)
                    .env("ZERO_TOKENZERO_RAW_BIN", &tokenzero)
                    .env_remove("ZEROSTACK_TEST_MODE");
            });
        let trial_runtime = trial_dir.path().join("runtime");
        let trial_identities = sample_tree(trial_session.id())
            .pids
            .into_iter()
            .filter_map(|pid| ProcessIdentity::capture(pid).ok())
            .collect::<Vec<_>>();
        let started = Instant::now();
        owner.kill().unwrap();
        owner.wait().unwrap();
        assert!(trial_session.wait().unwrap().success());
        crash_reap_ms.push(started.elapsed().as_millis() as u64);
        assert!(!trial_runtime.exists());
        assert!(
            trial_identities
                .iter()
                .all(|identity| !identity.is_live().unwrap_or(false)),
            "owner crash left a captured process alive"
        );
    }
    crash_reap_ms.sort_unstable();
    let crash_reap_p95_ms = percentile(&crash_reap_ms, 95);

    // Exact q6am threshold table. Missing FD evidence is a failure, not a
    // successful skip, because the canonical gate requires stable FDs.
    let mut thresholds = vec![
        threshold_at_most(
            "stress_rss_growth",
            "bytes",
            Q6AM_STRESS_RSS_GROWTH_BYTES,
            stress_growth_rss_bytes,
        ),
        threshold_at_most(
            "stress_thread_growth",
            "threads",
            Q6AM_STABLE_COUNT_DRIFT,
            stress_growth_threads,
        ),
        threshold_at_most(
            "stress_process_growth",
            "processes",
            Q6AM_STABLE_COUNT_DRIFT,
            stress_growth_processes,
        ),
        threshold_at_most(
            "active_tree_rss",
            "bytes",
            Q6AM_ACTIVE_RSS_CAP_BYTES,
            active_peak_rss_bytes,
        ),
        threshold_at_most(
            "idle_tree_rss_target",
            "bytes",
            Q6AM_IDLE_RSS_TARGET_BYTES,
            idle_max_rss_bytes,
        ),
        threshold_at_most(
            "idle_tree_rss_hard_cap",
            "bytes",
            Q6AM_IDLE_RSS_HARD_CAP_BYTES,
            idle_max_rss_bytes,
        ),
        threshold_at_most(
            "idle_rss_drift",
            "bytes",
            Q6AM_IDLE_RSS_DRIFT_BYTES,
            idle_rss_drift_bytes,
        ),
        threshold_at_most(
            "soak_process_drift",
            "processes",
            Q6AM_STABLE_COUNT_DRIFT,
            soak_process_drift,
        ),
        threshold_at_most(
            "soak_thread_drift",
            "threads",
            Q6AM_STABLE_COUNT_DRIFT,
            soak_thread_drift,
        ),
        threshold_less_than(
            "idle_cpu_average",
            "basis_points",
            Q6AM_IDLE_CPU_AVG_BP,
            idle_cpu_avg_bp,
        ),
        threshold_less_than(
            "idle_cpu_p99",
            "basis_points",
            Q6AM_IDLE_CPU_P99_BP,
            idle_cpu_p99_bp,
        ),
        threshold_at_most(
            "warm_entry_p50",
            "us",
            Q6AM_WARM_ENTRY_P50_US,
            warm_entry_p50_us,
        ),
        threshold_at_most(
            "warm_entry_p95",
            "us",
            Q6AM_WARM_ENTRY_P95_US,
            warm_entry_p95_us,
        ),
        threshold_at_most(
            "zsx_added_p95",
            "us",
            Q6AM_ZSX_ADDED_P95_US,
            zsx_added_p95_us,
        ),
        threshold_at_most(
            "cold_start_target",
            "ms",
            Q6AM_COLD_START_TARGET_MS,
            cold_start_ms,
        ),
        threshold_at_most(
            "cold_start_hard_cap",
            "ms",
            Q6AM_COLD_START_HARD_CAP_MS,
            cold_start_ms,
        ),
        threshold_at_most(
            "normal_shutdown_p95",
            "ms",
            Q6AM_SHUTDOWN_P95_MS,
            normal_teardown_p95_ms,
        ),
        threshold_at_most(
            "owner_crash_reap_p95",
            "ms",
            Q6AM_CRASH_REAP_P95_MS,
            crash_reap_p95_ms,
        ),
        threshold_true("runtime_cleanup", runtime_cleanup),
        threshold_true("hard_tree_memory_enforced", hard_tree_memory_enforced),
        threshold_true("heads_match", source_head == hub_head),
        threshold_true(
            "heads_valid",
            [
                &source_head,
                &hub_head,
                &fszero_head,
                &graphzero_head,
                &tokenzero_head,
            ]
            .into_iter()
            .all(|head| valid_git_head(head)),
        ),
        threshold_true("release_profile", !cfg!(debug_assertions)),
        threshold_true(
            "source_head_matches_repository",
            repository_head.as_deref() == Some(source_head.as_str()),
        ),
        threshold_true("repository_clean", repository_clean),
    ];
    thresholds.push(match stress_growth_fds {
        Some(growth) => {
            threshold_at_most("stress_fd_growth", "fds", Q6AM_STABLE_COUNT_DRIFT, growth)
        }
        None => json!({
            "name": "stress_fd_growth",
            "unit": "fds",
            "comparison": "required_evidence",
            "observed": null,
            "pass": false,
            "failure": "fd evidence unavailable",
        }),
    });
    thresholds.push(match soak_fd_drift {
        Some(drift) => threshold_at_most("soak_fd_drift", "fds", Q6AM_STABLE_COUNT_DRIFT, drift),
        None => json!({
            "name": "soak_fd_drift",
            "unit": "fds",
            "comparison": "required_evidence",
            "observed": null,
            "pass": false,
            "failure": "fd evidence unavailable",
        }),
    });
    let all_pass = thresholds.iter().all(|threshold| threshold["pass"] == true);

    let receipt = json!({
        "schema": "zerostack.lifecycle_receipt.v1",
        "producer": "crates/zero-codemode/tests/session_sidecar.rs::native_lifecycle_resource_evidence_q6am_receipt",
        "receipt_sha256": "0".repeat(64),
        "receipt_sha256_convention": "sha256(canonical_json(receipt_sha256=64_ascii_zeroes))",
        "canonical": canonical,
        "canonical_soak_seconds": Q6AM_CANONICAL_SOAK_SECONDS,
        "soak_seconds": soak_seconds,
        "settle_seconds": if canonical { Q6AM_SETTLE_SECONDS } else { 1 },
        "noncanonical_reason": if canonical {
            Value::Null
        } else {
            json!("developer short soak: ZEROSTACK_LIFECYCLE_SOAK_SECONDS != 1800; evidence only, cannot pass")
        },
        "pass": canonical && all_pass,
        "captured_at_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        "invocation": {
            "test_binary": std::env::current_exe().unwrap(),
            "test_name": "native_lifecycle_resource_evidence_q6am_receipt",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "soak_env": soak_seconds.to_string(),
            "receipt_path": receipt_path,
        },
        "heads": {
            "evidence_subject": source_head,
            "hub": hub_head,
            "engines": {
                "fszero": fszero_head,
                "graphzero": graphzero_head,
                "tokenzero": tokenzero_head,
            },
        },
        "artifacts": {
            "sidecar": {
                "path": env!("CARGO_BIN_EXE_zerostack-session"),
                "sha256": sidecar_sha,
            },
            "workers": {
                "fszero": { "path": fszero, "sha256": fszero_sha },
                "graphzero": { "path": graphzero, "sha256": graphzero_sha },
                "tokenzero": { "path": tokenzero, "sha256": tokenzero_sha },
            },
        },
        "repository": {
            "root": repository_root,
            "observed_head": repository_head,
            "status_porcelain": repository_status,
        },
        "resource_policy": resource_status["result"].clone(),
        "measurements": {
            "cold_start_ms": cold_start_ms,
            "cold_plan_ms": cold_plan_ms,
            "warm_plan_ms": warm_plan_ms,
            "stress_calls": 10_000,
            "stress_elapsed_seconds": stress_elapsed_seconds,
            "generation_drift": generation_drift,
            "baseline": {
                "wall_unix_ms": baseline.wall_unix_ms,
                "rss_bytes": baseline.rss_bytes,
                "threads": baseline.threads,
                "processes": baseline.processes,
                "cpu_micros": baseline.cpu_micros,
                "cpu_percent_bp": baseline.cpu_percent_bp,
                "fds": baseline.fds,
            },
            "post_stress": {
                "wall_unix_ms": post_stress.wall_unix_ms,
                "rss_bytes": post_stress.rss_bytes,
                "threads": post_stress.threads,
                "processes": post_stress.processes,
                "cpu_micros": post_stress.cpu_micros,
                "cpu_percent_bp": post_stress.cpu_percent_bp,
                "fds": post_stress.fds,
            },
            "stress_growth": {
                "rss_bytes": stress_growth_rss_bytes,
                "fds": stress_growth_fds,
                "threads": stress_growth_threads,
                "processes": stress_growth_processes,
            },
            "stress_samples": stress_samples.iter().map(|sample| json!({
                "wall_unix_ms": sample.wall_unix_ms,
                "rss_bytes": sample.rss_bytes,
                "threads": sample.threads,
                "processes": sample.processes,
                "cpu_micros": sample.cpu_micros,
                "cpu_percent_bp": sample.cpu_percent_bp,
                "fds": sample.fds,
            })).collect::<Vec<_>>(),
            "soak_samples": soak_samples.iter().map(|sample| json!({
                "wall_unix_ms": sample.wall_unix_ms,
                "rss_bytes": sample.rss_bytes,
                "threads": sample.threads,
                "processes": sample.processes,
                "cpu_micros": sample.cpu_micros,
                "cpu_percent_bp": sample.cpu_percent_bp,
                "fds": sample.fds,
            })).collect::<Vec<_>>(),
            "idle": {
                "rss_min_bytes": idle_min_rss_bytes,
                "rss_max_bytes": idle_max_rss_bytes,
                "rss_drift_bytes": idle_rss_drift_bytes,
                "cpu_delta_micros": idle_cpu_delta_micros,
                "cpu_average_basis_points": idle_cpu_avg_bp,
                "cpu_p99_basis_points": idle_cpu_p99_bp,
                "entry_probes": idle_us.len(),
                "entry_p50_us": warm_entry_p50_us,
                "entry_p95_us": warm_entry_p95_us,
            },
            "zsx": {
                "probes": zsx_us.len(),
                "p95_us": zsx_p95_us,
                "added_p95_us": zsx_added_p95_us,
            },
            "teardown": {
                "normal_samples_ms": normal_teardown_ms,
                "normal_p95_ms": normal_teardown_p95_ms,
                "owner_crash_samples_ms": crash_reap_ms,
                "owner_crash_p95_ms": crash_reap_p95_ms,
            },
            "runtime_cleanup": {
                "socket_gone": socket_gone,
                "runtime_dir_gone": runtime_gone,
                "process_tree_empty": tree_empty,
            },
        },
        "thresholds": thresholds,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "os_image": command_stdout("uname", &["-a"]),
            "toolchain": command_stdout("rustc", &["-Vv"]),
            "cpu": command_stdout("sysctl", &["-n", "machdep.cpu.brand_string"])
                .or_else(|| command_stdout("lscpu", &[])),
            "ram": command_stdout("sysctl", &["-n", "hw.memsize"])
                .or_else(|| command_stdout("free", &["-b"])),
            "process_sampler": "ps",
            "fd_evidence": baseline.fd_evidence,
        },
    });
    if let Some(parent) = std::path::Path::new(&receipt_path).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut receipt = receipt;
    let zeroed = zero_abi::canonical_json(&receipt);
    receipt["receipt_sha256"] = json!(zero_abi::sha256_hex(zeroed.as_bytes()));
    let sealed_digest = receipt["receipt_sha256"].as_str().unwrap().to_string();
    let mut digest_check = receipt.clone();
    digest_check["receipt_sha256"] = json!("0".repeat(64));
    assert_eq!(
        zero_abi::sha256_hex(zero_abi::canonical_json(&digest_check).as_bytes()),
        sealed_digest,
        "lifecycle receipt self-digest convention must verify"
    );
    let canonical_bytes = zero_abi::canonical_json(&receipt).into_bytes();
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)
        .unwrap_or_else(|error| panic!("create immutable receipt {receipt_path}: {error}"));
    output
        .write_all(&canonical_bytes)
        .and_then(|()| output.sync_all())
        .unwrap_or_else(|error| panic!("write receipt {receipt_path}: {error}"));
    println!(
        "zerostack lifecycle receipt canonical={canonical} pass={} written to {receipt_path}",
        receipt["pass"]
    );
    if canonical {
        assert!(
            receipt["pass"] == true,
            "q6am lifecycle gate failed: receipt at {receipt_path} records pass=false; inspect the thresholds array"
        );
    } else {
        assert_eq!(
            receipt["canonical"], false,
            "short noncanonical run must be labeled noncanonical"
        );
        assert_eq!(
            receipt["pass"], false,
            "a noncanonical short run must never emit a passing canonical receipt"
        );
    }
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
    assert_eq!(
        inline_public(&response["result"])["value"]["args"]["command"],
        "printf zsx"
    );

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
fn unknown_authenticated_request_is_typed_and_connection_survives() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    send(
        &mut stream,
        json!({"type":"future_control","id":700,"payload":{"x":1}}),
    );
    let rejected = read(&mut reader);
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert_eq!(rejected["id"], 700);
    assert_eq!(rejected["generation"], generation);
    assert_eq!(rejected["code"], "unknown_request_type");
    assert!(rejected["error"]
        .as_str()
        .unwrap()
        .contains("request rejected"));

    send(
        &mut stream,
        json!({
            "type":"execute","id":701,"generation":generation,
            "root":d.path(),"source":"return 701;"
        }),
    );
    let current = read(&mut reader);
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["result"], 701);
    send(
        &mut stream,
        json!({"type":"shutdown","id":702,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn authenticated_idle_connection_survives_and_shutdown_interrupts_it() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut idle_stream, mut idle_reader, generation) = connect_authenticated(&socket, &token);
    let (mut control, mut control_reader, control_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(control_generation, generation);

    thread::sleep(Duration::from_millis(600));
    send(
        &mut idle_stream,
        json!({
            "type":"execute","id":750,"generation":generation,
            "root":d.path(),"source":"return 750;","timeout_ms":1000
        }),
    );
    let settlement = read(&mut idle_reader);
    assert_eq!(settlement["ok"], true, "{settlement}");
    assert_eq!(settlement["result"], 750);

    let started = Instant::now();
    send(
        &mut control,
        json!({"type":"shutdown","id":751,"token":shutdown_token}),
    );
    assert_eq!(read(&mut control_reader)["ok"], true);
    assert!(session.wait().unwrap().success());
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "normal shutdown took {:?}",
        started.elapsed()
    );
}

#[test]
fn plan_failure_is_typed_and_connection_survives() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":800,
            "generation":generation,
            "root":d.path(),
            "source":"return (",
            "timeout_ms":1000
        }),
    );
    let failure = read(&mut reader);
    assert_eq!(failure["ok"], false, "{failure}");
    assert_eq!(failure["id"], 800);
    assert_eq!(failure["code"], "backend_execution");

    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":801,
            "generation":generation,
            "root":d.path(),
            "source":"return 801;",
            "timeout_ms":1000
        }),
    );
    let settlement = read(&mut reader);
    assert_eq!(settlement["ok"], true, "{settlement}");
    assert_eq!(settlement["result"], 801);
    send(
        &mut stream,
        json!({"type":"shutdown","id":802,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn catalog_failures_preserve_canonical_codes_and_connection_continuity() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    for (id, source, code, suggestion) in [
        (
            820,
            "return await zero.token.missing({});",
            "method_not_found",
            "closest methods:",
        ),
        (
            821,
            "return await zero.missing.read({});",
            "surface_not_found",
            "closest surfaces:",
        ),
    ] {
        send(
            &mut stream,
            json!({
                "type":"execute",
                "id":id,
                "generation":generation,
                "root":d.path(),
                "source":source,
                "timeout_ms":1000
            }),
        );
        let failure = read(&mut reader);
        assert_eq!(failure["ok"], false, "{failure}");
        assert_eq!(failure["id"], id, "{failure}");
        assert_eq!(failure["code"], code, "{failure}");
        assert!(
            failure["error"]
                .as_str()
                .is_some_and(|message| message.contains(suggestion)),
            "{failure}"
        );
    }
    send(
        &mut stream,
        json!({
            "type":"execute","id":822,"generation":generation,
            "root":d.path(),"source":"return 822;","timeout_ms":1000
        }),
    );
    let settlement = read(&mut reader);
    assert_eq!(settlement["ok"], true, "{settlement}");
    assert_eq!(settlement["result"], 822);
    send(
        &mut stream,
        json!({"type":"shutdown","id":823,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn aggregate_sidecar_spills_arbitrary_oversize_results_to_the_authorized_store() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":850,
            "generation":generation,
            "root":d.path(),
            "source":"const payload=Array.from({length:4096},(_,i)=>i%256);return {contract:{payload},env:{status:'ok'}};",
            "timeout_ms":1000
        }),
    );
    let settlement = read(&mut reader);
    assert_eq!(settlement["ok"], true, "{settlement}");
    let result = &settlement["result"];
    assert_eq!(result["spilled"], true, "{settlement}");
    assert!(serde_json::to_vec(result).unwrap().len() <= 2_000);
    assert_eq!(
        result["receipt"]["rawResultJsonBytes"],
        result["receipt"]["omittedBehindExactRefBytes"]
    );
    let sha = result["sha256"].as_str().unwrap();
    let resolved = ResolvedStore::resolve_from_process(d.path(), Engine::TokenZero, &[]);
    ensure_layout(&resolved).unwrap();
    let stored = SharedCas::open(resolved.cas_host())
        .get_verified(sha)
        .unwrap();
    let exact: Value = serde_json::from_slice(&stored).unwrap();
    assert_eq!(exact["contract"]["payload"].as_array().unwrap().len(), 4096);

    send(
        &mut stream,
        json!({"type":"shutdown","id":851,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}

#[test]
fn malformed_and_oversize_frames_return_typed_errors() {
    let (d, mut session, token, shutdown_token, _) = start(ProcessIdentity::current().unwrap());
    let socket = d.path().join("runtime/session.sock");
    let (mut stream, mut reader, generation) = connect_authenticated(&socket, &token);
    stream.write_all(b"not-json\n").unwrap();
    stream.flush().unwrap();
    let malformed = read(&mut reader);
    assert_eq!(malformed["ok"], false, "{malformed}");
    assert_eq!(malformed["code"], "invalid_frame");

    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":900,
            "generation":generation,
            "root":d.path(),
            "source":"return 900;",
            "timeout_ms":1000
        }),
    );
    let current = read(&mut reader);
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["result"], 900);

    let (mut oversized_stream, mut oversized_reader, oversized_generation) =
        connect_authenticated(&socket, &token);
    assert_eq!(oversized_generation, generation);
    let oversized = vec![b'x'; zero_codemode::session::MAX_SESSION_FRAME + 2];
    oversized_stream.write_all(&oversized).unwrap();
    oversized_stream.flush().unwrap();
    let rejection = read(&mut oversized_reader);
    assert_eq!(rejection["ok"], false, "{rejection}");
    assert_eq!(rejection["code"], "oversized_frame");

    send(
        &mut stream,
        json!({"type":"shutdown","id":901,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}
