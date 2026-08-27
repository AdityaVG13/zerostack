use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// Canonicalize a workspace root with the standard `bad root:` error class.
#[inline]
pub fn canonicalize_root(root: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(root).map_err(|e| format!("bad root: {e}"))
}

/// Rejection text for a write/rollback target that resolves outside the
/// workspace root. Writes are jailed to the session root; the declared
/// read-only scratch dir (`FSZERO_SCRATCH_DIR`) widens READS only and never
/// authorizes a write. Must keep the substring `outside root` so
/// `classify_detail_to_error_class` maps it to the `outside_root` class.
pub const ROLLBACK_OUTSIDE_ROOT: &str = "rollback path outside root: writes are jailed to the workspace root; the declared read-only scratch dir (FSZERO_SCRATCH_DIR) widens reads only, never writes; write the file under the session root and return a reference to it instead";

/// Canonicalize an existing path with the standard `not found:` error class.
#[inline]
pub fn canonicalize_existing(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|e| format!("not found: {e}"))
}

/// Pure path model used by cross-platform contract fixtures. Static tests may
/// exercise a non-host model, but must never label it as a host result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformPathModel {
    Unix,
    Windows,
}

impl PlatformPathModel {
    pub const fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

fn windows_reserved_component(component: &str) -> bool {
    let stem = component
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        || stem
            .strip_prefix("LPT")
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

/// Normalize one workspace-relative operation path under an explicit platform
/// model. This is lexical validation only; containment still requires host
/// canonicalization at the I/O boundary.
pub fn normalize_relative_for_platform(
    arg: &str,
    model: PlatformPathModel,
) -> Result<String, String> {
    if arg.is_empty() {
        return Err("empty path rejected".to_string());
    }
    if arg.as_bytes().contains(&0) {
        return Err("NUL path rejected".to_string());
    }
    let windows = model == PlatformPathModel::Windows;
    if (windows
        && (arg.starts_with(['/', '\\']) || {
            let bytes = arg.as_bytes();
            bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
        }))
        || (!windows && arg.starts_with('/'))
    {
        return Err("absolute path rejected".to_string());
    }

    let components = arg.split(|character| character == '/' || (windows && character == '\\'));
    let mut normalized = String::with_capacity(arg.len());
    for component in components {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err("parent traversal rejected".to_string());
        }
        if windows {
            if component.ends_with([' ', '.']) {
                return Err("Windows trailing dot/space rejected".to_string());
            }
            if component
                .chars()
                .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            {
                return Err("Windows reserved character rejected".to_string());
            }
            if windows_reserved_component(component) {
                return Err("Windows reserved name rejected".to_string());
            }
        }
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    if normalized.is_empty() {
        return Err("empty path rejected".to_string());
    }
    Ok(normalized)
}

pub fn sanitize_relative_arg(arg: &str) -> Result<PathBuf, String> {
    normalize_relative_for_platform(arg, PlatformPathModel::host()).map(PathBuf::from)
}

/// True when `target` is `root` or a path beneath `root` (component-safe, not string prefix).
///
/// Callers that pass non-canonical paths (missing-file rollback) MUST
/// `lexical_normalize` first — this helper only inspects the strip-prefix rest
/// and does not re-resolve `..` (fszero-w2g.23 / .48).
pub fn canonical_path_within_root(root_canon: &Path, target: &Path) -> bool {
    if target == root_canon {
        return true;
    }
    match target.strip_prefix(root_canon) {
        Ok(rest) if !rest.as_os_str().is_empty() => {
            !rest.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }) && matches!(
                rest.components().next(),
                Some(Component::Normal(_)) | Some(Component::CurDir)
            )
        }
        _ => false,
    }
}

fn unicode_match_key(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '\u{202f}' | '\u{00a0}' => ' ',
            other => other,
        })
        .nfc()
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_char != *right_char)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

fn not_found_with_candidates(path: &Path, source: &std::io::Error) -> String {
    let Some(parent) = path.parent() else {
        return format!("not found: {source}");
    };
    let needle = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let Ok(entries) = fs::read_dir(parent) else {
        return format!("not found: {source}");
    };
    let mut ranked: Vec<(usize, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((
                edit_distance(&unicode_match_key(needle), &unicode_match_key(&name)),
                name,
            ))
        })
        .collect();
    ranked.sort();
    let candidates: Vec<String> = ranked.into_iter().take(3).map(|(_, name)| name).collect();
    if candidates.is_empty() {
        format!("not found: {source}")
    } else {
        format!("not found: {source}; candidates: {}", candidates.join(", "))
    }
}

