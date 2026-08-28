//! Shared packaging CLI for exclusive surface binaries (`fszero-mcp` / `fszero-codemode`).
//!
//! Both artifacts share install/uninstall/doctor/sbom/catalog/raw-worker paths;
//! only the locked surface, help text, catalog function, and server entry differ.

use super::{
    COMMON_FLAGS, PackageSurface, SHIM_COMMANDS, args_has, assert_surface_compiled,
    capabilities_document, did_you_mean_suffix, die1, die2, exit_install_result,
    exit_uninstall_from_args, install_dry_run_document, install_surface, layout_document,
    package_identity, parse_binary_flag, parse_prefix_flag, print_version_line, sbom_document,
    semantic_contract_digest, version_flag_requested,
};
use crate::mcp_protocol::{
    SurfaceKind, raw_worker_requested, run_raw_worker_stdio, run_stdio_server,
};
use crate::mcp_rpc::{install_explicit_root, parse_root_flag, resolve_cli_root};
use crate::{DispatchSurface, FSZeroSession, dispatch_operation};
use crate::{doctor_diagnostics, doctor_root_report, doctor_smoke_plan};
use std::io::{self, Read};
use std::process;

/// Run the exclusive surface binary. Never returns (exits the process).
///
/// Server entry, env surface, and forbidden `--mode=` are derived from `surface`.
/// The MCP server callback defaults to the root library's always-fail stub; the
/// `fszero-mcp` package passes its hub `zero-codemode/fastmcp` adapter via
/// [`run_surface_bin_with_server`] (fszero-xg53).
pub fn run_surface_bin(
    surface: PackageSurface,
    args: &[String],
    help_blurb: &str,
    catalog: fn() -> Vec<serde_json::Value>,
) -> ! {
    run_surface_bin_with_server(
        surface,
        args,
        help_blurb,
        catalog,
        crate::mcp_protocol::run_fastmcp_server,
    )
}

/// Same as [`run_surface_bin`] with a package-owned MCP server callback.
///
/// The root library no longer links a FastMCP transport; the `fszero-mcp`
/// package owns the hub transport adapter and injects it here.
pub fn run_surface_bin_with_server(
    surface: PackageSurface,
    args: &[String],
    help_blurb: &str,
    catalog: fn() -> Vec<serde_json::Value>,
    server: fn() -> Result<(), String>,
) -> ! {
    super::install_stdout_pipe_safety();
    if let Some(root) = parse_root_flag(args) {
        if let Err(e) = install_explicit_root(&root) {
            die2(format!("{}: bad --root: {e}", surface.artifact_name()));
        }
    }

    if args_has(args, &["help", "--help", "-h"]) {
        print_help(surface, help_blurb);
        exit0();
    }
    if version_flag_requested(args) {
        print_version_line();
        exit0();
    }
    if args_has(args, &["capabilities"]) {
        let doc = capabilities_document(Some(surface));
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
        exit0();
    }
    if args_has(args, &["sbom"]) {
        println!(
            "{}",
            serde_json::to_string_pretty(&sbom_document(surface)).unwrap()
        );
        exit0();
    }
    if args_has(args, &["install"]) {
        run_install(surface, args);
    }
    if args_has(args, &["uninstall"]) {
        exit_uninstall_from_args(args);
    }
    if args_has(args, &["doctor", "--doctor"]) {
        if let Err(e) = assert_surface_compiled(surface) {
            die2(e);
        }
        run_doctor_command(surface, args);
    }
    if args_has(args, &["catalog", "tools"]) {
        println!("{}", serde_json::to_string_pretty(&catalog()).unwrap());
        exit0();
    }
    if args_has(args, &["layout"]) {
        let root = resolve_cli_root(args);
        println!(
            "{}",
            serde_json::to_string_pretty(&layout_document(&root)).unwrap()
        );
        exit0();
    }
    if args_has(args, &["batch"]) {
        run_batch_command(args);
    }

    let forbidden = surface.forbidden_mode_flag();
    if args_has(args, &[forbidden]) {
        die2(format!(
            "{}: artifact is locked to surface '{}'; refused {forbidden}. \
Install {} for the other catalog (mutually exclusive).",
            surface.artifact_name(),
            surface.as_str(),
            surface.other().artifact_name()
        ));
    }

    reject_unsupported_args(surface, args);

    if let Err(e) = assert_surface_compiled(surface) {
        die2(e);
    }

    if raw_worker_requested(args) {
        if let Err(e) = run_raw_worker_stdio(args) {
            die_art(surface, format!("raw-worker: {e}"));
        }
        exit0();
    }

    let err = match surface {
        PackageSurface::Mcp => server().err().map(|e| e.to_string()),
        PackageSurface::Codemode => run_stdio_server(SurfaceKind::CodeMode)
            .err()
            .map(|e| e.to_string()),
    };
    if let Some(e) = err {
        die_art(surface, e);
    }
    exit0();
}

