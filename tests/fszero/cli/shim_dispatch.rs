//! Advertise == dispatch for shim tokens `layout` and `--raw-worker`.
//!
//! `SHIM_COMMANDS` / completions must not offer verbs or flags the packaging
//! shim then rejects as unknown.

use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn fszero() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fszero"));
    for key in [
        "FSZERO_PRIVATE_WORKER",
        "FSZERO_ALLOW_BARE_SERVER",
        "FSZERO_STARTUP_INDEX",
        "FSZERO_ROOT",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fszero-shim-dispatch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn layout_is_advertised_and_dispatches() {
    let caps = fszero()
        .args(["capabilities", "--json"])
        .output()
        .expect("fszero capabilities");
    assert!(
        caps.status.success(),
        "capabilities failed: {}",
        String::from_utf8_lossy(&caps.stderr)
    );
    let doc: Value = serde_json::from_slice(&caps.stdout).expect("capabilities JSON");
    let commands = doc["commands"]
        .as_array()
        .expect("capabilities.commands array");
    assert!(
        commands.iter().any(|c| c.as_str() == Some("layout")),
        "layout missing from advertised commands: {commands:?}"
    );

    let root = temp_root();
    let out = fszero()
        .args(["layout", "--json", "--root"])
        .arg(&root)
        .output()
        .expect("fszero layout");
    assert!(
        out.status.success(),
        "layout dispatch failed status={:?} stderr={} stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let layout: Value = serde_json::from_slice(&out.stdout).expect("layout JSON");
    assert_eq!(layout["schema"], "fszero.layout/v1");
    assert!(layout["paths"]["workspace_root"].as_str().is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn raw_worker_is_advertised_and_dispatches() {
    let script = fszero()
        .args(["completions", "bash"])
        .output()
        .expect("fszero completions bash");
    assert!(
        script.status.success(),
        "completions failed: {}",
        String::from_utf8_lossy(&script.stderr)
    );
    let body = String::from_utf8_lossy(&script.stdout);
    assert!(
        body.contains("--raw-worker"),
        "completions must advertise --raw-worker:\n{body}"
    );

    let root = temp_root();
    let mut child = fszero()
        .args(["--raw-worker", "--root"])
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fszero --raw-worker");
    drop(child.stdin.take());
    let out = wait_output(child, Duration::from_secs(8));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "raw-worker dispatch failed status={:?} stderr={stderr} stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !stderr.contains("unknown command"),
        "shim rejected advertised --raw-worker: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn wait_output(mut child: std::process::Child, limit: Duration) -> Output {
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let mut stderr = child.stderr.take().expect("stderr pipe");
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let status = child.wait().expect("reap killed child");
                panic!(
                    "fszero --raw-worker did not exit within {limit:?} after stdin EOF (status={status:?})"
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => panic!("wait fszero --raw-worker: {err}"),
        }
    };
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let _ = stdout.read_to_end(&mut stdout_buf);
    let _ = stderr.read_to_end(&mut stderr_buf);
    Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    }
}
