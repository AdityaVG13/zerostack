//! `zerostack doctor` / `zerostack locate` contract tests (bead zerostack-9vou).
//!
//! Every case spawns the real `zerostack` binary against a synthetic install,
//! so the CLI grammar, exit codes, JSON shape, and human grammar are all
//! exercised end to end. Environment pins are cleared per spawn so a developer
//! machine's ambient ZEROSTACK_* state cannot leak into a case.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Environment variables that influence discovery, artifacts, or store
/// resolution. Cleared on every spawn so cases are hermetic.
const AMBIENT_VARS: [&str; 8] = [
    "ZEROSTACK_HOME",
    "ZEROSTACK_DEV_ROOT",
    "ZEROSTACK_NODE",
    "ZEROSTACK_RUNTIME_MODULE",
    "ZEROSTACK_SUBSTRATE_MODULE",
    "XDG_DATA_HOME",
    "ZEROSTACK_STORE_ROOT",
    "ZERO_STACK_STORE_ROOT",
];

fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zerostack"));
    for var in AMBIENT_VARS {
        cmd.env_remove(var);
    }
    cmd.env_remove("ZEROSTACK_SHARED_STORE");
    cmd.current_dir(cwd);
    cmd
}

fn run(cwd: &Path, args: &[&str], home: Option<&Path>) -> Output {
    let mut cmd = command(cwd);
    if let Some(home) = home {
        cmd.env("ZEROSTACK_HOME", home);
    }
    cmd.args(args).output().expect("spawn zerostack")
}

fn output(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 stderr")
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("parseable stdout JSON")
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn write_executable(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("executable parent directory");
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write binary");
    make_executable(&path);
    path
}

fn write_module(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("module parent directory");
    let path = dir.join(name);
    std::fs::write(&path, "module.exports = {};\n").expect("write module");
    path
}

/// A synthetic complete install: four executables, Node, two modules, and a
/// legacy store at `<cwd>/.tokenzero` with its journal directory.
struct Install {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
}

impl Install {
    fn complete() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let bin = home.join("bin");
        let lib = home.join("lib");
        std::fs::create_dir_all(&bin).expect("bin dir");
        std::fs::create_dir_all(&lib).expect("lib dir");
        for name in [
            "zerostack-codemode-host",
            "fszero-codemode",
            "graphzero-codemode",
            "tokenzero-codemode",
            "node",
        ] {
            write_executable(&bin, name);
        }
        write_module(&lib, "raw-runtime.js");
        write_module(&lib, "substrates.js");
        let store = tmp.path().join(".tokenzero");
        std::fs::create_dir_all(store.join("journal")).expect("store dirs");
        let cwd = tmp.path().canonicalize().expect("canonical temp root");
        Install {
            _tmp: tmp,
            home,
            cwd,
        }
    }
}

const COMPONENTS: [&str; 9] = [
    "aggregate_host",
    "binaries.fs",
    "binaries.graph",
    "binaries.token",
    "node",
    "runtime_module",
    "substrate_module",
    "store_root",
    "journal_dir",
];

/// Case 1: a synthetic complete install makes `doctor --json` exit 0 and locks
/// the schema, the nine-check order, the fixed fields, and nullability.
#[test]
fn complete_install_doctor_json_exits_0_and_locks_shape() {
    let install = Install::complete();
    let out = run(&install.cwd, &["doctor", "--json"], Some(&install.home));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(stderr(&out).is_empty(), "doctor never writes stderr");

    let report = json(&out);
    assert_eq!(report["schema"], "zerostack.doctor.v1");
    assert_eq!(report["ok"], true);
    let checks = report["checks"].as_array().expect("checks array");
    assert_eq!(checks.len(), 9);
    let components: Vec<&str> = checks
        .iter()
        .map(|check| check["component"].as_str().expect("component id"))
        .collect();
    assert_eq!(components, COMPONENTS);
    for (index, check) in checks.iter().enumerate() {
        let keys: Vec<&str> = check
            .as_object()
            .expect("check object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["component", "error", "ok", "path", "remediation", "source"],
            "fixed fields for {index}"
        );
        assert_eq!(check["ok"], true, "check {index} passes");
        assert!(check["path"].is_string(), "path reported for {index}");
        assert!(check["error"].is_null(), "no error for {index}");
        assert!(check["remediation"].is_null(), "no remediation for {index}");
        if index < 7 {
            assert!(check["source"].is_string(), "source copied for {index}");
        } else {
            assert!(check["source"].is_null(), "store checks have no source");
        }
    }
}

