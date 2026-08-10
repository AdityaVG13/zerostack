#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
use zero_codemode::session::SessionExecutor;
use zero_store::{Engine, ResolvedStore, SharedCas, ensure_layout};
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
    assert!(
        invalid_shell["message"]
            .as_str()
            .unwrap()
            .contains("unknown option 'raw'")
    );
    assert!(
        invalid_shell["message"]
            .as_str()
            .unwrap()
            .contains(r#"mode: "exact""#)
    );
    send(
        &mut s,
        json!({"type":"execute","id":3,"generation":generation,"root":"/","source":"return 1"}),
    );
    let root_error = read(&mut r);
    assert_eq!(root_error["ok"], false);
    assert_eq!(root_error["code"], "authorized_root_mismatch");
    let detail = root_error["error"].as_str().unwrap();
    assert!(detail.contains(&format!("authorized root {:?}", d.path())));
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
    assert!(
        forwarded["request_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );

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
    assert!(
        unapproved["error"]
            .as_str()
            .unwrap()
            .contains("worker approval required or denied")
    );

    send(
        &mut stream,
        json!({"type":"shutdown","id":14,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    session.wait().unwrap();
}
#[test]
fn aggregate_promise_all_dispatches_slow_calls_in_parallel() {
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
            "source":r#"const calls = Array.from({length: 6}, (_, sequence) => {
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
        (0..6)
            .map(|sequence| Value::String(format!("tz://blob/{sequence:064}")))
            .collect(),
    );
    assert_eq!(exact, expected);
    assert!(
        elapsed < Duration::from_secs(4),
        "six two-second calls did not finish in one parallel wave: {elapsed:?}"
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
    assert_eq!(
        inline_public(&response["result"]["fs"])["metadata"]["ownership"]["engine"],
        "fszero"
    );
    assert_eq!(
        inline_public(&response["result"]["graph"])["metadata"]["ownership"]["engine"],
        "graphzero"
    );
    assert_eq!(
        inline_public(&response["result"]["token"])["metadata"]["ownership"]["engine"],
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
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("request rejected")
    );

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
