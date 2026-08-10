//! Fixture-backed protocol tests for the direct-aggregate v2 host.
//!
//! Every test authorizes an explicit session root and pins all three raw
//! worker binaries to the conformance fixture, then speaks the v2 NDJSON
//! protocol over stdin/stdout. v2 executes plans directly on the aggregate
//! session, so the transport carries no delegate frames at all.

#![cfg(all(unix, feature = "worker-fixture"))]
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use zero_codemode::session::MAX_SESSION_FRAME;

const FIXTURE: &str = env!("CARGO_BIN_EXE_zero-codemode-worker-fixture");

struct Sidecar {
    _dir: TempDir,
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
    generation: u64,
}

impl Sidecar {
    fn spawn() -> Self {
        Self::spawn_with(|_| {})
    }

    fn spawn_with<F>(configure: F) -> Self
    where
        F: FnOnce(&mut Command),
    {
        let dir = TempDir::new().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_zerostack-codemode-host"));
        command
            .env("ZEROSTACK_SESSION_ROOT", dir.path())
            .env("ZEROSTACK_TEST_MODE", "1")
            .env("ZERO_FSZERO_RAW_BIN", FIXTURE)
            .env("ZERO_GRAPHZERO_RAW_BIN", FIXTURE)
            .env("ZERO_TOKENZERO_RAW_BIN", FIXTURE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        configure(&mut command);
        let mut child = command.spawn().expect("spawn host");
        let input = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (output_sender, output) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.expect("read frame");
                let value = serde_json::from_str(&line).expect("JSON frame");
                if output_sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut sidecar = Self {
            _dir: dir,
            child,
            input: Some(input),
            output,
            reader: Some(reader),
            generation: 0,
        };
        let ready = sidecar.read();
        assert_eq!(ready["protocol"], "zerostack-codemode-host/v2");
        assert_eq!(ready["version"], 2);
        let generation = ready["generation"].as_u64().expect("ready generation");
        assert_ne!(generation, 0, "ready generation must be nonzero");
        sidecar.generation = generation;
        sidecar
    }

    fn send(&mut self, value: Value) {
        let input = self.input.as_mut().expect("stdin open");
        serde_json::to_writer(&mut *input, &value).expect("encode frame");
        input.write_all(b"\n").expect("newline");
        input.flush().expect("flush");
    }

    fn read(&mut self) -> Value {
        self.output
            .recv_timeout(Duration::from_secs(5))
            .expect("host frame within five seconds")
    }

    fn close_input(&mut self) {
        self.input.take();
    }

    fn wait(&mut self) -> ExitStatus {
        self.child.wait().expect("host exit")
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn release_artifact_reports_owned_protocol_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_zerostack-codemode-host"))
        .arg("--version")
        .output()
        .expect("version smoke");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 version");
    assert!(stdout.contains("zerostack-codemode-host"));
    assert!(stdout.contains("zerostack-codemode-host/v2"));
}

#[test]
fn ready_v2_reports_authorized_session_generation() {
    let mut sidecar = Sidecar::spawn();
    assert_ne!(sidecar.generation, 0);
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-ready",
        "generation":sidecar.generation,
        "source":"return 1;"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    assert_eq!(result["generation"], sidecar.generation);
}

#[test]
fn direct_aggregate_execution_emits_no_delegate_frames() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-direct",
        "generation":sidecar.generation,
        "source": r#"
            const a = await zero.fs.compound('read', {path:'.'});
            const b = await zero.graph.index();
            const c = await zero.token.shell('echo direct');
            return {
                fs:a.content.value.value.args.path,
                graph:b.content.kind,
                token:c.content.value.value.args.command
            };
        "#
    }));
    // The first frame after execute is the result itself: the aggregate
    // session executed the plan across all three raw workers inside the host,
    // so no request/response exchange can precede it on this transport.
    let result = sidecar.read();
    assert_eq!(result["type"], "response");
    assert_eq!(result["kind"], "result");
    assert_eq!(result["generation"], sidecar.generation);
    assert!(result.get("delegate_id").is_none());
    let visible: Value = serde_json::from_str(
        result["contentItems"][0]["text"]
            .as_str()
            .expect("visible JSON"),
    )
    .expect("parse visible result");
    assert_eq!(visible["fs"], ".");
    assert_eq!(visible["graph"], "inline");
    assert_eq!(visible["token"], "echo direct");
}