fn canonicalize_existing_tolerant(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let Some(parent) = path.parent() else {
                return Err(format!("not found: {source}"));
            };
            let Some(needle) = path.file_name().and_then(|name| name.to_str()) else {
                return Err(not_found_with_candidates(path, &source));
            };
            let key = unicode_match_key(needle);
            let mut matches: Vec<(String, PathBuf)> = fs::read_dir(parent)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    (unicode_match_key(&name) == key).then(|| (name, entry.path()))
                })
                .collect();
            matches.sort_by(|left, right| left.0.cmp(&right.0));
            match matches.len() {
                1 => fs::canonicalize(&matches[0].1).map_err(|error| format!("not found: {error}")),
                2.. => Err(format!(
                    "ambiguous unicode path {}; candidates: {}",
                    path.display(),
                    matches
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                _ => Err(not_found_with_candidates(path, &source)),
            }
        }
        Err(source) => Err(format!("not found: {source}")),
    }
}

/// True when `arg` means the session workspace root (not a child path).
/// Accepts `.`, `./`, empty, and whitespace-only (fszero-n1qc).
pub fn is_session_root_arg(arg: &str) -> bool {
    let t = arg.trim();
    t.is_empty() || t == "." || t == "./" || t == ".\\" || t == ".//"
}

pub fn resolve_existing_path(root: Option<&Path>, arg: &str) -> Result<PathBuf, String> {
    if is_session_root_arg(arg) {
        if let Some(root) = root {
            return canonicalize_root(root).map_err(|e| {
                format!(
                    "{e}; path '.' means the session workspace root — ensure zero_execute root is an existing directory (absolute path preferred)"
                )
            });
        }
        return canonicalize_existing(Path::new(".")).map_err(|e| {
            format!("{e}; no session root set — pass path relative to a configured root, or set the workspace root")
        });
    }
    let rel = sanitize_relative_arg(arg).map_err(|e| {
        format!(
            "{e}; paths must be relative to the session root (use path:'.' for the root, or a child like 'src/'); absolute paths and '..' are rejected"
        )
    })?;
    if let Some(root) = root {
        let root_canon = canonicalize_root(root)?;
        let target_canon = canonicalize_existing_tolerant(&root_canon.join(&rel)).map_err(|e| {
            format!(
                "{e}; path must exist under the session root (use path:'.' to list the workspace root)"
            )
        })?;
        if !canonical_path_within_root(&root_canon, &target_canon) {
            return Err("resolved outside root".to_string());
        }
        Ok(target_canon)
    } else {
        canonicalize_existing_tolerant(&rel)
    }
}

pub fn ensure_path_under_root(root: Option<&Path>, path: &Path) -> Result<(), String> {
    if let Some(root) = root {
        revalidate_path_under_root(root, path)?;
    }
    Ok(())
}

/// Re-canonicalize `path` and verify it remains under `root` (TOCTOU guard).
pub fn revalidate_path_under_root(root: &Path, path: &Path) -> Result<PathBuf, String> {
    revalidate_path_under_root_canon(None, root, path)
}

/// Like [`revalidate_path_under_root`], but reuses a session-cached root
/// canonical path so warm ops avoid re-canonicalizing the workspace root.
pub fn revalidate_path_under_root_canon(
    root_canon: Option<&Path>,
    root: &Path,
    path: &Path,
) -> Result<PathBuf, String> {
    let owned;
    let root_canon = match root_canon {
        Some(c) => c,
        None => {
            owned = canonicalize_root(root)?;
            owned.as_path()
        }
    };
    let target_canon = canonicalize_existing(path)?;
    if !canonical_path_within_root(root_canon, &target_canon) {
        return Err("resolved outside root".to_string());
    }
    Ok(target_canon)
}

