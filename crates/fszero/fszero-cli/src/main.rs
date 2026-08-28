//! fszero — installer / re-exec **shim** only (fszero-ncib.3 process exclusivity).
//!
//! This binary must **not** contain both (or either) package surface runtimes.
//! Production installs replace the `fszero` name with a symlink to the selected
//! artifact (`fszero-mcp` or `fszero-codemode`). When this crate binary is run
//! as the packaging shim, server modes re-exec the selected surface binary.
//!
//! Build packages with one explicit SQLite link mode:
//!   cargo build -p fszero-cli
//!   cargo build -p fszero-mcp
//!   cargo build -p fszero-worker --bin fszero-codemode
//! Hosts without system SQLite use:
//!   cargo build -p <package> --no-default-features --features sqlite-bundled

#[cfg(any(
    all(feature = "sqlite-system", feature = "sqlite-bundled"),
    not(any(feature = "sqlite-system", feature = "sqlite-bundled"))
))]
compile_error!(
    "select exactly one SQLite link mode: default sqlite-system, or --no-default-features --features sqlite-bundled"
);

// Prefer building the shim without surface features (production install.sh).
// Cargo test may enable a surface feature for lib/integration tests (fszero-q3zy /
// oppj); refuse to *run* as a dual-purpose server here (fail closed at process start).
// A compile_error here would block `cargo test --features surface-codemode --lib`.
use fs_zero::packaging::{
    MUTATION_PLAN_SCHEMA, completion_script, install_dry_run_document, mutation_confirmation_error,
};
use fs_zero::{
    ALLOW_BARE_SERVER_ENV, COMMON_FLAGS, PackageSurface, SHIM_COMMANDS, SubstrateChildConfig,
    SubstrateDown, SupervisedChild, args_has, capabilities_document, did_you_mean_suffix, die1,
    die2, exit_install_result, exit_uninstall_from_args, fszero_store_sqlite_path,
    install_explicit_root, install_surface, layout_document, package_identity, parse_binary_flag,
    parse_prefix_flag, parse_root_flag, parse_surface_flag, print_version_line, resolve_cli_root,
    resolve_startup_surface, resolve_surface_binary, robot_triage_document, run_batch_command,
    run_raw_worker_stdio, sbom_document, semantic_contract_digest, shim_should_start_server,
    version_flag_requested,
};
use serde_json::json;
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn main() {
    // R-IDEA-008: piped head must not SIGPIPE-kill or panic the agent CLI.
    fs_zero::packaging::install_stdout_pipe_safety();
    if cfg!(any(feature = "surface-mcp", feature = "surface-codemode")) {
        die2(
            "fszero-ncib.3: the fszero binary is an installer/re-exec shim only and must not be \
run when built with surface-mcp or surface-codemode. Use fszero-mcp / fszero-codemode, or \
rebuild with default features only.",
        );
    }

    let args: Vec<String> = env::args().collect();
    let doctor_requested = args_has(&args, &["doctor", "--doctor"]);
    // Most commands require a canonical root before dispatch. Doctor must see
    // invalid roots itself so it can emit a stable machine-readable diagnosis.
    if !doctor_requested {
        if let Some(root) = parse_root_flag(&args) {
            if let Err(e) = install_explicit_root(&root) {
                die2(format!("fszero: bad --root: {e}"));
            }
        }
    }
    match help_route(&args) {
        HelpRoute::Global => {
            print_help();
            process::exit(0);
        }
        HelpRoute::Verb(verb) => {
            print!("{}", verb_help_text(&verb));
            process::exit(0);
        }
        HelpRoute::None => {}
    }
    if version_flag_requested(&args) {
        print_version_line();
        process::exit(0);
    }
    if args_has(&args, &["capabilities"]) {
        run_capabilities(&args);
        return;
    }
    if args_has(&args, &["completions"]) {
        run_completions(&args);
        return;
    }
    if args_has(&args, &["robot-triage", "--robot-triage"]) {
        run_robot_triage(&args);
        return;
    }
    if args_has(&args, &["robot-docs", "--robot-help"]) {
        run_robot_docs(&args);
        return;
    }
    if shim_catalog_requested(&args) {
        reexec_selected_surface(&args);
    }
    if args_has(&args, &["install"]) {
        run_install(&args);
        return;
    }
    if args_has(&args, &["uninstall"]) {
        exit_uninstall_from_args(&args);
    }
    if args_has(&args, &["sbom"]) {
        run_sbom(&args);
        return;
    }
    if doctor_requested {
        run_doctor(&args);
        return;
    }
    if args_has(&args, &["layout"]) {
        run_layout(&args);
        return;
    }
    if args_has(&args, &["migrate-cas"]) {
        run_migrate_cas(&args);
        return;
    }
    if args_has(&args, &["store-gc"]) {
        run_store_gc(&args);
        return;
    }
    if args_has(&args, &["telemetry"]) {
        run_telemetry(&args);
        return;
    }
    if args_has(&args, &["zeroref-fixture"]) {
        run_zeroref_fixture(&args);
        return;
    }
    if args_has(&args, &["batch"]) {
        run_batch_command(&args);
    }

    // Parent supervisor lives in the shim (process management only — no tool catalog).
    if args_has(&args, &["--supervise"]) {
        run_supervised(&args);
        return;
    }

    // Private worker protocol: same NDJSON loop as surface bins. Must run
    // before `shim_should_start_server`, which treats any `--mode=` token as a
    // stdio-server re-exec.
    if args
        .iter()
        .any(|a| a == "--raw-worker" || a == "--mode=raw-worker")
    {
        if let Err(e) = run_raw_worker_stdio(&args) {
            die1(format!("raw-worker: {e}"));
        }
        process::exit(0);
    }

    // MCP / CodeMode server: re-exec the selected surface artifact — never host
    // FastMCP or CodeMode catalogs in this packaging-shim process.
    // Bare `fszero` prints help (R-004): agents must not hang on stdio. Opt into
    // legacy bare-server with FSZERO_ALLOW_BARE_SERVER, or pass serve/--mode=.
    if shim_should_start_server(&args) {
        reexec_selected_surface(&args);
    }
    if args.len() == 1 {
        print_help();
        eprintln!(
            "fszero: bare invocation no longer starts a stdio server (agents hang). \
Use `fszero serve` or `fszero --mode=codemode|mcp`, or set {ALLOW_BARE_SERVER_ENV}=1 for legacy."
        );
        process::exit(0);
    }
    run_codemode_cli(args);
}

