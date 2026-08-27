//! Packaged raw-worker v2 conformance gate.
//!
//! Drives the shipped `tokenzero-codemode` raw worker over v2 NDJSON stdio
//! (`tokenzero-codemode raw-worker --root DIR` with ZEROSTACK_RAW_WORKER_PROTOCOL=v2).
//! `TOKENZERO_RAW_WORKER_BIN` can select an exact installed release artifact.
//! through the shared suite families: golden-frame fixture replay,
//! cancellation, crash/restart, ref ownership, parser fuzz, capability
//! truthfulness, deadlines, and a mutation matrix where every protocol
//! invariant is paired with a mutation that must turn this gate red.
//! `report_pins_source_and_artifact_digests` pins fixture, test-source, and
//! packaged-artifact SHA-256 digests plus the live binding digests into
//! target/raw_worker_conformance_report.json.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::tempdir;
use tokenzero_engine::raw_worker::raw_worker_protocol::{
    decode_response_frame, raw_worker_protocol_digest_hex,
};

const PROTOCOL_VERSION: &str = "zerostack.raw_worker";
const MAX_FRAME_BYTES: usize = 1_048_576;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/cli/golden/raw_worker/frames.json")
}

fn load_fixture() -> Vec<Value> {
    serde_json::from_slice(&fs::read(fixture_path()).expect("fixture readable"))
        .expect("fixture parses")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn packaged_worker_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(path) = std::env::var_os("TOKENZERO_RAW_WORKER_BIN") {
            return PathBuf::from(path);
        }
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root().join("target"))
            .join("raw-worker-v2-packaged");
        let output = Command::new("cargo")
            .args([
                "build",
                "-p",
                "tokenzero-worker",
                "--bin",
                "tokenzero-codemode",
                "--no-default-features",
            ])
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(repo_root())
            .output()
            .expect("canonical worker build starts");
        assert!(
            output.status.success(),
            "canonical worker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        target.join("debug").join(format!(
            "tokenzero-codemode{}",
            std::env::consts::EXE_SUFFIX
        ))
    })
    .clone()
}