/// Validate a rollback target without requiring the file to still exist.
/// Write-side jail: the target must resolve under `root`; the read-only
/// scratch allowlist does not apply here (see `ROLLBACK_OUTSIDE_ROOT`).
pub fn validate_rollback_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root_canon = canonicalize_root(root)?;
    // Always join relative args under `root` first. `path.exists()` follows
    // links and is CWD-relative for non-absolute inputs, which would both
    // write-through a tail symlink and jail against the process cwd.
    // Full lexical normalize BEFORE within-root check so
    // `nested/../../OUTSIDE` cannot pass via a first-component Normal guard
    // (fszero-w2g.23 / .48).
    let joined = if path.is_absolute() {
        lexical_normalize(path)
    } else {
        lexical_normalize(&root_canon.join(path))
    };
    if !canonical_path_within_root(&root_canon, &joined) {
        return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
    }
    // Mid-path aware (V6-F1 / ZS-SEC-001): a symlink in an ancestor of a
    // not-yet-existing target is resolved hop-by-hop so a write through
    // `root/sub/link-out/new.txt` cannot land outside root. A TAIL symlink
    // (live or dangling) is the write directory entry itself
    // (filesystem-v1 replace-link-entry) — never followed.
    resolve_missing_path(&root_canon, &joined, true)
}

/// Canonical parent + unfollowed last component. `rename` replaces this
/// directory entry instead of opening through the referent.
fn unfollowed_tail_symlink(root_canon: &Path, link: &Path) -> Result<PathBuf, String> {
    let parent = link
        .parent()
        .ok_or_else(|| ROLLBACK_OUTSIDE_ROOT.to_string())?;
    let parent_canon = fs::canonicalize(parent).map_err(|e| format!("rollback path: {e}"))?;
    if !canonical_path_within_root(root_canon, &parent_canon) {
        return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
    }
    let name = link
        .file_name()
        .ok_or_else(|| ROLLBACK_OUTSIDE_ROOT.to_string())?;
    Ok(parent_canon.join(name))
}

/// Resolve a write/rollback target for a root-jailed mutation.
///
/// Mid-path symlink hops that leave `root_canon` are refused (V6-F1 /
/// ZS-SEC-001). `joined` must already be lexically normalized and pass
/// `canonical_path_within_root` (the caller's contract). A tail symlink is
/// returned unfollowed so `atomic_write` rename-replaces the link inode
/// (filesystem-v1 / contract-v1); it is never write-through.
fn resolve_missing_path(
    root_canon: &Path,
    joined: &Path,
    replace_tail_symlink: bool,
) -> Result<PathBuf, String> {
    // Find the deepest ancestor that is stat-able (exists, or is itself a
    // possibly dangling symlink). Mid-path directory links resolve THROUGH
    // (canonicalize / dangling chase, within-root per hop). A tail symlink
    // on a write target is the directory entry being replaced — do not
    // follow it. Parent resolution (`guard_write_target_parent`) still
    // follows so a dir symlink to outside cannot jailbreak.
    let mut ancestor: &Path = joined;
    loop {
        match ancestor.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                let suffix = joined.strip_prefix(ancestor).unwrap_or(Path::new(""));
                if suffix.as_os_str().is_empty() && replace_tail_symlink {
                    return unfollowed_tail_symlink(root_canon, ancestor);
                }
                let resolved = resolve_dangling_link(root_canon, ancestor.to_path_buf())?;
                let out = if suffix.as_os_str().is_empty() {
                    resolved
                } else {
                    lexical_normalize(&resolved.join(suffix))
                };
                return if canonical_path_within_root(root_canon, &out) {
                    Ok(out)
                } else {
                    Err(ROLLBACK_OUTSIDE_ROOT.to_string())
                };
            }
            Ok(_) => {
                let canon =
                    fs::canonicalize(ancestor).map_err(|e| format!("rollback path: {e}"))?;
                if !canonical_path_within_root(root_canon, &canon) {
                    return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
                }
                let suffix = joined.strip_prefix(ancestor).unwrap_or(joined);
                let out = if suffix.as_os_str().is_empty() {
                    canon
                } else {
                    lexical_normalize(&canon.join(suffix))
                };
                return if canonical_path_within_root(root_canon, &out) {
                    Ok(out)
                } else {
                    Err(ROLLBACK_OUTSIDE_ROOT.to_string())
                };
            }
            Err(_) => match ancestor.parent() {
                Some(parent) if parent != ancestor => ancestor = parent,
                // Unreachable for root-joined targets (the root itself exists
                // and is stat-able); fail closed rather than trust the bare
                // lexical form.
                _ => return Err(ROLLBACK_OUTSIDE_ROOT.to_string()),
            },
        }
    }
}

