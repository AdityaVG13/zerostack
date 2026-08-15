//! `zsx mcp` is harness-owned: it must die when stdin closes or the parent
//! exits, and it must leave no child processes behind.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn zsx_bin() -> &'static str {
    env!("CARGO_BIN_EXE_zsx")
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn wait_dead(pid: u32, bound: Duration) {
    let started = Instant::now();
    while started.elapsed() < bound {
        if !pid_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    panic!("zsx mcp pid {pid} still alive after {bound:?}");
}

#[test]
fn mcp_exits_when_stdin_closes() {
    let directory = TempDir::new().unwrap();
    let mut child = Command::new(zsx_bin())
        .args(["mcp", "-C", directory.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsx mcp");
    let pid = child.id();
    drop(child.stdin.take());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            panic!("zsx mcp did not exit within 2s of stdin EOF");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "stdin EOF must exit 0, got {status:?} in {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_millis(750),
        "stdin EOF must be prompt, took {:?}",
        started.elapsed()
    );
    assert!(!pid_alive(pid), "pid {pid} must be gone after wait");
}

#[cfg(unix)]
#[test]
fn mcp_exits_when_parent_dies_even_if_stdin_stays_open() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().to_str().unwrap();
    let script = format!(
        r#""{bin}" mcp -C "{root}" &
echo $!
"#,
        bin = zsx_bin(),
    );
    let mut babysitter = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("babysitter");
    let keep_stdin = babysitter.stdin.take();
    let mut stdout = BufReader::new(babysitter.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).expect("zsx pid");
    let zsx_pid: u32 = line.trim().parse().expect("pid line");
    let status = babysitter.wait().expect("babysitter exit");
    assert!(status.success(), "babysitter: {status:?}");
    assert!(
        keep_stdin.is_some(),
        "test must keep the stdin write end open so this is not an EOF test"
    );
    wait_dead(zsx_pid, Duration::from_secs(2));
    drop(keep_stdin);
}

#[test]
fn mcp_source_never_daemonizes() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let banned = [
        "setsid",
        "nohup",
        "daemonize",
        "Command::spawn",
        "std::process::Command",
    ];
    let source = std::fs::read_to_string(manifest.join("src/mcp.rs")).unwrap();
    for token in banned {
        assert!(!source.contains(token), "mcp.rs must not contain {token:?}");
    }
    assert!(
        source.contains("install_parent_death_exit"),
        "parent-death exit must stay wired"
    );
}
