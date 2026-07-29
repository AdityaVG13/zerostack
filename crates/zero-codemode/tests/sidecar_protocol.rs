#![cfg(feature = "quickjs")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Sidecar {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Sidecar {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_zerostack-codemode-host"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn sidecar");
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut sidecar = Self {
            child,
            input,
            output,
        };
        let ready = sidecar.read();
        assert_eq!(ready["protocol"], "zerostack-codemode-host/v1");
        sidecar
    }

    fn send(&mut self, value: Value) {
        serde_json::to_writer(&mut self.input, &value).expect("encode frame");
        self.input.write_all(b"\n").expect("newline");
        self.input.flush().expect("flush");
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).expect("read frame");
        assert!(!line.is_empty(), "sidecar closed unexpectedly");
        serde_json::from_str(&line).expect("JSON frame")
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    assert!(stdout.contains("zerostack-codemode-host/v1"));
}

#[test]
fn delegate_round_trip_and_sandbox_are_owned() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-1",
        "source":"const value=await __zero.host.call({name:'echo',input:{x:1}});return {value,process:typeof process,require:typeof require,buffer:typeof Buffer};"
    }));
    let delegate = sidecar.read();
    assert_eq!(delegate["type"], "delegate_request");
    assert_eq!(delegate["payload"]["name"], "echo");
    sidecar.send(json!({
        "type":"delegate_response",
        "delegate_id":delegate["delegate_id"],
        "ok":true,
        "result":{"echoed":true}
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    assert_eq!(
        result["contentItems"][0]["text"],
        r#"{"buffer":"undefined","process":"undefined","require":"undefined","value":{"echoed":true}}"#
    );
}

#[test]
fn yielded_cell_can_be_cancelled_while_delegate_is_pending() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-2",
        "yield_ms":1,
        "source":"return await __zero.host.call({name:'slow',input:{}});"
    }));
    let first = sidecar.read();
    let second = sidecar.read();
    let frames = [first, second];
    assert!(frames
        .iter()
        .any(|frame| frame["type"] == "delegate_request"));
    assert!(frames.iter().any(|frame| frame["kind"] == "yielded"));

    sidecar.send(json!({
        "type":"execute",
        "id":3,
        "cell_id":"cell-3",
        "source":"return 3;"
    }));
    let capacity = sidecar.read();
    assert_eq!(capacity["ok"], false);
    assert_eq!(capacity["error"], "cell capacity exhausted");

    let started = Instant::now();
    sidecar.send(json!({"type":"terminate","id":2,"cell_id":"cell-2"}));
    let terminated = sidecar.read();
    assert_eq!(terminated["kind"], "terminated");
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn missing_cells_fail_closed_without_replay() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({"type":"wait","id":9,"cell_id":"missing","yield_ms":1}));
    let response = sidecar.read();
    assert_eq!(response["kind"], "missing");
    assert_eq!(response["missingCell"], true);
}