/// Write-time TOCTOU guard (V6-F1 / ZS-SEC-001): re-verify that the write
/// target's parent still resolves inside `root` immediately before
/// publication. Between path validation and the atomic write an attacker
/// could swap a parent directory for a symlink pointing outside the root;
/// this re-resolves the parent at the last moment (mid-path aware, tolerating
/// not-yet-created parents) and refuses any target whose parent leaves the
/// root. One walk + one canonicalize per write — cheap.
pub fn guard_write_target_parent(root: &Path, target: &Path) -> Result<(), String> {
    let root_canon = canonicalize_root(root)?;
    let Some(parent) = target.parent() else {
        return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
    };
    let joined = if parent.is_absolute() {
        // Engine targets are canonical root-joined paths; resolve through the
        // deepest stat-able ancestor (mid-path aware, tolerates missing
        // parents) and let the canonical within-root check do the verdict.
        lexical_normalize(parent)
    } else {
        let joined = lexical_normalize(&root_canon.join(parent));
        if !canonical_path_within_root(&root_canon, &joined) {
            return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
        }
        joined
    };
    resolve_missing_path(&root_canon, &joined, false)?;
    Ok(())
}

/// Chase a (possibly dangling) symlink chain lexically, keeping every hop
/// under `root_canon`. Non-links pass through unchanged.
fn resolve_dangling_link(root_canon: &Path, start: PathBuf) -> Result<PathBuf, String> {
    let mut current = start;
    let mut hops = 0;
    while current
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        hops += 1;
        if hops > 8 {
            return Err("rollback path: symlink chain too deep".to_string());
        }
        let target = fs::read_link(&current).map_err(|e| format!("rollback path: {e}"))?;
        let next = if target.is_absolute() {
            target
        } else {
            current.parent().unwrap_or(root_canon).join(&target)
        };
        // Canonicalize through the parent when it exists (directory-level
        // links resolve properly); otherwise normalize `..`/`.` lexically so
        // relative link targets cannot dodge the root check.
        current = match next.parent().and_then(|p| fs::canonicalize(p).ok()) {
            Some(parent_canon) => match next.file_name() {
                Some(name) => parent_canon.join(name),
                None => parent_canon,
            },
            None => lexical_normalize(&next),
        };
        if !canonical_path_within_root(root_canon, &current) {
            return Err(ROLLBACK_OUTSIDE_ROOT.to_string());
        }
    }
    Ok(current)
}

/// Resolve `.`/`..` components lexically (no filesystem access).
pub fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Durable atomic replace. Failures state whether publication occurred.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    atomic_write_with_outcome(path, content).map_err(|error| error.to_string())
}

/// Stable kind label for a write target. Missing paths are writable (create).
/// Regular files and (unfollowed) symlinks are allowed: rename-replace of a
/// symlink replaces the link entry, which is contract-v1. FIFOs, sockets,
/// devices, and directories are refused so we never block on a pipe or clobber
/// a special node with a regular file (fszero-ai-filesystem-excellence-jqf.1.1).
fn refuse_unsupported_write_kind(path: &Path) -> Result<(), AtomicWriteError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let kind = file_kind_label(&meta);
    if matches!(kind, "regular" | "symlink") {
        return Ok(());
    }
    Err(AtomicWriteError {
        stage: "prepare",
        published: false,
        message: format!("unsupported file kind: {kind} (not a regular file)"),
    })
}

/// Refuse opening a FIFO, socket, device, or directory for bounded file I/O.
///
/// `fs::read` / `File::open` on a FIFO blocks the worker. Metadata does not.
/// Call this before any content open (fszero-ai-filesystem-excellence-jqf.9.1).
/// Missing paths return Ok so the caller can emit not-found.
pub fn refuse_non_regular_file(path: &Path) -> Result<(), String> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(());
    };
    let kind = file_kind_label(&meta);
    if kind == "regular" {
        return Ok(());
    }
    Err(format!(
        "unsupported file kind: {kind} (not a regular file)"
    ))
}

fn file_kind_label(meta: &fs::Metadata) -> &'static str {
    let file_type = meta.file_type();
    if file_type.is_file() {
        return "regular";
    }
    if file_type.is_dir() {
        return "directory";
    }
    if file_type.is_symlink() {
        return "symlink";
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_fifo() {
            return "fifo";
        }
        if file_type.is_socket() {
            return "socket";
        }
        if file_type.is_block_device() {
            return "block-device";
        }
        if file_type.is_char_device() {
            return "char-device";
        }
    }
    "other"
}

