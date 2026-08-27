// Wire-level integration tests: stdio JSON-RPC against the built binary.
//
// Tests the release-blocker categories:
//   A: 9KB read exposes fz://blob ref on the wire (both MCP per-op and CodeMode).
//   B: All tests use env-lock guards to prevent process-global env pollution.
//   C: Foreign/unknown ref expansion gives corrective tiered errors.
//
// Wire formats:
//   CodeMode (legacy stdio): structuredContent at top-level result
//   MCP (FastMCP) success: content[1].text = structuredContent JSON
//   MCP (FastMCP) error:   content[0].text = exact structuredContent JSON; isError=true

#[path = "../common/mod.rs"]
mod common;

use common::{TestRoot, env_vars};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Parse the structuredContent from a tools/call response.
/// Handles both CodeMode (legacy) and MCP (FastMCP) formats.
fn parse_sc(resp: &Value) -> Value {
    // CodeMode (legacy): structuredContent at top level
    if let Some(sc) = resp["result"]["structuredContent"].as_object() {
        return serde_json::to_value(sc).unwrap_or_default();
    }
    // FastMCP: structured content in content items
    let content = match resp["result"]["content"].as_array() {
        Some(c) => c,
        None => return serde_json::json!({}),
    };
    if content.len() >= 2 {
        // Success: second item is the structuredContent JSON string
        let sc_text = content[1]["text"].as_str().unwrap_or("{}");
        return serde_json::from_str(sc_text).unwrap_or_else(|_| serde_json::json!({}));
    }
    if content.len() == 1 && resp["result"]["isError"].as_bool().unwrap_or(false) {
        // Error: first item is exact structuredContent JSON. FastMCP's pinned
        // protocol has no structuredContent field, so the router carries the
        // document in text while preserving isError=true.
        let text = content[0]["text"].as_str().unwrap_or("");
        return serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({}));
    }
    serde_json::json!({})
}

#[test]
fn fastmcp_error_parser_requires_exact_structured_json() {
    let document = serde_json::json!({
        "ack": "X0", "ok": false, "error": "missing path",
        "refs": ["fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
        "telemetry": {"kind": "tool.execute"}
    });
    let response = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {"content": [{"type": "text", "text": document.to_string()}], "isError": true}
    });
    assert!(
        response["error"].is_null(),
        "domain failure is a tools/call result"
    );
    assert_eq!(parse_sc(&response), document);

    let prefixed = serde_json::json!({
        "result": {"content": [{"type": "text", "text": format!("X0 {}", document)}], "isError": true}
    });
    assert_eq!(
        parse_sc(&prefixed),
        serde_json::json!({}),
        "ack-prefixed JSON must stay forbidden"
    );
}

fn primary_ack(resp: &Value) -> &str {
    // CodeMode: check structuredContent.ack first
    if let Some(ack) = resp["result"]["structuredContent"]["ack"].as_str() {
        return ack;
    }
    resp["result"]["content"][0]["text"].as_str().unwrap_or("?")
}

fn spawn_fszero(
    mode: &str,
    root: &std::path::Path,
) -> (Child, ChildStdin, BufReader<std::process::ChildStdout>) {
    // Prefer exclusive surface artifacts (process exclusivity). Fall back to
    // shim only if surfaces are not built.
    let cwd = std::env::current_dir().unwrap();
    let bin = match mode {
        "mcp" => {
            let surface = cwd.join("../../target/release/fszero-mcp");
            if surface.is_file() {
                surface
            } else {
                cwd.join("../../target/release/fszero")
            }
        }
        "codemode" => {
            let surface = cwd.join("../../target/release/fszero-codemode");
            if surface.is_file() {
                surface
            } else {
                cwd.join("../../target/release/fszero")
            }
        }
        _ => cwd.join("../../target/release/fszero"),
    };
    assert!(
        bin.is_file(),
        "build surface binary: cargo build --release -p fszero-mcp (or -p fszero-worker --bin fszero-codemode); use --no-default-features --features sqlite-bundled without system SQLite"
    );

    let mut cmd = Command::new(&bin);
    // Dedicated surface bins refuse --mode= (process exclusivity). Only the
    // multi-surface shim accepts --mode=mcp|codemode.
    let is_surface = bin
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "fszero-mcp" || n == "fszero-codemode");
    if !is_surface {
        cmd.arg(format!("--mode={mode}"));
    }
    cmd.env("FSZERO_ROOT", root);
    cmd.env_remove("FSZERO_SKIP_STARTUP_INDEX");
    cmd.env_remove("FSZERO_SKIP_GITIGNORE");
    cmd.env_remove("ZEROSTACK_STORE_ROOT");
    cmd.env_remove("ZERO_STACK_STORE_ROOT");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().expect("spawn fszero surface");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let reader = BufReader::new(stdout);
    (child, stdin, reader)
}