#[test]
fn yielded_cell_resumes_and_settles_on_later_wait() {
    let mut sidecar = Sidecar::spawn_with(|command| {
        command.env("ZEROSTACK_FSZERO_RAW_ARGS", "sleep");
    });
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-slow",
        "generation":sidecar.generation,
        "yield_ms":1,
        "source":"return await zero.fs.compound('read', {path:'.'});"
    }));
    let yielded = sidecar.read();
    assert_eq!(yielded["kind"], "yielded");
    assert_eq!(yielded["id"], 1);
    assert_eq!(yielded["cellId"], "cell-slow");
    assert_eq!(yielded["generation"], sidecar.generation);
    let started = Instant::now();
    sidecar.send(json!({"type":"wait","id":2,"cell_id":"cell-slow","yield_ms":10_000}));
    let result = sidecar.read();
    assert_eq!(result["id"], 2);
    assert_eq!(result["kind"], "result");
    assert!(
        started.elapsed() >= Duration::from_millis(1_500),
        "sleep worker must keep the cell pending until the wait settles"
    );
    let visible: Value = serde_json::from_str(
        result["contentItems"][0]["text"]
            .as_str()
            .expect("visible JSON"),
    )
    .expect("parse visible result");
    assert_eq!(visible["content"]["value"]["value"]["args"]["path"], ".");
}

#[test]
fn terminate_cancels_in_flight_and_reuse_is_healthy() {
    let mut sidecar = Sidecar::spawn_with(|command| {
        command.env("ZEROSTACK_FSZERO_RAW_ARGS", "hold");
    });
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-held",
        "generation":sidecar.generation,
        "source":"return await zero.fs.compound('read', {path:'.'});"
    }));
    let started = Instant::now();
    sidecar.send(json!({"type":"terminate","id":2,"cell_id":"cell-held"}));
    let terminated = sidecar.read();
    assert_eq!(terminated["kind"], "terminated");
    assert_eq!(terminated["id"], 2);
    assert_eq!(terminated["cellId"], "cell-held");
    let generation = terminated["generation"]
        .as_u64()
        .expect("terminated generation");
    assert_ne!(
        generation, sidecar.generation,
        "terminate must roll the session generation forward"
    );
    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "terminate must cancel the held backend promptly"
    );
    // Healthy reuse: the replaced session executes a fresh plan with the new
    // generation and a worker that is not held.
    sidecar.send(json!({
        "type":"execute",
        "id":3,
        "cell_id":"cell-next",
        "generation":generation,
        "source":"return await zero.token.shell('echo reuse');"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    assert_eq!(result["generation"], generation);
    let visible: Value = serde_json::from_str(
        result["contentItems"][0]["text"]
            .as_str()
            .expect("visible JSON"),
    )
    .expect("parse visible result");
    assert_eq!(
        visible["content"]["value"]["value"]["args"]["command"],
        "echo reuse"
    );
}

#[test]
fn model_visible_error_text_is_bounded() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-error-budget",
        "generation":sidecar.generation,
        "source":"throw new Error('x'.repeat(10000));"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    let error = result["errorText"].as_str().unwrap();
    assert!(error.len() <= 1_024, "{}", error.len());
    assert!(!error.contains(&"x".repeat(2_000)));
}

#[test]
fn oversized_input_frame_is_rejected_and_reader_resynchronizes() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"oversized",
        "generation":sidecar.generation,
        "source":"x".repeat(MAX_SESSION_FRAME + 128)
    }));
    let rejection = sidecar.read();
    assert_eq!(rejection["type"], "protocol_error");
    assert!(
        rejection["error"]
            .as_str()
            .is_some_and(|error| error.contains("input frame exceeded")),
        "{rejection}"
    );

    sidecar.send(json!({
        "type":"execute",
        "id":2,
        "cell_id":"after-oversized",
        "generation":sidecar.generation,
        "source":"return 1;"
    }));
    let result = sidecar.read();
    assert_eq!(result["id"], 2, "{result}");
    assert_eq!(result["kind"], "result", "{result}");
}

#[test]
fn result_frame_reports_wall_clock_duration() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-duration",
        "generation":sidecar.generation,
        "source":"return 1;"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    assert!(
        result["durationMs"].is_u64(),
        "expected durationMs in {result}"
    );
}

#[test]
fn missing_cells_fail_closed_without_replay() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({"type":"wait","id":9,"cell_id":"missing","yield_ms":1}));
    let response = sidecar.read();
    assert_eq!(response["kind"], "missing");
    assert_eq!(response["missingCell"], true);
    assert_eq!(response["generation"], sidecar.generation);
    sidecar.send(json!({"type":"terminate","id":10,"cell_id":"missing"}));
    let terminated = sidecar.read();
    assert_eq!(terminated["kind"], "missing");
    assert_eq!(terminated["missingCell"], true);
    assert_eq!(terminated["generation"], sidecar.generation);
}

#[test]
fn shutdown_responds_then_exits_cleanly() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({"type":"shutdown","id":7}));
    let response = sidecar.read();
    assert_eq!(response["ok"], true);
    assert_eq!(response["id"], 7);
    let status = sidecar.wait();
    assert!(status.success(), "host must exit cleanly after shutdown");
}

#[test]
fn stdin_eof_shuts_down_explicitly() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-eof",
        "generation":sidecar.generation,
        "source":"return 1;"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    sidecar.close_input();
    let status = sidecar.wait();
    assert!(status.success(), "host must exit cleanly on stdin EOF");
}