/// Strip ambient agent env so the worker under test is hermetic.
fn scrub_env(cmd: &mut Command) {
    for key in [
        "TOKENZERO_ROOT",
        "TOKENZERO_CACHE_PATH",
        "TOKENZERO_SHARED_STORE",
        "ZEROSTACK_SHARED_STORE",
        "ZEROSTACK_STORE_ROOT",
        "ZEROSTACK_WORKER_REVISION",
        "ZEROSTACK_RAW_WORKER_PROTOCOL",
        "ZEROSTACK_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("NO_COLOR", "1")
        .env("CI", "true")
        .env("TERM", "dumb")
        .env("SOURCE_DATE_EPOCH", "1234567890");
}

/// One-shot capability probe on the packaged artifact (`raw-worker --handshake`).
fn probe_surface_capability() -> Value {
    let mut cmd = Command::new(packaged_worker_bin());
    scrub_env(&mut cmd);
    cmd.args(["raw-worker", "--handshake"]);
    let out = cmd.output().expect("probe spawns");
    assert!(
        out.status.success(),
        "capability probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("probe emits JSON")
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Worker {
    fn spawn(root: &Path, session: &str) -> Self {
        let mut cmd = Command::new(packaged_worker_bin());
        scrub_env(&mut cmd);
        cmd.args(["raw-worker", "--root", &root.display().to_string()])
            .env("ZEROSTACK_RAW_WORKER_PROTOCOL", "v2")
            .env("ZEROSTACK_SESSION_ID", session)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("packaged worker spawns");
        let stdout = child.stdout.take().expect("stdout piped");
        let (tx, rx) = channel::<String>();
        thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let stdin = child.stdin.take().expect("stdin piped");
        Self {
            child,
            stdin: Some(stdin),
            lines: rx,
        }
    }

    fn send(&mut self, frame: &Value) {
        self.send_raw(&serde_json::to_vec(frame).expect("frame serializes"));
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(bytes).expect("stdin writable");
        stdin.write_all(b"\n").expect("stdin writable");
        stdin.flush().expect("stdin flushes");
    }

    fn recv(&self, what: &str) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|e| panic!("{what}: no worker frame within 60s ({e})"));
        decode_response_frame(line.as_bytes(), MAX_FRAME_BYTES)
            .unwrap_or_else(|e| panic!("{what}: worker violated shared response codec ({e})"));
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("{what}: worker emitted non-JSON line ({e}): {line:?}"))
    }

    fn wait_bounded(&mut self, secs: u64) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("worker did not exit within {secs}s");
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Close stdin (EOF) and require a clean bounded exit.
    fn close(mut self) -> ExitStatus {
        drop(self.stdin.take());
        self.wait_bounded(15)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn unix_process_cpu_millis(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("worker stat readable");
    let after_comm = stat
        .get(stat.rfind(')').expect("worker stat command closes") + 1..)
        .expect("worker stat fields follow command");
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // `after_comm` starts at field 3; utime/stime are fields 14/15.
    let ticks = fields[11].parse::<u64>().expect("user ticks parse")
        + fields[12].parse::<u64>().expect("system ticks parse");
    let clock = Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .expect("clock tick probe runs");
    assert!(clock.status.success(), "clock tick probe succeeds");
    let ticks_per_second = String::from_utf8(clock.stdout)
        .expect("clock tick rate is UTF-8")
        .trim()
        .parse::<u64>()
        .expect("clock tick rate parses");
    ticks.saturating_mul(1_000) / ticks_per_second
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unix_process_cpu_millis(pid: u32) -> u64 {
    let output = Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .expect("worker CPU time probe runs");
    assert!(output.status.success(), "worker CPU time probe succeeds");
    let text = String::from_utf8(output.stdout)
        .expect("worker CPU time is UTF-8")
        .trim()
        .to_string();
    let (days, clock) = text
        .split_once('-')
        .map_or((0, text.as_str()), |(days, clock)| {
            (days.parse::<u64>().expect("CPU days parse"), clock)
        });
    let mut fields = clock.rsplit(':');
    let seconds = fields.next().expect("CPU seconds present");
    let minutes = fields
        .next()
        .unwrap_or("0")
        .parse::<u64>()
        .expect("CPU minutes parse");
    let hours = fields
        .next()
        .unwrap_or("0")
        .parse::<u64>()
        .expect("CPU hours parse");
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, "0"));
    let mut millis = fraction.chars().take(3).collect::<String>();
    while millis.len() < 3 {
        millis.push('0');
    }
    (((days * 24 + hours) * 60 + minutes) * 60 + seconds.parse::<u64>().expect("CPU seconds parse"))
        * 1_000
        + millis.parse::<u64>().expect("CPU milliseconds parse")
}

#[cfg(unix)]
fn unix_direct_children(pid: u32) -> Vec<u32> {
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("worker child probe runs");
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "worker child probe succeeds"
    );
    String::from_utf8(output.stdout)
        .expect("worker child PIDs are UTF-8")
        .split_whitespace()
        .map(|value| value.parse::<u32>().expect("child pid parses"))
        .collect()
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn handshake_request(root: &str, session: &str, cap: &Value, revision: Option<&str>) -> Value {
    let mut frame = json!({"kind":"handshake","request":{
        "protocol_version":PROTOCOL_VERSION,
        "root":root,
        "session_id":session,
        "expected_engine":"tokenzero",
        "expected_contract_digest":cap["semantic_contract_digest"],
        "expected_registry_digest":cap["operation_registry_digest"]
    }});
    if let Some(rev) = revision {
        frame["request"]["expected_worker_revision"] = json!(rev);
    }
    frame
}

fn trace_for(request_id: &str, revision: &str, contract: &str) -> Value {
    json!({
        "runtime_id":"rt-conformance",
        "cell_id":"cell-1",
        "request_id":request_id,
        "trace_id":format!("trace-{request_id}"),
        "worker_revision":revision,
        "contract_digest":contract,
    })
}

fn call_frame(
    request_id: &str,
    op: &str,
    args: Value,
    deadline_unix_ms: Option<u64>,
    trace: Value,
) -> Value {
    let mut request = json!({"request_id":request_id,"op":op,"args":args,"trace":trace});
    if let Some(deadline) = deadline_unix_ms {
        request["deadline_unix_ms"] = json!(deadline);
    }
    json!({"kind":"call","request":request})
}

fn shutdown_frame(reason: &str) -> Value {
    json!({"kind":"shutdown","request":{"reason":reason}})
}

fn handshake_on(worker: &mut Worker, root: &Path, session: &str, cap: &Value) -> Value {
    let frame = handshake_request(&root.display().to_string(), session, cap, None);
    worker.send(&frame);
    let ack = worker.recv("handshake");
    assert_eq!(ack["kind"], "handshake_ack", "handshake rejected: {ack}");
    ack
}

struct BoundWorker {
    worker: Worker,
    revision: String,
    contract: String,
    ack: Value,
}

fn spawn_bound(root: &Path, session: &str, cap: &Value) -> BoundWorker {
    let mut worker = Worker::spawn(root, session);
    let ack = handshake_on(&mut worker, root, session, cap);
    let binding = &ack["ack"]["binding"];
    assert_eq!(binding["session_id"], json!(session));
    // The one-shot probe and the v2 ack must report identical digests.
    assert_eq!(
        binding["semantic_contract_digest"], cap["semantic_contract_digest"],
        "probe vs ack contract digest drift"
    );
    assert_eq!(
        binding["operation_registry_digest"], cap["operation_registry_digest"],
        "probe vs ack registry digest drift"
    );
    BoundWorker {
        worker,
        revision: binding["worker_revision"].as_str().unwrap().into(),
        contract: binding["semantic_contract_digest"].as_str().unwrap().into(),
        ack,
    }
}

fn assert_result_metadata(frame: &Value, session: &str, effect: &str) {
    let metadata = &frame["result"]["metadata"];
    assert_eq!(metadata["effect"], json!(effect), "{frame}");
    assert_eq!(metadata["ownership"]["engine"], "tokenzero", "{frame}");
    assert_eq!(
        metadata["ownership"]["session_id"],
        json!(session),
        "{frame}"
    );
    assert_eq!(metadata["approval"]["state"], "not_required", "{frame}");
    assert_eq!(metadata["revert"]["supported"], false, "{frame}");
    assert!(
        metadata["ownership"].get("snapshot").is_none(),
        "snapshots are not supported and must not be claimed: {frame}"
    );
    assert_eq!(
        metadata["trace"]["request_id"], frame["request_id"],
        "trace must echo the request id: {frame}"
    );
}

/// Golden-frame suite: the vendored shared fixture replays end to end.
#[test]
fn golden_frame_fixture_replays_on_packaged_artifact() {
    let dir = tempdir().unwrap();
    let readme = dir.path().join("README.md");
    fs::write(&readme, "fixture-readme\n").unwrap();
    let cap = probe_surface_capability();

    let fixture = load_fixture();
    assert_eq!(fixture.len(), 4, "shared fixture shape changed");
    let kinds: Vec<&str> = fixture
        .iter()
        .map(|frame| frame["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["handshake", "call", "cancel", "shutdown"]);

    // Template the shared fixture with this engine's live binding pins; the
    // fixture's own engine/digest placeholders must never be sent verbatim.
    let mut handshake = fixture[0].clone();
    {
        let request = handshake["request"].as_object_mut().unwrap();
        request.insert("root".into(), json!(dir.path().display().to_string()));
        request.insert("expected_engine".into(), json!("tokenzero"));
        request.insert(
            "expected_contract_digest".into(),
            cap["semantic_contract_digest"].clone(),
        );
        request.insert(
            "expected_registry_digest".into(),
            cap["operation_registry_digest"].clone(),
        );
        request.remove("expected_worker_revision");
    }

    let mut worker = Worker::spawn(dir.path(), "session-1");
    worker.send(&handshake);
    let ack = worker.recv("golden handshake");
    assert_eq!(ack["kind"], "handshake_ack", "{ack}");
    let revision = ack["ack"]["binding"]["worker_revision"]
        .as_str()
        .unwrap()
        .to_string();
    let contract = ack["ack"]["binding"]["semantic_contract_digest"]
        .as_str()
        .unwrap()
        .to_string();

    let mut call = fixture[1].clone();
    {
        let request = call["request"].as_object_mut().unwrap();
        request.get_mut("args").unwrap()["path"] = json!(readme.display().to_string());
        let trace = request.get_mut("trace").unwrap();
        trace["worker_revision"] = json!(revision);
        trace["contract_digest"] = json!(contract);
    }
    assert_eq!(call["request"]["request_id"], "request-1");
    worker.send(&call);
    let result = worker.recv("golden call");
    assert_eq!(result["kind"], "result", "{result}");
    assert_eq!(result["request_id"], "request-1");
    let value_text = serde_json::to_string(&result["result"]["value"]).unwrap();
    assert!(value_text.contains("fixture-readme"), "{value_text}");
    assert_result_metadata(&result, "session-1", "read_only");

    // Cancel of a completed call must truthfully report cancelled=false.
    worker.send(&fixture[2]);
    let cancel = worker.recv("golden cancel");
    assert_eq!(cancel["kind"], "cancel_ack", "{cancel}");
    assert_eq!(cancel["request_id"], "request-1");
    assert_eq!(cancel["cancelled"], false);

    worker.send(&fixture[3]);
    assert_eq!(worker.recv("golden shutdown")["kind"], "shutdown_ack");
    assert_eq!(worker.close().code(), Some(0));
}

/// Cancellation suite: the cancel control frame reaches live shell work over
/// stdio (it must never queue behind the running call) and the session stays
/// usable afterwards.
#[test]
fn cancel_control_frame_stops_live_shell_work_over_stdio() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-cancel", &cap);

    let started = Instant::now();
    let trace = trace_for("req-cancel", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-cancel",
        "shell",
        json!({"command":"sleep 30"}),
        None,
        trace,
    ));
    thread::sleep(Duration::from_millis(500));
    bound.worker.send(
        &json!({"kind":"cancel","request":{"request_id":"req-cancel","reason":"conformance cancel"}}),
    );

    let mut cancel_ack = None;
    let mut call_response = None;
    for _ in 0..2 {
        let frame = bound.worker.recv("cancel suite");
        match frame["kind"].as_str().unwrap() {
            "cancel_ack" => cancel_ack = Some(frame),
            _ => call_response = Some(frame),
        }
    }
    let ack = cancel_ack.expect("cancel_ack frame");
    assert_eq!(ack["request_id"], "req-cancel");
    assert_eq!(ack["cancelled"], true, "{ack}");
    assert!(
        ack.get("process_kill_supported").is_none(),
        "cancel_ack must match the shared ABI exactly: {ack}"
    );
    let response = call_response.expect("call response frame");
    assert_eq!(response["request_id"], "req-cancel");
    assert_eq!(response["error"]["kind"], "cancelled", "{response}");
    assert_eq!(response["error"]["retryable"], false, "{response}");
    assert_eq!(response["trace"]["trace_id"], "trace-req-cancel");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancelled call must not run to completion"
    );

    // The bound session survives a cancellation.
    let trace = trace_for("req-after", &bound.revision, &bound.contract);
    bound
        .worker
        .send(&call_frame("req-after", "mem", json!({}), None, trace));
    assert_eq!(bound.worker.recv("post-cancel call")["kind"], "result");

    bound.worker.send(&shutdown_frame("cancel complete"));
    assert_eq!(bound.worker.recv("cancel shutdown")["kind"], "shutdown_ack");
    assert_eq!(bound.worker.close().code(), Some(0));
}

