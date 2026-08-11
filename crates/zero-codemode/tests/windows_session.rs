#![cfg(all(windows, feature = "worker-fixture"))]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zero_codemode::session::{
    MAX_SESSION_FRAME, SESSION_PROTOCOL, SESSION_SHUTDOWN_TOKEN_ENV, SESSION_TOKEN_ENV,
};
use zero_process::{PipeConnection, ProcessIdentity, VerifiedChild};

fn pipe_name(runtime: &std::path::Path) -> String {
    format!(
        r"\\.\pipe\{}",
        runtime.file_name().unwrap().to_str().unwrap()
    )
}

fn send(stream: &mut PipeConnection, value: Value) {
    let mut frame = serde_json::to_vec(&value).unwrap();
    assert!(frame.len() < MAX_SESSION_FRAME);
    frame.push(b'\n');
    stream.write_all(&frame).unwrap();
    stream.flush().unwrap();
}

fn read(reader: &mut BufReader<PipeConnection>) -> Value {
    let mut frame = Vec::new();
    let count = reader.read_until(b'\n', &mut frame).unwrap();
    assert!(count > 0 && frame.len() <= MAX_SESSION_FRAME);
    serde_json::from_slice(&frame).unwrap()
}

#[test]
fn named_pipe_protocol_auth_execution_shutdown_and_cleanup() {
    let directory = TempDir::new().unwrap();
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let runtime = directory.path().join(format!("zerostack-session-{nonce}"));
    let endpoint = pipe_name(&runtime);
    let token = "a".repeat(64);
    let shutdown_token = "b".repeat(64);
    let owner = ProcessIdentity::current().unwrap().encode();
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-session"));
    command
        .args([
            "serve",
            "--root",
            directory.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &owner,
        ])
        .env(SESSION_TOKEN_ENV, &token)
        .env(SESSION_SHUTDOWN_TOKEN_ENV, &shutdown_token)
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
        .env(
            "ZEROSTACK_DESCENDANT_PID_FILE",
            directory.path().join("worker-descendant.pid"),
        )
        .env("ZEROSTACK_TOKENZERO_RAW_ARGS", "spawn-descendant-normal")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let (sidecar, pipes) =
        VerifiedChild::spawn_tree_with_pipes(command, "windows-test", 0).unwrap();
    let mut ready_reader = BufReader::new(pipes.stdout.unwrap());
    let mut ready_frame = String::new();
    ready_reader.read_line(&mut ready_frame).unwrap();
    let ready: Value = serde_json::from_str(&ready_frame).unwrap();
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["protocol"], SESSION_PROTOCOL);
    let generation = ready["generation"]
        .as_u64()
        .filter(|value| *value != 0)
        .unwrap();

    let mut rejected = PipeConnection::connect(&endpoint).unwrap();
    rejected.set_read_timeout(Some(Duration::from_secs(3)));
    let mut rejected_reader = BufReader::new(rejected.try_clone().unwrap());
    send(
        &mut rejected,
        json!({"type":"hello","protocol":SESSION_PROTOCOL,"token":"wrong"}),
    );
    assert_eq!(read(&mut rejected_reader)["ok"], false);

    let mut stream = PipeConnection::connect(&endpoint).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10)));
    stream.set_write_timeout(Some(Duration::from_secs(2)));
    assert!(stream.peer_is_current_user().unwrap());
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":SESSION_PROTOCOL,"token":token}),
    );
    let hello = read(&mut reader);
    assert_eq!(hello["ok"], true, "{hello}");
    assert_eq!(hello["generation"], generation);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":1,
            "generation":generation,
            "root":directory.path(),
            "source":"return await zero.token.shell('echo native')",
            "timeout_ms":5_000
        }),
    );
    let result = read(&mut reader);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["id"], 1);
    assert_eq!(result["generation"], generation);
    let descendant_pid = wait_for_pid(&directory.path().join("worker-descendant.pid"));
    let descendant = ProcessIdentity::capture(descendant_pid).unwrap();

    let started = Instant::now();
    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    let shutdown = read(&mut reader);
    assert_eq!(shutdown["ok"], true, "{shutdown}");
    assert!(sidecar.wait_for_exit(Duration::from_secs(1)));
    sidecar.revoke().unwrap();
    assert!(!descendant.is_live().unwrap_or(false));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "native session cleanup exceeded one second: {:?}",
        started.elapsed()
    );
    assert!(PipeConnection::connect(&endpoint).is_err());
}

#[test]
fn compat_server_binds_execute_response_to_session_protocol() {
    // The zsx client is in-process now; this keeps the retained
    // zerostack-session compat server's protocol binding covered over the
    // native named-pipe transport.
    let directory = TempDir::new().unwrap();
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let runtime = directory.path().join(format!("zerostack-session-{nonce}"));
    let endpoint = pipe_name(&runtime);
    let token = "a".repeat(64);
    let shutdown_token = "b".repeat(64);
    let owner = ProcessIdentity::current().unwrap().encode();
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-session"));
    command
        .args([
            "serve",
            "--root",
            directory.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &owner,
        ])
        .env(SESSION_TOKEN_ENV, &token)
        .env(SESSION_SHUTDOWN_TOKEN_ENV, &shutdown_token)
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
    let (sidecar, pipes) =
        VerifiedChild::spawn_tree_with_pipes(command, "windows-test", 0).unwrap();
    let mut ready_reader = BufReader::new(pipes.stdout.unwrap());
    let mut ready_frame = String::new();
    ready_reader.read_line(&mut ready_frame).unwrap();
    let ready: Value = serde_json::from_str(&ready_frame).unwrap();
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["protocol"], SESSION_PROTOCOL);
    let generation = ready["generation"]
        .as_u64()
        .filter(|value| *value != 0)
        .unwrap();

    let mut stream = PipeConnection::connect(&endpoint).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10)));
    stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":SESSION_PROTOCOL,"token":token}),
    );
    let hello = read(&mut reader);
    assert_eq!(hello["ok"], true, "{hello}");
    let started = Instant::now();
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":1,
            "generation":generation,
            "root":directory.path(),
            "source":"return await zero.token.shell('echo zsx-native')",
            "timeout_ms":5_000
        }),
    );
    let response = read(&mut reader);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "compat server execute exceeded ten seconds: {:?}",
        started.elapsed()
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["protocol"], SESSION_PROTOCOL);
    assert_eq!(response["generation"], generation);
    send(
        &mut stream,
        json!({"type":"shutdown","id":2,"token":shutdown_token}),
    );
    let shutdown = read(&mut reader);
    assert_eq!(shutdown["ok"], true, "{shutdown}");
    assert!(sidecar.wait_for_exit(Duration::from_secs(1)));
    sidecar.revoke().unwrap();
    assert!(PipeConnection::connect(&endpoint).is_err());
}

