//! Minimal stdio MCP fake substrate for harness self-tests (pass + fail paths).

use crate::checks::CheckId;
use crate::patterns::validate_refs_in_response;
use crate::schema::{validate_document, SchemaName};
use crate::{CheckResult, ConformanceReport, GateStatus, Ns, Surface};
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

fn result(id: CheckId, status: GateStatus, details: Vec<String>) -> CheckResult {
    CheckResult {
        id: id.as_str().into(),
        name: id.semantic_label().into(),
        passed: status == GateStatus::Pass,
        status,
        skip_reason: None,
        details,
    }
}

fn check_g1_exposure(tools_result: Result<Vec<Value>>) -> CheckResult {
    match tools_result {
        Ok(tools) => {
            let codemode_count = count_codemode_tools(&tools);
            let status = if codemode_count == 3 {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            };
            result(
                CheckId::G1Exposure,
                status,
                vec![format!("tool_count={codemode_count}")],
            )
        }
        Err(error) => result(
            CheckId::G1Exposure,
            GateStatus::Fail,
            vec![error.to_string()],
        ),
    }
}

fn check_g3_capabilities(cap_result: Result<Value>) -> CheckResult {
    match cap_result {
        Ok(capabilities) => {
            match validate_document(SchemaName::CapabilityManifest, &capabilities) {
                Ok(()) => result(CheckId::G3Telemetry, GateStatus::Pass, Vec::new()),
                Err(error) => result(
                    CheckId::G3Telemetry,
                    GateStatus::Fail,
                    vec![error.to_string()],
                ),
            }
        }
        Err(error) => result(
            CheckId::G3Telemetry,
            GateStatus::Fail,
            vec![error.to_string()],
        ),
    }
}

fn check_g2_refs(
    ns: &str,
    checks: &mut Vec<CheckResult>,
    execution: Result<Value>,
    bad_refs: bool,
) {
    match execution {
        Ok(response) => {
            let validation = validate_refs_in_response(ns, &response);
            let (status, details) = match (bad_refs, validation) {
                (false, Ok(())) => (GateStatus::Pass, Vec::new()),
                (false, Err(error)) => (GateStatus::Fail, vec![error]),
                (true, _) => (GateStatus::Fail, vec!["expected bad refs".into()]),
            };
            checks.push(result(CheckId::G2Refs, status, details));
        }
        Err(error) => checks.push(result(
            CheckId::G2Refs,
            GateStatus::Fail,
            vec![error.to_string()],
        )),
    }
}

fn run_smoke_pipeline(ns: &str, backend: &mut impl SmokeBackend) -> ConformanceReport {
    let mut checks = vec![check_g1_exposure(backend.list_tools_after_init())];
    checks.push(check_g3_capabilities(backend.describe_capabilities(ns)));
    let bad_refs = backend.bad_refs();
    check_g2_refs(ns, &mut checks, backend.execute_noop(ns), bad_refs);
    let namespace = match ns {
        "fz" => Ns::Fz,
        "tz" => Ns::Tz,
        "gz" => Ns::Gz,
        _ => panic!("unsupported fake namespace {ns:?}"),
    };
    ConformanceReport::new(namespace, "fake", Surface::Codemode, checks)
}

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

    pub fn run_harness_smoke(&mut self, ns: &str) -> ConformanceReport {
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

    pub(crate) fn run_harness_smoke_in_process(ns: &str, bad_refs: bool) -> ConformanceReport {
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

    fn status_vector(report: &ConformanceReport) -> Vec<(String, GateStatus)> {
        report
            .checks
            .iter()
            .map(|c| (c.id.clone(), c.status))
            .collect()
    }

    #[test]
    fn fake_substrate_passing_smoke_reports_g1_and_g2_pass() {
        let report = run_harness_smoke_in_process("gz", false);
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == CheckId::G1Exposure.as_str() && c.status == GateStatus::Pass),
            "{report:?}"
        );
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == CheckId::G2Refs.as_str() && c.status == GateStatus::Pass),
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
                .any(|c| c.id == CheckId::G2Refs.as_str() && c.status == GateStatus::Fail),
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
            .any(|(id, st)| id == CheckId::G1Exposure.as_str() && *st == GateStatus::Pass));
        assert!(pass
            .iter()
            .any(|(id, st)| id == CheckId::G2Refs.as_str() && *st == GateStatus::Pass));

        let bad = status_vector(&run_harness_smoke_in_process("gz", true));
        assert!(bad
            .iter()
            .any(|(id, st)| id == CheckId::G2Refs.as_str() && *st == GateStatus::Fail));
    }
}

