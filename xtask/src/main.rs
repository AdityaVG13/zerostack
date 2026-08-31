use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};

const REQUIRED_PATHS: &[&str] = &[
    "Cargo.toml",
    "README.md",
    "bindings/node/package.json",
    "bindings/node/loader.js",
    "bindings/node/zero-kernel.d.ts",
    "contracts/README.md",
    "contracts/SurfaceMatrix.toml",
    "contracts/zeroref-fixtures.json",
    "crates/zerostack/zerostack-conformance/CONTRACT.md",
    "demo/run.js",
    "docs/architecture.md",
    "fuzz/Cargo.toml",
    "packaging/README.md",
    "xtask/Cargo.toml",
    "crates/zerostack",
    "crates/fszero",
    "crates/graphzero",
    "crates/tokenzero",
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask must live directly below the repository root")?;
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("help" | "--help" | "-h") => print_help(),
        Some("doctor") => doctor(root, args.any(|arg| arg == "--json"))?,
        Some("understand") => understand(root, args.collect())?,
        Some("check") => {
            cargo(root, &["fmt", "--all", "--check"])?;
            cargo(root, &["check", "--workspace"])?;
        }
        Some("test-targeted") => {
            let package = args
                .next()
                .ok_or("usage: cargo run --manifest-path xtask/Cargo.toml -- test-targeted <package> [test-filter]")?;
            let mut command = vec!["test".to_owned(), "-p".to_owned(), package];
            command.extend(args);
            cargo_owned(root, &command)?;
        }
        Some("bench") => {
            let bench = args
                .next()
                .ok_or("usage: cargo run --manifest-path xtask/Cargo.toml -- bench <bench-name> [extra cargo args]")?;
            let mut command = vec!["bench".to_owned(), "--bench".to_owned(), bench];
            command.extend(args);
            cargo_owned(root, &command)?;
        }
        Some("docs") => cargo(root, &["doc", "--workspace", "--no-deps"])?,
        Some("release") => {
            return Err(
                "no public ZeroStack package exists; this repository is source-only".to_owned(),
            );
        }
        Some("ci") => {
            doctor(root, true)?;
            understand(root, vec!["--check".to_owned()])?;
        }
        Some(other) => {
            return Err(format!(
                "unknown command {other:?}; run cargo run --manifest-path xtask/Cargo.toml -- help"
            ));
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "ZeroStack repository workflow

  cargo run --manifest-path xtask/Cargo.toml -- doctor [--json]
  cargo run --manifest-path xtask/Cargo.toml -- understand [--check]
  cargo run --manifest-path xtask/Cargo.toml -- check
  cargo run --manifest-path xtask/Cargo.toml -- test-targeted <package> [filter]
  cargo run --manifest-path xtask/Cargo.toml -- bench <bench-name> [args]
  cargo run --manifest-path xtask/Cargo.toml -- docs
  cargo run --manifest-path xtask/Cargo.toml -- ci

There is no release command that tags, publishes, or builds distribution assets.
No command runs an unbounded test suite."
    );
}