fn shim_catalog_requested(args: &[String]) -> bool {
    matches!(args.get(1).map(String::as_str), Some("catalog" | "tools"))
}

/// Re-exec the immutably selected surface artifact (fszero-mcp | fszero-codemode).
fn reexec_selected_surface(args: &[String]) -> ! {
    let (_surface, bin) = match resolve_surface_binary(args) {
        Ok(v) => v,
        Err(e) => die2(e),
    };
    // Drop argv0; strip shim-only serve tokens so the surface sees a bare/server start.
    let child_args: Vec<&str> = args
        .iter()
        .skip(1)
        .filter(|a| a.as_str() != "serve" && a.as_str() != "--serve")
        .map(String::as_str)
        .collect();
    fs_zero::record_process_start();
    let err = process::Command::new(&bin).args(&child_args).exec();
    die2(format!("fszero: re-exec {} failed: {err}", bin.display()));
}

#[derive(Debug, PartialEq, Eq)]
enum HelpRoute {
    None,
    Global,
    Verb(String),
}

/// Help belongs to the verb unless argv[1] is itself a help flag.
fn help_route(args: &[String]) -> HelpRoute {
    fn is_help(s: &str) -> bool {
        matches!(s, "help" | "--help" | "-h")
    }
    match args.get(1) {
        None => HelpRoute::None,
        Some(first) if is_help(first) => HelpRoute::Global,
        Some(first) => {
            if args[2..].iter().any(|a| is_help(a)) {
                HelpRoute::Verb(first.clone())
            } else {
                HelpRoute::None
            }
        }
    }
}