fn rpc(
    req: &Value,
    stdin: &mut ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
) -> Value {
    let line = serde_json::to_string(req).unwrap();
    writeln!(stdin, "{}", line).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();
    serde_json::from_str(&resp_line).unwrap()
}

fn initialize(stdin: &mut ChildStdin, reader: &mut BufReader<std::process::ChildStdout>) {
    let _init = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.0.0"}
            }
        }),
        stdin,
        reader,
    );
    let note = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    writeln!(stdin, "{}", serde_json::to_string(&note).unwrap()).unwrap();
    stdin.flush().unwrap();
}

#[test]
fn per_op_read_9kb_exposes_blob_ref_in_wire_refs() {
    let _lock = lock();
    let _env = env_vars(&[
        ("FSZERO_SKIP_STARTUP_INDEX", None),
        ("FSZERO_SKIP_GITIGNORE", None),
        ("ZEROSTACK_STORE_ROOT", None),
        ("ZERO_STACK_STORE_ROOT", None),
    ]);

    let root = TestRoot::new("wire_read_blob");
    let content: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    root.write("data.bin", &content);

    let (mut child, mut stdin, mut reader) = spawn_fszero("mcp", root.path());
    initialize(&mut stdin, &mut reader);

    let read_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "method": "tools/call",
            "params": {
                "name": "fszero.read",
                "arguments": {"path": "data.bin"}
            }
        }),
        &mut stdin,
        &mut reader,
    );

    let sc = parse_sc(&read_resp);
    assert!(sc["ok"].as_bool().unwrap(), "read failed: {sc:?}");

    let refs = sc["refs"].as_array().expect("refs must be array");
    assert!(
        !refs.is_empty(),
        "refs must be non-empty for 9KB read: {sc:?}"
    );

    let blob_refs: Vec<&str> = refs
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| s.starts_with("fz://blob/"))
        .collect();
    assert!(
        !blob_refs.is_empty(),
        "no fz://blob ref in wire refs: {refs:?}"
    );

    let blob_ref = blob_refs[0].to_string();
    eprintln!("blob_ref = {blob_ref}");

    // Verify the blob ref is valid (64 hex chars after fz://blob/)
    let hash_part = blob_ref.strip_prefix("fz://blob/").unwrap();
    assert_eq!(hash_part.len(), 64, "blob hash must be 64 hex chars");
    assert!(
        hash_part.chars().all(|c| c.is_ascii_hexdigit()),
        "blob hash must be hex"
    );

    // Expand that blob ref to verify it's resolvable
    let expand_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3,
            "method": "tools/call",
            "params": {
                "name": "fszero.expand",
                "arguments": {"arg": &blob_ref}
            }
        }),
        &mut stdin,
        &mut reader,
    );

    let expand_sc = parse_sc(&expand_resp);
    assert!(
        expand_sc["ok"].as_bool().unwrap(),
        "expand of own blob ref must succeed: {expand_sc:?}"
    );

    // Verify the ack is a 2-char token
    let ack = primary_ack(&read_resp);
    assert_eq!(ack.len(), 2, "per-op ack should be 2 chars, got: {ack}");
    assert!(ack.starts_with('R'), "read ack should start with R: {ack}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn expand_from_different_cwd_returns_byte_exact() {
    let _lock = lock();
    let _env = env_vars(&[
        ("FSZERO_SKIP_STARTUP_INDEX", None),
        ("FSZERO_SKIP_GITIGNORE", None),
        ("ZEROSTACK_STORE_ROOT", None),
        ("ZERO_STACK_STORE_ROOT", None),
    ]);

    let root_a = TestRoot::new("wire_cross_a");
    let root_b = TestRoot::new("wire_cross_b");

    let content: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    root_a.write("payload.bin", &content);
    root_b.write(
        "placeholder.txt",
        "different repo
",
    );

    let blob_ref = {
        let (mut child_a, mut stdin_a, mut reader_a) = spawn_fszero("mcp", root_a.path());
        initialize(&mut stdin_a, &mut reader_a);

        let read_resp = rpc(
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "method": "tools/call",
                "params": {"name": "fszero.read", "arguments": {"path": "payload.bin"}}
            }),
            &mut stdin_a,
            &mut reader_a,
        );

        let sc = parse_sc(&read_resp);
        let blob_ref = sc["refs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .find(|s| s.starts_with("fz://blob/"))
            .expect("no blob ref")
            .to_string();

        drop(stdin_a);
        let _ = child_a.wait();
        blob_ref
    };

    let (mut child_b, mut stdin_b, mut reader_b) = spawn_fszero("mcp", root_b.path());
    initialize(&mut stdin_b, &mut reader_b);

    let expand_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "fszero.expand", "arguments": {"arg": &blob_ref}}
        }),
        &mut stdin_b,
        &mut reader_b,
    );

    let expand_sc = parse_sc(&expand_resp);
    let expand_ok = expand_sc["ok"].as_bool().unwrap_or(false);

    if expand_ok {
        // Success MUST carry the exact bytes on the wire: an ok ack with an
        // empty payload is the silent-failure class this test exists to kill.
        let got = expand_sc["payload"]
            .as_str()
            .expect("ok cross-root expand must include structuredContent.payload");
        let want = String::from_utf8_lossy(&content);
        assert_eq!(
            got.len(),
            want.len(),
            "cross-root expand payload must be byte-length-exact"
        );
        assert_eq!(got, want, "cross-root expand must be byte-exact");
        eprintln!("cross-root expand succeeded (ref-index active)");
    } else {
        let err_text = serde_json::to_string(&expand_sc).unwrap();
        assert!(
            err_text.contains("tiers tried")
                || err_text.contains("not found")
                || err_text.contains("ref_not_found"),
            "cross-root expand failure should be descriptive: {err_text}"
        );
        eprintln!("cross-root expand failed as expected without ref-index");
    }

    drop(stdin_b);
    let _ = child_b.wait();
}