/// Map MCP snake / bare aliases onto canonical CodeMode `fs.*Many` ops.
///
/// R-PAR-REC-005 / fszero-2qdw.11: agents paste `fszero.multi_list` / `multi_list`
/// into `fszero batch` because those names appear in the MCP catalog. Accept
/// them additively; response `operation` is always the canonical camel form.
pub fn normalize_batch_operation(raw: &str) -> Option<&'static str> {
    match raw {
        "fs.multiRead" | "fs.multi_read" | "multi_read" | "fszero.multi_read" => Some("fs.multiRead"),
        "fs.multiStat" | "fs.multi_stat" | "multi_stat" | "fszero.multi_stat" => Some("fs.multiStat"),
        "fs.multiSearch" | "fs.multi_search" | "multi_search" | "fszero.multi_search" => {
            Some("fs.multiSearch")
        }
        "fs.multiList" | "fs.multi_list" | "multi_list" | "fszero.multi_list" => Some("fs.multiList"),
        "fs.multiAstSearch"
        | "fs.multi_ast_search"
        | "multi_ast_search"
        | "fszero.multi_ast_search" => Some("fs.multiAstSearch"),
        _ => None,
    }
}

/// Execute one vectorized many-op plan from a JSON file or stdin ("-").
/// Accepted envelope: {"operation"|"op"|"call": "fs.*Many"|snake aliases, "args": {...}}.
pub fn run_batch_command(args: &[String]) -> ! {
    let Some(position) = args.iter().position(|arg| arg == "batch") else {
        die2("batch command missing");
    };
    let source = args.get(position + 1).map(String::as_str).unwrap_or("-");
    let text = if source == "-" {
        let mut text = String::new();
        if let Err(error) = io::stdin().read_to_string(&mut text) {
            die2(format!("batch stdin: {error}"));
        }
        text
    } else {
        match std::fs::read_to_string(source) {
            Ok(text) => text,
            Err(error) => die2(format!("batch plan {source}: {error}")),
        }
    };
    let plan: serde_json::Value = match serde_json::from_str(&text) {
        Ok(plan) => plan,
        Err(error) => die2(format!("batch plan JSON: {error}")),
    };
    let raw_operation = plan
        .get("operation")
        .or_else(|| plan.get("op"))
        .or_else(|| plan.get("call"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let Some(operation) = normalize_batch_operation(raw_operation) else {
        die2(format!(
            "batch operation must be fs.multiRead, fs.multiStat, fs.multiSearch, fs.multiList, or fs.multiAstSearch \
(or MCP snake aliases fszero.multi_read / multi_list / …) (got {raw_operation:?})"
        ));
    };
    let op_args = plan
        .get("args")
        .or_else(|| plan.get("arguments"))
        .unwrap_or(&plan);
    let root = resolve_cli_root(args);
    let mut session = match FSZeroSession::try_with_repo_store(&root) {
        Ok(session) => session,
        Err(error) => die2(format!("batch durable store open failed: {error}")),
    };
    let outcome = dispatch_operation(&mut session, DispatchSurface::Cli, operation, op_args);
    let rows = outcome
        .recovery_key
        .as_deref()
        .and_then(|key| session.expand(key))
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let document = serde_json::json!({
        "operation": operation, "ok": outcome.result.ok, "ack": outcome.result.ack,
        "ref": outcome.recovery_key, "stats": outcome.result.value, "results": rows,
        "error": outcome.result.error,
    });
    println!("{}", serde_json::to_string_pretty(&document).unwrap());
    process::exit(if outcome.result.ok { 0 } else { 1 });
}

/// First argv element this artifact does not implement, if any.
///
/// Reached only after every implemented subcommand has been matched, so
/// anything left is a typo. `--root` (and its `=` form) is the one flag the
/// bare server accepts; `--raw-worker` selects the private worker protocol.
pub fn unsupported_arg(args: &[String]) -> Option<&str> {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--raw-worker" | "--mode=raw-worker" => {}
            "--root" => {
                rest.next();
            }
            other if other.starts_with("--root=") => {}
            other => return Some(other),
        }
    }
    None
}

