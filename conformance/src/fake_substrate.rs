//! Minimal stdio MCP fake substrate for harness self-tests (pass + fail paths).

use crate::checks::{CheckId, CheckOutcome, CheckStatus, HarnessReport};
use crate::patterns::validate_refs_in_response;
use crate::schema::{validate_document, SchemaName};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const FAKE_HASH: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

/// Backend ops shared by subprocess and in-process smoke pipelines.
trait SmokeBackend {
    fn list_tools_after_init(&mut self) -> Result<Vec<Value>>;
    fn describe_capabilities(&mut self, ns: &str) -> Result<Value>;
    fn execute_noop(&mut self, ns: &str) -> Result<Value>;
    fn bad_refs(&self) -> bool;
}

fn is_codemode_tool_name(name: &str) -> bool {
    name.ends_with("_execute_code")
        || name.ends_with("_codemode_search")
        || name.ends_with("_codemode_describe")
}

fn count_codemode_tools(tools: &[Value]) -> usize {
    tools
        .iter()
        .filter(|t| {
            t.get("name")
                .and_then(|n| n.as_str())
                .map(is_codemode_tool_name)
                .unwrap_or(false)
        })
        .count()
}

fn check_g1_exposure(tools_result: Result<Vec<Value>>) -> CheckOutcome {
    match tools_result {
        Ok(tools) => {
            let codemode_count = count_codemode_tools(&tools);
            CheckOutcome {
                id: CheckId::G1Exposure,
                status: if codemode_count == 3 {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: Some(format!("tool_count={codemode_count}")),
            }
        }
        Err(e) => CheckOutcome {
            id: CheckId::G1Exposure,
            status: CheckStatus::Fail,
            detail: Some(e.to_string()),
        },
    }
}

fn check_g3_capabilities(cap_result: Result<Value>) -> CheckOutcome {
    match cap_result {
        Ok(cap) => CheckOutcome {
            id: CheckId::G3Telemetry,
            status: if validate_document(SchemaName::CapabilityManifest, &cap).is_ok() {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            detail: Some("capabilities schema".into()),
        },
        Err(e) => CheckOutcome {
            id: CheckId::G3Telemetry,
            status: CheckStatus::Fail,
            detail: Some(e.to_string()),
        },
    }
}

fn push_execute_checks(
    checks: &mut Vec<CheckOutcome>,
    ns: &str,
    bad_refs: bool,
    exec_result: Result<Value>,
) {
    match exec_result {
        Ok(body) => {
            checks.push(CheckOutcome {
                id: CheckId::G2Refs,
                status: if validate_refs_in_response(ns, &body).is_ok() {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: if bad_refs {
                    Some("expected bad refs".into())
                } else {
                    None
                },
            });
            if let Some(tel) = body.get("telemetry") {
                checks.push(CheckOutcome {
                    id: CheckId::G3Telemetry,
                    status: if validate_document(SchemaName::Telemetry, tel).is_ok() {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    detail: Some("execute telemetry".into()),
                });
            }
        }
        Err(e) => checks.push(CheckOutcome {
            id: CheckId::G2Refs,
            status: CheckStatus::Fail,
            detail: Some(e.to_string()),
        }),
    }
}

/// Shared G1 / caps / execute smoke pipeline for both MCP backends.
fn run_smoke_pipeline(ns: &str, backend: &mut impl SmokeBackend) -> HarnessReport {
    let mut checks = Vec::new();
    checks.push(check_g1_exposure(backend.list_tools_after_init()));
    checks.push(check_g3_capabilities(backend.describe_capabilities(ns)));
    push_execute_checks(
        &mut checks,
        ns,
        backend.bad_refs(),
        backend.execute_noop(ns),
    );
    HarnessReport {
        contract_version: "1.0".into(),
        ns: ns.to_string(),
        substrate_binary: "fake-in-process".into(),
        checks,
    }
}

/// Scripted codemode MCP server subprocess (`current_exe --fake-codemode-mcp <ns>`).
pub struct FakeCodemodeSubstrate {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    bad_refs: bool,
}

impl FakeCodemodeSubstrate {
    pub fn spawn_passing(ns: &str) -> Result<Self> {
        Self::spawn_inner(ns, false)
    }

    pub fn spawn_failing_refs(ns: &str) -> Result<Self> {
        Self::spawn_inner(ns, true)
    }

    fn spawn_inner(ns: &str, bad_refs: bool) -> Result<Self> {
        let mut cmd = Command::new(std::env::current_exe().context("current_exe")?);
        cmd.arg("--fake-codemode-mcp").arg(ns);
        if bad_refs {
            cmd.arg("--bad-refs");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn fake MCP")?;
        let stdin = child.stdin.take().context("stdin")?;
        let stdout = child.stdout.take().context("stdout")?;
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            bad_refs,
        })
    }

    pub fn run_harness_smoke(&mut self, ns: &str) -> HarnessReport {
        run_smoke_pipeline(ns, self)
    }

    fn mcp_request(&mut self, request: Value) -> Result<Value> {
        let line = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;
        let mut buf = String::new();
        self.reader.read_line(&mut buf)?;
        let resp: Value = serde_json::from_str(&buf).context("parse MCP response")?;
        if resp.get("error").is_some() {
            bail!("MCP error: {}", resp);
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    fn mcp_initialize_and_list_tools(&mut self) -> Result<Vec<Value>> {
        let _ = self.mcp_request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "conformance", "version": "0.1.0" }
            }
        }))?;
        let result = self.mcp_request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default())
    }

    fn mcp_describe_capabilities(&mut self, ns: &str) -> Result<Value> {
        let result = self.mcp_request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": format!("{ns}_codemode_describe"),
                "arguments": { "name": "capabilities" }
            }
        }))?;
        parse_tool_json_result(&result)
    }

    fn mcp_execute_noop(&mut self, ns: &str) -> Result<Value> {
        let result = self.mcp_request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": format!("{ns}_execute_code"),
                "arguments": { "plan": "return 1;" }
            }
        }))?;
        parse_tool_json_result(&result)
    }
}

