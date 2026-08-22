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
    let direct = Command::new("reg")
        .args(["query", USER_ENVIRONMENT])
        .output()
        .unwrap();
    let observed = windows_user_path();
    if direct.status.success() {
        assert_eq!(
            observed.unwrap(),
            parse_windows_environment_path(&direct.stdout).unwrap()
        );
    } else {
        assert!(
            observed.is_err(),
            "a native reg query failure must not become an absent user Path"
        );
    }
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
    write_windows_user_path("C:\\x").expect("noop write");
    delete_windows_user_path().expect("noop delete");
}