#[test]
fn seq_ref_expand_returns_corrective_scoped_error() {
    let _lock = lock();
    let _env = env_vars(&[
        ("FSZERO_SKIP_STARTUP_INDEX", None),
        ("FSZERO_SKIP_GITIGNORE", None),
        ("ZEROSTACK_STORE_ROOT", None),
        ("ZERO_STACK_STORE_ROOT", None),
    ]);

    let root = TestRoot::new("wire_seq_ref");
    root.write(
        "test.txt", "hello
",
    );

    // MCP per-op expand of seq-ref
    let (mut child, mut stdin, mut reader) = spawn_fszero("mcp", root.path());
    initialize(&mut stdin, &mut reader);

    let expand_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "fszero.expand", "arguments": {"arg": "fz://seq/read/42"}}
        }),
        &mut stdin,
        &mut reader,
    );

    assert!(
        expand_resp["error"].is_null(),
        "domain failure must not be a JSON-RPC exception"
    );
    assert_eq!(expand_resp["result"]["isError"], true);
    let error_text = expand_resp["result"]["content"][0]["text"]
        .as_str()
        .expect("FastMCP error text");
    assert!(
        error_text.starts_with('{'),
        "error text must be exact JSON: {error_text}"
    );
    let direct_error: Value = serde_json::from_str(error_text).expect("structured error JSON");

    let sc = parse_sc(&expand_resp);
    assert_eq!(sc, direct_error);
    let ok = sc["ok"].as_bool().unwrap_or(true);
    assert!(!ok, "seq-ref expand must fail. sc={sc:?}");

    // Error refs should be non-empty (carries the seq ref for diagnostics)
    let refs = sc["refs"].as_array().unwrap();
    assert!(
        !refs.is_empty(),
        "seq-ref expand should carry error info refs: {sc:?}"
    );

    // Verify the ack indicates error
    let ack = primary_ack(&expand_resp);
    assert!(ack.contains("X0"), "error ack should contain X0: {ack}");

    drop(stdin);
    let _ = child.wait();

    // CodeMode: uncaught seq-ref expansion must carry a corrective typed message.
    let (mut cm_child, mut cm_stdin, mut cm_reader) = spawn_fszero("codemode", root.path());
    initialize(&mut cm_stdin, &mut cm_reader);

    // Use uncaught path - the error message is in the ack/error
    let cm_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "fz_execute_code",
                "arguments": {
                    "plan": "return zero.token.expand('fz://seq/read/42')",
                    "envelope": "v2"
                }
            }
        }),
        &mut cm_stdin,
        &mut cm_reader,
    );

    let cm_ack = primary_ack(&cm_resp);
    assert!(
        cm_ack.contains("err"),
        "CodeMode seq-ref expand should fail: {cm_ack}"
    );

    // The error must contain the corrective message from the typed ref boundary.
    assert!(
        cm_ack.contains("seq_ref_scoped") || cm_ack.contains("execution-scoped"),
        "CodeMode seq-ref error should be corrective: {cm_ack}"
    );

    // Verify the structured error has the right kind
    let cm_sc = parse_sc(&cm_resp);
    let cm_full = serde_json::to_string(&cm_sc).unwrap();
    assert!(
        cm_full.contains("seq_ref_scoped") || cm_full.contains("execution-scoped"),
        "structured seq-ref error should be corrective: {cm_full}"
    );

    drop(cm_stdin);
    let _ = cm_child.wait();
}

