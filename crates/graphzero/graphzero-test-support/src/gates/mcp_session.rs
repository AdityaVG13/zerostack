//! Shared MCP stdio test session for integration tests.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

fn graphzero_bin() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_graphzero")
        .map(std::path::PathBuf::from)
        .expect("set CARGO_BIN_EXE_graphzero (run via `cargo test -p graphzero-cli`)")
}

pub fn graphzero() -> Command {
    Command::new(graphzero_bin())
}

pub fn run_cli(repo: &Path, args: &[&str]) -> Output {
    graphzero()
        .args(args)
        .arg(repo)
        .output()
        .expect("failed to execute GraphZero CLI process")
}

pub fn mcp_tool_json(tool: &str, arguments: Value) -> Value {
    let mut mcp = McpSession::start();
    mcp.handshake();
    let resp = mcp.request(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    }));
    mcp.shutdown();
    serde_json::from_str(&tool_text(&resp["result"]))
        .expect("MCP tool response text must contain valid JSON")
}

pub fn assert_cli_mcp_fields_match(
    repo: &Path,
    cli_args: &[&str],
    tool: &str,
    mcp_arguments: Value,
    fields: &[&str],
) {
    let cli_out = run_cli(repo, cli_args);
    assert!(cli_out.status.success(), "{:?}", cli_out.stderr);
    let cli_json: Value = serde_json::from_slice(&cli_out.stdout)
        .expect("GraphZero CLI stdout must contain valid JSON");
    let mcp_json = mcp_tool_json(tool, mcp_arguments);
    for field in fields {
        assert_eq!(cli_json[field], mcp_json[field], "field {field}");
    }
}

pub struct McpSession {
    child: Child,
    rx: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_thread: Option<JoinHandle<()>>,
    timeout: Duration,
}

impl McpSession {
    pub fn start() -> Self {
        Self::start_with_timeout(Duration::from_secs(30))
    }

    pub fn start_with_timeout(timeout: Duration) -> Self {
        let mut command = graphzero();
        command.arg("serve");
        Self::start_command(command, timeout)
    }

    fn start_command(mut command: Command, timeout: Duration) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn GraphZero MCP server process");

        let stdout = child
            .stdout
            .take()
            .expect("spawned MCP server must expose piped stdout");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::clone(&stderr);
        let mut stderr_pipe = child
            .stderr
            .take()
            .expect("spawned MCP server must expose piped stderr");
        let stderr_thread = thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf);
            if !buf.is_empty() {
                stderr_buf
                    .lock()
                    .expect("failed to lock captured MCP stderr buffer")
                    .extend(buf);
            }
        });

        Self {
            child,
            rx,
            stderr,
            stderr_thread: Some(stderr_thread),
            timeout,
        }
    }

    pub fn request(&mut self, req: Value) -> Value {
        let line = self.request_line(req);
        serde_json::from_str(&line).expect("MCP server response must be valid JSON-RPC")
    }

    pub fn request_line(&mut self, req: Value) -> String {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .expect("running MCP server must expose piped stdin");
        let line = serde_json::to_string(&req).expect("failed to serialize MCP JSON-RPC request");
        writeln!(stdin, "{line}").expect("failed to write MCP request to child stdin");
        stdin.flush().expect("failed to flush MCP child stdin");
        match self.rx.recv_timeout(self.timeout) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.abort("timed out waiting for MCP response")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.abort("MCP stdout closed before response")
            }
        }
    }

    pub fn notify(&mut self, req: Value) {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .expect("running MCP server must expose piped stdin");
        let line = serde_json::to_string(&req).expect("failed to serialize MCP JSON-RPC request");
        writeln!(stdin, "{line}").expect("failed to write MCP request to child stdin");
        stdin.flush().expect("failed to flush MCP child stdin");
    }

    pub fn handshake(&mut self) {
        self.request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "t", "version": "0" }
            }
        }));
        // FastMCP requires the initialized notification before processing other requests.
        self.notify(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    pub fn shutdown(mut self) {
        drop(self.child.stdin.take());
        if self.wait_for_exit(Duration::from_secs(2)).is_none() {
            self.kill_child();
        }
    }

    fn abort(&mut self, reason: &str) -> String {
        self.kill_child();
        panic!("{reason}: {}", self.diagnostic());
    }

    fn kill_child(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(stderr_thread) = self.stderr_thread.take() {
            let _ = stderr_thread.join();
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn diagnostic(&mut self) -> String {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => "still running".to_string(),
            Err(err) => format!("status unavailable: {err}"),
        };
        let stderr_bytes = self
            .stderr
            .lock()
            .expect("failed to lock captured MCP stderr buffer");
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        format!("child status={status}; stderr={stderr:?}")
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.kill_child();
        }
    }
}

pub fn result_value(resp: &Value) -> &Value {
    resp.get("result")
        .expect("MCP response must contain a result field")
}

pub fn tool_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|o| o.get("text"))
        .and_then(|t| t.as_str())
        .expect("MCP result must contain text in its first content item")
        .to_string()
}

#[cfg(test)]
#[path = "../../../../../tests/graphzero/unit/graphzero-test-support/mcp_session_tests.rs"]
mod tests;