/// Deterministic in-process RACC substrate used only by hub conformance tests.
#[derive(Clone, Debug)]
pub struct RaccFakeSubstrate {
    pub certificate_mutation: crate::racc::CertificateMutation,
    pub receipt_mutation: crate::racc::ReceiptMutation,
    pub skip_irreversible_gate: bool,
    pub budget_mutation: crate::racc::BudgetMutation,
    pub second_certificate_fetch: bool,
    pub residency_mutation: crate::racc::ResidencyMutation,
    pub task_transaction_mutation: crate::racc::TaskTransactionMutation,
}

impl Default for RaccFakeSubstrate {
    fn default() -> Self {
        Self {
            certificate_mutation: crate::racc::CertificateMutation::None,
            receipt_mutation: crate::racc::ReceiptMutation::None,
            skip_irreversible_gate: false,
            budget_mutation: crate::racc::BudgetMutation::None,
            second_certificate_fetch: false,
            residency_mutation: crate::racc::ResidencyMutation::None,
            task_transaction_mutation: crate::racc::TaskTransactionMutation::None,
        }
    }
}

fn fake_provenance() -> crate::racc::Provenance {
    crate::racc::Provenance {
        parser_id: "fixture-parser".into(),
        parser_version: "1".into(),
        index_id: "fixture-index".into(),
        index_version: "4".into(),
        operator_id: "fixture-operator".into(),
        operator_version: "2".into(),
    }
}

fn fake_count(source: &[u8], pattern: &[u8]) -> u64 {
    if pattern.is_empty() {
        return 0;
    }
    source
        .windows(pattern.len())
        .filter(|window| *window == pattern)
        .count() as u64
}

fn fake_certificate(fixture: &crate::racc::QueryFixture) -> crate::racc::RaccCertificate {
    use crate::racc::{CompletenessWitness as W, TypedQuery as Q};
    let query = fixture.query.clone();
    let (payload, completeness) = match &query {
        Q::ReadSpan { start, end, .. } => (
            vec![fixture.source[*start as usize..*end as usize].to_vec()],
            W::ReadSpan,
        ),
        Q::ExactSearch { scope, pattern } => {
            let count = fake_count(fixture.source, pattern);
            (
                vec![pattern.clone(); count as usize],
                W::ExactSearch {
                    scope: scope.clone(),
                    pattern: pattern.clone(),
                    scope_len: fixture.source.len() as u64,
                    match_count: count,
                },
            )
        }
        Q::Definition { symbol } => (
            vec![b"fn target".to_vec()],
            W::Definition {
                symbol: *symbol,
                index_id: "fixture-index".into(),
                index_version: "4".into(),
            },
        ),
        Q::References { symbol } => (
            vec![b"target()".to_vec(), b"target".to_vec()],
            W::References {
                symbol: *symbol,
                index_id: "fixture-index".into(),
                index_version: "4".into(),
                match_count: 2,
            },
        ),
        Q::AstClosure {
            seeds,
            relations,
            radius,
        } => (
            vec![b"1,2,3,4".to_vec()],
            W::AstClosure {
                seeds: seeds.clone(),
                relations: *relations,
                radius: *radius,
                parser_id: "fixture-parser".into(),
                parser_version: "1".into(),
                visited_nodes: 4,
            },
        ),
        Q::CallPath { source, target } => (
            vec![b"7>8>9".to_vec()],
            W::CallPath {
                source: *source,
                target: *target,
                edge_count: 2,
            },
        ),
        Q::DataflowSlice { sink } => (
            vec![b"1>3>4".to_vec()],
            W::DataflowSlice {
                sink: *sink,
                visited_nodes: 3,
            },
        ),
        Q::Diff { old, new } => (
            vec![b"+ target".to_vec()],
            W::Diff {
                old: old.clone(),
                new: new.clone(),
            },
        ),
        Q::BuildReceipt { command } => (
            vec![b"exit=0".to_vec()],
            W::BuildReceipt {
                command: *command,
                exit_code: 0,
                stdout_digest: crate::racc::digest_hex(b"built"),
                stderr_digest: crate::racc::digest_hex(b""),
            },
        ),
        Q::TestTrace { test } => (
            vec![b"pass".to_vec()],
            W::TestTrace {
                test: *test,
                exit_code: 0,
                trace_digest: crate::racc::digest_hex(b"trace-13"),
            },
        ),
    };
    crate::racc::RaccCertificate {
        schema_version: 1,
        domain: "zerostack.racc.typed-query.v1".into(),
        query,
        payload,
        provenance: fake_provenance(),
        completeness,
    }
}