#[test]
fn unknown_blob_ref_expand_returns_tiered_not_found() {
    let _lock = lock();
    let _env = env_vars(&[
        ("FSZERO_SKIP_STARTUP_INDEX", None),
        ("FSZERO_SKIP_GITIGNORE", None),
        ("ZEROSTACK_STORE_ROOT", None),
        ("ZERO_STACK_STORE_ROOT", None),
    ]);

    let root = TestRoot::new("wire_unknown_blob");
    root.write(
        "test.txt", "hello
",
    );

    // MCP per-op expand of unknown blob ref
    let (mut child, mut stdin, mut reader) = spawn_fszero("mcp", root.path());
    initialize(&mut stdin, &mut reader);

    let fake_ref = "fz://blob/deadbeef00000000000000000000000000000000000000000000000000000000";
    let expand_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "fszero.expand", "arguments": {"arg": fake_ref}}
        }),
        &mut stdin,
        &mut reader,
    );

    let sc = parse_sc(&expand_resp);
    let ok = sc["ok"].as_bool().unwrap_or(true);
    assert!(!ok, "unknown blob ref expand must fail: {sc:?}");

    // MCP error wire format: refs[] carries the looked-up ref, ack is X0.
    // The full tiered error message is generated in product code
    // (expand_with_tiers) but FastMCP exposes only the structured fields.
    let refs = sc["refs"].as_array().unwrap();
    assert!(!refs.is_empty(), "error response must carry refs: {sc:?}");
    let ack = sc["ack"].as_str().unwrap_or("");
    assert_eq!(ack, "X0", "error ack should be X0: {sc:?}");
    assert!(
        refs.iter()
            .any(|v| v.as_str().map_or(false, |s| s.contains("deadbeef"))),
        "error refs should reference the looked-up blob: {refs:?}"
    );

    drop(stdin);
    let _ = child.wait();

    // CodeMode: uncaught expand of unknown blob - must give descriptive error
    let (mut cm_child, mut cm_stdin, mut cm_reader) = spawn_fszero("codemode", root.path());
    initialize(&mut cm_stdin, &mut cm_reader);

    let cm_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "fz_execute_code",
                "arguments": {
                    "plan": "return zero.token.expand('fz://blob/deadbeef00000000000000000000000000000000000000000000000000000000000')",
                    "envelope": "v2"
                }
            }
        }),
        &mut cm_stdin,
        &mut cm_reader,
    );

    let cm_ack = primary_ack(&cm_resp);
    assert!(
        cm_ack.contains("err"),
        "CodeMode unknown blob expand should fail: {cm_ack}"
    );

    // The 67-char hash is not a v1 identity: the strict ZeroRef layer
    // rejects it typed as `malformed` (fszero-c6q.2) instead of missing
    // through the tiers to ref_not_found.
    assert!(
        cm_ack.contains("malformed")
            || cm_ack.contains("ref_not_found")
            || cm_ack.contains("tiers")
            || cm_ack.contains("not found"),
        "CodeMode error should be descriptive: {cm_ack}"
    );

    drop(cm_stdin);
    let _ = cm_child.wait();
}