/// Every argv this artifact does not implement exits nonzero with the usage
/// line (fszero-9k60). Unknown args used to fall through to the stdio server,
/// which sees EOF on a non-tty stdin and exits 0 with no output — so a typo
/// like the shim's `codemode <plan>` form looked like a successful run.
fn reject_unsupported_args(surface: PackageSurface, args: &[String]) {
    let Some(bad) = unsupported_arg(args) else {
        return;
    };
    let name = surface.artifact_name();
    let mut candidates: Vec<&str> = SHIM_COMMANDS.to_vec();
    candidates.extend_from_slice(COMMON_FLAGS);
    let hint = did_you_mean_suffix(bad, &candidates);
    die2(format!(
        "{name}: unsupported argument {bad:?}{hint}\n\
usage: {name} | {name} doctor [--json] | {name} capabilities [--json] | {name} layout [--json] | {name} sbom | {name} install|uninstall | {name} catalog | {name} batch [plan.json|-]"
    ));
}

#[inline]
fn die_art(surface: PackageSurface, detail: impl std::fmt::Display) -> ! {
    die1(format!("{}: {detail}", surface.artifact_name()));
}

#[inline]
fn exit0() -> ! {
    process::exit(0);
}

fn print_help(surface: PackageSurface, blurb: &str) {
    let id = package_identity(surface);
    println!(
        "{name} — {blurb}\n\
         semantic_contract_digest: {digest}\n\
         usage:\n\
           {name}                         (stdio server)\n\
           {name} doctor [--root PATH] [--json]\n\
           {name} capabilities [--json]\n\
           {name} layout [--json] [--root PATH]\n\
           {name} sbom\n\
           {name} install [--prefix DIR] [--binary PATH] [--dry-run]\n\
           {name} uninstall [--prefix DIR] (--dry-run | --yes)\n\
           {name} catalog | tools\n\
           {name} batch [plan.json|-] [--root PATH]\n\
           {name} --raw-worker            (private worker protocol)\n\
         \n\
         Robot: doctor --json · capabilities [--json] · layout · --version/-V · sbom · catalog · batch\n\
         Exit codes: 0 ok · 1 runtime (die1) · 2 usage/argv (die2)\n\
         Env: FSZERO_ROOT, FSZERO_PACKAGE_SURFACE, ZEROSTACK_STORE_ROOT (+ shared-store opt-in)\n\
         identity: {id}",
        name = surface.artifact_name(),
        blurb = blurb,
        digest = semantic_contract_digest(),
        id = id
    );
}

fn run_install(surface: PackageSurface, args: &[String]) {
    if let Some(requested) = super::flag_value(args, &["--surface"]) {
        if PackageSurface::parse(requested).ok() != Some(surface) {
            die2(format!(
                "{}: install --surface must be '{}' for this artifact (got {requested:?})",
                surface.artifact_name(),
                surface.as_str()
            ));
        }
    }
    let prefix = parse_prefix_flag(args);
    let binary = parse_binary_flag(args, surface.artifact_name());
    if args_has(args, &["--dry-run"]) {
        let plan = install_dry_run_document(surface, &prefix, &binary);
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        exit0();
    }
    exit_install_result(install_surface(surface, &prefix, &binary))
}

/// Build the one stable `doctor --json` document used by every binary.
/// JSON mode emits only this object on stdout; human mode prints report rows.
fn doctor_json_document(
    identity: &serde_json::Value,
    workspace_root: &str,
    diagnostics: &crate::DoctorReport,
    smoke: Option<Result<&String, &String>>,
) -> serde_json::Value {
    let (smoke_ok, smoke_code, ack, error): (bool, &str, Option<&str>, Option<&str>) = match smoke {
        Some(Ok(ack)) => (true, "FSZ-DOC-SMOKE-OK", Some(ack.as_str()), None),
        Some(Err(error)) => (false, "FSZ-DOC-SMOKE-001", None, Some(error.as_str())),
        None => (false, "FSZ-DOC-SMOKE-SKIPPED", None, None),
    };
    serde_json::json!({
        "schema": crate::DOCTOR_SCHEMA,
        "ok": diagnostics.ok && smoke_ok,
        "package": identity,
        "workspace_root": workspace_root,
        "diagnostics": diagnostics.diagnostics,
        "smoke": {"ok": smoke_ok, "code": smoke_code, "ack": ack, "error": error},
    })
}

