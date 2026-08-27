use assert_cmd::prelude::*;
use serde_json::{Value, json};
#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
use tempfile::{TempDir, tempdir};
fn hook_output(payload: &str, mode: Option<&str>, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::cargo_bin("tokenzero").unwrap();
    command.args(["hook", "claude-code"]);
    if let Some(mode) = mode {
        command.args(["--mode", mode]);
    }
    command.envs(envs.iter().copied());
    let mut child = command
        .env("NO_COLOR", "1")
        .env("CI", "true")
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn bash_payload(command: &str) -> String {
    json!({
        "session_id": "conformance",
        "cwd": "/tmp",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command, "description": "conformance case"}
    })
    .to_string()
}
fn rewritten_command(original: &str) -> String {
    let output = hook_output(&bash_payload(original), None, &[]);
    assert!(
        output.status.success(),
        "hook failed for {original:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("non-JSON hook output for {original:?}: {err}"));
    let hook = &decision["hookSpecificOutput"];
    assert_eq!(hook["hookEventName"], "PreToolUse");
    assert_eq!(hook["permissionDecision"], "allow");
    assert_eq!(
        hook["updatedInput"]["description"], "conformance case",
        "sibling tool_input keys must survive the rewrite"
    );
    hook["updatedInput"]["command"]
        .as_str()
        .unwrap()
        .to_string()
}
fn assert_silent(payload: &str, mode: Option<&str>, envs: &[(&str, &str)], label: &str) {
    let output = hook_output(payload, mode, envs);
    assert!(output.status.success(), "hook must exit 0 for {label}");
    assert!(
        output.stdout.is_empty(),
        "expected no decision for {label}, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
fn assert_passthrough(command: &str) {
    assert_silent(&bash_payload(command), None, &[], &format!("{command:?}"));
}
fn run_sh(command: &str, cwd: &Path) -> Output {
    Command::new("sh")
        .args(["-c", command])
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("CI", "true")
        .env("TERM", "dumb")
        .env("TOKENZERO_CACHE_PATH", cwd.join("recovery-cache.json"))
        .env("TOKENZERO_REF_INDEX", "0")
        .output()
        .unwrap()
}
fn exit_parity(original: &str) -> (Output, Output, TempDir) {
    let dir = tempdir().unwrap();
    let rewritten = rewritten_command(original);
    let original_output = run_sh(original, dir.path());
    let wrapped_output = run_sh(&rewritten, dir.path());
    assert_eq!(
        original_output.status.code(),
        wrapped_output.status.code(),
        "exit-code parity broken for {original:?}\nrewritten: {rewritten}\nwrapped stdout:\n{}\nwrapped stderr:\n{}",
        String::from_utf8_lossy(&wrapped_output.stdout),
        String::from_utf8_lossy(&wrapped_output.stderr)
    );
    (original_output, wrapped_output, dir)
}
fn combined_ref(capsule: &str) -> Option<String> {
    capsule
        .lines()
        .find_map(|line| line.split("combined_ref:").nth(1))
        .map(|reference| reference.trim().to_string())
}
fn expand_ref(reference: &str, cwd: &Path) -> String {
    let output = Command::cargo_bin("tokenzero")
        .unwrap()
        .args(["expand", reference, "--raw"])
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("TOKENZERO_CACHE_PATH", cwd.join("recovery-cache.json"))
        .env("TOKENZERO_REF_INDEX", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "expand {reference} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}
fn assert_bytes_recoverable(expected: &str, wrapped: &Output, cwd: &Path) {
    let capsule = String::from_utf8_lossy(&wrapped.stdout).to_string();
    if let Some(reference) = combined_ref(&capsule) {
        let recovered = expand_ref(&reference, cwd);
        assert!(
            recovered.contains(expected),
            "original bytes not recoverable\nexpected:\n{expected}\nrecovered:\n{recovered}\ncapsule:\n{capsule}"
        );
    } else {
        assert_eq!(
            capsule.trim_end_matches('\n'),
            expected.trim_end_matches('\n'),
            "passthrough render diverged from original stdout"
        );
    }
}
fn assert_output_parity(original: &str) {
    let (original_output, wrapped_output, dir) = exit_parity(original);
    let expected = String::from_utf8_lossy(&original_output.stdout).to_string();
    assert!(
        !expected.is_empty(),
        "output-parity case {original:?} produced no stdout to compare"
    );
    assert_bytes_recoverable(&expected, &wrapped_output, dir.path());
}
struct OutputCase {
    id: &'static str,
    command: &'static str,
}
struct PassthroughCase {
    id: &'static str,
    run: fn(),
}
macro_rules! passthrough_cases { ($($name:ident $body:block)*) => { $(fn $name() $body)* const CASES: &[PassthroughCase] = &[$(PassthroughCase { id: stringify!($name), run: $name }),*]; } }
passthrough_cases! {
exit_code_parity_for_true_false_and_explicit_codes { for (command,expected) in [("true",0),("false",1),("sh -c 'exit 3'",3)] {assert_eq!(exit_parity(command).0.status.code(),Some(expected),"{command}");} }
output_parity_contract_matrix { let cases = [OutputCase {id: "pipe_output_and_exit_parity",command: "printf 'a\nb\n' | grep a"},OutputCase {id: "and_chain_parity",command: "echo one && echo two"},OutputCase {id: "semicolon_sequence_parity",command: "echo a; echo b"},OutputCase {id: "single_quote_parity",command: "echo 'single quoted text'"},OutputCase {id: "double_quote_with_embedded_single_quote_parity",command: "echo \"it's quoted\""},OutputCase {id: "dollar_home_expands_at_run_time_not_earlier",command: "echo \"$HOME\""},OutputCase {id: "shell_variables_resolve_inside_the_wrapper_not_before",command: "TZ_CONF=inner-value; echo \"$TZ_CONF\""},OutputCase {id: "unicode_parity",command: "echo '— 日本語'"},];for case in cases {assert_output_parity(case.command);eprintln!("case={}",case.id);} }
failing_pipe_keeps_masked_exit_zero { assert_eq!(exit_parity("false | cat").0.status.code(),Some(0)); }
backticks_in_single_quotes_stay_literal { let (original,wrapped,dir) = exit_parity("echo 'tick `date` tock'");assert_eq!(String::from_utf8_lossy(&original.stdout),"tick `date` tock\n");assert_bytes_recoverable("tick `date` tock",&wrapped,dir.path()); }
embedded_newline_runs_both_lines { let (original,wrapped,dir) = exit_parity("echo first\necho second");assert_eq!(String::from_utf8_lossy(&original.stdout),"first\nsecond\n");assert_bytes_recoverable("first\nsecond",&wrapped,dir.path()); }
large_output_capsule_recovers_exact_bytes_via_combined_ref { let (original,wrapped,dir) = exit_parity("seq 1 5000");assert_eq!(original.status.code(),Some(0));let capsule = String::from_utf8_lossy(&wrapped.stdout);let reference = combined_ref(&capsule).unwrap_or_else(|| panic!("expected a capsule with refs for large output:\n{capsule}"));let recovered = expand_ref(&reference,dir.path());assert!(recovered.contains("1\n2\n3\n"),"{recovered}");assert!(recovered.contains("4999\n5000"),"{recovered}"); }
stderr_and_exit_code_parity { let (original,wrapped,dir) = exit_parity("sh -c 'echo err >&2; exit 4'");assert_eq!(original.status.code(),Some(4));assert_bytes_recoverable("err",&wrapped,dir.path()); }
skip_cases_pass_through_unwrapped { for command in ["cd /tmp && ls","make && cd build && ctest","git clone x && cd x && npm install","export FOO=1 && make","echo bg &","server -d & sleep 1 && curl localhost:8080","cat <<EOF\nheredoc body\nEOF","vim file","make && vim notes.txt","tokenzero doctor --json","echo this mentions tokenzero","TOKENZERO_NO_WRAP=1 npm test","","   ",] {assert_passthrough(command);} }
quoted_operators_and_redirects_still_wrap { for command in ["echo 'a & b'","echo \"a & b\"","cargo test 2>&1","cdk deploy && ls",] {let output = hook_output(&bash_payload(command),None,&[]);assert!(output.status.success());assert!(!output.stdout.is_empty(),"expected a rewrite decision for {command:?}");} }
malformed_json_exits_zero_with_no_output { assert_silent("{this is not json",None,&[],"malformed JSON"); }
non_bash_tool_exits_zero_with_no_output { let payload = json!({"hook_event_name": "PreToolUse","tool_name": "Read","tool_input":{"file_path": "/tmp/x"}}).to_string();assert_silent(&payload,None,&[],"non-Bash tool"); }
missing_tool_input_exits_zero_with_no_output { assert_silent(&json!({"tool_name": "Bash"}).to_string(),None,&[],"missing tool input",); }
no_wrap_env_disables_rewrites { assert_silent(&bash_payload("true"),None,&[("TOKENZERO_NO_WRAP","1")],"TOKENZERO_NO_WRAP=1",); }
no_wrap_env_zero_keeps_wrapping_on { let output = hook_output(&bash_payload("true"),None,&[("TOKENZERO_NO_WRAP","0")]);assert!(output.status.success());assert!(!output.stdout.is_empty(),"TOKENZERO_NO_WRAP=0 must keep wrapping enabled"); }
guide_mode_denies_with_tokenzero_steer { let output = hook_output(&bash_payload("true"),Some("guide"),&[]);assert!(output.status.success());let decision:Value = serde_json::from_slice(&output.stdout).unwrap();let hook = &decision["hookSpecificOutput"];assert_eq!(hook["hookEventName"],"PreToolUse");assert_eq!(hook["permissionDecision"],"deny");assert!(hook["permissionDecisionReason"].as_str().unwrap().contains("TokenZero"));assert!(hook.get("updatedInput").is_none()); }
off_mode_always_passes_through { assert_silent(&bash_payload("true"),Some("off"),&[],"off mode"); }
unknown_mode_fails_open_to_passthrough { assert_silent(&bash_payload("true"),Some("rewirte"),&[],"unknown mode"); }
}
#[test]
fn passthrough_conformance_contract_matrix() {
    for case in CASES {
        (case.run)();
        eprintln!("case={}", case.id);
    }
}

/// tokenzero-3h4n guard: the exact nested login-shell npm/node probe must
/// survive the production hook rewrite byte-for-byte and produce the same
/// output as native `zsh -lic` execution. Any quoting collapse, argv
/// re-split, or login-env bypass breaks the stdout parity below.
#[cfg(unix)]
#[test]
fn nested_zsh_login_probe_preserves_authored_quoting_and_output() {
    let probe = "zsh -lic 'printf \"npm-path: \"; command -v npm; printf \"npm-type: \"; type -a npm; printf \"npm-version: \"; npm --version; printf \"node-version: \"; node --version'";
    let rewritten = rewritten_command(probe);
    // The hook re-encodes quoting for the outer `run -- sh -c '...'` layer
    // (standard `'"'"'` single-quote embedding), so the authored payload must
    // survive semantically: interpreter flags and every probe segment must
    // still be present, never split into separate argv words. Exact bytes are
    // verified below by executing the rewritten command.
    for fragment in [
        "zsh -lic",
        "npm-path: ",
        "command -v npm",
        "npm-type: ",
        "type -a npm",
        "npm-version: ",
        "node-version: ",
    ] {
        assert!(
            rewritten.contains(fragment),
            "probe fragment {fragment:?} was lost in the rewrite\noriginal:  {probe}\nrewritten: {rewritten}"
        );
    }
    let dir = tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let write_executable = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    };
    // The production boundary owns authored argv, not zsh itself. Use a
    // deterministic zsh-shaped fixture so this regression also runs on Spark
    // images without zsh. The fixture rejects any altered `-lic` argv, then
    // runs the exact authored command through bash for `type -a` semantics.
    write_executable(
        "zsh",
        "#!/bin/sh\n[ \"$1\" = -lic ] || { printf 'bad zsh argv: <%s>\\n' \"$1\" >&2; exit 64; }\nshift\nexec /bin/bash -c \"$1\"\n",
    );
    write_executable("npm", "#!/bin/sh\nprintf 'fixture-npm\\n'\n");
    write_executable("node", "#!/bin/sh\nprintf 'fixture-node\\n'\n");
    let run_probe = |command: &str| {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(dir.path())
            .env_clear()
            .env("HOME", dir.path())
            .env("PATH", format!("{}:/bin:/usr/bin", bin.display()))
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env(
                "TOKENZERO_CACHE_PATH",
                dir.path().join("recovery-cache.json"),
            )
            .env("TOKENZERO_REF_INDEX", "0")
            .output()
            .unwrap()
    };
    let native = run_probe(probe);
    let wrapped = run_probe(&rewritten);
    assert_eq!(
        native.status.code(),
        wrapped.status.code(),
        "exit-code parity broken for {probe:?}\nrewritten: {rewritten}\nwrapped stdout:\n{}\nwrapped stderr:\n{}",
        String::from_utf8_lossy(&wrapped.stdout),
        String::from_utf8_lossy(&wrapped.stderr)
    );
    let native_stdout = String::from_utf8_lossy(&native.stdout).to_string();
    // Every probe segment must run (login PATH resolves npm/node); a missing
    // executable or a collapsed segment fails on its own label.
    for label in ["npm-path:", "npm-type:", "npm-version:", "node-version:"] {
        assert!(
            native_stdout.contains(label),
            "probe segment {label:?} produced no output; login environment not exercised:\n{native_stdout}\nnative stderr:\n{}\nnative exit: {:?}",
            String::from_utf8_lossy(&native.stderr),
            native.status.code()
        );
    }
    // Exact stdout bytes survive the wrapper (inline or via combined ref).
    assert_bytes_recoverable(&native_stdout, &wrapped, dir.path());
}