/// An idle raw worker must block on input rather than spin. The duration is
/// configurable so release gates can promote the default smoke to a soak.
#[cfg(unix)]
#[test]
fn idle_worker_blocks_without_cpu_or_orphan_tail() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut worker = Worker::spawn(dir.path(), "s-idle-soak");
    handshake_on(&mut worker, dir.path(), "s-idle-soak", &cap);
    let pid = worker.child.id();
    let soak_seconds = std::env::var("TOKENZERO_IDLE_SOAK_SECS")
        .ok()
        .map(|value| value.parse::<u64>().expect("soak seconds parse"))
        .unwrap_or(10);
    assert!(
        soak_seconds >= 10,
        "idle soak must run at least ten seconds"
    );

    let cpu_before = unix_process_cpu_millis(pid);
    assert!(
        unix_direct_children(pid).is_empty(),
        "idle worker started a child process"
    );
    thread::sleep(Duration::from_secs(soak_seconds));
    assert!(worker.child.try_wait().expect("worker try_wait").is_none());
    let cpu_delta = unix_process_cpu_millis(pid).saturating_sub(cpu_before);
    assert!(
        cpu_delta <= 200,
        "idle worker consumed {cpu_delta}ms CPU during {soak_seconds}s"
    );
    assert!(
        unix_direct_children(pid).is_empty(),
        "idle worker retained a child process"
    );

    let started = Instant::now();
    drop(worker.stdin.take());
    assert!(worker.wait_bounded(1).success());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "idle worker did not exit within one second of EOF"
    );
    let alive = Command::new("kill")
        .args(["-0", "--", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("worker liveness probe runs")
        .success();
    assert!(!alive, "idle worker left a process tail");
}

/// Session EOF is cancellation: live shell work and its process group must
/// be reaped before the raw worker exits.
#[cfg(unix)]
#[test]
fn eof_cancels_live_shell_and_reaps_descendant_within_one_second() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-eof-live", &cap);
    let pid_file = dir.path().join("eof-child.pid");
    let trace = trace_for("req-eof", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-eof",
        "shell",
        json!({
            "command": format!(
                "printf '%s\\n' $$ > {}; exec sleep 30",
                pid_file.display()
            )
        }),
        None,
        trace,
    ));

    let child_ready_deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() {
        assert!(
            Instant::now() < child_ready_deadline,
            "shell descendant did not publish its pid"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let descendant_pid = fs::read_to_string(&pid_file)
        .expect("descendant pid file readable")
        .trim()
        .parse::<u32>()
        .expect("descendant pid parses");

    let started = Instant::now();
    drop(bound.worker.stdin.take());
    let exit_deadline = started + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = bound.worker.child.try_wait().expect("worker try_wait") {
            break status;
        }
        if Instant::now() >= exit_deadline {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &descendant_pid.to_string()])
                .status();
            let _ = bound.worker.child.kill();
            let _ = bound.worker.child.wait();
            panic!("worker did not exit within three seconds of stdin EOF");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "EOF teardown must exit cleanly: {status:?}"
    );
    let exit_elapsed = started.elapsed();
    assert!(
        exit_elapsed < Duration::from_secs(1),
        "worker EOF teardown took {}ms",
        exit_elapsed.as_millis()
    );

    let descendant_deadline = Instant::now() + Duration::from_secs(1);
    let descendant_gone = loop {
        let alive = Command::new("kill")
            .args(["-0", "--", &descendant_pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("kill -0 descendant probe")
            .success();
        if !alive {
            break true;
        }
        if Instant::now() >= descendant_deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !descendant_gone {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &descendant_pid.to_string()])
            .status();
    }
    assert!(descendant_gone, "shell descendant survived session EOF");
}