impl SmokeBackend for FakeCodemodeSubstrate {
    fn list_tools_after_init(&mut self) -> Result<Vec<Value>> {
        self.mcp_initialize_and_list_tools()
    }

    fn describe_capabilities(&mut self, ns: &str) -> Result<Value> {
        self.mcp_describe_capabilities(ns)
    }

    fn execute_noop(&mut self, ns: &str) -> Result<Value> {
        self.mcp_execute_noop(ns)
    }

    fn bad_refs(&self) -> bool {
        self.bad_refs
    }
}

impl Drop for FakeCodemodeSubstrate {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn parse_tool_json_result(result: &Value) -> Result<Value> {
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("text"))
        .and_then(|t| t.as_str())
        .context("tool result text")?;
    serde_json::from_str(text).context("tool JSON")
}

fn tool_json_response(body: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": body.to_string() }],
        "isError": false
    })
}

fn capabilities_json(ns: &str) -> Value {
    json!({
        "contract_version": "1.0",
        "ns": ns,
        "mutation": "readonly",
        "plan_forms": ["recipe", "json", "js"],
        "limits": { "max_output_bytes": 65536, "max_logical_ops": 1000 }
    })
}

fn execute_json(ns: &str, bad_refs: bool) -> Value {
    let execution_id = "cm://exec/1719859200123-abcdef012345";
    let telemetry_ref = if bad_refs {
        format!("{ns}://codemode/execution/bad-id-only")
    } else {
        format!("{ns}://codemode/execution/cm_exec_01/telemetry")
    };
    json!({
        "execution_id": execution_id,
        "telemetry_ref": telemetry_ref,
        "steps_ref": format!("{ns}://codemode/execution/cm_exec_01/steps"),
        "result_ref": format!("{ns}://blob/{FAKE_HASH}"),
        "telemetry": {
            "kind": "codemode.execute",
            "status": "ok",
            "logical_ops": 1,
            "physical_ops": 1,
            "batched_ops": 0,
            "internal_actions": 1,
            "cache_hits": 0,
            "cache_misses": 0,
            "store_writes": 0,
            "wall_ms": 1,
            "bytes_materialized": 4
        }
    })
}

fn handle_tools_call(ns: &str, bad_refs: bool, name: &str) -> Value {
    if name.ends_with("_codemode_describe") {
        tool_json_response(capabilities_json(ns))
    } else if name.ends_with("_execute_code") {
        tool_json_response(execute_json(ns, bad_refs))
    } else {
        tool_json_response(json!({}))
    }
}

/// Shared MCP method dispatch for the fake fixture (stdio + in-process).
fn dispatch_fake_mcp(ns: &str, bad_refs: bool, request: &Value) -> Value {
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
    match method {
        "initialize" => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fake-codemode", "version": "0.1.0" }
        }),
        "tools/list" => json!({
            "tools": [
                { "name": format!("{ns}_execute_code") },
                { "name": format!("{ns}_codemode_search") },
                { "name": format!("{ns}_codemode_describe") }
            ]
        }),
        "tools/call" => {
            let name = request
                .pointer("/params/name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            handle_tools_call(ns, bad_refs, name)
        }
        _ => json!({}),
    }
}

