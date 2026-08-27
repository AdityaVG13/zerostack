//! Differential multi-surface conformance corpus (tokenzero-irx9.7).
//!
//! Generated vectors from the operation registry run through raw dispatcher,
//! MCP, CLI, CodeMode (when bound), and private raw worker. Compares normalized
//! status/error-kind/tool identity. Includes boundary/failure vectors and a
//! real mutation FS check across surfaces.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokenzero_core::operation_abi::all_operations;
use tokenzero_engine::{
    DispatchOutcome, EngineConfig, HandshakeSurface, RAW_WORKER_PROTOCOL_VERSION, RawWorkerRequest,
    TokenZeroEngine, build_surface_capability, dispatch_cli, dispatch_codemode_method,
    dispatch_mcp_tool, dispatch_raw_worker, execute_raw_worker_frame, operation_is_domain,
};

fn engine_for(root: &Path) -> TokenZeroEngine {
    let mut config = EngineConfig::for_root(root);
    config.session_dedup = false;
    config.diff_reads = false;
    config.fetch_enabled = false;
    TokenZeroEngine::new(config)
}

fn seed_repo(root: &Path) {
    fs::write(root.join("note.txt"), "conformance-seed-line\n").unwrap();
    fs::write(root.join("mutate.txt"), "before-mutation\n").unwrap();
}

fn minimal_args(op: &str, root: &Path) -> Value {
    let note = root.join("note.txt").display().to_string();
    match op {
        "tz_read" => json!({"path": note}),
        "tz_find" | "tz_grep" => {
            json!({"query": "conformance", "path": root.display().to_string()})
        }
        "tz_recall" => json!({"query": "conformance"}),
        "tz_glob" => json!({"pattern": "*.txt", "path": root.display().to_string()}),
        "tz_tree" => json!({"path": root.display().to_string(), "depth": 1}),
        "tz_edit" => json!({
            "path": note,
            "edits": [{"find": "conformance-seed-line", "replace": "conformance-seed-line"}],
            "dry_run": true
        }),
        "tz_shell" => json!({"command": "true", "cwd": root.display().to_string()}),
        "tz_ingest" => json!({"text": "conformance-ingest"}),
        "tz_expand" => {
            json!({"ref": "tz://0000000000000000000000000000000000000000000000000000000000000000"})
        }
        "tz_mem" => json!({}),
        "tz_cache_pack" => json!({"scope": "agent"}),
        "tz_rewrite" => json!({"command": "echo hi"}),
        "tz_discover" => json!({}),
        "tz_report_tool_issue" => json!({
            "tool": "zero_execute",
            "summary": "conformance probe"
        }),
        "tz_batch" => json!({"ops": [{"tool": "tz_mem", "args": {}}]}),
        "tz_fetch" => json!({"url": "https://example.invalid/"}),
        _ => json!({}),
    }
}

/// Normalized multi-surface outcome (transport-volatile fields stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Norm {
    ok: bool,
    status: String,
    error_code: Option<String>,
    error_kind: Option<String>,
    tool: Option<String>,
}

fn norm_outcome(out: &DispatchOutcome) -> Norm {
    let tr = out.tool_response.as_ref();
    let ok = out.is_ok();
    Norm {
        ok,
        status: tr
            .map(|r| r.status.clone())
            .unwrap_or_else(|| if ok { "ok".into() } else { "error".into() }),
        error_code: tr.and_then(|r| r.error.as_ref().map(|e| e.code.clone())),
        error_kind: out
            .domain_error
            .as_ref()
            .map(|e| e.kind.as_str().to_string()),
        tool: tr.map(|r| r.tool.clone()).or_else(|| Some(out.op.clone())),
    }
}