#[derive(Debug, Clone)]
pub struct AtomicWriteError {
    pub stage: &'static str,
    pub published: bool,
    pub message: String,
}
impl std::fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "atomic write {}{}: {}",
            self.stage,
            if self.published {
                " after publication"
            } else {
                ""
            },
            self.message
        )
    }
}

// Process-environment failpoints poison unrelated parallel libtest cases.
#[cfg(test)]
std::thread_local! {
    static TEST_ATOMIC_FAILPOINT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub struct TestAtomicFailpointGuard(Option<&'static str>);

#[cfg(test)]
impl Drop for TestAtomicFailpointGuard {
    fn drop(&mut self) {
        TEST_ATOMIC_FAILPOINT.with(|failpoint| failpoint.set(self.0));
    }
}

#[cfg(test)]
pub fn test_atomic_failpoint(value: Option<&'static str>) -> TestAtomicFailpointGuard {
    let previous = TEST_ATOMIC_FAILPOINT.with(|failpoint| failpoint.replace(value));
    TestAtomicFailpointGuard(previous)
}

fn atomic_failpoint(stage: &'static str) -> Result<(), AtomicWriteError> {
    #[cfg(test)]
    let injected = TEST_ATOMIC_FAILPOINT.with(|failpoint| failpoint.get() == Some(stage));
    #[cfg(not(test))]
    let injected = std::env::var("FSZERO_ATOMIC_FAILPOINT").ok().as_deref() == Some(stage);
    if injected {
        Err(AtomicWriteError {
            stage,
            published: false,
            message: "fault injection".into(),
        })
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn xattr_requires_exact_copy(name: &std::ffi::OsStr) -> bool {
    name != std::ffi::OsStr::new("com.apple.provenance")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xattr_requires_exact_copy(_name: &std::ffi::OsStr) -> bool {
    true
}

#[cfg(unix)]
fn exact_copy_xattrs(path: &Path) -> Result<Vec<(std::ffi::OsString, Vec<u8>)>, String> {
    let names = xattr::list(path).map_err(|error| error.to_string())?;
    let mut attrs = Vec::new();
    for name in names {
        if !xattr_requires_exact_copy(&name) {
            continue;
        }
        let value = xattr::get(path, &name)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("missing xattr {}", name.to_string_lossy()))?;
        attrs.push((name, value));
    }
    attrs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(attrs)
}

#[cfg(unix)]
fn copy_xattrs_exact(src: &Path, dst: &Path) -> Result<(), String> {
    let attrs = exact_copy_xattrs(src)?;
    for (name, value) in &attrs {
        xattr::set(dst, name, value).map_err(|error| error.to_string())?;
    }
    if attrs == exact_copy_xattrs(dst)? {
        Ok(())
    } else {
        Err("xattr verification mismatch".into())
    }
}
#[cfg(not(unix))]
fn copy_xattrs_exact(_src: &Path, _dst: &Path) -> Result<(), String> {
    Ok(())
}

/// Absolute-durable file barrier: `sync_all` (POSIX fsync), then macOS
/// `fcntl(F_FULLFSYNC)` so the barrier matches SQLite FULL / marketed class.
pub fn full_sync_file(file: &fs::File) -> Result<(), String> {
    // Keep `durability`/`fsync`/`FULLFSYNC` in the detail so
    // `classify_detail_to_error_class` maps to `durability_unavailable`.
    file.sync_all()
        .map_err(|error| format!("durability unavailable: fsync failed: {error}"))?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn fcntl(fd: i32, command: i32, ...) -> i32;
        }
        const F_FULLFSYNC: i32 = 51;
        // SAFETY: `file` owns a live fd for this call. F_FULLFSYNC is Darwin
        // fcntl command 51 and takes no varargs, so the two-argument form
        // matches the libc ABI. The kernel does not retain `fd`. `-1` reports
        // failure via errno.
        if unsafe { fcntl(file.as_raw_fd(), F_FULLFSYNC) } == -1 {
            return Err(format!(
                "durability unavailable: FULLFSYNC failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn flush_dir(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const std::ffi::c_void,
            creation: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn FlushFileBuffers(handle: isize) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives this call.
    // GENERIC_READ (0x8000_0000) + FILE_SHARE_READ|WRITE|DELETE (1|2|4) +
    // OPEN_EXISTING (3) + FILE_FLAG_BACKUP_SEMANTICS (0x0200_0000) is the
    // documented way to open a directory handle. NULL security and no
    // template handle are valid. The result is either INVALID_HANDLE_VALUE
    // (-1) or an owned handle we CloseHandle below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0x8000_0000,
            1 | 2 | 4,
            std::ptr::null(),
            3,
            0x0200_0000,
            0,
        )
    };
    if handle == -1 {
        return Err("CreateFileW directory handle unsupported".into());
    }
    // SAFETY: `handle` is a live kernel handle from CreateFileW, not
    // INVALID_HANDLE_VALUE. FlushFileBuffers does not close it.
    let flushed = unsafe { FlushFileBuffers(handle) } != 0;
    // SAFETY: `handle` is uniquely owned here and has not been closed.
    // CloseHandle consumes it; it is not reused after this call.
    unsafe {
        CloseHandle(handle);
    }
    if flushed {
        Ok(())
    } else {
        Err("FlushFileBuffers failed".into())
    }
}
#[cfg(unix)]
fn flush_dir(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}
#[cfg(not(any(unix, windows)))]
fn flush_dir(_path: &Path) -> Result<(), String> {
    Err("directory flush unsupported on this platform".into())
}

#[cfg(windows)]
fn replace_file(src: &Path, dst: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let src: Vec<u16> = src
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let dst: Vec<u16> = dst
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    // SAFETY: `src` and `dst` are NUL-terminated UTF-16 buffers that outlive
    // this call. Flags MOVEFILE_REPLACE_EXISTING (1) | MOVEFILE_WRITE_THROUGH
    // (8) request replace-in-place with write-through durability. The API
    // does not retain the pointers.
    if unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), 1 | 8) } == 0 {
        Err("MoveFileExW atomic replacement failed".into())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn replace_file(src: &Path, dst: &Path) -> Result<(), String> {
    fs::rename(src, dst).map_err(|error| error.to_string())
}

static ATOMIC_TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn create_atomic_temp(parent: &Path, name: &str) -> Result<(PathBuf, fs::File), AtomicWriteError> {
    for _ in 0..32 {
        let sequence = ATOMIC_TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = parent.join(format!(
            ".{name}.fszero-write-{}-{sequence}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(AtomicWriteError {
                    stage: "temp_write",
                    published: false,
                    message: error.to_string(),
                });
            }
        }
    }
    Err(AtomicWriteError {
        stage: "temp_write",
        published: false,
        message: "could not reserve unique temp file".into(),
    })
}

/// Per-phase wall attribution for `atomic_write_with_outcome`, emitted as one
/// JSON line on stderr when `FSZERO_ATOMIC_WRITE_PHASES` is set (fszero-1unf).
/// Zero-cost when the env var is absent. Phases: prepare_dir_sync, temp_write,
/// full_sync, rename, dir_sync (us).
struct AtomicWritePhaseTimer {
    enabled: bool,
    started: std::time::Instant,
    last: std::time::Instant,
    entries: Vec<(&'static str, u64)>,
}

impl AtomicWritePhaseTimer {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: atomic_write_phases_enabled(),
            started: now,
            last: now,
            entries: Vec::new(),
        }
    }

    fn mark(&mut self, name: &'static str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        let us = now.saturating_duration_since(self.last).as_micros() as u64;
        self.entries.push((name, us));
        self.last = now;
    }

    fn finish(self, path: &Path) {
        if !self.enabled {
            return;
        }
        #[cfg(test)]
        record_test_phases(&self.entries);
        let phases: serde_json::Map<String, serde_json::Value> = self
            .entries
            .into_iter()
            .map(|(name, us)| (name.to_string(), serde_json::json!(us)))
            .collect();
        let total_us = self.started.elapsed().as_micros() as u64;
        eprintln!(
            "{}",
            serde_json::json!({
                "atomic_write_phases_us": phases,
                "total_us": total_us,
                "path": path.display().to_string(),
            })
        );
    }
}