/// Entry point for the fake MCP child (`current_exe --fake-codemode-mcp <ns> [--bad-refs]`).
///
/// Callers pass rebuilt argv `[prog, ns, optional --bad-refs]` (see binary main).
pub fn fake_mcp_main(args: &[String]) -> Result<()> {
    let ns = args
        .get(1)
        .map(|s| s.as_str())
        .context("--fake-codemode-mcp <ns>")?;
    let bad_refs = args.iter().any(|a| a == "--bad-refs");
    let reader = BufReader::new(std::io::stdin().lock());
    let mut out = std::io::stdout().lock();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = serde_json::from_str(&line)?;
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let result = dispatch_fake_mcp(ns, bad_refs, &req);
        writeln!(
            out,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        )?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod in_process {
    use super::*;

    pub(crate) fn run_harness_smoke_in_process(ns: &str, bad_refs: bool) -> HarnessReport {
        let mut state = InProcessFake {
            ns: ns.to_string(),
            bad_refs,
        };
        run_smoke_pipeline(ns, &mut state)
    }

    struct InProcessFake {
        ns: String,
        bad_refs: bool,
    }

    impl SmokeBackend for InProcessFake {
        fn list_tools_after_init(&mut self) -> Result<Vec<Value>> {
            let _ = dispatch_fake_mcp(
                &self.ns,
                self.bad_refs,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {}
                }),
            );
            let result = dispatch_fake_mcp(
                &self.ns,
                self.bad_refs,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list"
                }),
            );
            Ok(result
                .get("tools")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default())
        }

        fn describe_capabilities(&mut self, ns: &str) -> Result<Value> {
            let result = dispatch_fake_mcp(
                ns,
                self.bad_refs,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": format!("{ns}_codemode_describe"),
                        "arguments": { "name": "capabilities" }
                    }
                }),
            );
            parse_tool_json_result(&result)
        }

        fn execute_noop(&mut self, ns: &str) -> Result<Value> {
            let result = dispatch_fake_mcp(
                ns,
                self.bad_refs,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": format!("{ns}_execute_code"),
                        "arguments": { "plan": "return 1;" }
                    }
                }),
            );
            parse_tool_json_result(&result)
        }

        fn bad_refs(&self) -> bool {
            self.bad_refs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use in_process::run_harness_smoke_in_process;

    fn status_vector(report: &HarnessReport) -> Vec<(CheckId, CheckStatus)> {
        report
            .checks
            .iter()
            .map(|c| (c.id, c.status.clone()))
            .collect()
    }

    #[test]
    fn fake_substrate_passing_smoke_reports_g1_and_g2_pass() {
        let report = run_harness_smoke_in_process("gz", false);
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == CheckId::G1Exposure && c.status == CheckStatus::Pass),
            "{report:?}"
        );
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == CheckId::G2Refs && c.status == CheckStatus::Pass),
            "{report:?}"
        );
    }

    #[test]
    fn fake_substrate_bad_refs_fails_g2() {
        let report = run_harness_smoke_in_process("gz", true);
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == CheckId::G2Refs && c.status == CheckStatus::Fail),
            "{report:?}"
        );
    }

    #[test]
    fn dispatch_fake_mcp_initialize_list_call_order_shapes() {
        let init = dispatch_fake_mcp(
            "gz",
            false,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        );
        assert_eq!(init["protocolVersion"], "2025-06-18");
        assert_eq!(init["serverInfo"]["name"], "fake-codemode");

        let list = dispatch_fake_mcp(
            "gz",
            false,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        );
        let tools = list["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["name"], "gz_execute_code");

        let call = dispatch_fake_mcp(
            "gz",
            false,
            &json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"gz_codemode_describe","arguments":{"name":"capabilities"}}
            }),
        );
        let cap = parse_tool_json_result(&call).expect("cap json");
        assert_eq!(cap["ns"], "gz");
        assert_eq!(cap["contract_version"], "1.0");
    }

    #[test]
    fn in_process_status_vector_stable_for_pass_and_bad_refs() {
        let pass = status_vector(&run_harness_smoke_in_process("gz", false));
        assert!(pass
            .iter()
            .any(|(id, st)| *id == CheckId::G1Exposure && *st == CheckStatus::Pass));
        assert!(pass
            .iter()
            .any(|(id, st)| *id == CheckId::G2Refs && *st == CheckStatus::Pass));

        let bad = status_vector(&run_harness_smoke_in_process("gz", true));
        assert!(bad
            .iter()
            .any(|(id, st)| *id == CheckId::G2Refs && *st == CheckStatus::Fail));
    }
}