fn start_session_for_crash(
    owner: &ProcessIdentity,
) -> (TempDir, String, std::process::Child, u64, String) {
    let directory = TempDir::new().unwrap();
    let nonce = format!(
        "crash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let runtime = directory.path().join(format!("zerostack-session-{nonce}"));
    let endpoint = pipe_name(&runtime);
    let token = "c".repeat(64);
    let shutdown_token = "d".repeat(64);
    let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-session"));
    command
        .args([
            "serve",
            "--root",
            directory.path().to_str().unwrap(),
            "--runtime-dir",
            runtime.to_str().unwrap(),
            "--owner",
            &owner.encode(),
        ])
        .env(SESSION_TOKEN_ENV, &token)
        .env(SESSION_SHUTDOWN_TOKEN_ENV, shutdown_token)
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
        .env(
            "ZEROSTACK_DESCENDANT_PID_FILE",
            directory.path().join("worker-descendant.pid"),
        )
        .env("ZEROSTACK_TOKENZERO_RAW_ARGS", "spawn-descendant-normal")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut session = command.spawn().unwrap();
    let mut ready_reader = BufReader::new(session.stdout.take().unwrap());
    let mut ready_frame = String::new();
    ready_reader.read_line(&mut ready_frame).unwrap();
    let ready: Value = serde_json::from_str(&ready_frame).unwrap();
    assert_eq!(ready["type"], "ready", "{ready}");
    let generation = ready["generation"].as_u64().unwrap();
    (directory, endpoint, session, generation, token)
}

fn start_nested_worker(
    directory: &std::path::Path,
    endpoint: &str,
    generation: u64,
    token: &str,
) -> ProcessIdentity {
    let mut stream = PipeConnection::connect(endpoint).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    send(
        &mut stream,
        json!({"type":"hello","protocol":SESSION_PROTOCOL,"token":token}),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    send(
        &mut stream,
        json!({
            "type":"execute",
            "id":1,
            "generation":generation,
            "root":directory,
            "source":"return await zero.token.shell('echo crash-proof')",
            "timeout_ms":5_000
        }),
    );
    assert_eq!(read(&mut reader)["ok"], true);
    ProcessIdentity::capture(wait_for_pid(&directory.join("worker-descendant.pid"))).unwrap()
}

fn wait_for_session_tree_cleanup(
    session: &mut std::process::Child,
    endpoint: &str,
    descendant: &ProcessIdentity,
    started: Instant,
) -> Duration {
    while started.elapsed() < Duration::from_secs(1) {
        let session_gone = session.try_wait().unwrap().is_some();
        let descendant_gone = !descendant.is_live().unwrap_or(false);
        if session_gone && descendant_gone && PipeConnection::connect(endpoint).is_err() {
            return started.elapsed();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = session.kill();
    let _ = session.wait();
    panic!("session tree survived one-second cleanup bound");
}

#[test]
fn owner_and_sidecar_crash_reap_nested_workers_below_one_second_p95() {
    const RUNS: usize = 10;
    let mut owner_crash = Vec::with_capacity(RUNS);
    let mut sidecar_crash = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let mut owner = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .spawn()
            .unwrap();
        let owner_identity = ProcessIdentity::capture(owner.id()).unwrap();
        let (directory, endpoint, mut session, generation, token) =
            start_session_for_crash(&owner_identity);
        let descendant = start_nested_worker(directory.path(), &endpoint, generation, &token);
        let started = Instant::now();
        owner.kill().unwrap();
        owner.wait().unwrap();
        owner_crash.push(wait_for_session_tree_cleanup(
            &mut session,
            &endpoint,
            &descendant,
            started,
        ));

        let owner_identity = ProcessIdentity::current().unwrap();
        let (directory, endpoint, mut session, generation, token) =
            start_session_for_crash(&owner_identity);
        let descendant = start_nested_worker(directory.path(), &endpoint, generation, &token);
        let started = Instant::now();
        session.kill().unwrap();
        sidecar_crash.push(wait_for_session_tree_cleanup(
            &mut session,
            &endpoint,
            &descendant,
            started,
        ));
    }
    for (label, samples) in [
        ("owner crash", &mut owner_crash),
        ("sidecar crash", &mut sidecar_crash),
    ] {
        samples.sort_unstable();
        let p95 = samples[(samples.len() * 95).div_ceil(100) - 1];
        eprintln!("{label} lifecycle p95={p95:?} samples={}", samples.len());
        assert!(p95 < Duration::from_secs(1), "{label} p95 was {p95:?}");
    }
}

fn wait_for_pid(path: &std::path::Path) -> u32 {
    for _ in 0..100 {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("worker descendant pid was not published");
}
