use super::path::sanitize_relative_arg;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Hard ceiling for recursive listing/glob descent (jqf.9.5). User `--depth`
/// may request more; the walk still stops here so a deep chain cannot blow
/// the stack. Matches the AST walker cap.
const MAX_LIST_DEPTH: usize = 64;

#[derive(Debug, Clone)]
struct LsSpec {
    path: String,
    depth: usize,
    budget: usize,
    is_glob: bool,
}

/// Parse `--name=N` or `--name N` from `parts[*i]`, advancing `i` on bare form.
fn parse_usize_flag(
    parts: &[&str],
    i: &mut usize,
    eq: &str,
    bare: &str,
    err: &str,
) -> Result<Option<usize>, String> {
    let p = parts[*i];
    if let Some(v) = p.strip_prefix(eq) {
        return v.parse().map(Some).map_err(|_| err.into());
    }
    if p == bare {
        *i += 1;
        if *i >= parts.len() {
            return Err(err.into());
        }
        return parts[*i].parse().map(Some).map_err(|_| err.into());
    }
    Ok(None)
}

/// Rejection of a non-relative `ls` path, with the active root and a corrected
/// example (fszero-r2ia). "absolute path rejected" alone gave the caller no way
/// to know paths are root-relative, nor which root is active.
fn relative_path_help(root: &Path, arg: &str, reason: &str) -> String {
    let suggestion = arg
        .strip_prefix(root.to_string_lossy().as_ref())
        .map(|rest| rest.trim_start_matches('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or("src");
    format!(
        "{reason}: ls paths are relative to the session root, never absolute or '..'. \
active root: {root}. use path:'.' for the root itself, or a child like path:'{suggestion}'",
        root = root.display()
    )
}

fn parse_ls_spec(root: &Path, arg: Option<&str>) -> Result<LsSpec, String> {
    let raw = arg.unwrap_or(".").trim();
    let mut path: Option<String> = None;
    let mut depth = 0usize;
    let mut budget = 10_000usize;
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 0usize;
    while i < parts.len() {
        if let Some(v) = parse_usize_flag(&parts, &mut i, "--depth=", "--depth", "bad depth")? {
            depth = v;
        } else if let Some(v) =
            parse_usize_flag(&parts, &mut i, "--budget=", "--budget", "bad budget")?
        {
            if v == 0 {
                return Err("budget must be positive".into());
            }
            budget = v;
        } else if path.is_none() {
            path = Some(parts[i].to_string());
        } else {
            return Err(format!("unknown ls arg: {}", parts[i]));
        }
        i += 1;
    }
    let path = path.unwrap_or_else(|| ".".to_string());
    if path.chars().any(|c| matches!(c, '[' | ']' | '{' | '}')) {
        return Err("unsupported glob syntax".into());
    }
    let is_glob = has_glob_meta(&path);
    // Treat `.` / `./` / empty as root marker; do not run sanitize (`.` → empty path rejected).
    if !super::path::is_session_root_arg(&path) {
        sanitize_relative_arg(&path).map_err(|e| relative_path_help(root, &path, &e))?;
    } else {
        // Normalize all root forms to a single marker for downstream checks.
        // (kept as "." so collect_ls_manifest / do_ls root-path branches match)
    }
    let path = if super::path::is_session_root_arg(&path) {
        ".".to_string()
    } else {
        path
    };
    Ok(LsSpec {
        path,
        depth,
        budget,
        is_glob,
    })
}

pub fn format_ls_manifest(entries: &[String], budget_hit: bool) -> String {
    let mut body = entries.join("\n");
    if budget_hit {
        body.push_str("\n# budget_hit=true");
    }
    body
}

pub fn format_stat_manifest(path: &Path, meta: &fs::Metadata) -> String {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "path={}\nsize={}\nis_dir={}\nis_file={}\nis_symlink={}\nmtime={modified}\n",
        path.display(),
        meta.len(),
        meta.is_dir(),
        meta.is_file(),
        meta.file_type().is_symlink()
    )
}

fn has_glob_meta(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?'))
}

pub struct LsCollectResult {
    pub entries: Vec<String>,
    pub budget_hit: bool,
}

/// ZeroStack-family store directories: the stack's own runtime state must
/// never pay for its own existence in visible listing tokens
/// (fszero-store-self-noise-xh8). Deliberately narrower than
/// `is_ignored_dir_name`: build dirs, deps, and VCS internals stay visible in
/// listings because they are real workspace content an agent may act on.
pub fn is_zerostack_store_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".zerostack" | ".fszero" | ".tokenzero" | ".graphzero" | ".asgrep" | ".greplm" | ".ln"
    )
}