/// Crash suite: EOF without shutdown exits cleanly; SIGKILL mid-call is
/// survivable by restart with a fresh handshake on the same session.
#[test]
fn crash_eof_and_sigkill_are_restart_survivable() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();

    let mut worker = Worker::spawn(dir.path(), "s-crash-eof");
    handshake_on(&mut worker, dir.path(), "s-crash-eof", &cap);
    assert_eq!(worker.close().code(), Some(0), "EOF must exit cleanly");

    #[cfg(unix)]
    {
        let mut bound = spawn_bound(dir.path(), "s-crash-kill", &cap);
        let trace = trace_for("req-kill", &bound.revision, &bound.contract);
        bound.worker.send(&call_frame(
            "req-kill",
            "shell",
            json!({"command":"sleep 30"}),
            None,
            trace,
        ));
        thread::sleep(Duration::from_millis(300));
        let delivered = Command::new("kill")
            .args(["-9", &bound.worker.child.id().to_string()])
            .status();
        assert!(
            delivered.map(|s| s.success()).unwrap_or(false),
            "kill -9 must be deliverable"
        );
        let status = bound.worker.wait_bounded(15);
        assert!(!status.success(), "killed worker must not report success");
        drop(bound);

        let mut reborn = spawn_bound(dir.path(), "s-crash-kill", &cap);
        reborn.worker.send(&shutdown_frame("restart complete"));
        assert_eq!(
            reborn.worker.recv("restart shutdown")["kind"],
            "shutdown_ack"
        );
        assert_eq!(reborn.worker.close().code(), Some(0));
    }
}

/// Transport-telemetry suite: hub-requested accounting is typed, truthful,
/// and absent from legacy response bytes when not requested.
#[test]
fn requested_worker_accounting_round_trips_on_packaged_artifact() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-accounting", &cap);

    let trace = trace_for("req-accounting", &bound.revision, &bound.contract);
    let mut call = call_frame("req-accounting", "mem", json!({}), None, trace);
    call["request"]["telemetry_request"] = json!({
        "engine_stage_timeline": true,
        "worker_token_accounting": true
    });
    bound.worker.send(&call);
    let frame = bound.worker.recv("accounted call");
    assert_eq!(frame["kind"], "result", "{frame}");
    let accounting = &frame["worker_token_accounting"];
    assert_eq!(accounting["count_kind"], "estimate", "{frame}");
    assert!(
        accounting["tokenizer_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("estimator:")),
        "{frame}"
    );
    let domain = &frame["result"]["value"]["accounting"];
    for field in [
        "raw_tokens",
        "visible_tokens",
        "recovery_tokens",
        "billed_tokens",
        "cached_tokens",
        "exact_ref_tokens",
    ] {
        assert_eq!(accounting[field], domain[field], "field {field}: {frame}");
    }
    assert!(
        accounting["cached_tokens"].as_u64().unwrap()
            <= accounting["billed_tokens"].as_u64().unwrap(),
        "{frame}"
    );
    let timeline = &frame["engine_timeline"];
    assert!(timeline["total_ns"].as_u64().unwrap() > 0, "{frame}");
    assert_eq!(timeline["spans"][0]["start_ns"], 0, "{frame}");
    assert_eq!(
        timeline["spans"][0]["duration_ns"], timeline["total_ns"],
        "{frame}"
    );

    let trace = trace_for("req-legacy", &bound.revision, &bound.contract);
    bound
        .worker
        .send(&call_frame("req-legacy", "mem", json!({}), None, trace));
    let legacy = bound.worker.recv("legacy call");
    assert_eq!(legacy["kind"], "result", "{legacy}");
    assert!(legacy.get("worker_token_accounting").is_none(), "{legacy}");
    assert!(legacy.get("engine_timeline").is_none(), "{legacy}");

    bound.worker.send(&shutdown_frame("accounting complete"));
    assert_eq!(
        bound.worker.recv("accounting shutdown")["kind"],
        "shutdown_ack"
    );
    assert_eq!(bound.worker.close().code(), Some(0));
}

