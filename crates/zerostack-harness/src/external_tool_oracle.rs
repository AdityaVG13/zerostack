//! External-tool oracle adapters. Exit-code smoke only; no CI workflow.

use std::process::Command;

use crate::oracle::{ExternalTool, ScenarioError};

pub fn probe(tool: ExternalTool) -> Result<(), ScenarioError> {
    let (program, args): (&str, &[&str]) = match tool {
        ExternalTool::Miri => ("cargo", &["+nightly", "miri", "--version"]),
        ExternalTool::Clippy => ("cargo", &["clippy", "--version"]),
    };
    let output = Command::new(program).args(args).output().map_err(|error| {
        ScenarioError::new(
            "external-tool",
            format!("{program} {} failed to spawn: {error}", args.join(" ")),
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ScenarioError::new(
            "external-tool",
            format!(
                "{program} {} exit={:?} stderr={}",
                args.join(" "),
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ),
        ))
    }
}

/// Dispatch used by `scenario()`. Does not run full `miri test` / `-D warnings`.
pub fn run(tool: ExternalTool) -> Result<(), ScenarioError> {
    probe(tool)
}