fn atomic_write_phases_enabled() -> bool {
    #[cfg(test)]
    if TEST_ATOMIC_WRITE_PHASES.with(|flag| flag.get()) {
        return true;
    }
    match std::env::var("FSZERO_ATOMIC_WRITE_PHASES") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "yes"),
        Err(_) => false,
    }
}

#[cfg(test)]
std::thread_local! {
    static TEST_ATOMIC_WRITE_PHASES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LAST_ATOMIC_WRITE_PHASES: std::cell::RefCell<Vec<(&'static str, u64)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn record_test_phases(entries: &[(&'static str, u64)]) {
    LAST_ATOMIC_WRITE_PHASES.with(|cell| {
        *cell.borrow_mut() = entries.to_vec();
    });
}

pub fn atomic_write_with_outcome(path: &Path, content: &[u8]) -> Result<(), AtomicWriteError> {
    let mut phases = AtomicWritePhaseTimer::new();
    let parent = path.parent().ok_or_else(|| AtomicWriteError {
        stage: "prepare",
        published: false,
        message: "missing parent".into(),
    })?;
    // Prove directory durability is supported before the irreversible rename.
    flush_dir(parent).map_err(|message| AtomicWriteError {
        stage: "prepare",
        published: false,
        message,
    })?;
    phases.mark("prepare_dir_sync");
    let name = path
        .file_name()
        .ok_or_else(|| AtomicWriteError {
            stage: "prepare",
            published: false,
            message: "missing file name".into(),
        })?
        .to_string_lossy();
    refuse_unsupported_write_kind(path)?;
    // `symlink_metadata`: a tail symlink is replaced, not followed
    // (filesystem-v1 / contract-v1 replace-link-entry). Mode/xattr carry
    // applies only to a regular file at this directory entry.
    let existing = fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.file_type().is_file());
    atomic_failpoint("temp_write")?;
    let (tmp, mut file) = create_atomic_temp(parent, &name)?;
    if let Err(error) = file.write_all(content) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AtomicWriteError {
            stage: "temp_write",
            published: false,
            message: error.to_string(),
        });
    }
    if let Some(metadata) = existing {
        if let Err(error) = file.set_permissions(metadata.permissions()) {
            drop(file);
            let _ = fs::remove_file(&tmp);
            return Err(AtomicWriteError {
                stage: "metadata",
                published: false,
                message: error.to_string(),
            });
        }
        if let Err(message) = copy_xattrs_exact(path, &tmp) {
            drop(file);
            let _ = fs::remove_file(&tmp);
            return Err(AtomicWriteError {
                stage: "metadata",
                published: false,
                message,
            });
        }
    }
    phases.mark("temp_write");
    if let Err(error) = atomic_failpoint("temp_sync") {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(message) = full_sync_file(&file) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AtomicWriteError {
            stage: "temp_sync",
            published: false,
            message,
        });
    }
    phases.mark("full_sync");
    drop(file);
    if let Err(error) = atomic_failpoint("rename") {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(message) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(AtomicWriteError {
            stage: "rename",
            published: false,
            message,
        });
    }
    phases.mark("rename");
    if let Err(mut error) = atomic_failpoint("dir_sync") {
        error.published = true;
        return Err(error);
    }
    flush_dir(parent).map_err(|message| AtomicWriteError {
        stage: "dir_sync",
        published: true,
        message,
    })?;
    phases.mark("dir_sync");
    phases.finish(path);
    Ok(())
}

