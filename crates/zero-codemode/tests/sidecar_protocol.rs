use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Sidecar {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
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
            child,
            input,
            output,
            reader: Some(reader),
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
        self.output
            .recv_timeout(Duration::from_secs(2))
            .expect("sidecar frame within two seconds")
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
    assert!(
        frames
            .iter()
            .any(|frame| frame["type"] == "delegate_request")
    );
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
fn model_visible_error_text_is_bounded() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-error-budget",
        "source":"throw new Error('x'.repeat(10000));"
    }));
    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    let error = result["errorText"].as_str().unwrap();
    assert!(error.len() <= 1_024, "{}", error.len());
    assert!(error.ends_with("... [truncated]"));
}

#[test]
fn result_frame_reports_wall_clock_duration() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-duration",
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
}

#[test]
fn every_delegate_in_a_multi_call_plan_carries_cell_provenance() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-multi",
        "source":"const a=await __zero.host.call({name:'shell',input:{command:'echo one'}});const b=await __zero.host.call({name:'shell',input:{command:'echo two'}});return [a,b];"
    }));

    let mut commands = Vec::new();
    for _ in 0..2 {
        let delegate = sidecar.read();
        assert_eq!(delegate["type"], "delegate_request");
        assert_eq!(delegate["cell_id"], "cell-multi");
        commands.push(
            delegate["payload"]["input"]["command"]
                .as_str()
                .expect("command string")
                .to_owned(),
        );
        sidecar.send(json!({
            "type":"delegate_response",
            "delegate_id":delegate["delegate_id"],
            "ok":true,
            "result":{"ok":true}
        }));
    }
    assert_eq!(commands, vec!["echo one".to_owned(), "echo two".to_owned()]);

    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
}

#[test]
fn promise_all_emits_every_delegate_before_any_response_and_settles_fifo() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-concurrent",
        "source":r#"const completionOrder = [];
            const calls = Array.from({length: 6}, (_, sequence) =>
              __zero.host.call({name:'shell',input:{sequence}}).then(value => {
                completionOrder.push(value.sequence);
                return value.sequence;
              }));
            const values = await Promise.all(calls);
            return {completionOrder, values};"#
    }));

    let mut delegates = Vec::new();
    for expected in 0..6 {
        let delegate = sidecar.read();
        assert_eq!(delegate["type"], "delegate_request");
        assert_eq!(delegate["cell_id"], "cell-concurrent");
        assert_eq!(delegate["payload"]["input"]["sequence"], expected);
        delegates.push(delegate);
    }
    for delegate in delegates {
        let sequence = delegate["payload"]["input"]["sequence"].clone();
        sidecar.send(json!({
            "type":"delegate_response",
            "delegate_id":delegate["delegate_id"],
            "ok":true,
            "result":{"sequence":sequence}
        }));
    }

    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
    let visible: Value = serde_json::from_str(
        result["contentItems"][0]["text"]
            .as_str()
            .expect("visible JSON"),
    )
    .expect("parse visible result");
    assert_eq!(
        visible,
        json!({
            "completionOrder": [0, 1, 2, 3, 4, 5],
            "values": [0, 1, 2, 3, 4, 5],
        })
    );
}

#[test]
fn mixed_capability_plan_keeps_cell_provenance_on_both_surfaces() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-mixed",
        "source":"const listed=await __zero.host.call({name:'fs',input:{operation:'list',path:'.'}});const shown=await __zero.host.call({name:'shell',input:{command:'echo mixed'}});return {listed,shown};"
    }));

    let mut names = Vec::new();
    for _ in 0..2 {
        let delegate = sidecar.read();
        assert_eq!(delegate["type"], "delegate_request");
        assert_eq!(delegate["cell_id"], "cell-mixed");
        names.push(
            delegate["payload"]["name"]
                .as_str()
                .expect("capability name")
                .to_owned(),
        );
        sidecar.send(json!({
            "type":"delegate_response",
            "delegate_id":delegate["delegate_id"],
            "ok":true,
            "result":{"ok":true}
        }));
    }
    assert_eq!(names, vec!["fs".to_owned(), "shell".to_owned()]);

    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
}

#[test]
fn dynamically_built_oversized_command_keeps_cell_provenance() {
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({
        "type":"execute",
        "id":1,
        "cell_id":"cell-concat",
        "source":"const tail='x'.repeat(900);const command='echo '+JSON.stringify('start-'+tail);return await __zero.host.call({name:'shell',input:{command}});"
    }));

    let delegate = sidecar.read();
    assert_eq!(delegate["type"], "delegate_request");
    assert_eq!(delegate["cell_id"], "cell-concat");
    let command = delegate["payload"]["input"]["command"]
        .as_str()
        .expect("command string");
    assert!(
        command.len() > 900,
        "command should exceed the reported inline threshold"
    );
    assert!(command.contains("start-xxx"));
    sidecar.send(json!({
        "type":"delegate_response",
        "delegate_id":delegate["delegate_id"],
        "ok":true,
        "result":{"ok":true}
    }));

    let result = sidecar.read();
    assert_eq!(result["kind"], "result");
}