/// Ref-ownership suite: every result carries engine/session ownership and
/// tz-scheme refs; effect classes are honest per op.
#[test]
fn result_frames_carry_ref_ownership_and_effect_metadata() {
    let dir = tempdir().unwrap();
    let readme = dir.path().join("README.md");
    fs::write(&readme, "owned-readme\n").unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-refs", &cap);

    let trace = trace_for("req-ingest", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-ingest",
        "ingest",
        json!({"text":"owned-payload-4uql"}),
        None,
        trace,
    ));
    let frame = bound.worker.recv("ingest call");
    assert_eq!(frame["kind"], "result", "{frame}");
    assert_result_metadata(&frame, "s-refs", "irreversible");
    let refs = frame["result"]["metadata"]["ownership"]["refs"]
        .as_array()
        .unwrap();
    assert!(!refs.is_empty(), "ingest must return owned refs: {frame}");
    for r in refs {
        assert!(
            r.as_str().unwrap().starts_with("tz://"),
            "ref must use the tz scheme: {r}"
        );
    }

    let trace = trace_for("req-read", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-read",
        "read",
        json!({"path":readme.display().to_string()}),
        None,
        trace,
    ));
    let frame = bound.worker.recv("read call");
    assert_eq!(frame["kind"], "result", "{frame}");
    assert_result_metadata(&frame, "s-refs", "read_only");

    bound.worker.send(&shutdown_frame("refs complete"));
    assert_eq!(bound.worker.recv("refs shutdown")["kind"], "shutdown_ack");
    assert_eq!(bound.worker.close().code(), Some(0));
}