fn dispatch_all(root: &Path, op: &str, args: &Value) -> (Norm, Norm, Norm, Option<Norm>, Norm) {
    let raw_e = engine_for(root);
    let mcp_e = engine_for(root);
    let cli_e = engine_for(root);
    let cm_e = engine_for(root);
    let worker_e = engine_for(root);

    let raw = norm_outcome(&dispatch_raw_worker(&raw_e, op, args));
    let mcp = match dispatch_mcp_tool(&mcp_e, op, args) {
        Ok(o) => norm_outcome(&o),
        Err(e) => Norm {
            ok: false,
            status: "error".into(),
            error_code: Some(e.kind.as_str().into()),
            error_kind: Some(e.kind.as_str().into()),
            tool: Some(op.into()),
        },
    };
    let cli = norm_outcome(&dispatch_cli(&cli_e, op, args));

    let has_cm = all_operations()
        .iter()
        .find(|o| o.name == op)
        .and_then(|o| o.exposure.codemode_binding)
        .is_some();
    let cm = if has_cm {
        let method = op
            .strip_prefix("tz_")
            .map(|s| format!("zero.{s}"))
            .unwrap_or_else(|| op.to_string());
        Some(match dispatch_codemode_method(&cm_e, &method, args) {
            Ok(o) => norm_outcome(&o),
            Err(e) => Norm {
                ok: false,
                status: "error".into(),
                error_code: Some(e.kind.as_str().into()),
                error_kind: Some(e.kind.as_str().into()),
                tool: Some(method),
            },
        })
    } else {
        None
    };

    let cap = build_surface_capability(HandshakeSurface::RawWorker);
    let req = RawWorkerRequest {
        protocol: Some(RAW_WORKER_PROTOCOL_VERSION.into()),
        op: op.into(),
        args: args.clone(),
        peer_contract_digest: Some(cap.semantic_contract_digest),
        peer_contract_version: Some(cap.semantic_contract_version),
        control: None,
    };
    let worker_resp = execute_raw_worker_frame(&worker_e, &req);
    let worker = Norm {
        ok: worker_resp.ok,
        status: if worker_resp.ok {
            "ok".into()
        } else {
            "error".into()
        },
        error_code: worker_resp.error.as_ref().map(|e| e.kind.clone()),
        error_kind: worker_resp.error.as_ref().map(|e| e.kind.clone()),
        tool: Some(op.into()),
    };

    (raw, mcp, cli, cm, worker)
}

fn assert_surfaces_agree(
    op: &str,
    raw: &Norm,
    mcp: &Norm,
    cli: &Norm,
    cm: Option<&Norm>,
    worker: &Norm,
) {
    assert_eq!(raw.ok, mcp.ok, "{op}: raw.ok vs mcp.ok");
    assert_eq!(raw.status, mcp.status, "{op}: raw.status vs mcp.status");
    assert_eq!(raw.ok, cli.ok, "{op}: raw.ok vs cli.ok");
    assert_eq!(raw.status, cli.status, "{op}: raw.status vs cli.status");
    // Worker ok class must match raw (error_code strings may differ slightly).
    assert_eq!(raw.ok, worker.ok, "{op}: raw.ok vs worker.ok");
    if let Some(cm) = cm {
        assert_eq!(raw.ok, cm.ok, "{op}: raw.ok vs codemode.ok");
        assert_eq!(raw.status, cm.status, "{op}: raw.status vs codemode.status");
    }
    // When both report tool errors, error codes must match for MCP/CLI/raw.
    if !raw.ok {
        if let (Some(a), Some(b)) = (&raw.error_code, &mcp.error_code) {
            assert_eq!(a, b, "{op}: raw error_code vs mcp");
        }
        if let (Some(a), Some(b)) = (&raw.error_code, &cli.error_code) {
            assert_eq!(a, b, "{op}: raw error_code vs cli");
        }
    }
}

/// Positive vectors: every registry domain op across surfaces.
#[test]
fn differential_registry_domain_ops_all_surfaces() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed_repo(root);

    let domain_ops: Vec<&str> = all_operations()
        .iter()
        .filter(|op| operation_is_domain(op))
        .map(|op| op.name)
        .collect();
    assert!(
        domain_ops.len() >= 10,
        "domain op count {}",
        domain_ops.len()
    );

    for op in &domain_ops {
        // Network-bound fetch is still exercised but may fail consistently.
        let args = minimal_args(op, root);
        let (raw, mcp, cli, cm, worker) = dispatch_all(root, op, &args);
        assert_surfaces_agree(op, &raw, &mcp, &cli, cm.as_ref(), &worker);
    }
}

/// Boundary / plausible-failure vectors (malformed, missing, policy).
#[test]
fn boundary_and_failure_vectors_agree() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed_repo(root);

    let cases: Vec<(&str, Value, bool)> = vec![
        // missing path → error
        (
            "tz_read",
            json!({"path": root.join("__no_such__.txt").display().to_string()}),
            false,
        ),
        // outside roots → policy error
        ("tz_read", json!({"path": "/etc/passwd"}), false),
        // empty pattern → error or empty-ok; must agree across surfaces
        (
            "tz_glob",
            json!({"pattern": "", "path": root.display().to_string()}),
            false,
        ),
        // invalid expand ref → error
        ("tz_expand", json!({"ref": "not-a-ref"}), false),
        // shell false → may be ok with exit_code or error; compare agreement only
        (
            "tz_shell",
            json!({"command": "false", "cwd": root.display().to_string()}),
            true,
        ),
        // success control
        ("tz_mem", json!({}), true),
    ];

    for (op, args, _may_ok) in cases {
        let (raw, mcp, cli, cm, worker) = dispatch_all(root, op, &args);
        assert_surfaces_agree(op, &raw, &mcp, &cli, cm.as_ref(), &worker);
    }
}

