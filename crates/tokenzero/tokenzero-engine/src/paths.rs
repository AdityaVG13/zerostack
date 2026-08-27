use crate::*;

pub(crate) fn comparable_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    normalize_windows_verbatim(canonicalize_existing_prefix(&absolute))
}

pub(crate) fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut cursor = path;
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        if let Ok(mut canonical) = cursor.canonicalize() {
            for part in suffix.iter().rev() {
                canonical.push(part);
            }
            return canonical;
        }
        let Some(name) = cursor.file_name() else {
            return path.to_path_buf();
        };
        suffix.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return path.to_path_buf();
        };
        cursor = parent;
    }
}

#[cfg(windows)]
pub(crate) fn normalize_windows_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
pub(crate) fn normalize_windows_verbatim(path: PathBuf) -> PathBuf {
    path
}