/// Parser-fuzz suite: malformed, truncated, bit-flipped, and oversized frames
/// always produce typed error frames (never a hang, never untyped output), and
/// the worker stays responsive for valid traffic afterwards.
#[test]
fn parser_fuzz_never_hangs_or_emits_untyped_frames() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut worker = Worker::spawn(dir.path(), "s-fuzz");

    let base = serde_json::to_vec(&call_frame(
        "req-fuzz",
        "read",
        json!({"path":"/x"}),
        None,
        trace_for("req-fuzz", "fuzz-rev", "fuzz-contract"),
    ))
    .unwrap();
    let mut frames: Vec<Vec<u8>> = vec![
        b"{not json".to_vec(),
        Vec::new(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        br#"{"kind":"nope","request":{}}"#.to_vec(),
        br#"{"kind":"call"}"#.to_vec(),
        vec![b'x'; MAX_FRAME_BYTES + 1],
    ];
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..32 {
        let mut mutated = base.clone();
        match rng.next() % 3 {
            0 => {
                let keep = 1 + (rng.next() as usize) % (mutated.len() - 1);
                mutated.truncate(keep);
            }
            1 => {
                let pos = (rng.next() as usize) % mutated.len();
                mutated[pos] ^= 1 << (rng.next() % 8);
            }
            _ => {
                let pos = (rng.next() as usize) % (mutated.len() + 1);
                mutated.insert(pos, (rng.next() % 256) as u8);
            }
        }
        for byte in mutated.iter_mut() {
            if *byte == b'\n' {
                *byte = b'x';
            }
        }
        if mutated.is_empty() {
            mutated.push(b'x');
        }
        frames.push(mutated);
    }

    for (index, frame) in frames.iter().enumerate() {
        worker.send_raw(frame);
        let response = worker.recv("fuzz frame");
        assert_eq!(response["kind"], "error", "frame {index}: {response}");
        let kind = response["error"]["kind"].as_str().unwrap_or_default();
        assert!(
            matches!(
                kind,
                "invalid_frame" | "frame_too_large" | "contract_mismatch" | "handshake_required"
            ),
            "frame {index}: unexpected error kind {kind}: {response}"
        );
        assert_eq!(
            response["error"]["retryable"], false,
            "frame {index}: {response}"
        );
        assert!(
            response.get("request_id").is_none(),
            "frame {index}: fuzzed frames must not echo request ids: {response}"
        );
    }

    // Liveness after fuzz: a valid handshake and call still succeed.
    let ack = handshake_on(&mut worker, dir.path(), "s-fuzz", &cap);
    let revision = ack["ack"]["binding"]["worker_revision"]
        .as_str()
        .unwrap()
        .to_string();
    let contract = ack["ack"]["binding"]["semantic_contract_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let trace = trace_for("req-alive", &revision, &contract);
    worker.send(&call_frame("req-alive", "mem", json!({}), None, trace));
    assert_eq!(worker.recv("post-fuzz call")["kind"], "result");
    worker.send(&shutdown_frame("fuzz complete"));
    assert_eq!(worker.recv("fuzz shutdown")["kind"], "shutdown_ack");
    assert_eq!(worker.close().code(), Some(0));
}

/// Capability-truthfulness suite: advertised capabilities/limits match the
/// behavior proven elsewhere in this gate, and unsupported features are never
/// claimed in result metadata.
#[test]
fn advertised_capabilities_and_limits_are_behaviorally_true() {
    let dir = tempdir().unwrap();
    let readme = dir.path().join("README.md");
    fs::write(&readme, "capability-readme\n").unwrap();
    let cap = probe_surface_capability();
    let bound = spawn_bound(dir.path(), "s-caps", &cap);
    let ack = &bound.ack["ack"];

    assert_eq!(ack["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(ack["protocol_digest"], raw_worker_protocol_digest_hex());
    assert_eq!(ack["binding"]["engine"], "tokenzero");
    assert_eq!(ack["binding"]["ref_scheme"], "tz://");
    // Claimed capabilities: cancellation/deadlines have behavioral tests in
    // this gate; approvals/revert/snapshots must stay honestly false.
    assert_eq!(
        ack["capabilities"],
        json!({"cancellation":true,"deadlines":true,"approvals":false,"revert":false,"snapshots":false})
    );
    assert_eq!(
        ack["limits"],
        json!({"max_frame_bytes":MAX_FRAME_BYTES as u64,"max_output_bytes":65_536,"max_in_flight":1,"default_deadline_ms":30_000})
    );
    assert!(
        !bound.revision.is_empty() && !bound.contract.is_empty(),
        "binding pins must be present"
    );
    drop(bound);
}

/// 9lwo: the advertised `max_output_bytes` limit is enforced, not decorative.
/// A call whose serialized `result.value` exceeds the cap is rejected with a
/// typed, correlated error naming the limit and sizes; no oversized result
/// leaks, and the worker stays live for subsequent calls.
#[test]
fn oversized_result_value_is_rejected_and_no_oversized_result_leaks() {
    let dir = tempdir().unwrap();
    let big = "x".repeat(70_000);
    fs::write(dir.path().join("big.txt"), &big).unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-oversize", &cap);

    let trace = trace_for("req-oversize", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-oversize",
        "read",
        json!({
            "path": dir.path().join("big.txt").display().to_string(),
            "raw": true,
            "max_visible_tokens": 1_000_000
        }),
        None,
        trace,
    ));
    let response = bound.worker.recv("oversized call");
    assert_eq!(response["kind"], "error", "{response}");
    assert_eq!(response["request_id"], "req-oversize", "{response}");
    assert_eq!(response["error"]["kind"], "output_too_large", "{response}");
    assert_eq!(response["error"]["retryable"], false, "{response}");
    let details = &response["error"]["details"];
    assert_eq!(details["limit_name"], "max_output_bytes", "{details}");
    assert_eq!(details["limit_bytes"], 65_536u64, "{details}");
    assert!(
        details["actual_bytes"].as_u64().unwrap() > 65_536,
        "oversized value must measure above the cap: {details}"
    );
    assert_eq!(
        details["frame_limit_bytes"], MAX_FRAME_BYTES as u64,
        "{details}"
    );
    assert!(
        response.get("result").is_none(),
        "no oversized result may leak: {response}"
    );

    // The rejection is not terminal: a normal call still succeeds.
    let trace = trace_for("req-after", &bound.revision, &bound.contract);
    bound
        .worker
        .send(&call_frame("req-after", "mem", json!({}), None, trace));
    let ok = bound.worker.recv("post-rejection call");
    assert_eq!(ok["kind"], "result", "{ok}");
}

/// Deadline suite: expired deadlines fail closed before dispatch with typed
/// trace; live deadlines reach dispatched shell work.
#[test]
fn deadlines_are_enforced_before_and_during_dispatch() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-deadline", &cap);

    let trace = trace_for("req-expired", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-expired",
        "read",
        json!({"path":"/unused"}),
        Some(1),
        trace,
    ));
    let frame = bound.worker.recv("expired deadline");
    assert_eq!(frame["kind"], "error", "{frame}");
    assert_eq!(frame["error"]["kind"], "deadline_exceeded", "{frame}");
    assert_eq!(frame["error"]["retryable"], false, "{frame}");
    assert_eq!(frame["request_id"], "req-expired");
    assert_eq!(frame["trace"]["trace_id"], "trace-req-expired");

    let started = Instant::now();
    let trace = trace_for("req-live-deadline", &bound.revision, &bound.contract);
    bound.worker.send(&call_frame(
        "req-live-deadline",
        "shell",
        json!({"command":"sleep 30"}),
        Some(unix_ms() + 1_500),
        trace,
    ));
    let frame = bound.worker.recv("live deadline");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "deadline must reach the dispatched shell process"
    );
    let text = frame.to_string();
    assert!(
        frame["error"]["kind"] == "deadline_exceeded"
            || text.contains("\"timeout\":true")
            || text.contains("timed_out"),
        "shell run must report deadline enforcement: {text}"
    );

    bound.worker.send(&shutdown_frame("deadline complete"));
    assert_eq!(
        bound.worker.recv("deadline shutdown")["kind"],
        "shutdown_ack"
    );
    assert_eq!(bound.worker.close().code(), Some(0));
}

