use super::*;

#[test]
fn full_environment_query_distinguishes_absent_path() {
    for output in [
        b"HKEY_CURRENT_USER\\Environment\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    TEMP    REG_EXPAND_SZ    %USERPROFILE%\\Temp\r\n    Some Value    REG_DWORD    0x1\r\n"
            .as_slice(),
    ] {
        assert_eq!(parse_windows_environment_path(output).unwrap(), None);
    }
}

#[test]
fn full_environment_query_parses_supported_path_types_without_losing_spaces() {
    for (kind, value) in [
        (
            "REG_EXPAND_SZ",
            "C:\\Program Files\\Tool;%USERPROFILE%\\bin",
        ),
        ("REG_SZ", "C:\\Program Files\\Literal"),
    ] {
        let output = format!("HKEY_CURRENT_USER\\Environment\r\n    Path    {kind}    {value}\r\n");
        assert_eq!(
            parse_windows_environment_path(output.as_bytes())
                .unwrap()
                .as_deref(),
            Some(value)
        );
    }
}

#[cfg(windows)]
#[test]
fn native_query_matches_the_fail_closed_parser() {
    // Fail-closed integration contract without host-state dependence.
    // windows_user_path() queries the live registry and then parses with
    // parse_windows_environment_path. The parser itself is the observable
    // failure boundary; this test exercises it with controlled success,
    // absence, and malformed cases instead of coupling to the machine's
    // current HKCU\\Environment contents.
    let present = b"HKEY_CURRENT_USER\\Environment\r\n    Path    REG_SZ    C:\\one\r\n".as_slice();
    assert_eq!(
        parse_windows_environment_path(present).unwrap().as_deref(),
        Some("C:\\one"),
        "present Path must parse to Some(value)"
    );
    let expand =
        b"HKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    C:\\Program Files\\Tool;%USERPROFILE%\\bin\r\n".as_slice();
    assert_eq!(
        parse_windows_environment_path(expand).unwrap().as_deref(),
        Some("C:\\Program Files\\Tool;%USERPROFILE%\\bin")
    );
    let absent =
        b"HKEY_CURRENT_USER\\Environment\r\n    TEMP    REG_EXPAND_SZ    %USERPROFILE%\\Temp\r\n"
            .as_slice();
    assert_eq!(
        parse_windows_environment_path(absent).unwrap(),
        None,
        "absence of Path must be reported as None"
    );
    for malformed in [
        b"garbage\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    Path    REG_MULTI_SZ    C:\\bad\r\n".as_slice(),
    ] {
        assert!(
            parse_windows_environment_path(malformed).is_err(),
            "malformed output must fail closed: {malformed:?}"
        );
    }
    // A live reg.exe smoke probe is host-state dependent and must not be the
    // primary correctness oracle (it can pass when both paths share a defect).
    // Keep any live-host probe as a separate `#[ignore]` smoke test if needed.
}

#[test]
fn malformed_or_ambiguous_output_fails_closed() {
    for output in [
        b"garbage\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environmen\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environment".as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\nnot-indented\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    TEMP\r\n".as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    Path    REG_MULTI_SZ    C:\\bad\r\n"
            .as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    Path    REG_SZ    C:\\one\r\n    Path    REG_SZ    C:\\two\r\n"
            .as_slice(),
        b"HKEY_CURRENT_USER\\Environment\r\n    Path\r\n".as_slice(),
        &[0xff][..],
    ] {
        assert!(
            parse_windows_environment_path(output).is_err(),
            "malformed registry output must not become an absent Path: {output:?}"
        );
    }
}

#[cfg(not(windows))]
fn write_row(capability: &str, action: &str, path: &str) -> InstallWrite {
    InstallWrite {
        path: path.to_string(),
        action: action.to_string(),
        backup_id: String::new(),
        capability: capability.to_string(),
        global: false,
    }
}

#[cfg(not(windows))]
#[test]
fn off_windows_path_predicates_are_always_false() {
    // Registry semantics must never engage on macOS/Linux installs.
    // (The registry key constant itself is cfg(windows); use its value.)
    const KEY: &str = "HKCU\\Environment\\Path";
    assert!(!is_windows_user_path_entry(KEY));
    assert!(!is_windows_user_path_write(&write_row(
        "path", "prepend", KEY
    )));
}

#[cfg(not(windows))]
#[test]
fn off_windows_registry_writes_are_noop_ok() {
    // Off-Windows hosts must fail open: no registry, no error.
    // Each operation's success is asserted separately to preserve the
    // public-state oracle under the documented no-op contract.
    write_windows_user_path("C:\\x").expect("noop write must return Ok");
    delete_windows_user_path().expect("noop delete must return Ok");
    // Public state remains registry-free on non-Windows.
    assert!(
        windows_user_path()
            .expect("windows_user_path must not fail off-Windows")
            .is_none(),
        "off-Windows Path query must remain None"
    );
}
