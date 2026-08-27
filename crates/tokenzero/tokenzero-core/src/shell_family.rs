use crate::render::domain::{
    git_subcommand_index, is_repo_inventory_command, is_search_shell_command,
    shell_command_basename,
};
use crate::shell_parse::{
    looks_diagnostic, looks_status_table, shell_analysis_command, split_shell_words,
};

pub fn shell_family(command: &str, stdout: &str, stderr: &str) -> String {
    let combined = format!("{stdout}\n{stderr}");
    shell_family_with_combined(command, stdout, &combined)
}

pub(crate) fn shell_family_with_combined(command: &str, stdout: &str, combined: &str) -> String {
    let analysis = shell_analysis_command(command);
    let words = split_shell_words(&analysis);
    let first = words
        .first()
        .map(|word| shell_command_basename(word))
        .unwrap_or_default();
    let second = (first == "git")
        .then(|| git_subcommand_index(&words))
        .flatten()
        .and_then(|index| words.get(index))
        .or_else(|| words.get(1))
        .map(String::as_str)
        .unwrap_or_default();
    let family = if is_repo_inventory_command(command) || is_repo_inventory_command(&analysis) {
        "repo-inventory"
    } else if first == "diff"
        || first == "git" && ["diff", "show"].contains(&second)
        || combined.starts_with("diff --git")
        || combined.contains("\n@@ ")
    {
        "diff"
    } else if ["test", "[", "[[", "cmp"].contains(&first.as_str()) {
        "predicate"
    } else if first == "cargo" && ["test", "build", "check", "clippy"].contains(&second) {
        if second == "test" { "test" } else { "build" }
    } else if is_search_shell_command(command) {
        "search"
    } else if ["pytest", "unittest"].contains(&first.as_str())
        || ["python -m pytest", "python -m unittest"]
            .iter()
            .any(|needle| command.contains(needle))
    {
        "python-test"
    } else if first == "go" && second == "test" {
        "go-test"
    } else if ["jest", "vitest"].contains(&first.as_str())
        || ["npm", "pnpm", "yarn"].contains(&first.as_str()) && second == "test"
    {
        "test"
    } else if ["eslint", "tsc", "ruff", "mypy", "clippy"].contains(&first.as_str()) {
        "lint"
    } else if ["docker", "kubectl"].contains(&first.as_str()) || looks_status_table(combined) {
        "status"
    } else if serde_json::from_str::<serde_json::Value>(stdout.trim()).is_ok()
        || combined.contains("<testsuite")
        || combined
            .lines()
            .any(|line| line.starts_with("ok ") || line.starts_with("not ok "))
    {
        "structured"
    } else if looks_diagnostic(combined) {
        "diagnostic"
    } else {
        "generic"
    };
    family.to_string()
}