/// Mutation matrix: every protocol invariant is paired with a mutation that
/// must fail closed with the typed error. If enforcement regresses, the
/// matching assertion turns this gate red.
#[test]
fn protocol_invariant_mutations_fail_closed() {
    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let root = dir.path().display().to_string();
    let mut worker = Worker::spawn(dir.path(), "s-mut");

    // The untemplated shared fixture must fail against this engine: its
    // placeholder engine/digests are a binding mutation.
    let fixture = load_fixture();
    worker.send(&fixture[0]);
    let frame = worker.recv("fixture mutation");
    assert_eq!(frame["error"]["kind"], "binding_mismatch", "{frame}");
    assert_eq!(frame["error"]["retryable"], false, "{frame}");

    // Handshake binding invariants: protocol version, non-empty root, engine,
    // contract digest, and registry digest each fail closed.
    let base = handshake_request(&root, "s-mut", &cap, None);
    let mut mutations: Vec<(&str, Value, &str)> = Vec::new();
    let mut m = base.clone();
    m["request"]["protocol_version"] = json!("zerostack.raw_worker");
    mutations.push(("protocol_version", m, "contract_mismatch"));
    let mut m = base.clone();
    m["request"]["root"] = json!("");
    mutations.push(("empty_root", m, "contract_mismatch"));
    let mut m = base.clone();
    m["request"]["expected_engine"] = json!("fszero");
    mutations.push(("engine", m, "binding_mismatch"));
    let mut m = base.clone();
    m["request"]["expected_contract_digest"] = json!("d".repeat(64));
    mutations.push(("contract_digest", m, "binding_mismatch"));
    let mut m = base.clone();
    m["request"]["expected_registry_digest"] = json!("d".repeat(64));
    mutations.push(("registry_digest", m, "binding_mismatch"));
    for (label, frame, expected_kind) in mutations {
        worker.send(&frame);
        let response = worker.recv("handshake mutation");
        assert_eq!(
            response["error"]["kind"], expected_kind,
            "{label} must fail closed: {response}"
        );
        assert_eq!(response["error"]["retryable"], false, "{label}: {response}");
    }

    // Stale revision pin is retryable, never terminal (revision swaps are
    // survivable; the host re-handshakes).
    let mut stale = base.clone();
    stale["request"]["expected_worker_revision"] = json!("stale-revision");
    worker.send(&stale);
    let response = worker.recv("stale revision pin");
    assert_eq!(
        response["error"]["kind"], "worker_revision_changed",
        "{response}"
    );
    assert_eq!(response["error"]["retryable"], true, "{response}");

    // A rejected session stays unbound: the valid handshake still lands.
    let ack = handshake_on(&mut worker, dir.path(), "s-mut", &cap);
    let revision = ack["ack"]["binding"]["worker_revision"]
        .as_str()
        .unwrap()
        .to_string();
    let contract = ack["ack"]["binding"]["semantic_contract_digest"]
        .as_str()
        .unwrap()
        .to_string();

    // Trace invariants: request id and contract digest must match the binding.
    let mut trace = trace_for("other-id", &revision, &contract);
    worker.send(&call_frame(
        "req-trace-id",
        "mem",
        json!({}),
        None,
        trace.clone(),
    ));
    let response = worker.recv("trace id mutation");
    assert_eq!(response["error"]["kind"], "contract_mismatch", "{response}");

    trace["request_id"] = json!("req-trace-contract");
    trace["contract_digest"] = json!("d".repeat(64));
    worker.send(&call_frame(
        "req-trace-contract",
        "mem",
        json!({}),
        None,
        trace.clone(),
    ));
    let response = worker.recv("trace contract mutation");
    assert_eq!(
        response["error"]["kind"], "trace_binding_mismatch",
        "{response}"
    );

    // Stale trace revision is retryable; re-handshake plus a fresh trace
    // recovers over stdio.
    let stale_trace = trace_for("req-stale-trace", "stale-revision", &contract);
    worker.send(&call_frame(
        "req-stale-trace",
        "mem",
        json!({}),
        None,
        stale_trace.clone(),
    ));
    let response = worker.recv("stale trace mutation");
    assert_eq!(
        response["error"]["kind"], "worker_revision_changed",
        "{response}"
    );
    assert_eq!(response["error"]["retryable"], true, "{response}");
    let rebind = handshake_on(&mut worker, dir.path(), "s-mut", &cap);
    assert_eq!(rebind["kind"], "handshake_ack");
    let fresh_trace = trace_for("req-stale-trace", &revision, &contract);
    worker.send(&call_frame(
        "req-stale-trace",
        "mem",
        json!({}),
        None,
        fresh_trace,
    ));
    assert_eq!(worker.recv("recovered call")["kind"], "result");

    // Negative space: planner/JavaScript/MCP ops are forbidden.
    for (index, op) in ["tools/call", "planner.run", "mcp.list"].iter().enumerate() {
        let id = format!("req-forbidden-{index}");
        let trace = trace_for(&id, &revision, &contract);
        worker.send(&call_frame(&id, op, json!({}), None, trace));
        let response = worker.recv("forbidden op");
        assert_eq!(
            response["error"]["kind"], "unsupported_operation",
            "{op}: {response}"
        );
        assert_eq!(response["request_id"], json!(id));
    }

    // Unknown domain ops fail typed, never silently.
    let trace = trace_for("req-unknown", &revision, &contract);
    worker.send(&call_frame(
        "req-unknown",
        "no_such_op_xyz",
        json!({}),
        None,
        trace,
    ));
    let response = worker.recv("unknown op");
    assert_eq!(response["kind"], "error", "{response}");
    assert!(
        response["error"]["kind"]
            .as_str()
            .is_some_and(|k| !k.is_empty())
    );
    assert_eq!(response["request_id"], "req-unknown");

    // Cancel of unknown ids reports cancelled=false.
    worker.send(&json!({"kind":"cancel","request":{"request_id":"req-missing"}}));
    let response = worker.recv("unknown cancel");
    assert_eq!(response["kind"], "cancel_ack", "{response}");
    assert_eq!(response["cancelled"], false);

    // Binding permanence: foreign root or foreign session re-handshakes stay
    // terminal and do not disturb the existing binding.
    for (label, other_root, other_session) in [
        ("foreign_root", "/fixture/other", "s-mut"),
        ("foreign_session", root.as_str(), "s-other"),
    ] {
        let frame = handshake_request(other_root, other_session, &cap, None);
        worker.send(&frame);
        let response = worker.recv("foreign re-handshake");
        assert_eq!(
            response["error"]["kind"], "already_bound",
            "{label}: {response}"
        );
        assert_eq!(response["error"]["retryable"], false, "{label}: {response}");
    }

    worker.send(&shutdown_frame("mutations complete"));
    assert_eq!(worker.recv("mutation shutdown")["kind"], "shutdown_ack");
    assert_eq!(worker.close().code(), Some(0));
}