/// Shared shim/surface doctor runner. Both entry points use this exact JSON and
/// human-output contract; dedicated surfaces validate their feature before entry.
pub fn run_doctor_command(surface: PackageSurface, args: &[String]) -> ! {
    let root = resolve_cli_root(args);
    let diagnostics = doctor_diagnostics(&root);
    let smoke = diagnostics.ok.then(|| doctor_smoke_plan(&root));
    let smoke_ok = smoke.as_ref().is_some_and(|result| result.is_ok());
    let ok = diagnostics.ok && smoke_ok;
    let report = diagnostics.ok.then(|| doctor_root_report(&root));
    let workspace_root = report
        .as_ref()
        .and_then(|value| value.get("workspace_root"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| root.display().to_string());
    let identity = package_identity(surface);

    if args_has(args, &["--json"]) || args_has(args, &["--jsno"]) {
        if args_has(args, &["--jsno"]) && !args_has(args, &["--json"]) {
            eprintln!("fszero doctor: treating --jsno as --json (did-you-mean)");
        }
        let document = doctor_json_document(
            &identity,
            &workspace_root,
            &diagnostics,
            smoke.as_ref().map(Result::as_ref),
        );
        println!("{}", serde_json::to_string_pretty(&document).unwrap());
        process::exit(if ok { 0 } else { 1 });
    }

    for row in &diagnostics.diagnostics {
        eprintln!(
            "doctor: code={} severity={:?} subsystem={} remediation={}",
            row.code, row.severity, row.subsystem, row.remediation
        );
    }
    if !diagnostics.ok {
        process::exit(1);
    }

    println!(
        "package: artifact={} surface={} semantic_contract_digest={} abi_digest={} version={}",
        identity["artifact"].as_str().unwrap_or("?"),
        identity["surface"].as_str().unwrap_or("?"),
        identity["semantic_contract_digest"].as_str().unwrap_or("?"),
        identity["abi_digest"].as_str().unwrap_or("?"),
        identity["package_version"].as_str().unwrap_or("?"),
    );

    let report = report.expect("diagnostics-ok doctor report");
    match smoke.expect("diagnostics-ok smoke") {
        Ok(ack) => {
            println!("doctor: ok ack={ack} code=FSZ-DOC-SMOKE-OK");
            print_capabilities(&report);
            print_root_report(&report);
            process::exit(0);
        }
        Err(_) => {
            eprintln!("doctor: FAIL code=FSZ-DOC-SMOKE-001");
            print_capabilities(&report);
            print_root_report(&report);
            process::exit(1);
        }
    }
}

/// Print the machine-readable capability descriptor plus remediation without
/// absolute private paths.
fn print_capabilities(report: &serde_json::Value) {
    let Some(capabilities) = report.get("capabilities") else {
        return;
    };
    println!("capabilities: {capabilities}");
    if let Some(remediation) = capabilities
        .get("remediation")
        .and_then(|value| value.as_array())
    {
        for note in remediation.iter().filter_map(|value| value.as_str()) {
            println!("remediation: {note}");
        }
    }
}

fn print_root_report(report: &serde_json::Value) {
    // fszero-52by / child-root: multi-project misconfig must show both roots
    // without a debugger (workspace = FS ops; store = durable metadata).
    if let Some(ws) = report
        .get("workspace_root")
        .and_then(|value| value.as_str())
    {
        println!("workspace_root={ws}");
    }
    if let Some(sr) = report.get("store_root").and_then(|value| value.as_str()) {
        println!("store_root={sr}");
    }
    if let Some(mode) = report
        .get("effective_root_mode")
        .and_then(|value| value.as_str())
    {
        println!("root_mode: {mode}");
    }
    if let Some(version) = report
        .get("layout_version")
        .and_then(|value| value.as_str())
    {
        println!("layout_version: {version}");
    }
    if let Some(health) = report.get("store_health") {
        let durable = health
            .get("durable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let cas_attached = health
            .get("cas_attached")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let cas_writable = health
            .get("cas_writable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let violations = health
            .get("integrity_violations")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        println!(
            "store_health: durable={durable} cas_attached={cas_attached} cas_writable={cas_writable} integrity_violations={violations}"
        );
    }
    if let Some(health) = report.get("fz_runtime_health") {
        let consecutive = health
            .get("consecutive_failures")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let fail_open = health
            .get("fail_open")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let native_fallback = health
            .get("native_fallback")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        println!(
            "fz_runtime_health: consecutive_failures={consecutive} fail_open={fail_open} native_fallback={native_fallback}"
        );
    }
    if let Some(migration) = report.get("migration_legacy") {
        let rows = migration
            .get("legacy_blob_rows")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let available = migration
            .get("migration_available")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        println!("migration_legacy: legacy_blob_rows={rows} migration_available={available}");
    }
    if let Some(peer) = report.get("peer_incompatibility") {
        let note = peer
            .get("note")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if !note.is_empty() {
            println!("peer_incompatibility: {note}");
        }
    }
}