/// Case 2: a missing install exits 1 and locks every error and remediation.
#[test]
fn missing_install_exits_1_and_locks_errors_and_remediations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("empty home");
    let isolated_home = tmp.path().join("isolated-home");
    std::fs::create_dir_all(&isolated_home).expect("isolated home");
    let isolated_path = tmp.path().join("isolated-path");
    std::fs::create_dir_all(&isolated_path).expect("isolated path");
    let canonical_tmp = tmp.path().canonicalize().expect("canonical temp root");

    let mut cmd = command(tmp.path());
    cmd.env("ZEROSTACK_HOME", &home);
    cmd.env("HOME", &isolated_home);
    cmd.env("PATH", &isolated_path);
    let out = cmd.args(["doctor", "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(1));

    let report = json(&out);
    assert_eq!(report["ok"], false);
    let checks = report["checks"].as_array().expect("checks array");
    assert_eq!(checks.len(), 9);

    let expected: Vec<(&str, &str, String)> = vec![
        (
            "aggregate_host",
            "no executable candidate resolved",
            "install into $ZEROSTACK_HOME/bin or set ZEROSTACK_DEV_ROOT".to_owned(),
        ),
        (
            "binaries.fs",
            "no executable candidate resolved",
            "install into $ZEROSTACK_HOME/bin or set ZEROSTACK_DEV_ROOT".to_owned(),
        ),
        (
            "binaries.graph",
            "no executable candidate resolved",
            "install into $ZEROSTACK_HOME/bin or set ZEROSTACK_DEV_ROOT".to_owned(),
        ),
        (
            "binaries.token",
            "no executable candidate resolved",
            "install into $ZEROSTACK_HOME/bin or set ZEROSTACK_DEV_ROOT".to_owned(),
        ),
        (
            "node",
            "no executable candidate resolved",
            "set ZEROSTACK_NODE to a Node executable".to_owned(),
        ),
        (
            "runtime_module",
            "no readable regular-file candidate resolved",
            "set ZEROSTACK_RUNTIME_MODULE to a readable module file".to_owned(),
        ),
        (
            "substrate_module",
            "no readable regular-file candidate resolved",
            "set ZEROSTACK_SUBSTRATE_MODULE to a readable module file".to_owned(),
        ),
        (
            "store_root",
            "store root is not a directory",
            format!(
                "Create directory: {}",
                canonical_tmp.join(".tokenzero").display()
            ),
        ),
        (
            "journal_dir",
            "journal directory is not a directory",
            format!(
                "Create directory: {}",
                canonical_tmp.join(".tokenzero").join("journal").display()
            ),
        ),
    ];
    for (index, check) in checks.iter().enumerate() {
        let (component, error, remediation) = &expected[index];
        assert_eq!(check["component"].as_str(), Some(*component));
        assert_eq!(check["ok"], false);
        assert_eq!(check["error"].as_str(), Some(*error));
        assert_eq!(check["remediation"].as_str(), Some(remediation.as_str()));
        if index < 7 {
            assert!(
                check["path"].is_null(),
                "no path for unresolved {component}"
            );
            assert!(
                check["source"].is_null(),
                "no source for unresolved {component}"
            );
        } else {
            assert!(check["path"].is_string(), "store path still reported");
        }
    }
}

/// Case 3: a regular non-executable binary and a directory-shaped binary fail
/// while a later valid candidate wins.
#[cfg(unix)]
#[test]
fn non_executable_and_directory_candidates_lose_to_later_valid_binary() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let bin = home.join("bin");
    let lib = home.join("lib");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(&lib).expect("lib");

    // Home: a regular file without an execute bit. It must not shadow anything.
    let home_host = bin.join("zerostack-codemode-host");
    std::fs::write(&home_host, "not executable\n").expect("plain file");
    std::fs::set_permissions(&home_host, std::fs::Permissions::from_mode(0o644)).expect("mode");

    // Dev checkout: a directory named like the binary. It must not shadow.
    let dev_root = tmp.path().join("dev");
    std::fs::create_dir_all(
        dev_root
            .join("ZeroStack")
            .join("target")
            .join("release")
            .join("zerostack-codemode-host"),
    )
    .expect("directory-shaped candidate");

    // XDG data dir: a real executable. It must win.
    let xdg = tmp.path().join("xdg");
    let xdg_host = write_executable(
        &xdg.join("zerostack").join("bin"),
        "zerostack-codemode-host",
    );

    for name in [
        "fszero-codemode",
        "graphzero-codemode",
        "tokenzero-codemode",
        "node",
    ] {
        write_executable(&bin, name);
    }
    write_module(&lib, "raw-runtime.js");
    write_module(&lib, "substrates.js");
    let store = tmp.path().join(".tokenzero");
    std::fs::create_dir_all(store.join("journal")).expect("store");

    let mut cmd = command(tmp.path());
    cmd.env("ZEROSTACK_HOME", &home);
    cmd.env("ZEROSTACK_DEV_ROOT", &dev_root);
    cmd.env("XDG_DATA_HOME", &xdg);
    let out = cmd.args(["doctor", "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));

    let report = json(&out);
    let host_check = &report["checks"][0];
    assert_eq!(host_check["component"], "aggregate_host");
    assert_eq!(host_check["ok"], true);
    assert_eq!(host_check["path"], xdg_host.to_str().unwrap());
    assert_eq!(host_check["source"], "xdg_data");
    assert_eq!(report["ok"], true);
}

/// Case 3 (non-unix): a directory-shaped candidate loses to a later valid one.
#[cfg(not(unix))]
#[test]
fn directory_candidate_loses_to_later_valid_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let bin = home.join("bin");
    let lib = home.join("lib");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(&lib).expect("lib");
    std::fs::create_dir(bin.join("zerostack-codemode-host")).expect("directory candidate");

    let xdg = tmp.path().join("xdg");
    let xdg_host = write_executable(
        &xdg.join("zerostack").join("bin"),
        "zerostack-codemode-host",
    );
    for name in [
        "fszero-codemode",
        "graphzero-codemode",
        "tokenzero-codemode",
        "node",
    ] {
        write_executable(&bin, name);
    }
    write_module(&lib, "raw-runtime.js");
    write_module(&lib, "substrates.js");
    let store = tmp.path().join(".tokenzero");
    std::fs::create_dir_all(store.join("journal")).expect("store");

    let mut cmd = command(tmp.path());
    cmd.env("ZEROSTACK_HOME", &home);
    cmd.env("XDG_DATA_HOME", &xdg);
    let out = cmd.args(["doctor", "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let host_check = &json(&out)["checks"][0];
    assert_eq!(host_check["ok"], true);
    assert_eq!(host_check["path"], xdg_host.to_str().unwrap());
    assert_eq!(host_check["source"], "xdg_data");
}

/// Case 4: a non-file (directory-shaped) module fails; a real module passes.
#[test]
fn non_file_module_fails() {
    let install = Install::complete();
    let lib = install.home.join("lib");
    std::fs::remove_file(lib.join("raw-runtime.js")).expect("remove module");
    std::fs::create_dir(lib.join("raw-runtime.js")).expect("directory-shaped module");

    let out = run(&install.cwd, &["doctor", "--json"], Some(&install.home));
    assert_eq!(out.status.code(), Some(1));

    let report = json(&out);
    let runtime = &report["checks"][5];
    assert_eq!(runtime["component"], "runtime_module");
    assert_eq!(runtime["ok"], false);
    assert_eq!(
        runtime["error"],
        "no readable regular-file candidate resolved"
    );
    assert_eq!(
        runtime["remediation"],
        "set ZEROSTACK_RUNTIME_MODULE to a readable module file"
    );
    assert_eq!(
        report["checks"][6]["ok"], true,
        "substrate module still passes"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::remove_dir(lib.join("raw-runtime.js")).expect("remove directory module");
        let unreadable = write_module(&lib, "raw-runtime.js");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
            .expect("make module unreadable");
        let out = run(&install.cwd, &["doctor", "--json"], Some(&install.home));
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
            .expect("restore module permissions");
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            json(&out)["checks"][5]["error"],
            "no readable regular-file candidate resolved"
        );
    }
}

/// Case 5: missing and non-directory store roots and journal dirs fail.
#[test]
fn missing_and_non_directory_store_roots_fail() {
    // Missing entirely.
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let bin = home.join("bin");
    let lib = home.join("lib");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::create_dir_all(&lib).expect("lib");
    for name in [
        "zerostack-codemode-host",
        "fszero-codemode",
        "graphzero-codemode",
        "tokenzero-codemode",
        "node",
    ] {
        write_executable(&bin, name);
    }
    write_module(&lib, "raw-runtime.js");
    write_module(&lib, "substrates.js");

    let out = run(tmp.path(), &["doctor", "--json"], Some(&home));
    assert_eq!(out.status.code(), Some(1));
    let report = json(&out);
    assert_eq!(
        report["checks"][7]["error"],
        "store root is not a directory"
    );
    assert_eq!(
        report["checks"][8]["error"],
        "journal directory is not a directory"
    );
    let canonical_tmp = tmp.path().canonicalize().expect("canonical temp root");
    assert_eq!(
        report["checks"][7]["remediation"],
        format!(
            "Create directory: {}",
            canonical_tmp.join(".tokenzero").display()
        )
    );

    // Present but not a directory.
    std::fs::write(tmp.path().join(".tokenzero"), "not a directory\n").expect("file store root");
    let out = run(tmp.path(), &["doctor", "--json"], Some(&home));
    assert_eq!(out.status.code(), Some(1));
    let report = json(&out);
    assert_eq!(report["checks"][7]["ok"], false);
    assert_eq!(report["checks"][8]["ok"], false);
    assert_eq!(
        report["checks"][8]["remediation"],
        format!(
            "Create directory: {}",
            canonical_tmp.join(".tokenzero").join("journal").display()
        )
    );
}

/// Case 6: human output matches the exact line grammar and summary.
#[test]
fn human_output_matches_exact_grammar_and_summary() {
    let install = Install::complete();
    let out = run(&install.cwd, &["doctor"], Some(&install.home));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(stderr(&out).is_empty());

    let home = install.home.to_string_lossy();
    let cwd = install.cwd.to_string_lossy();
    let expected = format!(
        "OK aggregate_host: {home}/bin/zerostack-codemode-host [zerostack_home]\n\
         OK binaries.fs: {home}/bin/fszero-codemode [zerostack_home]\n\
         OK binaries.graph: {home}/bin/graphzero-codemode [zerostack_home]\n\
         OK binaries.token: {home}/bin/tokenzero-codemode [zerostack_home]\n\
         OK node: {home}/bin/node [zerostack_home]\n\
         OK runtime_module: {home}/lib/raw-runtime.js [zerostack_home]\n\
         OK substrate_module: {home}/lib/substrates.js [zerostack_home]\n\
         OK store_root: {cwd}/.tokenzero [-]\n\
         OK journal_dir: {cwd}/.tokenzero/journal [-]\n\
         ZeroStack doctor: OK (9 checks)\n"
    );
    assert_eq!(output(&out), expected);

    // Failed install: ERROR + FIX per failure, then the FAILED summary.
    let tmp = tempfile::tempdir().expect("tempdir");
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).expect("empty home");
    let isolated_home = tmp.path().join("isolated-home");
    let isolated_path = tmp.path().join("isolated-path");
    std::fs::create_dir_all(&isolated_home).expect("isolated home");
    std::fs::create_dir_all(&isolated_path).expect("isolated path");
    let mut cmd = command(tmp.path());
    cmd.env("ZEROSTACK_HOME", &empty);
    cmd.env("HOME", isolated_home);
    cmd.env("PATH", isolated_path);
    let out = cmd.args(["doctor"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).is_empty(), "doctor failure stays on stdout");

    let text = output(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 19, "9 ERROR + 9 FIX + summary");
    for component in COMPONENTS {
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(&format!("ERROR {component}:"))),
            "ERROR line for {component}: {text}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(&format!("FIX {component}:"))),
            "FIX line for {component}: {text}"
        );
    }
    assert_eq!(lines[18], "ZeroStack doctor: FAILED (9/9 checks failed)");
}