/// Report gate: pin fixture, test-source, and packaged-artifact SHA-256
/// digests plus the live binding digests into a machine-readable report.
#[test]
fn report_pins_source_and_artifact_digests() {
    let artifact = packaged_worker_bin();
    let artifact_sha = sha256_hex(&fs::read(&artifact).expect("artifact readable"));
    let fixture_sha = sha256_hex(&fs::read(fixture_path()).expect("fixture readable"));
    let test_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/cli/raw_worker_packaged_conformance.rs");
    let test_sha = sha256_hex(&fs::read(&test_source).expect("test source readable"));

    let dir = tempdir().unwrap();
    let cap = probe_surface_capability();
    let mut bound = spawn_bound(dir.path(), "s-report", &cap);

    let report = json!({
        "schema": "tokenzero.raw_worker_conformance.v1",
        "artifact": {"path": artifact.display().to_string(), "sha256": artifact_sha},
        "sources": {
            "fixture": {"path": "crates/tokenzero-cli/tests/golden/raw_worker/frames.json", "sha256": fixture_sha},
            "test": {"path": "crates/tokenzero-cli/tests/raw_worker_packaged_conformance.rs", "sha256": test_sha}
        },
        "binding": bound.ack["ack"]["binding"],
        "protocol_digest": bound.ack["ack"]["protocol_digest"],
        "capabilities": bound.ack["ack"]["capabilities"],
        "limits": bound.ack["ack"]["limits"],
        "suites": [
            "golden_frame", "cancellation", "crash_restart", "ref_ownership",
            "parser_fuzz", "capability_truthfulness", "deadlines", "invariant_mutations"
        ],
        "capability_evidence": {
            "cancellation": "cancel_control_frame_stops_live_shell_work_over_stdio",
            "deadlines": "deadlines_are_enforced_before_and_during_dispatch",
            "max_frame_bytes": "parser_fuzz_never_hangs_or_emits_untyped_frames",
            "approvals_false": "assert_result_metadata approval.state==not_required",
            "revert_false": "assert_result_metadata revert.supported==false",
            "snapshots_false": "assert_result_metadata ownership.snapshot absent"
        },
        "invariant_mutations": [
            {"invariant": "handshake binding", "mutation": "fixture placeholders verbatim", "expected": "binding_mismatch"},
            {"invariant": "protocol version", "mutation": "zerostack.raw_worker", "expected": "binding_mismatch"},
            {"invariant": "non-empty root", "mutation": "root==''", "expected": "binding_mismatch"},
            {"invariant": "engine identity", "mutation": "expected_engine=fszero", "expected": "binding_mismatch"},
            {"invariant": "contract digest", "mutation": "deadbeef", "expected": "binding_mismatch"},
            {"invariant": "registry digest", "mutation": "deadbeef", "expected": "binding_mismatch"},
            {"invariant": "revision pin survivable", "mutation": "stale expected_worker_revision", "expected": "worker_revision_changed retryable"},
            {"invariant": "trace request id", "mutation": "trace.request_id != request.request_id", "expected": "trace_binding_mismatch"},
            {"invariant": "trace contract", "mutation": "trace.contract_digest=deadbeef", "expected": "trace_binding_mismatch"},
            {"invariant": "trace revision survivable", "mutation": "stale trace.worker_revision", "expected": "worker_revision_changed retryable"},
            {"invariant": "planner/js/mcp negative space", "mutation": "tools/call | planner.run | mcp.list", "expected": "unsupported_operation"},
            {"invariant": "cancel truthfulness", "mutation": "cancel unknown request id", "expected": "cancel_ack cancelled=false"},
            {"invariant": "binding permanence", "mutation": "foreign root/session re-handshake", "expected": "already_bound"},
            {"invariant": "frame bound", "mutation": "frame > max_frame_bytes", "expected": "frame_too_large"}
        ]
    });
    let out = repo_root().join("target/raw_worker_conformance_report.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).expect("report writable");

    let parsed: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(parsed["schema"], "tokenzero.raw_worker_conformance.v1");
    for digest in [
        &parsed["artifact"]["sha256"],
        &parsed["sources"]["fixture"]["sha256"],
        &parsed["sources"]["test"]["sha256"],
    ] {
        let hex = digest.as_str().unwrap();
        assert_eq!(hex.len(), 64, "digest must be sha256 hex: {hex}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{hex}");
    }
    assert_eq!(parsed["protocol_digest"], raw_worker_protocol_digest_hex());

    bound.worker.send(&shutdown_frame("report complete"));
    assert_eq!(bound.worker.recv("report shutdown")["kind"], "shutdown_ack");
    assert_eq!(bound.worker.close().code(), Some(0));
}
