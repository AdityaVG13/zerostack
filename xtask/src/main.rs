use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};

const KNOWLEDGE_PATH: &str = "docs/knowledge/workspace.json";

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
                .ok_or("usage: cargo xtask test-targeted <package> [test-filter]")?;
            let mut command = vec!["test".to_owned(), "-p".to_owned(), package];
            command.extend(args);
            cargo_owned(root, &command)?;
        }
        Some("bench") => {
            let bench = args
                .next()
                .ok_or("usage: cargo xtask bench <bench-name> [extra cargo args]")?;
            let mut command = vec!["bench".to_owned(), "--bench".to_owned(), bench];
            command.extend(args);
            cargo_owned(root, &command)?;
        }
        Some("docs") => cargo(root, &["doc", "--workspace", "--no-deps"])?,
        Some("release") => release(root, &repository_kind(root)?)?,
        Some("ci") => {
            doctor(root, true)?;
            understand(root, vec!["--check".to_owned()])?;
            cargo(root, &["fmt", "--all", "--check"])?;
            cargo(root, &["check", "--workspace"])?;
        }
        Some(other) => return Err(format!("unknown command {other:?}; run cargo xtask help")),
    }
    Ok(())
}

fn print_help() {
    println!(
        "ZeroStack repository workflow

  cargo xtask doctor [--json]
  cargo xtask understand [--write|--check]
  cargo xtask check
  cargo xtask test-targeted <package> [filter]
  cargo xtask bench <bench-name> [args]
  cargo xtask docs
  cargo xtask release
  cargo xtask ci

No command runs an unbounded test suite."
    );
}

fn doctor(root: &Path, json: bool) -> Result<(), String> {
    let checks = ["cargo", "rustc", "git"];
    let mut states = Vec::new();
    for tool in checks {
        let ok = Command::new(tool)
            .arg("--version")
            .current_dir(root)
            .output()
            .is_ok_and(|out| out.status.success());
        states.push((tool, ok));
    }
    let manifest = root.join("Cargo.toml").is_file();
    let healthy = manifest && states.iter().all(|(_, ok)| *ok);
    if json {
        let tools = states
            .iter()
            .map(|(name, ok)| format!(r#""{name}":{ok}"#))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            r#"{{"schemaVersion":1,"healthy":{healthy},"manifest":{manifest},"tools":{{{tools}}}}}"#
        );
    } else {
        println!("repository: {}", repository_kind(root)?);
        println!("manifest: {}", state(manifest));
        for (tool, ok) in &states {
            println!("{tool}: {}", state(*ok));
        }
    }
    if healthy {
        Ok(())
    } else {
        Err("doctor found missing prerequisites".to_owned())
    }
}

fn state(ok: bool) -> &'static str {
    if ok { "ok" } else { "missing" }
}

fn understand(root: &Path, args: Vec<String>) -> Result<(), String> {
    if args.len() > 1
        || args
            .first()
            .is_some_and(|arg| !matches!(arg.as_str(), "--write" | "--check"))
    {
        return Err("usage: cargo xtask understand [--write|--check]".to_owned());
    }
    let generated = inventory(root)?;
    match args.first().map(String::as_str) {
        Some("--write") => {
            let path = root.join(KNOWLEDGE_PATH);
            fs::create_dir_all(path.parent().expect("knowledge path has parent"))
                .map_err(io_error)?;
            fs::write(&path, &generated).map_err(io_error)?;
            println!("wrote {KNOWLEDGE_PATH}");
        }
        Some("--check") => {
            let current = fs::read_to_string(root.join(KNOWLEDGE_PATH)).map_err(|_| {
                format!("{KNOWLEDGE_PATH} is missing; run cargo xtask understand --write")
            })?;
            if current != generated {
                return Err(format!(
                    "{KNOWLEDGE_PATH} is stale; run cargo xtask understand --write"
                ));
            }
            println!("knowledge inventory is current");
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
            ) {
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
    for (needle, name) in [
        ("tokenzero-core", "tokenzero"),
        ("graphzero-types", "graphzero"),
        ("fszero-codemode", "fszero"),
    ] {
        if manifest.contains(needle) {
            return Ok(name.to_owned());
        }
    }
    Err("unrecognized ZeroStack repository".to_owned())
}

fn release(root: &Path, repository: &str) -> Result<(), String> {
    match repository {
        "tokenzero" => {
            cargo(root, &["build", "--release", "--bin", "tokenzero"])?;
            cargo(
                root,
                &[
                    "build",
                    "--release",
                    "--bin",
                    "tokenzero-codemode",
                    "--no-default-features",
                    "--features",
                    "surface-codemode",
                ],
            )
        }
        "fszero" => {
            cargo(root, &["build", "--release"])?;
            cargo(
                root,
                &[
                    "build",
                    "--release",
                    "-p",
                    "fszero-codemode",
                    "--bin",
                    "fszero-codemode",
                ],
            )
        }
        "graphzero" => {
            cargo(root, &["build", "--release"])?;
            cargo(
                root,
                &[
                    "build",
                    "--release",
                    "--bin",
                    "graphzero-codemode",
                    "--no-default-features",
                    "--features",
                    "surface-codemode",
                ],
            )
        }
        _ => Err(format!("no release recipe for {repository}")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_array_is_stable_and_escaped() {
        assert_eq!(
            json_array(&["a".to_owned(), "b\"c".to_owned()]),
            "[\"a\", \"b\\\"c\"]"
        );
    }

    #[test]
    fn repository_is_detected_without_using_directory_names() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        assert!(matches!(
            repository_kind(&root).as_deref(),
            Ok("tokenzero" | "fszero" | "graphzero")
        ));
    }
}