fn honest_receipt() -> crate::racc::DominanceReceipt {
    use crate::racc::{Charges, DominanceReceipt, PhaseArithmetic};
    let mut phases = vec![
        PhaseArithmetic {
            phase: "explore".into(),
            charges: Charges {
                successful_trials: 2,
                failed_trials: 1,
                retries: 1,
                verification_calls: 2,
                recovery_calls: 1,
                expansions: 2,
                failed_expansions: 1,
                fallback_charges: 1,
            },
            reported_total: 0,
        },
        PhaseArithmetic {
            phase: "verify".into(),
            charges: Charges {
                successful_trials: 1,
                failed_trials: 1,
                retries: 1,
                verification_calls: 3,
                recovery_calls: 1,
                expansions: 1,
                failed_expansions: 1,
                fallback_charges: 2,
            },
            reported_total: 0,
        },
    ];
    for phase in &mut phases {
        phase.reported_total = phase.charges.total();
    }
    let target_identity = "fixture-target".to_string();
    let target_digest = crate::racc::digest_hex(b"fixture-target-v1");
    let replay_identity =
        crate::racc::canonical_replay_identity(&target_identity, &target_digest, &phases);
    DominanceReceipt {
        schema_version: 1,
        target_identity,
        target_digest,
        phases,
        replay_identity,
    }
}

impl crate::racc::RaccSubstrate for RaccFakeSubstrate {
    fn certified_query(
        &mut self,
        fixture: &crate::racc::QueryFixture,
    ) -> crate::racc::RaccCertificate {
        use crate::racc::{CertificateMutation as M, CompletenessWitness as W, TypedQuery as Q};
        let mut certificate = fake_certificate(fixture);
        match self.certificate_mutation {
            M::None => {}
            M::OmitPayload => {
                certificate.payload.pop();
            }
            M::ExtraPayload => certificate.payload.push(b"extra".to_vec()),
            M::StaleIndex => certificate.provenance.index_version = "stale".into(),
            M::StaleParser => certificate.provenance.parser_version = "stale".into(),
            M::StaleOperator => certificate.provenance.operator_version = "stale".into(),
            M::WrongDomain => certificate.domain = "wrong.domain".into(),
            M::WrongQueryParameters => certificate.query = Q::Definition { symbol: 999 },
            M::WrongWitnessKind => certificate.completeness = W::ReadSpan,
        }
        certificate
    }

    fn dominance_receipt(&mut self) -> crate::racc::DominanceReceipt {
        use crate::racc::ReceiptMutation as M;
        let mut receipt = honest_receipt();
        match self.receipt_mutation {
            M::None => return receipt,
            M::ReplayIdentity => receipt.replay_identity = "forged".into(),
            M::PhaseArithmetic => receipt.phases[0].reported_total += 1,
            M::OmitFailedTrials => receipt.phases[0].charges.failed_trials = 0,
            M::OmitRetries => receipt.phases[0].charges.retries = 0,
            M::OmitVerificationCalls => receipt.phases[0].charges.verification_calls = 0,
            M::OmitRecoveryCalls => receipt.phases[0].charges.recovery_calls = 0,
            M::OmitExpansions => receipt.phases[0].charges.expansions = 0,
            M::OmitFailedExpansions => receipt.phases[0].charges.failed_expansions = 0,
            M::OmitFallbackCharges => receipt.phases[0].charges.fallback_charges = 0,
        }
        if !matches!(
            self.receipt_mutation,
            M::ReplayIdentity | M::PhaseArithmetic
        ) {
            receipt.phases[0].reported_total = receipt.phases[0].charges.total();
            receipt.replay_identity = crate::racc::canonical_replay_identity(
                &receipt.target_identity,
                &receipt.target_digest,
                &receipt.phases,
            );
        }
        receipt
    }

    fn irreversible_without_evidence(&mut self) -> crate::racc::IrreversibleDecision {
        if self.skip_irreversible_gate {
            crate::racc::IrreversibleDecision::CommittedCompressed
        } else {
            crate::racc::IrreversibleDecision::RawFallback
        }
    }

