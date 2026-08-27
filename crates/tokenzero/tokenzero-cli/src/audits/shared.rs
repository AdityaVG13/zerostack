use std::path::Path;

pub(crate) type ProtectedAnchorCaseDef = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
);

pub(crate) const PROTECTED_ANCHOR_CASES_DEF: &[ProtectedAnchorCaseDef] = &[
    (
        "failing_test_assertion",
        "nonzero test output keeps exit code, failing test, path line, assertion, stderr ref, and combined ref",
        &[
            "tests::alpha",
            "src/lib.rs:42",
            "assertion failed",
            "left: 1",
            "right: 2",
            "status: command_failed",
            "exit_code: 101",
            "stderr_ref:",
            "combined_ref:",
        ],
        "echo 'running 1 test'; echo 'test tests::alpha ... FAILED'; echo 'src/lib.rs:42:9: assertion failed: left == right' >&2; echo 'left: 1' >&2; echo 'right: 2' >&2; echo 'error: test failed' >&2; exit 101",
        "Write-Output 'running 1 test'; Write-Output 'test tests::alpha ... FAILED'; [Console]::Error.WriteLine('src/lib.rs:42:9: assertion failed: left == right'); [Console]::Error.WriteLine('left: 1'); [Console]::Error.WriteLine('right: 2'); [Console]::Error.WriteLine('error: test failed'); exit 101",
    ),
    (
        "warning_changed_file",
        "warning output keeps warning and changed-file anchors",
        &[
            "warning: unused import",
            "M src/main.rs",
            "modified: src/lib.rs",
            "combined_ref:",
        ],
        "echo 'warning: unused import'; echo 'M src/main.rs'; echo 'modified: src/lib.rs'",
        "Write-Output 'warning: unused import'; Write-Output 'M src/main.rs'; Write-Output 'modified: src/lib.rs'",
    ),
    (
        "diff_hunk",
        "diff output keeps changed path, hunk, and added line anchors",
        &[
            "diff --git",
            "src/main.rs",
            "@@ -1 +1 @@",
            "+new",
            "combined_ref:",
        ],
        "printf 'diff --git a/src/main.rs b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n'",
        "Write-Output 'diff --git a/src/main.rs b/src/main.rs'; Write-Output '@@ -1 +1 @@'; Write-Output '-old'; Write-Output '+new'",
    ),
];

pub(crate) fn run_json_args(root: &str, cache: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "--json".to_string(),
        "--cache-path".to_string(),
        cache.to_string(),
        "--allowed-root".to_string(),
        root.to_string(),
        "--cwd".to_string(),
        root.to_string(),
    ]
}

pub(crate) fn one_shot_shell_args(root: &Path, cache: &Path, command: &str) -> Vec<String> {
    let mut args = run_json_args(&root.to_string_lossy(), &cache.to_string_lossy());
    args.push("--".to_string());
    if cfg!(windows) {
        args.extend([
            "powershell".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
            command.to_string(),
        ]);
    } else {
        args.extend(["sh".to_string(), "-c".to_string(), command.to_string()]);
    }
    args
}