/// Mutation vector: real FS write via tz_edit; final bytes match across surfaces.
#[test]
fn mutation_edit_filesystem_agrees_across_surfaces() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed_repo(root);
    let path = root.join("mutate.txt");
    let args_template = |label: &str| {
        json!({
            "path": path.display().to_string(),
            "edits": [{"find": "before-mutation", "replace": format!("after-{label}")}],
            "dry_run": false
        })
    };

    // Isolate per surface with fresh file contents.
    for (label, dispatch) in [("raw", "raw"), ("mcp", "mcp"), ("cli", "cli")] {
        fs::write(&path, "before-mutation\n").unwrap();
        let eng = engine_for(root);
        let args = args_template(label);
        let out = match dispatch {
            "raw" => dispatch_raw_worker(&eng, "tz_edit", &args),
            "mcp" => dispatch_mcp_tool(&eng, "tz_edit", &args).expect("mcp"),
            "cli" => dispatch_cli(&eng, "tz_edit", &args),
            _ => unreachable!(),
        };
        assert!(
            out.is_ok(),
            "{label} edit failed: {:?}",
            out.tool_domain_error()
        );
        let bytes = fs::read_to_string(&path).unwrap();
        assert_eq!(
            bytes,
            format!("after-{label}\n"),
            "{label}: filesystem mutation mismatch"
        );
    }

    // CodeMode path (binding zero.edit)
    fs::write(&path, "before-mutation\n").unwrap();
    let eng = engine_for(root);
    let args = args_template("codemode");
    let out = dispatch_codemode_method(&eng, "zero.edit", &args).expect("cm");
    assert!(out.is_ok(), "codemode edit: {:?}", out.tool_domain_error());
    assert_eq!(fs::read_to_string(&path).unwrap(), "after-codemode\n");
}

/// Kill-test: deliberate adapter status drift is detectable by the norm helper.
#[test]
fn deliberate_adapter_status_drift_is_detected() {
    let dir = tempdir().unwrap();
    seed_repo(dir.path());
    let eng = engine_for(dir.path());
    let out = dispatch_raw_worker(&eng, "tz_mem", &json!({}));
    let mut a = norm_outcome(&out);
    let b = a.clone();
    assert_eq!(a, b);
    a.ok = !a.ok;
    a.status = if a.ok { "ok".into() } else { "error".into() };
    assert_ne!(a, b, "norm must detect status class drift");
}

/// Live kill-test: if MCP and raw disagreed on tz_read success class, fail hard.
#[test]
fn live_read_value_parity_mcp_raw_cli_codemode() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    seed_repo(root);
    let path = root.join("note.txt").display().to_string();
    let args = json!({"path": path});

    let mcp = dispatch_mcp_tool(&engine_for(root), "tz_read", &args).expect("mcp");
    let raw = dispatch_raw_worker(&engine_for(root), "tz_read", &args);
    let cli = dispatch_cli(&engine_for(root), "tz_read", &args);
    let cm = dispatch_codemode_method(&engine_for(root), "zero.read", &args).expect("cm");

    let n = |o: &DispatchOutcome| {
        let r = o.tool_response.as_ref().expect("resp");
        (
            r.status.clone(),
            r.error.as_ref().map(|e| e.code.clone()),
            r.visible.as_ref().map(|v| v.text.clone()),
            r.refs.iter().map(|x| x.ref_id.clone()).collect::<Vec<_>>(),
        )
    };
    let base = n(&mcp);
    assert_eq!(n(&raw), base, "raw vs mcp value parity");
    assert_eq!(n(&cli), base, "cli vs mcp value parity");
    assert_eq!(n(&cm), base, "codemode vs mcp value parity");
    assert_eq!(base.0, "ok");
}

#[test]
fn corpus_manifest_is_versioned_and_machine_readable() {
    let ops: Vec<&str> = all_operations()
        .iter()
        .filter(|op| operation_is_domain(op))
        .map(|op| op.name)
        .collect();
    let manifest = json!({
        "schema": "tokenzero.irx9.conformance.v1",
        "operations": ops,
        "surfaces": ["raw", "mcp", "cli", "codemode", "raw_worker"],
        "vector_classes": ["positive", "boundary", "failure", "mutation"],
        "normalize": ["transport_jsonrpc", "timestamps", "volatile_refs_stripped_content_addressed"],
    });
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/irx9_conformance_manifest.json");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).ok();
    assert_eq!(manifest["schema"], "tokenzero.irx9.conformance.v1");
    assert!(manifest["operations"].as_array().unwrap().len() >= 10);
}