    fn expansion_budget(&mut self) -> crate::racc::BudgetObservation {
        use crate::racc::BudgetMutation as M;
        match self.budget_mutation {
            M::None => crate::racc::BudgetObservation {
                requested: vec![2, 4, 8, 16],
                measured_costs: vec![2, 4, 7, 12],
                reported_costs: vec![2, 4, 7, 12],
            },
            M::Nonnested => crate::racc::BudgetObservation {
                requested: vec![2, 5, 8, 16],
                measured_costs: vec![2, 4, 7, 12],
                reported_costs: vec![2, 4, 7, 12],
            },
            M::UnderreportedCost => crate::racc::BudgetObservation {
                requested: vec![2, 4, 8, 16],
                measured_costs: vec![2, 4, 7, 20],
                reported_costs: vec![2, 4, 7, 12],
            },
        }
    }

    fn inline_fetch(
        &mut self,
        fixture: &crate::racc::QueryFixture,
    ) -> crate::racc::InlineObservation {
        let certificate = fake_certificate(fixture);
        crate::racc::InlineObservation {
            payload: certificate.payload.concat(),
            certificate,
            round_trips: if self.second_certificate_fetch { 2 } else { 1 },
        }
    }

    fn residency_round_trip(
        &mut self,
        objects: &[crate::racc::StoredFixture],
    ) -> Vec<(crate::racc::StoreLookup, crate::racc::StoreLookup)> {
        use crate::racc::{ResidencyMutation as M, StoreLookup};
        objects
            .iter()
            .enumerate()
            .map(|(index, object)| {
                let mut bytes = object.bytes.clone();
                if self.residency_mutation == M::Corruption && index == 0 {
                    bytes[0] ^= 0xff;
                }
                let resident = StoreLookup::Hit {
                    bytes,
                    metadata: object.metadata.clone(),
                };
                let removed = if self.residency_mutation == M::SilentMiss && index == 0 {
                    StoreLookup::Hit {
                        bytes: Vec::new(),
                        metadata: Default::default(),
                    }
                } else {
                    StoreLookup::Miss {
                        id: object.id.clone(),
                    }
                };
                (resident, removed)
            })
            .collect()
    }

    fn task_attempt(
        &mut self,
        case: crate::racc::TaskAttemptCase,
    ) -> crate::racc::TaskAttemptObservation {
        use crate::racc::{
            TaskAcceptanceReceiptDocument as Receipt, TaskAttemptCase as C,
            TaskAttemptDisposition as D, TaskEffectClass as E, TaskTransactionMutation as M,
        };
        let artifact = crate::racc::digest_hex(b"fixture-artifact");
        let journal = crate::racc::digest_hex(b"fixture-journal");
        let cost = 13;
        match case {
            C::PassingVerifier => {
                let receipt = Receipt {
                    schema_version: 1,
                    task_id: "fixture-task".into(),
                    verifier_command_id: 41,
                    verifier_environment_digest: crate::racc::digest_hex(b"fixture-verifier-env"),
                    outcome: "passed".into(),
                    exit_code: 0,
                    expected_artifact_digests: vec![artifact.clone()],
                    observed_artifact_digests: vec![artifact.clone()],
                    journal_id: journal.clone(),
                    attempt_cost: cost,
                };
                crate::racc::TaskAttemptObservation {
                    effect_class: E::Reversible,
                    exit_code: Some(0),
                    expected_artifact_digests: vec![artifact.clone()],
                    observed_artifact_digests: vec![artifact],
                    journal_id: journal,
                    attempt_cost: cost,
                    charged_attempt_cost: cost,
                    receipt: if self.task_transaction_mutation == M::MissingReceiptCommit {
                        None
                    } else {
                        Some(receipt)
                    },
                    disposition: D::Committed,
                }
            }
            C::FailingVerifier => crate::racc::TaskAttemptObservation {
                effect_class: E::ApprovalRequired,
                exit_code: Some(17),
                expected_artifact_digests: vec![artifact.clone()],
                observed_artifact_digests: vec![artifact],
                journal_id: journal,
                attempt_cost: cost,
                charged_attempt_cost: if self.task_transaction_mutation == M::MissingCharge {
                    0
                } else {
                    cost
                },
                receipt: None,
                disposition: D::RawRollback,
            },
            C::Irreversible => crate::racc::TaskAttemptObservation {
                effect_class: E::Irreversible,
                exit_code: if self.task_transaction_mutation == M::AllowIrreversible {
                    Some(0)
                } else {
                    None
                },
                expected_artifact_digests: Vec::new(),
                observed_artifact_digests: Vec::new(),
                journal_id: journal,
                attempt_cost: 0,
                charged_attempt_cost: 0,
                receipt: None,
                disposition: if self.task_transaction_mutation == M::AllowIrreversible {
                    D::Committed
                } else {
                    D::RejectedIrreversible
                },
            },
        }
    }
}