/// Case 7: an incomplete `locate --json` emits a valid full manifest and exits 1.
#[test]
fn incomplete_locate_json_emits_manifest_and_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).expect("empty home");
    let isolated_home = tmp.path().join("isolated-home");
    std::fs::create_dir_all(&isolated_home).expect("isolated home");
    let isolated_path = tmp.path().join("isolated-path");
    std::fs::create_dir_all(&isolated_path).expect("isolated path");

    let mut cmd = command(tmp.path());
    cmd.env("ZEROSTACK_HOME", &empty);
    cmd.env("HOME", &isolated_home);
    cmd.env("PATH", &isolated_path);
    let out = cmd.args(["locate", "--json"]).output().expect("spawn");
    assert_eq!(out.status.code(), Some(1));

    let manifest = json(&out);
    assert_eq!(manifest["schema"], "zerostack.locate.v1");
    for key in [
        "schema",
        "order",
        "aggregate_host",
        "binaries",
        "node",
        "runtime_module",
        "substrate_module",
        "store_root",
        "journal_dir",
        "versions",
        "capabilities",
    ] {
        assert!(manifest.get(key).is_some(), "manifest has {key}");
    }
    for key in [
        "aggregate_host",
        "node",
        "runtime_module",
        "substrate_module",
    ] {
        assert_eq!(manifest[key]["resolved"], false, "{key} unresolved");
    }
    for key in ["fs", "graph", "token"] {
        assert_eq!(
            manifest["binaries"][key]["resolved"], false,
            "{key} unresolved"
        );
    }
    assert_eq!(manifest["capabilities"].as_array().expect("array").len(), 0);
    let err = stderr(&out);
    assert!(
        err.contains("locate incomplete"),
        "failure summary on stderr: {err}"
    );
}