/// Escape hatch for store inspection: `FSZERO_LS_SHOW_STORES=1`.
fn hide_store_dirs() -> bool {
    std::env::var("FSZERO_LS_SHOW_STORES").ok().as_deref() != Some("1")
}

fn is_hidden_store_dir(path: &Path, is_dir: bool) -> bool {
    is_dir
        && hide_store_dirs()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_zerostack_store_dir_name)
}

#[inline]
fn rel_under(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).display().to_string()
}

#[inline]
fn ls_entry_name(base: &Path, p: &Path, is_dir: bool) -> String {
    let rel = rel_under(base, p);
    if is_dir { format!("{rel}/") } else { rel }
}

fn list_direct_entries(target: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    if let Ok(rd) = fs::read_dir(target) {
        for e in rd.flatten() {
            let p = e.path();
            let Some(meta) = listable_meta(&p) else {
                continue;
            };
            entries.push(ls_entry_name(target, &p, meta.is_dir()));
        }
    }
    entries
}

pub fn collect_ls_entries(target: &Path, depth: usize, budget: usize) -> LsCollectResult {
    let depth = depth.min(MAX_LIST_DEPTH);
    let mut entries = if depth == 0 {
        list_direct_entries(target)
    } else {
        let mut entries = Vec::new();
        collect_recursive_listing(target, target, depth, budget, &mut entries);
        entries
    };
    entries.sort();
    LsCollectResult {
        budget_hit: entries.len() >= budget,
        entries,
    }
}

pub fn collect_glob_ls_entries(root: &Path, pattern: &str, budget: usize) -> LsCollectResult {
    let entries = list_glob_matches(root, pattern, budget);
    LsCollectResult {
        budget_hit: entries.len() >= budget,
        entries,
    }
}

/// Early-out when budget full or directory unreadable.
fn open_list_dir(dir: &Path, budget: usize, out_len: usize) -> Option<fs::ReadDir> {
    if out_len >= budget {
        return None;
    }
    fs::read_dir(dir).ok()
}

fn collect_recursive_listing(
    dir: &Path,
    base: &Path,
    depth: usize,
    budget: usize,
    out: &mut Vec<String>,
) {
    let root_dev = listing_root_dev(base);
    collect_recursive_listing_inner(dir, base, depth, budget, out, root_dev);
}

fn collect_recursive_listing_inner(
    dir: &Path,
    base: &Path,
    depth: usize,
    budget: usize,
    out: &mut Vec<String>,
    root_dev: Option<u64>,
) {
    let Some(rd) = open_list_dir(dir, budget, out.len()) else {
        return;
    };
    // Dirent fast path (fszero-gauntlet-r0): entry.file_type() reads the
    // readdir d_type on APFS/ext4 — no per-entry lstat. A metadata call remains
    // only for directories that are about to be descended, where the
    // cross-device check needs st_dev. Classification matches listable_meta:
    // symlinks, non-file/dir kinds, and hidden store dirs stay excluded.
    // The file_name OsStr is captured once per entry: it is both the sort key
    // (siblings share a parent, so Path order == name order) and, via the
    // threaded `rel_prefix`, enough to build the manifest line without
    // strip_prefix/display over full paths.
    let mut entries: Vec<(std::ffi::OsString, PathBuf, bool)> = Vec::new();
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else {
            continue;
        };
        if ft.is_symlink() || (!ft.is_file() && !ft.is_dir()) {
            continue;
        }
        let p = e.path();
        let is_dir = ft.is_dir();
        if is_dir && is_hidden_store_dir(&p, true) {
            continue;
        }
        let name = p.file_name().unwrap_or(p.as_os_str()).to_os_string();
        entries.push((name, p, is_dir));
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (name, p, is_dir) in entries {
        if out.len() >= budget {
            break;
        }
        let rel = match rel_prefix(dir, base) {
            Some(prefix) if prefix.is_empty() => name.to_string_lossy().into_owned(),
            Some(prefix) => format!("{prefix}/{}", name.to_string_lossy()),
            None => ls_entry_name(base, &p, false),
        };
        if is_dir {
            out.push(format!("{rel}/"));
            if depth > 0 {
                let meta = fs::symlink_metadata(&p).ok();
                let crosses = match meta.as_ref() {
                    Some(m) => crosses_listing_device(m, root_dev),
                    None => true,
                };
                if !crosses {
                    collect_recursive_listing_inner_at(
                        &p,
                        base,
                        rel,
                        depth - 1,
                        budget,
                        out,
                        root_dev,
                    );
                }
            }
        } else {
            out.push(rel);
        }
    }
}

