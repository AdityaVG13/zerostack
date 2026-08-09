#![cfg(all(unix, feature = "worker-fixture"))]
#![forbid(unsafe_code)]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::Path,
    process::{Command, Stdio},
};
use tempfile::TempDir;
use zerostack_machine_permit::session_owner::ProcessIdentity;

const ZERO_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn write_worker_wrapper(path: &Path) {
    let script = format!(
        r#"#!/bin/sh
set -eu
if [ "${{1:-}}" = "capabilities" ] && [ "${{2:-}}" = "--json" ]; then
  [ "${{ZEROSTACK_RAW_WORKER_PROTOCOL:-}}" = "v2" ] || exit 64
  printf '%s\n' '{{"package":{{"abi_digest":"{ZERO_DIGEST}"}}}}'
  exit 0
fi
if [ "${{1:-}}" = "--help" ]; then
  [ "${{ZEROSTACK_RAW_WORKER_PROTOCOL:-}}" = "v2" ] || exit 64
  printf '%s\n' 'semantic_contract_digest: {ZERO_DIGEST}'
  exit 0
fi
if [ "${{1:-}}" = "raw-worker" ] && [ "${{2:-}}" = "--handshake" ]; then
  if [ -n "${{ZEROSTACK_RAW_WORKER_PROTOCOL+x}}" ]; then
    exit 0
  fi
  printf '%s\n' '{{"semantic_contract_digest":"{ZERO_DIGEST}"}}'
  exit 0
fi
if [ "$(basename "$0")" = "tokenzero" ] && [ "${{ZEROSTACK_RAW_WORKER_PROTOCOL:-}}" != "v2" ]; then
  printf '%s\n' 'TokenZero serve mode lost ZEROSTACK_RAW_WORKER_PROTOCOL=v2' >&2
  exit 64
fi
exec "$ZEROSTACK_TEST_FIXTURE_BIN" normal
"#,
    );
    fs::write(path, script).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn send(stream: &mut UnixStream, value: Value) {
    serde_json::to_writer(&mut *stream, &value).unwrap();
    stream.write_all(b"\n").unwrap();
}

fn read(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    serde_json::from_str(&line).unwrap()
}

#[test]
fn tokenzero_probe_drops_serve_selector_but_worker_launch_keeps_it() {
    let directory = TempDir::new().unwrap();
    let runtime = directory.path().join("runtime");
    let wrappers = directory.path().join("workers");
    fs::create_dir(&wrappers).unwrap();
    let fs_worker = wrappers.join("fs-worker");
    let graph_worker = wrappers.join("gz-raw-worker");
    let graph_probe = wrappers.join("graphzero-codemode");
    let token_worker = wrappers.join("tokenzero");
    for path in [&fs_worker, &graph_worker, &graph_probe, &token_worker] {
        write_worker_wrapper(path);
    }

    let token = "a".repeat(64);
    let shutdown_token = "b".repeat(64);
    let mut session = Command::new(env!("CARGO_BIN_EXE_zerostack-session"))
        .args([
            "serve",
            "--root",
            directory.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &ProcessIdentity::current().unwrap().encode(),
        ])
        .env("ZEROSTACK_SESSION_TOKEN", &token)
        .env("ZEROSTACK_SESSION_SHUTDOWN_TOKEN", &shutdown_token)
        .env("ZEROSTACK_SESSION_ROOT", directory.path())
        .env("ZEROSTACK_RAW_WORKER_PROTOCOL", "v2")
        .env(
            "ZEROSTACK_TEST_FIXTURE_BIN",
            env!("CARGO_BIN_EXE_zero-codemode-worker-fixture"),
        )
        .env("ZERO_FSZERO_RAW_BIN", &fs_worker)
        .env("ZERO_GRAPHZERO_RAW_BIN", &graph_worker)
        .env("ZERO_TOKENZERO_RAW_BIN", &token_worker)
        .env_remove("ZEROSTACK_TEST_MODE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut ready = String::new();
    let read_bytes = BufReader::new(session.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if read_bytes == 0 {
        let mut stderr = String::new();
        session
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        panic!(
            "session exited before ready: status={:?} stderr={stderr:?}",
            session.try_wait().unwrap()
        );
    }
    let ready: Value = serde_json::from_str(&ready).unwrap();
    assert_eq!(ready["type"], "ready");
    let generation = ready["generation"].as_u64().unwrap();

    let socket = runtime.join("session.sock");
    let mut stream = UnixStream::connect(socket).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":"zerostack-session/v1","token":token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":1,
            "generation":generation,
            "root":directory.path(),
            "source":"return await zero.token.find('needle');",
        }),
    );
    let response = read(&mut reader);
    assert_eq!(response["ok"], true, "{response}");
    let result = &response["result"]["content"]["value"];
    assert_eq!(response["result"]["content"]["kind"], "inline");
    assert_eq!(result["value"]["args"]["query"], "needle");
    assert_eq!(result["metadata"]["ownership"]["engine"], "tokenzero");

    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    assert!(session.wait().unwrap().success());
}