pub fn sync_file(path: &Path) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    full_sync_file(&file)
}

/// Extended attributes of `path` as a deterministic JSON object
/// (name -> hex-encoded value). `Some("{}")` = readable, none present;
/// `None` = unknown (unreadable or non-unix platform). fszero-l4g.
pub fn xattrs_of(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        let mut map = serde_json::Map::new();
        let names = xattr::list(path).ok()?;
        for name in names {
            let Ok(Some(value)) = xattr::get(path, &name) else {
                continue;
            };
            map.insert(
                name.to_string_lossy().into_owned(),
                serde_json::Value::String(fszero_core::operation_schemas::hex_encode_pub(&value)),
            );
        }
        Some(serde_json::Value::Object(map).to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Restore the journaled xattr set exactly: journaled attrs are re-set,
/// attrs present now but absent from the journal are removed. Best-effort
/// per attribute (system-managed attrs such as com.apple.provenance may
/// refuse writes); hard failures are reported joined. Empty `journaled`
/// means unknown and is skipped entirely.
pub fn restore_xattrs(path: &Path, journaled: &str) -> Result<(), String> {
    if journaled.is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(journaled)
        else {
            return Err("journaled xattrs unparseable".to_string());
        };
        let mut failures = Vec::new();
        if let Ok(names) = xattr::list(path) {
            for name in names {
                let key = name.to_string_lossy().into_owned();
                if !map.contains_key(&key) {
                    let _ = xattr::remove(path, &name);
                }
            }
        }
        for (name, value) in &map {
            let Some(hex) = value.as_str() else { continue };
            let Some(bytes) = hex_decode(hex) else {
                failures.push(format!("{name}: bad hex"));
                continue;
            };
            if let Err(e) = xattr::set(path, name.as_str(), &bytes) {
                failures.push(format!("{name}: {e}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join(", "))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(unix)]
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Permission bits (including setuid/setgid/sticky, `& 0o7777`) as i64;
/// -1 = unavailable or non-unix platform.
pub fn mode_of(path: &Path) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| i64::from(m.permissions().mode() & 0o7777))
            .unwrap_or(-1)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        -1
    }
}

/// Restore journaled permission bits (fszero-7be). Negative = unknown,
/// skipped; no-op on non-unix platforms.
pub fn set_mode(path: &Path, mode: i64) -> Result<(), String> {
    if mode < 0 {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode as u32))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// File mtime as nanoseconds since UNIX_EPOCH (0 = unavailable or pre-1970).
pub fn mtime_ns_of(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

/// Journal-relevant metadata snapshot before a mutating write (mtime, mode, xattrs).
#[inline]
pub fn file_meta_snapshot(path: &Path) -> (i64, i64, String) {
    (
        mtime_ns_of(path),
        mode_of(path),
        xattrs_of(path).unwrap_or_default(),
    )
}

/// Restore a journaled mtime after materializing journaled content
/// (fszero-md6: build systems key on mtime; drift causes spurious or —
/// worse — skipped rebuilds). 0 means unknown and is skipped.
pub fn set_mtime_ns(path: &Path, mtime_ns: i64) -> Result<(), String> {
    if mtime_ns <= 0 {
        return Ok(());
    }
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(mtime_ns as u64);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .map_err(|e| e.to_string())
}

pub fn escape_like_pattern(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    for ch in pat.chars() {
        match ch {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Read-only scratch allowlist roots. Opt-in only, via `FSZERO_SCRATCH_DIR`:
/// a `:`-separated list of directories, where `tmp` means the OS temp dir.
/// These roots are readable; every write path stays root-jailed.
pub fn scratch_read_roots() -> Vec<PathBuf> {
    let Ok(raw) = std::env::var("FSZERO_SCRATCH_DIR") else {
        return Vec::new();
    };
    raw.split(':')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if entry == "tmp" {
                std::env::temp_dir()
            } else {
                PathBuf::from(entry)
            }
        })
        .filter_map(|dir| fs::canonicalize(&dir).ok())
        .collect()
}

/// Rejection text that names the sanctioned alternative.
pub fn scratch_read_hint() -> String {
    let roots = scratch_read_roots();
    if roots.is_empty() {
        "absolute path rejected; absolute reads need an opt-in read-only scratch dir: set FSZERO_SCRATCH_DIR (directory list, 'tmp' for the OS temp dir), or pass a path relative to the session root".to_string()
    } else {
        let allowed: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        format!(
            "absolute path rejected; absolute reads are allowed only under the declared read-only scratch dir(s): {}; move the artifact there, or pass a path relative to the session root",
            allowed.join(", ")
        )
    }
}

/// Resolve an absolute read argument against the read-only scratch allowlist.
/// Reads only — writes never take this path.
pub fn resolve_scratch_read_path(arg: &str) -> Result<PathBuf, String> {
    let target = canonicalize_existing(Path::new(arg))?;
    if scratch_read_roots()
        .iter()
        .any(|root| canonical_path_within_root(root, &target))
    {
        Ok(target)
    } else {
        Err(scratch_read_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::full_sync_file;
    use std::io::Write;

    #[test]
    fn full_sync_file_succeeds_on_host_tempfile() {
        let mut file = tempfile::tempfile().expect("host tempfile");
        file.write_all(b"full-sync-barrier")
            .expect("write host tempfile");
        full_sync_file(&file)
            .expect("durability barrier (sync_all + macOS F_FULLFSYNC) must succeed on this host");
    }
}
