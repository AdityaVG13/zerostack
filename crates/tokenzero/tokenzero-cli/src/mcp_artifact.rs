use super::{measure_rss_mb, p95_f64, write_artifacts};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tempfile::tempdir;

fn spawn_server(exe: &Path, root: &Path, cache: &Path) -> Result<Child> {
    Ok(Command::new(exe)
        .arg("mcp-server")
        .arg("--allowed-root")
        .arg(root)
        .arg("--cache-path")
        .arg(cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}
fn initialize(id: usize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"tokenzero-mcp-smoke","version":env!("CARGO_PKG_VERSION")}}})
}
fn write_request(stdin: &mut impl Write, request: &Value) -> Result<()> {
    writeln!(stdin, "{request}")?;
    Ok(())
}
fn response(stdout: &str, id: usize) -> impl Iterator<Item = Value> + '_ {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(move |payload| payload.get("id") == Some(&json!(id)))
}

pub(crate) fn run_mcp_artifact(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    iterations: usize,
) -> Result<Value> {
    let temp = tempdir()?;
    fs::write(temp.path().join("sample.txt"), "alpha\nbeta\n")?;
    let exe = std::env::current_exe()?;
    let cache_path = temp.path().join("cache.json");
    let mut missing = [0usize; 12];
    let mut unexpected_exits = 0usize;
    let mut disconnect_failures = 0usize;
    let mut cache_race_failures = 0usize;
    let mut rss_samples = Vec::new();
    for idx in 0..iterations {
        let mut child = spawn_server(&exe, temp.path(), &cache_path)?;
        if let Some(rss) = measure_rss_mb(child.id()) {
            rss_samples.push(rss)
        }
        {
            let stdin = child.stdin.as_mut().context("missing mcp stdin")?;
            writeln!(stdin, "{{bad json")?;
            let sample = temp.path().join("sample.txt");
            let requests = [
                initialize(idx),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":idx+500,"method":"server/not-a-method","params":{}}),
                json!({"jsonrpc":"2.0","id":idx+1000,"method":"tools/list","params":{}}),
                json!({"jsonrpc":"2.0","id":idx+1100,"method":"tools/list","params":{"_meta":{"tokenzero/toolCluster":"material"}}}),
                json!({"jsonrpc":"2.0","id":idx+1250,"method":"resources/list","params":{}}),
                json!({"jsonrpc":"2.0","id":idx+1300,"method":"resources/read","params":{"uri":"resource://tokenzero/tools"}}),
                json!({"jsonrpc":"2.0","id":idx+1500,"method":"tools/call","params":{"name":"no_such_tool","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":idx+2000,"method":"tools/call","params":{"name":"read","arguments":{"path":&sample}}}),
                json!({"jsonrpc":"2.0","id":idx+2001,"method":"tools/call","params":{"name":"read","arguments":{"path":&sample}}}),
                json!({"jsonrpc":"2.0","id":idx+2002,"method":"tools/call","params":{"name":"read","arguments":{"path":&sample}}}),
            ];
            for request in &requests {
                write_request(stdin, request)?;
            }
        }
        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let resources_id = idx + 1250;
        let read_id = idx + 1300;
        let checks = [
            mcp_stdout_has_successful_initialize(&stdout, idx),
            stderr.contains("Parse error") || stderr.contains("JSON error"),
            stdout.contains("Method not found"),
            stdout.contains("no_such_tool") && stdout.contains("Method not found"),
            stdout.contains("\"additionalProperties\":false") && stdout.contains("\"inputSchema\""),
            stdout.contains("\"resources\""),
            mcp_stdout_has_resource_uri(&stdout, resources_id, "resource://tokenzero/tools"),
            mcp_stdout_has_resource_content(
                &stdout,
                read_id,
                "resource://tokenzero/tools",
                "tools",
            ),
            stdout.contains("\"code\"") && stdout.contains("\"message\""),
            stdout.contains("\"tools\""),
            stdout.matches("alpha\\nbeta").count() >= 3,
            output.status.success(),
        ];
        for (count, passed) in missing.iter_mut().zip(checks) {
            *count += usize::from(!passed)
        }
        unexpected_exits += usize::from(!output.status.success());
        let mut disconnect_child = spawn_server(&exe, temp.path(), &cache_path)?;
        if let Some(stdin) = disconnect_child.stdin.as_mut() {
            write!(stdin, "{{partial-json")?
        }
        drop(disconnect_child.stdin.take());
        disconnect_failures += usize::from(!disconnect_child.wait_with_output()?.status.success());
        let mut race_children = Vec::new();
        for race in 0..4 {
            let mut child = spawn_server(&exe, temp.path(), &cache_path)?;
            if let Some(stdin) = child.stdin.as_mut() {
                for request in [
                    initialize(1),
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                    json!({"jsonrpc":"2.0","id":idx+3000+race,"method":"tools/call","params":{"name":"read","arguments":{"path":temp.path().join("sample.txt")}}}),
                ] {
                    write_request(stdin, &request)?
                }
            }
            race_children.push(child);
        }
        for child in race_children {
            let output = child.wait_with_output()?;
            cache_race_failures += usize::from(
                !output.status.success()
                    || !String::from_utf8_lossy(&output.stdout).contains("alpha\\nbeta"),
            );
        }
    }
    let ok = missing.iter().all(|&count| count == 0)
        && disconnect_failures == 0
        && cache_race_failures == 0;
    let report = json!({
        "schema_version":"tokenzero.rust_mcp_churn.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"iterations":iterations,
        "initialize_successes_observed":iterations-missing[0],"initialize_failures":missing[0],"malformed_requests":iterations,
        "parse_errors_observed":iterations-missing[1],"unknown_methods_observed":iterations-missing[2],"unknown_tools_observed":iterations-missing[3],
        "tool_schema_failures":missing[4],"resource_discovery_failures":missing[5],"resource_tools_present_failures":missing[6],
        "resource_tools_read_failures":missing[7],"structured_error_data_failures":missing[8],"tool_cluster_filter_failures":missing[9],
        "parallel_read_batches":iterations,"parallel_read_failures":missing[10],"disconnects":iterations,"disconnect_failures":disconnect_failures,
        "cache_race_processes":iterations*4,"cache_race_failures":cache_race_failures,"unexpected_exits":unexpected_exits,
        "rss_mb_p95":p95_f64(&mut rss_samples),"accelerated":iterations>1});
    write_artifacts(
        &output_json,
        output_md.as_deref(),
        &report,
        "Rust MCP artifact",
    )?;
    Ok(report)
}
fn mcp_stdout_has_successful_initialize(stdout: &str, id: usize) -> bool {
    response(stdout, id).any(|p| {
        p.get("error").is_none()
            && p["result"]["protocolVersion"] == "2024-11-05"
            && p["result"]["serverInfo"]["name"] == "tokenzero"
    })
}
fn mcp_stdout_has_resource_uri(stdout: &str, id: usize, uri: &str) -> bool {
    response(stdout, id).any(|p| {
        p.get("error").is_none()
            && p["result"]["resources"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r["uri"] == uri))
    })
}
fn mcp_stdout_has_resource_content(stdout: &str, id: usize, uri: &str, text: &str) -> bool {
    response(stdout, id).any(|p| {
        p.get("error").is_none()
            && p["result"]["contents"].as_array().is_some_and(|rows| {
                rows.iter().any(|r| {
                    r["uri"] == uri && r["text"].as_str().is_some_and(|s| s.contains(text))
                })
            })
    })
}
