use super::*;

#[test]
fn safety_finds_mutations_behind_shell_prefixes() {
    for command in [
        "{ rm -rf /tmp/tokenzero-bypass; }",
        "TOKENZERO_TEST=1 rm -rf /tmp/tokenzero-bypass",
        "time rm -rf /tmp/tokenzero-bypass",
        "command rm -rf /tmp/tokenzero-bypass",
    ] {
        let result = rewrite_command(command, "on", true);
        assert!(!result.safe, "unexpectedly vouched: {command}");
        assert!(
            result.reason.contains("mutation") || result.reason.contains("dispatcher"),
            "mutation prefix was not classified for {command}: {}",
            result.reason
        );
    }
}

#[test]
fn embedded_interpreter_payloads_are_never_vouched() {
    for command in [
        "python3 -c 'from pathlib import Path; Path(\"x\").unlink()'",
        "node -e 'require(\"fs\").rmSync(\"x\")'",
        "cmd.exe /C del x",
        "pwsh -Command 'Remove-Item x'",
    ] {
        let result = rewrite_command(command, "on", true);
        assert!(!result.safe, "unexpectedly vouched: {command}");
        assert_eq!(
            result.reason, "embedded interpreter payload left unmodified",
            "embedded payload was not classified for {command}"
        );
    }
}

#[test]
fn windows_executable_suffixes_and_backslashes_reach_safety_rules() {
    let words = vec![
        r"C:\Tools\RM.EXE".to_string(),
        "-rf".to_string(),
        "target".to_string(),
    ];
    assert_eq!(executable_name(&words[0]), "RM");
    assert_eq!(
        unsafe_reason_for_words(&words).as_deref(),
        Some("unsafe destructive mutation left unmodified")
    );
}