#[test]
fn codemode_read_9kb_exposes_blob_ref_in_plan_result() {
    let _lock = lock();
    let _env = env_vars(&[
        ("FSZERO_SKIP_STARTUP_INDEX", None),
        ("FSZERO_SKIP_GITIGNORE", None),
        ("ZEROSTACK_STORE_ROOT", None),
        ("ZERO_STACK_STORE_ROOT", None),
    ]);

    let root = TestRoot::new("wire_cm_read_blob");
    let content: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    root.write("big.dat", &content);

    let (mut child, mut stdin, mut reader) = spawn_fszero("codemode", root.path());
    initialize(&mut stdin, &mut reader);

    // CodeMode JS plan: read file, examine payload structure
    let cm_resp = rpc(
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "fz_execute_code",
                "arguments": {
                    "plan": "const r = fs.read({ path: 'big.dat' }); return { ok: r.ok, ref: r.ref, hasPayload: !!r.payload, payloadIsObject: typeof r.payload === 'object', payloadRef: r.payload && r.payload.ref, capsule: r.payload && r.payload.capsule };",
                    "envelope": "v2"
                }
            }
        }),
        &mut stdin,
        &mut reader,
    );

    // Ref-first wire: large reads surface fz://blob on the ack/ref path
    // (session-cost C6). Ack may be bare fz://blob/... rather than "ok fz…".
    let sc = parse_sc(&cm_resp);
    let wire_ref = sc["ref"]
        .as_str()
        .or_else(|| sc["ack"].as_str())
        .unwrap_or("");
    assert!(
        wire_ref.starts_with("fz://blob/"),
        "CodeMode 9KB read must expose fz://blob on wire ref/ack, got: {wire_ref}"
    );
    assert!(
        wire_ref.len() > 30,
        "blob ref should be full sha256 hash: {wire_ref}"
    );

    let value = &sc["value"]["result"];
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "read op should succeed: {value:?}"
    );
    let plan_ref = value["ref"].as_str().unwrap_or("");
    assert!(
        plan_ref.starts_with("fz://blob/"),
        "plan result.ref should be fz://blob/ for 9KB file: {plan_ref}"
    );

    // Capsule shape is optional on ref-first 9KB serves; the session-cost
    // contract is the durable fz://blob identity on the wire, not inline bytes.

    drop(stdin);
    let _ = child.wait();
}