fn doctor(root: &Path, json: bool) -> Result<(), String> {
    let checks = ["cargo", "rustc", "git"];
    let mut tools = Vec::new();
    for tool in checks {
        let ok = Command::new(tool)
            .arg("--version")
            .current_dir(root)
            .output()
            .is_ok_and(|out| out.status.success());
        tools.push((tool, ok));
    }
    let missing: Vec<&str> = REQUIRED_PATHS
        .iter()
        .copied()
        .filter(|path| !root.join(path).exists())
        .collect();
    let kind = repository_kind(root);
    let tools_ok = tools.iter().all(|(_, ok)| *ok);
    let healthy = tools_ok && missing.is_empty() && kind.is_ok();
    if json {
        let tool_fields = tools
            .iter()
            .map(|(name, ok)| format!(r#""{name}":{ok}"#))
            .collect::<Vec<_>>()
            .join(",");
        let missing_json = json_array(
            &missing
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>(),
        );
        let kind_json = match &kind {
            Ok(name) => format!("\"{name}\""),
            Err(_) => "null".to_owned(),
        };
        println!(
            r#"{{"schemaVersion":1,"healthy":{healthy},"repository":{kind_json},"missing":{missing_json},"tools":{{{tool_fields}}}}}"#
        );
    } else {
        println!("repository: {}", kind.as_deref().unwrap_or("unrecognized"));
        for (tool, ok) in &tools {
            println!("{tool}: {}", state(*ok));
        }
        if missing.is_empty() {
            println!("layout: ok");
        } else {
            println!("layout: missing {}", missing.join(", "));
        }
    }
    kind?;
    if healthy {
        Ok(())
    } else {
        Err("doctor found missing prerequisites or required files".to_owned())
    }
}

fn state(ok: bool) -> &'static str {
    if ok { "ok" } else { "missing" }
}

fn understand(root: &Path, args: Vec<String>) -> Result<(), String> {
    if args.len() > 1
        || args
            .first()
            .is_some_and(|arg| !matches!(arg.as_str(), "--check"))
    {
        return Err(
            "usage: cargo run --manifest-path xtask/Cargo.toml -- understand [--check]".to_owned(),
        );
    }
    let generated = inventory(root)?;
    match args.first().map(String::as_str) {
        Some("--check") => {
            let missing: Vec<&str> = REQUIRED_PATHS
                .iter()
                .copied()
                .filter(|path| !root.join(path).exists())
                .collect();
            if !missing.is_empty() {
                return Err(format!("required paths missing: {}", missing.join(", ")));
            }
            println!("required layout is present");
        }
        None => print!("{generated}"),
        _ => unreachable!(),
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<String, String> {
    let mut manifests = Vec::new();
    let mut scripts = Vec::new();
    let mut workflows = Vec::new();
    let mut documentation = Vec::new();
    walk(root, root, &mut |relative, path| {
        let value = relative.replace('\\', "/");
        if path.file_name().is_some_and(|name| name == "Cargo.toml") && value != "xtask/Cargo.toml"
        {
            manifests.push(value);
        } else if value.starts_with("scripts/") {
            scripts.push(value);
        } else if value.starts_with(".github/workflows/")
            && matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("yml" | "yaml")
            )
        {
            workflows.push(value);
        } else if (value.starts_with("docs/") || !value.contains('/'))
            && path.extension().and_then(|x| x.to_str()) == Some("md")
            && !value.starts_with("docs/internal/")
        {
            documentation.push(value);
        }
    })
    .map_err(io_error)?;
    for list in [
        &mut manifests,
        &mut scripts,
        &mut workflows,
        &mut documentation,
    ] {
        list.sort();
    }
    Ok(format!(
        r#"{{
  "schemaVersion": 1,
  "repository": "{}",
  "cargoManifests": {},
  "scripts": {},
  "workflows": {},
  "documentation": {}
}}
"#,
        repository_kind(root)?,
        json_array(&manifests),
        json_array(&scripts),
        json_array(&workflows),
        json_array(&documentation)
    ))
}

fn walk(root: &Path, dir: &Path, visit: &mut dyn FnMut(String, &Path)) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if matches!(
                name.to_str(),
                Some(
                    ".git"
                        | ".beads"
                        | ".pi-subagents"
                        | ".prosecution"
                        | "node_modules"
                        | "target"
                )
            ) || name.to_str() == Some("internal")
                && dir.file_name().and_then(|n| n.to_str()) == Some("docs")
            {
                continue;
            }
            walk(root, &path, visit)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            visit(relative.to_string_lossy().into_owned(), &path);
        }
    }
    Ok(())
}

fn json_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\"")))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn repository_kind(root: &Path) -> Result<String, String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).map_err(io_error)?;
    if manifest.contains("zero-kernel")
        && manifest.contains("crates/fszero/")
        && manifest.contains("crates/graphzero/")
        && manifest.contains("crates/tokenzero/")
    {
        return Ok("zerostack".to_owned());
    }
    Err("unrecognized ZeroStack repository".to_owned())
}

fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    let owned = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    cargo_owned(root, &owned)
}

fn cargo_owned(root: &Path, args: &[String]) -> Result<(), String> {
    println!("+ cargo {}", args.join(" "));
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(io_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed with {status}", args.join(" ")))
    }
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}