/// Workspace-relative prefix of `dir` under `base`, or `None` when `dir` does
/// not lexically sit under `base` (fallback callers then use the slow path).
fn rel_prefix(dir: &Path, base: &Path) -> Option<String> {
    dir.strip_prefix(base)
        .ok()
        .map(|rel| rel.display().to_string())
}

fn collect_recursive_listing_inner_at(
    dir: &Path,
    base: &Path,
    rel_prefix: String,
    depth: usize,
    budget: usize,
    out: &mut Vec<String>,
    root_dev: Option<u64>,
) {
    let _ = rel_prefix; // wired in pass 3b; see below
    collect_recursive_listing_inner(dir, base, depth, budget, out, root_dev)
}

fn list_glob_matches(root: &Path, pattern: &str, budget: usize) -> Vec<String> {
    let mut entries = Vec::new();
    collect_glob_candidates(root, root, pattern, budget, &mut entries);
    entries.sort();
    entries
}

fn collect_glob_candidates(
    dir: &Path,
    base: &Path,
    pattern: &str,
    budget: usize,
    out: &mut Vec<String>,
) {
    let root_dev = listing_root_dev(base);
    collect_glob_candidates_inner(dir, base, pattern, budget, out, root_dev, 0);
}

fn collect_glob_candidates_inner(
    dir: &Path,
    base: &Path,
    pattern: &str,
    budget: usize,
    out: &mut Vec<String>,
    root_dev: Option<u64>,
    depth_from_root: usize,
) {
    let Some(rd) = open_list_dir(dir, budget, out.len()) else {
        return;
    };
    for e in rd.flatten() {
        if out.len() >= budget {
            break;
        }
        let p = e.path();
        let Some(meta) = listable_meta(&p) else {
            continue;
        };
        let rel = rel_under(base, &p);
        if glob_match(pattern, &rel) {
            out.push(rel.clone());
        }
        if meta.is_dir()
            && !crosses_listing_device(&meta, root_dev)
            && depth_from_root < MAX_LIST_DEPTH
        {
            collect_glob_candidates_inner(
                &p,
                base,
                pattern,
                budget,
                out,
                root_dev,
                depth_from_root + 1,
            );
        }
    }
}

/// Metadata for a listable entry (skips broken links, symlinks, FIFOs,
/// sockets, devices, and hidden stores).
fn listable_meta(p: &Path) -> Option<fs::Metadata> {
    let meta = fs::symlink_metadata(p).ok()?;
    if meta.file_type().is_symlink() || is_hidden_store_dir(p, meta.is_dir()) {
        return None;
    }
    if !meta.is_file() && !meta.is_dir() {
        return None;
    }
    Some(meta)
}

fn listing_root_dev(base: &Path) -> Option<u64> {
    fs::metadata(base).ok().and_then(|meta| unix_dev(&meta))
}

fn crosses_listing_device(meta: &fs::Metadata, root_dev: Option<u64>) -> bool {
    match (unix_dev(meta), root_dev) {
        (Some(dev), Some(root)) => dev != root,
        _ => false,
    }
}

#[cfg(unix)]
fn unix_dev(meta: &fs::Metadata) -> Option<u64> {
    Some(meta.dev())
}

#[cfg(not(unix))]
fn unix_dev(_meta: &fs::Metadata) -> Option<u64> {
    None
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_at(&p, 0, &t, 0)
}