fn verb_help_text(verb: &str) -> String {
    let usage = match verb {
        "completions" => "fszero completions bash|zsh|fish",
        "install" => {
            "fszero install [--surface mcp|codemode] [--prefix DIR] [--binary PATH] [--dry-run]"
        }
        "uninstall" => "fszero uninstall [--prefix DIR] (--dry-run | --yes)",
        "migrate-cas" => "fszero migrate-cas [--root PATH] (--dry-run | --yes)",
        "store-gc" => "fszero store-gc [--root PATH] (--dry-run | --yes) [--max-bytes N]",
        "layout" => "fszero layout [--json] [--root PATH]",
        _ => {
            return format!(
                "fszero {verb} \u{2014} verb-local help\n\nUSAGE: fszero {verb} [FLAGS]\nMachine-readable contract: fszero capabilities\nGlobal help: fszero --help\n"
            );
        }
    };
    format!(
        "fszero {verb} \u{2014} verb-local help\n\nUSAGE: {usage}\nSafety: --dry-run prints a JSON plan without mutation; --yes explicitly authorizes destructive actions.\nMachine-readable contract: fszero capabilities\nGlobal help: fszero --help\n"
    )
}

fn print_help() {
    let surface = resolve_startup_surface(&env::args().collect::<Vec<_>>()).ok();
    let digest = semantic_contract_digest();
    println!(
        "fszero — installer / re-exec shim (never embeds both surface runtimes)\n\
         \n\
         Artifacts: fszero-mcp | fszero-codemode | fszero (shim or symlink to selected)\n\
         Selection is immutable until reinstall. Dual surface features fail at compile time.\n\
         Selection matrix (CodeMode-first): fresh installs -> fszero-codemode (default); legacy MCP -> fszero-mcp (--surface mcp)\n\
         Semantic contract digest: {digest}\n"
    );
    if let Some(s) = surface {
        let id = package_identity(s);
        println!(
            "resolved_surface: {} artifact={}\n",
            s.as_str(),
            id["artifact"].as_str().unwrap_or("?")
        );
    }
    println!(
        "usage:\n\
           fszero help | --help | -h\n\
           fszero install [--surface mcp|codemode] [mcp|codemode] [--prefix DIR] [--binary PATH] [--dry-run]\n\
           fszero uninstall [--prefix DIR] (--dry-run | --yes)\n\
           fszero sbom [--surface mcp|codemode]\n\
           fszero --version | -V\n\
           fszero doctor [--root PATH] [--json]\n\
           fszero capabilities [--json]\n\
           fszero layout [--json] [--root PATH]\n\
           fszero completions bash|zsh|fish\n\
           fszero catalog | tools  (re-exec selected surface; exact JSON)\n\
           fszero robot-triage [--json]\n\
           fszero robot-docs [--json]\n\
           fszero serve | fszero --mode=mcp|codemode  (stdio server via selected surface)\n\
           fszero --supervise …                 (parent supervisor; re-execs surface)\n\
           fszero --raw-worker | --mode=raw-worker  (private worker protocol)\n\
           fszero batch <plan.json|-> [--root PATH]\n\
           fszero migrate-cas [--root PATH] (--dry-run | --yes)\n\
           fszero store-gc [--root PATH] (--dry-run | --yes) [--max-bytes N]\n\
           fszero telemetry inspect|dry-run [--telemetry|--no-telemetry] [--root PATH]\n\
           fszero zeroref-fixture …\n\
         \n\
         Robot / machine surfaces:\n\
           doctor --json | capabilities [--json] | layout | catalog | tools | robot-triage | robot-docs | sbom | batch | telemetry inspect\n\
         \n\
         Exit codes: 0 ok · 1 runtime/operational (die1) · 2 usage/contract/argv (die2)\n\
         Env (common): FSZERO_ROOT, FSZERO_INSTALL_PREFIX, FSZERO_PACKAGE_SURFACE,\n\
           FSZERO_SHARED_STORE, ZEROSTACK_STORE_ROOT, {ALLOW_BARE_SERVER_ENV}\n\
         Bare `fszero` prints this help (no hang). Legacy bare-server: {ALLOW_BARE_SERVER_ENV}=1.\n\
         Process exclusivity: one artifact, one catalog, no dual defaults.\n\
         Full dictionaries: `fszero capabilities --json`."
    );
}

fn run_completions(args: &[String]) {
    // fszero completions <bash|zsh|fish>
    let shell = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-') && a.as_str() != "completions");
    let Some(shell) = shell else {
        die1("usage: fszero completions bash|zsh|fish");
    };
    match completion_script(shell) {
        Ok(script) => {
            print!("{script}");
            process::exit(0);
        }
        Err(e) => die1(e),
    }
}

fn run_capabilities(args: &[String]) {
    let surface = resolve_startup_surface(args).ok();
    let doc = capabilities_document(surface);
    // Robot-default: always JSON (R-001). Human prose is not a capabilities surface.
    let _ = args_has(args, &["--json"]); // accepted; JSON is the only encoding
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    process::exit(0);
}

