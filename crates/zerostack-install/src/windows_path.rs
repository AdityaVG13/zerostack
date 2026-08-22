use crate::*;

#[cfg(windows)]
const USER_ENVIRONMENT: &str = "HKCU\\Environment";

#[cfg(windows)]
pub(crate) fn is_real_windows_user_root(root: &Path) -> bool {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .is_some_and(|profile| paths_equal(&profile, root))
}

#[cfg(windows)]
pub(crate) fn is_windows_user_path_write(row: &InstallWrite) -> bool {
    row.capability == "path" && row.action == "prepend" && is_windows_user_path_entry(&row.path)
}

#[cfg(not(windows))]
pub(crate) fn is_windows_user_path_write(_: &InstallWrite) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn is_windows_user_path_entry(path: &str) -> bool {
    path.eq_ignore_ascii_case(WINDOWS_USER_PATH_REGISTRY)
}

#[cfg(not(windows))]
pub(crate) fn is_windows_user_path_entry(_: &str) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn paths_equal(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string()
    };
    normalize(a).eq_ignore_ascii_case(&normalize(b))
}

#[cfg(windows)]
pub(crate) fn windows_path_with_tokenzero_bin(root: &Path, previous: Option<&str>) -> String {
    let bin = root.join(".tokenzero").join("bin").display().to_string();
    let mut entries = vec![bin.clone()];
    entries.extend(
        previous
            .into_iter()
            .flat_map(|value| value.split(';'))
            .map(str::trim)
            .filter(|entry| !entry.is_empty() && !paths_equal(Path::new(entry), Path::new(&bin)))
            .map(str::to_owned),
    );
    entries.join(";")
}

#[cfg(not(windows))]
pub(crate) fn windows_path_with_tokenzero_bin(_: &Path, previous: Option<&str>) -> String {
    previous.unwrap_or_default().to_string()
}

#[cfg(any(windows, test))]
const USER_ENVIRONMENT_QUERY_HEADER: &str = "HKEY_CURRENT_USER\\Environment";

#[cfg(any(windows, test))]
fn parse_windows_environment_path(stdout: &[u8]) -> std::io::Result<Option<String>> {
    let text = std::str::from_utf8(stdout).map_err(|error| {
        Error::new(
            ErrorKind::InvalidData,
            format!("reg query output is not valid UTF-8: {error}"),
        )
    })?;
    if !text.ends_with('\n') {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "reg query output is truncated before its final line ending",
        ));
    }
    let mut saw_header = false;
    let mut path = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !saw_header {
            if line.starts_with(char::is_whitespace)
                || !trimmed.eq_ignore_ascii_case(USER_ENVIRONMENT_QUERY_HEADER)
            {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "reg query output is missing the expected HKCU Environment header",
                ));
            }
            saw_header = true;
            continue;
        }
        if trimmed.eq_ignore_ascii_case(USER_ENVIRONMENT_QUERY_HEADER) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "reg query returned a duplicate HKCU Environment header",
            ));
        }
        if !line.starts_with(char::is_whitespace) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "reg query returned an unrecognized non-indented row",
            ));
        }

        let mut cursor = 0usize;
        let mut kind_span = None;
        for token in trimmed.split_whitespace() {
            let relative = trimmed[cursor..].find(token).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "reg query row token boundaries are malformed",
                )
            })?;
            let start = cursor + relative;
            let end = start + token.len();
            if token.starts_with("REG_") {
                kind_span = Some((start, end));
                break;
            }
            cursor = end;
        }
        let (kind_start, kind_end) = kind_span.ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidData,
                "reg query row is missing a registry value type",
            )
        })?;
        let name = trimmed[..kind_start].trim_end();
        let kind = &trimmed[kind_start..kind_end];
        if name.is_empty()
            || kind.len() == "REG_".len()
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "reg query row has an invalid value name or type",
            ));
        }
        if !name.eq_ignore_ascii_case("Path") {
            continue;
        }
        if path.is_some() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "reg query returned duplicate user Path values",
            ));
        }
        if !matches!(kind, "REG_EXPAND_SZ" | "REG_SZ") {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("reg query returned unsupported user Path type {kind:?}"),
            ));
        }
        path = Some(trimmed[kind_end..].trim_start().to_string());
    }
    if !saw_header {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "reg query output is empty; user Path absence was not established",
        ));
    }
    Ok(path)
}

#[cfg(windows)]
pub(crate) fn windows_user_path() -> std::io::Result<Option<String>> {
    // Query the whole key: a successful command with no Path row means the
    // value is genuinely absent. Any command/access failure stays distinct and
    // must abort before a replacement value can be computed.
    let output = Command::new("reg")
        .args(["query", USER_ENVIRONMENT])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim().chars().take(512).collect::<String>();
        return Err(Error::other(format!(
            "reg query HKCU\\Environment failed with status {}; refusing to modify user Path{}{}",
            output.status,
            if detail.is_empty() { "" } else { ": " },
            detail,
        )));
    }
    if output.stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "reg query succeeded with unexpected stderr; refusing to modify user Path",
        ));
    }
    parse_windows_environment_path(&output.stdout)
}

#[cfg(not(windows))]
pub(crate) fn windows_user_path() -> std::io::Result<Option<String>> {
    Ok(None)
}

#[cfg(windows)]
fn update_windows_user_path(args: &[&str], failure: &'static str) -> std::io::Result<()> {
    Command::new("reg")
        .args(args)
        .status()?
        .success()
        .then_some(())
        .ok_or_else(|| Error::other(failure))
}

#[cfg(windows)]
pub(crate) fn write_windows_user_path(value: &str) -> std::io::Result<()> {
    update_windows_user_path(
        &[
            "add",
            USER_ENVIRONMENT,
            "/v",
            "Path",
            "/t",
            "REG_EXPAND_SZ",
            "/d",
            value,
            "/f",
        ],
        "failed to update HKCU user Path",
    )
}

#[cfg(not(windows))]
pub(crate) fn write_windows_user_path(_: &str) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub(crate) fn delete_windows_user_path() -> std::io::Result<()> {
    update_windows_user_path(
        &["delete", USER_ENVIRONMENT, "/v", "Path", "/f"],
        "failed to delete HKCU user Path",
    )
}

#[cfg(not(windows))]
pub(crate) fn delete_windows_user_path() -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/install/inline/windows_path__parse_tests.rs"]
mod parse_tests;