/// Case 8: a complete locate exits 0.
#[test]
fn complete_locate_exits_0() {
    let install = Install::complete();

    let out = run(&install.cwd, &["locate", "--json"], Some(&install.home));
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let manifest = json(&out);
    assert_eq!(manifest["schema"], "zerostack.locate.v1");
    assert_eq!(manifest["aggregate_host"]["resolved"], true);
    for key in ["fs", "graph", "token"] {
        assert_eq!(manifest["binaries"][key]["resolved"], true);
    }
    for key in ["node", "runtime_module", "substrate_module"] {
        assert_eq!(manifest[key]["resolved"], true);
    }
    let capabilities: Vec<&str> = manifest["capabilities"]
        .as_array()
        .expect("array")
        .iter()
        .map(|value| value.as_str().expect("capability"))
        .collect();
    assert_eq!(capabilities, ["fs", "graph", "token"]);

    let out = run(&install.cwd, &["locate"], Some(&install.home));
    assert_eq!(out.status.code(), Some(0));
    let text = output(&out);
    assert!(text.contains("aggregate_host"), "human locate lists host");
    assert!(text.contains("store_root"), "human locate lists store");
    assert!(text.contains("journal_dir"), "human locate lists journal");
}

/// Case 9: unsupported arguments exit 2; help and version exit 0.
#[test]
fn unsupported_arguments_exit_2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for args in [
        &[][..],
        &["--json"][..],
        &["doctor", "extra"][..],
        &["doctor", "--json", "--json"][..],
        &["locate", "extra"][..],
        &["locate", "--json", "extra"][..],
        &["-x"][..],
        &["LOCATE"][..],
        &["--help", "extra"][..],
    ] {
        let out = run(tmp.path(), args, None);
        assert_eq!(
            out.status.code(),
            Some(2),
            "args {args:?} must exit 2, stderr: {}",
            stderr(&out)
        );
        assert!(
            stderr(&out).contains("usage"),
            "usage on stderr for {args:?}"
        );
    }

    for args in [
        &["--help"][..],
        &["-h"][..],
        &["--version"][..],
        &["-V"][..],
    ] {
        let out = run(tmp.path(), args, None);
        assert_eq!(out.status.code(), Some(0), "args {args:?} must exit 0");
    }

    let version = output(&run(tmp.path(), &["--version"], None));
    assert!(version.starts_with("zerostack "), "version line: {version}");
}