fn run_robot_triage(args: &[String]) {
    let surface = resolve_startup_surface(args).ok();
    let doc = robot_triage_document(surface);
    // Robot-default: always JSON (R-001). Human prose is not a capabilities surface.
    let _ = args_has(args, &["--json"]); // accepted; JSON is the only encoding
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    process::exit(0);
}

fn run_robot_docs(_args: &[String]) {
    println!("{}", fs_zero::robot_docs_guide());
}

fn run_install(args: &[String]) {
    let surface = match parse_surface_flag(args) {
        Ok(Some(s)) => s,
        Ok(None) => {
            eprintln!(
                "fszero install: defaulting to --surface codemode (canonical FSZero surface)"
            );
            PackageSurface::Codemode
        }
        Err(e) => die2(e),
    };
    let prefix = parse_prefix_flag(args);
    let binary = parse_binary_flag(args, "fszero");
    if args_has(args, &["--dry-run"]) {
        let plan = install_dry_run_document(surface, &prefix, &binary);
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        process::exit(0);
    }
    exit_install_result(install_surface(surface, &prefix, &binary))
}

fn run_sbom(args: &[String]) {
    // R-015 / fszero-x7n7.3: same default path as install (CodeMode-first via
    // resolve_startup_surface); never silent Mcp when unset.
    let surface = match parse_surface_flag(args) {
        Ok(Some(s)) => s,
        Ok(None) => resolve_startup_surface(args).unwrap_or(PackageSurface::Codemode),
        Err(e) => die2(e),
    };
    let mut doc = sbom_document(surface);
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("surface".into(), serde_json::json!(surface.as_str()));
        obj.insert(
            "surface_source".into(),
            serde_json::json!(if parse_surface_flag(args).ok().flatten().is_some() {
                "flag"
            } else {
                "resolve_startup_surface"
            }),
        );
    }
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    process::exit(0);
}

fn run_doctor(args: &[String]) {
    // F-CLI-012 / F-CLI-088: unresolved surface is CodeMode-first, matching
    // install and sbom. Never silent Mcp when install-state / env / --mode=
    // cannot resolve a surface on the packaging shim.
    let surface = resolve_startup_surface(args).unwrap_or(PackageSurface::Codemode);
    fs_zero::packaging::surface_bin::run_doctor_command(surface, args)
}

/// Read-only workspace/store inventory (F-SURF-102). Always JSON; `--json` is accepted.
fn run_layout(args: &[String]) {
    let _ = args_has(args, &["--json"]);
    let root = resolve_cli_root(args);
    println!(
        "{}",
        serde_json::to_string_pretty(&layout_document(&root)).unwrap()
    );
    process::exit(0);
}

/// Explicit legacy→CAS migration trigger (fszero-c6q.3), same root
/// resolution as `doctor`. Output is counts only — never blob contents,
/// never private absolute paths; the per-object detail lives in the store
/// manifest under `cas/migration`.
fn run_migrate_cas(args: &[String]) {
    let root = resolve_cli_root(args);
    if args_has(args, &["--dry-run"]) {
        let plan = json!({
            "schema": MUTATION_PLAN_SCHEMA, "action": "migrate-cas", "dry_run": true,
            "would_mutate": true, "requires_yes": true, "root": root.display().to_string(),
            "condition": "verified legacy fz://blob rows are present",
            "effects": ["scan legacy blob rows", "publish verified bytes into the canonical CAS", "record the cas/migration manifest"],
        });
        println!("{}", serde_json::to_string_pretty(&plan).unwrap());
        process::exit(0);
    }
    if !args_has(args, &["--yes"]) {
        die2(mutation_confirmation_error("migrate-cas"));
    }
    let mut sess = fs_zero::FSZeroSession::with_repo_store(&root);
    match sess.migrate_blobs_to_cas() {
        Ok(r) => {
            println!(
                "migrate-cas: ok migrated={} already={} skipped_nonblob={} corrupt={} missing={}",
                r.migrated, r.already, r.skipped_nonblob, r.corrupt, r.missing
            );
            process::exit(0);
        }
        Err(e) => die1(format!("migrate-cas: FAIL {e}")),
    }
}