fn glob_match_at(pattern: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }
    if pattern[pi] == '*' {
        if pi + 1 < pattern.len() && pattern[pi + 1] == '*' {
            if pi + 2 < pattern.len()
                && pattern[pi + 2] == '/'
                && glob_match_at(pattern, pi + 3, text, ti)
            {
                return true;
            }
            let next = pi + 2;
            return (ti..=text.len()).any(|idx| glob_match_at(pattern, next, text, idx));
        }
        return (ti..=text.len()).any(|idx| {
            text[ti..idx].iter().all(|c| *c != '/') && glob_match_at(pattern, pi + 1, text, idx)
        });
    }
    if ti == text.len() {
        return false;
    }
    if pattern[pi] == '?' {
        return text[ti] != '/' && glob_match_at(pattern, pi + 1, text, ti + 1);
    }
    pattern[pi] == text[ti] && glob_match_at(pattern, pi + 1, text, ti + 1)
}

pub fn collect_ls_manifest(root: &Path, arg: Option<&str>) -> Result<String, String> {
    let spec = parse_ls_spec(root, arg)?;
    if spec.is_glob {
        let root_canon = canonicalize_root(root)?;
        let result = collect_glob_ls_entries(&root_canon, &spec.path, spec.budget);
        return Ok(format_ls_manifest(&result.entries, result.budget_hit));
    }
    let target = if spec.path == "." {
        root.to_path_buf()
    } else {
        resolve_existing_path(Some(root), &spec.path)?
    };
    let result = collect_ls_entries(&target, spec.depth, spec.budget);
    Ok(format_ls_manifest(&result.entries, result.budget_hit))
}

impl FSZeroSession {
    pub fn do_ls(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let Some(root) = root else {
            let listing = "(no-root)".to_string();
            let _ = self.recovery.put("ls", listing.as_bytes());
            return ls_ack(0);
        };
        let spec = match parse_ls_spec(root, arg) {
            Ok(spec) => spec,
            Err(e) => return format!("bad ls: {e}"),
        };
        if spec.is_glob {
            let root_canon = match canonicalize_root(root) {
                Ok(p) => p,
                Err(e) => return super::op_result::bad_path(e),
            };
            let result = collect_glob_ls_entries(&root_canon, &spec.path, spec.budget);
            self.store_ls_manifest(&result.entries, result.budget_hit);
            return ls_ack(result.entries.len());
        }
        let target = if spec.path == "." {
            root.to_path_buf()
        } else {
            match self.resolve_existing_path_cached(Some(root), &spec.path) {
                Ok(p) => p,
                Err(e) => return super::op_result::bad_path(e),
            }
        };
        if spec.depth == 0 {
            // Warm path: join once without cloning the entry Vec (fszero warm L).
            let warm = self.caches.ls.get(&target).and_then(|(cached, mtime)| {
                let curr = fs::metadata(&target).ok()?.modified().ok()?;
                (curr == *mtime).then(|| (cached.len(), format_ls_manifest(cached, false)))
            });
            if let Some((len, listing)) = warm {
                self.store_ls_listing(&listing, false);
                return ls_ack(len);
            }
        }
        let result = collect_ls_entries(&target, spec.depth, spec.budget);
        let len = result.entries.len();
        let budget_hit = result.budget_hit;
        let listing = format_ls_manifest(&result.entries, budget_hit);
        if spec.depth == 0 {
            let m = fs::metadata(&target)
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or_else(SystemTime::now);
            self.caches.ls.insert(target, (result.entries, m));
        }
        self.store_ls_listing(&listing, budget_hit);
        ls_ack(len)
    }

    fn store_ls_listing(&mut self, listing: &str, budget_hit: bool) {
        self.recovery
            .put_key("ls_budget_hit", if budget_hit { b"true" } else { b"false" });
        self.recovery.put_key("ls_manifest", listing.as_bytes());
    }

    fn store_ls_manifest(&mut self, entries: &[String], budget_hit: bool) {
        self.store_ls_listing(&entries.join("\n"), budget_hit);
    }

    pub fn do_stat(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let path_arg = arg.unwrap_or(".");
        let full_path = match resolve_existing_path(root, path_arg) {
            Ok(p) => p,
            Err(e) => return super::op_result::op0("stat", super::op_result::bad_path(e)),
        };
        let meta = match fs::symlink_metadata(&full_path) {
            Ok(meta) => meta,
            Err(e) => return super::op_result::op0("stat", super::op_result::metadata_failed(e)),
        };
        let payload = format_stat_manifest(&full_path, &meta);
        let _ = self.recovery.put_named_payload("stat", payload.as_bytes());
        format!("stat:{} bytes", payload.len())
    }
}

fn ls_ack(n: usize) -> String {
    format!("ls:{n} entries")
}
