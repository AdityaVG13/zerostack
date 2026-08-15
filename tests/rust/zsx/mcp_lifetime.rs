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
    assert!(
        source.contains("exit_if_detached_launch"),
        "detached-launch refusal must stay wired"
    );
    assert!(
        source.contains("lifetime") && source.contains("harness-stdio"),
        "zero_wait must advertise harness-stdio lifetime"
    );
}

#[test]
fn plugin_launch_path_is_zsx_mcp_not_python_or_engine() {
    let home = std::env::var("HOME").unwrap_or_default();
    let plugin = std::path::PathBuf::from(home).join(".grok/plugins/zerostack");
    if !plugin.is_dir() {
        return;
    }

    let mcp_json = std::fs::read_to_string(plugin.join(".mcp.json")).expect(".mcp.json");
    let parsed: serde_json::Value = serde_json::from_str(&mcp_json).expect("json");
    let server = &parsed["mcpServers"]["zerostack"];
    let command = server["command"].as_str().unwrap_or("");
    let args = server["args"].as_array().cloned().unwrap_or_default();
    assert!(
        command.ends_with("/zsx") || command.ends_with("/bin/zsx"),
        "plugin must exec the Mach-O, not Python: {command}"
    );
    assert_eq!(
        args,
        vec![serde_json::json!("mcp")],
        "plugin args must be [\"mcp\"], got {args:?}"
    );
    assert!(
        !command.contains("python") && !command.contains("zsx_mcp.py"),
        "plugin must not launch the Python wrap: {command}"
    );
    for forbidden in ["fszero", "graphzero", "tokenzero"] {
        assert!(
            parsed["mcpServers"].get(forbidden).is_none(),
            "engine MCP {forbidden} must not be registered"
        );
    }

    let hook = std::fs::read_to_string(plugin.join("hooks/ensure-live.sh")).expect("hook");
    for token in ["nohup", "setsid", "disown"] {
        assert!(!hook.contains(token), "ensure-live.sh must not {token}");
    }
    for line in hook.lines() {
        let trimmed = line.trim();
        let backgrounds = trimmed.ends_with(" &") || trimmed.ends_with("\t&");
        assert!(
            !(backgrounds && trimmed.contains("rebuild")),
            "ensure-live.sh must not background a rebuild: {trimmed}"
        );
    }

    let wrap = plugin.join("servers/zsx_mcp.py");
    if wrap.is_file() {
        let source = std::fs::read_to_string(&wrap).expect("wrap");
        assert!(!source.contains("subprocess"), "zsx_mcp.py must not spawn");
        assert!(
            source.contains("refuses") || source.contains("is not a server"),
            "zsx_mcp.py must fail closed"
        );
    }

    let skill =
        std::fs::read_to_string(plugin.join("skills/zerostack-codemode/SKILL.md")).expect("skill");
    assert!(
        skill.contains("Harnesses must not"),
        "skill must list what harnesses must not do"
    );
    for needle in ["Python", "LaunchAgent", "engine MCP", "pid 1"] {
        assert!(
            skill.contains(needle),
            "skill MUST NOT list missing {needle}"
        );
    }

    let rebuild = std::fs::read_to_string(plugin.join("scripts/rebuild.sh")).expect("rebuild");
    for token in ["nohup", "setsid", "disown"] {
        assert!(!rebuild.contains(token), "rebuild.sh must not {token}");
    }
    for line in rebuild.lines() {
        let trimmed = line.trim();
        assert!(
            !(trimmed.ends_with(" &") && trimmed.contains("cargo")),
            "rebuild.sh must stay in the foreground: {trimmed}"
        );
    }
    assert!(
        rebuild.contains("mv -f") && rebuild.contains(".zsx."),
        "rebuild.sh must install via same-dir rename, not truncate a mapped inode"
    );
    assert!(
        !rebuild.contains("cp -f") || rebuild.contains("mv -f"),
        "rebuild.sh must not cp -f onto the live bin path"
    );
    assert!(
        !rebuild.contains("exit 3"),
        "rebuild.sh must not refuse when a Grok session has bin/zsx mapped"
    );

    let hooks_json = std::fs::read_to_string(plugin.join("hooks/hooks.json")).expect("hooks");
    let hooks: serde_json::Value = serde_json::from_str(&hooks_json).expect("hooks json");
    assert!(
        hooks["hooks"].get("SessionStart").is_some(),
        "only SessionStart fingerprint is allowed"
    );
    assert!(
        hooks["hooks"].get("SessionEnd").is_none(),
        "no SessionEnd daemon teardown hook"
    );

    let plist_count = std::fs::read_dir(&plugin)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("plist"))
        .count();
    assert_eq!(plist_count, 0, "plugin must not ship a LaunchAgent plist");
}

fn mcp_output_with_env(pairs: &[(&str, &str)]) -> std::process::Output {
    let directory = TempDir::new().unwrap();
    let mut command = Command::new(zsx_bin());
    command
        .args(["mcp", "-C", directory.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in pairs {
        command.env(key, value);
    }
    command.output().expect("zsx mcp")
}

#[test]
fn mcp_cli_help_states_must_not() {
    let output = Command::new(zsx_bin())
        .arg("--help")
        .output()
        .expect("zsx --help");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    for needle in [
        "Not a sidecar",
        "Not a LaunchAgent",
        "Not a Python wrapper",
        "fszero/graphzero/tokenzero",
        "pid 1",
        "wrap zsx in Python",
        "truncate a mapped bin/zsx",
    ] {
        assert!(text.contains(needle), "help missing {needle:?}:\n{text}");
    }
}

#[test]
fn mcp_refuses_our_launchd_xpc_name() {
    let output = mcp_output_with_env(&[("XPC_SERVICE_NAME", "ai.zerostack.zsx")]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("LaunchAgent") || err.contains("XPC"),
        "stderr: {err}"
    );
}

#[test]
fn mcp_refuses_systemd_listen_pid() {
    let output = mcp_output_with_env(&[("LISTEN_PID", "1")]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("systemd"), "stderr: {err}");
}

#[test]
fn mcp_allows_unrelated_inherited_xpc_name() {
    // Grok / other GUI hosts inherit application.com.* XPC names. That is
    // not us becoming a LaunchAgent.
    let directory = TempDir::new().unwrap();
    let mut child = Command::new(zsx_bin())
        .args(["mcp", "-C", directory.path().to_str().unwrap()])
        .env("XPC_SERVICE_NAME", "application.com.xai.grok")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsx mcp");
    drop(child.stdin.take());
    let status = child.wait().expect("wait");
    assert!(
        status.success(),
        "inherited host XPC name must not refuse: {status:?}"
    );
}