/// One-shot forensic/salvage sibling retention (fszero-o9wx).
///
/// `--dry-run` prints a structured JSON plan and deletes nothing; mutation
/// requires `--yes`. Only `<store>.forensic-*` / `<store>.salvage-*` sibling
/// directories next to the resolved store are ever removed; the live store,
/// AST index, packs, and unrelated files are untouched.
fn run_store_gc(args: &[String]) {
    let root = resolve_cli_root(args);
    let flags = match parse_store_gc_flags(&args[2..]) {
        Ok(flags) => flags,
        Err(error) => die2(format!("store-gc: {error}")),
    };
    let budget = flags
        .budget
        .unwrap_or_else(fs_zero::snapshot_retention_budget);
    let store = fszero_store_sqlite_path(&root);
    let plan = if flags.dry_run {
        fs_zero::store_gc_plan(&store, budget)
    } else {
        if !flags.confirmed {
            die2(mutation_confirmation_error("store-gc"));
        }
        fs_zero::store_gc_apply(&store, budget)
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => die1(format!("store-gc: {error}")),
    };
    let doc = json!({
        "schema": "fszero-store-gc/v1", "action": "store-gc",
        "dry_run": flags.dry_run, "would_mutate": !plan.delete.is_empty(),
        "requires_yes": true, "root": root.display().to_string(),
        "plan": plan,
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    process::exit(0);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreGcCli {
    dry_run: bool,
    confirmed: bool,
    budget: Option<u64>,
}

/// Strict flag parse for `store-gc`: any unknown flag or malformed byte value
/// is a usage error (exit 2), never silently ignored.
fn parse_store_gc_flags(args: &[String]) -> Result<StoreGcCli, String> {
    let mut out = StoreGcCli {
        dry_run: false,
        confirmed: false,
        budget: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => out.dry_run = true,
            "--yes" => out.confirmed = true,
            "--root" => {
                i += 1;
                if i >= args.len() {
                    return Err("--root requires a path".to_string());
                }
            }
            flag if flag.starts_with("--root=") => {}
            "--max-bytes" => {
                i += 1;
                if i >= args.len() {
                    return Err("--max-bytes requires a byte value".to_string());
                }
                out.budget = Some(parse_byte_budget(&args[i])?);
            }
            flag if flag.starts_with("--max-bytes=") => {
                out.budget = Some(parse_byte_budget(&flag["--max-bytes=".len()..])?);
            }
            other => {
                return Err(format!(
                    "unknown flag {other:?}; store-gc accepts --dry-run, --yes, --max-bytes N, --root PATH"
                ));
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Byte budget values are plain unsigned byte counts (same grammar as
/// FSZERO_SNAPSHOT_RETENTION_BYTES); anything else is a usage error.
fn parse_byte_budget(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("--max-bytes requires a byte value".to_string());
    }
    trimmed.parse::<u64>().map_err(|_| {
        format!(
            "invalid --max-bytes value {raw:?}: expected an unsigned byte count such as 1073741824"
        )
    })
}

/// Shareable telemetry inspect/dry-run (fszero-97v). Default off; opt-out wins.
/// Prints the exact allowlisted payload JSON and never uploads (exporter=none).
fn run_telemetry(args: &[String]) {
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    if sub != "inspect" && sub != "dry-run" {
        die2("usage: fszero telemetry inspect|dry-run [--telemetry|--no-telemetry] [--root PATH]");
    }
    let mut cli_opt_in = false;
    let mut cli_opt_out = false;
    let mut root = resolve_cli_root(args);
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--telemetry" => cli_opt_in = true,
            "--no-telemetry" => cli_opt_out = true,
            "--root" => {
                i += 1;
                if i >= args.len() {
                    die2("--root requires a path");
                }
                root = PathBuf::from(&args[i]);
            }
            other => {
                if let Some(rest) = other.strip_prefix("--root=") {
                    root = PathBuf::from(rest);
                } else {
                    die2(format!("unknown telemetry flag: {other}"));
                }
            }
        }
        i += 1;
    }
    if let Ok(resolved) = install_explicit_root(&root) {
        root = resolved;
    }
    let store = fs_zero::telemetry_store_root(&root);
    let env_value = env::var(fs_zero::TELEMETRY_ENV).ok();
    let config = fs_zero::load_telemetry_config(&store);
    let enabled = fs_zero::resolve_telemetry(cli_opt_in, cli_opt_out, config, env_value.as_deref());
    let inspection = match fs_zero::inspect_telemetry(&store, enabled) {
        Ok(v) => v,
        Err(e) => die1(format!("telemetry inspect: FAIL {e}")),
    };
    // Truthful no-export: even when enabled, FSZero has no exporter.
    let _ = fs_zero::export_shareable_telemetry(&inspection);
    let body = fs_zero::inspection_json(&inspection);
    println!("{}", serde_json::to_string_pretty(&body).unwrap());
    process::exit(0);
}

/// Parent supervisor: spawn the **selected surface binary** (not this shim)
/// without `--supervise`, tee child stderr to disk, proxy stdio. On child
/// death, emit structured `substrate_down` JSON on parent stderr and exit
/// non-zero (no hang, no silent partial state).
const SUPERVISE_FORWARD_ENVS: &[&str] = &[
    "FSZERO_ROOT",
    "ZEROSTACK_STORE_ROOT",
    "FSZERO_STARTUP_INDEX",
    "FSZERO_SHARED_STORE",
    "ZEROSTACK_SHARED_STORE",
    "FSZERO_SKIP_GITIGNORE",
];
fn run_supervised(args: &[String]) {
    let (_surface, program) = match resolve_surface_binary(args) {
        Ok(v) => v,
        Err(e) => die2(format!("fszero --supervise: {e}")),
    };
    let child_args: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|a| *a != "--supervise")
        .cloned()
        .collect();
    let stderr_dir = env::var("FSZERO_STDERR_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir().join(format!("fszero-child-stderr-{}", process::id())));

    let mut cfg = SubstrateChildConfig::new(&program, &stderr_dir)
        .args(child_args)
        .startup_probe(Duration::from_millis(80));
    // Forward root / store env so the inner server sees the same workspace.
    // FSZERO_ROOT is the effective zero_execute / --root workspace; store env
    // is independent (shared durable store must not rewrite workspace root).
    for key in SUPERVISE_FORWARD_ENVS.iter().copied() {
        if let Ok(v) = env::var(key) {
            cfg = cfg.env(key, v);
        }
    }
    // If --root was only on argv (not yet visible as env for some reason),
    // still force the child workspace.
    if let Some(root) = parse_root_flag(args) {
        if let Ok(resolved) = install_explicit_root(&root) {
            cfg = cfg.env("FSZERO_ROOT", resolved.display().to_string());
        }
    }

    let mut child = match SupervisedChild::spawn(cfg) {
        Ok(c) => c,
        Err(down) => {
            emit_substrate_down(&down);
            process::exit(exit_code_from_down(&down));
        }
    };

    let mut child_stdin = child.take_stdin().expect("child stdin");
    let mut child_stdout = child.take_stdout().expect("child stdout");
    let dead = Arc::new(AtomicBool::new(false));

    let dead_out = Arc::clone(&dead);
    let out_thread = thread::spawn(move || {
        let mut stdout = io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match child_stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = stdout.flush();
                }
                Err(_) => break,
            }
        }
        dead_out.store(true, Ordering::SeqCst);
    });

    // stdin pump: never join this thread on the death path. While the MCP/hub
    // client keeps parent stdin open, `stdin.read` blocks forever — joining
    // would hang process::exit and silence substrate_down (Wave A field bug).
    let dead_in = Arc::clone(&dead);
    let _in_thread = thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 8192];
        loop {
            if dead_in.load(Ordering::SeqCst) {
                break;
            }
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if child_stdin.write_all(&buf[..n]).is_err() {
                        dead_in.store(true, Ordering::SeqCst);
                        break;
                    }
                    if child_stdin.flush().is_err() {
                        dead_in.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Poll for death while stdio pumps run — structured loud failure, no hang.
    // Never join the stdin pump: with an open MCP/hub pipe, stdin.read blocks
    // forever and would silence substrate_down. process::exit reaps threads.
    let down = loop {
        if let Some(down) = child.poll_death() {
            dead.store(true, Ordering::SeqCst);
            break down;
        }
        if dead.load(Ordering::SeqCst) {
            break child.wait_dead();
        }
        thread::sleep(Duration::from_millis(20));
    };
    join_with_timeout(out_thread, Duration::from_millis(200));
    if down.exit_code == Some(0) && down.signal.is_none() {
        process::exit(0);
    }
    emit_substrate_down(&down);
    process::exit(exit_code_from_down(&down));
}

/// Join a thread only until `limit`; abandon if still running (never hang).
fn join_with_timeout(handle: thread::JoinHandle<()>, limit: Duration) {
    let start = std::time::Instant::now();
    loop {
        if handle.is_finished() {
            let _ = handle.join();
            return;
        }
        if start.elapsed() >= limit {
            std::mem::forget(handle);
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn emit_substrate_down(down: &SubstrateDown) {
    eprintln!("{}", down.to_json_string());
}

fn exit_code_from_down(down: &SubstrateDown) -> i32 {
    match down.exit_code {
        Some(0) | None => 1,
        Some(c) if (1..=255).contains(&c) => c,
        Some(c) => (c.rem_euclid(256)).max(1),
    }
}

/// ZeroRef v1 conformance fixture surface (fszero-c6q.6): non-interactive
/// producer/consumer helper for the three-binary matrix. `put` emits JSON on
/// stdout; `expand` writes exact bytes to stdout (or `--out`) and diagnostics on
/// stderr.
fn run_zeroref_fixture(args: &[String]) {
    let action = match fs_zero::core::zeroref_fixture::parse_args(&args[2..]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::to_string(&json!({
                    "schema": "zeroref-fixture/v1", "ok": false,
                    "binary": fs_zero::core::zeroref_fixture::binary_identity(), "error_class": "malformed",
                    "exit_code": 2, "message": e, "ref": null, "os": std::env::consts::OS,
                })).unwrap()
            );
            process::exit(2);
        }
    };

    let emit_error = |err: &fs_zero::core::zeroref_fixture::FixtureError| {
        eprintln!(
            "{}",
            serde_json::to_string(&fs_zero::core::zeroref_fixture::error_diag(err)).unwrap()
        );
        process::exit(fs_zero::core::zeroref_fixture::exit_code_for_class(
            err.class,
        ));
    };
    let emit_ok = |doc: &serde_json::Value, use_stderr: bool| -> ! {
        let pretty = serde_json::to_string_pretty(doc).unwrap();
        if use_stderr {
            eprintln!("{pretty}");
        } else {
            println!("{pretty}");
        }
        process::exit(0);
    };

    match action {
        fs_zero::core::zeroref_fixture::ZerorefFixtureAction::Descriptor { store_root } => {
            match fs_zero::core::zeroref_fixture::run_descriptor(&store_root) {
                Ok(doc) => emit_ok(&doc, false),
                Err(e) => emit_error(&e),
            }
        }
        fs_zero::core::zeroref_fixture::ZerorefFixtureAction::Put {
            store_root,
            shared_root,
            input,
            max_object_bytes,
        } => {
            match fs_zero::core::zeroref_fixture::run_put(
                &store_root,
                shared_root.as_deref(),
                input.as_deref(),
                max_object_bytes,
            ) {
                Ok(doc) => emit_ok(&doc, false),
                Err(e) => emit_error(&e),
            }
        }
        fs_zero::core::zeroref_fixture::ZerorefFixtureAction::Expand {
            store_root,
            shared_root,
            reference,
            out,
        } => {
            match fs_zero::core::zeroref_fixture::run_expand(
                &store_root,
                shared_root.as_deref(),
                &reference,
                out.as_deref(),
            ) {
                Ok(result) => emit_ok(&result.diag, true),
                Err(e) => emit_error(&e),
            }
        }
    }
}

fn run_codemode_cli(args: Vec<String>) {
    if args.len() < 2 {
        print_help();
        process::exit(2);
    }
    let cmd = &args[1];
    if cmd == "codemode" {
        die2(
            "fszero codemode is retired. Model execution is ZeroKernel (`z.read`, `z.edit`, `z.apply`). This binary is an operator installer/re-exec shim only.",
        );
    }
    let hint = did_you_mean_suffix(cmd, SHIM_COMMANDS);
    let flag_hint = if cmd.starts_with('-') {
        did_you_mean_suffix(cmd, COMMON_FLAGS)
    } else {
        String::new()
    };
    die2(format!(
        "unknown command {cmd:?}; use serve, --mode=mcp|--mode=codemode, --supervise, --raw-worker, doctor, install, uninstall, sbom, capabilities, catalog, tools, layout, batch, migrate-cas, store-gc, telemetry, zeroref-fixture{hint}{flag_hint}",
    ));
}
