use crate::*;

#[derive(Default)]
pub(crate) struct SearchStats {
    pub(crate) visited_files: usize,
    pub(crate) matched_files: usize,
    pub(crate) matched_lines: usize,
    pub(crate) truncated_by_results: bool,
    pub(crate) truncated_by_visit: bool,
    /// Host-op wall deadline exceeded mid-walk (CodeMode hard_max_wall_ms).
    pub(crate) truncated_by_wall: bool,
    /// rg output rows that did not parse back into matches — a parity canary
    /// (silent row loss would otherwise read as "no match there").
    pub(crate) unparsed_rows: usize,
    /// rg `--threads` cap actually used. `1` is serial; omit from claims as
    /// concurrent search. Internal walker is also serial (`0` until set).
    pub(crate) search_threads: u32,
    /// Directories or files the walker could not list/read, or rg exit 2
    /// with empty stderr (`--no-messages` IO). A zero-hit with this > 0 is
    /// not a proven miss.
    pub(crate) unreadable_entries: usize,
}

/// Hard recursion bound for the internal directory walkers. Deep enough for
/// any real source tree; a backstop so a symlink cycle cannot blow the stack
/// even if the per-entry symlink skip is ever bypassed.
pub(crate) const MAX_WALK_DEPTH: usize = 64;

/// True when the path is a symlink. The walkers must not follow symlinks: a
/// cycle inside an allowed root would otherwise recurse until the stack or
/// the wall-clock budget is exhausted (`collect_tree` is depth-bounded; these
/// walkers historically were not). symlink_metadata does not traverse.
pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn sorted_entries(path: &Path, unreadable: &mut usize) -> Option<Vec<PathBuf>> {
    let reader = match fs::read_dir(path) {
        Ok(reader) => reader,
        Err(_) => {
            *unreadable = unreadable.saturating_add(1);
            return None;
        }
    };
    let mut entries = Vec::new();
    for entry in reader {
        match entry {
            Ok(entry) => entries.push(entry.path()),
            Err(_) => *unreadable = unreadable.saturating_add(1),
        }
    }
    entries.sort();
    Some(entries)
}

pub(crate) fn max_search_visited_files(max_results: usize) -> usize {
    if max_results == 0 {
        return 0;
    }
    max_results
        .saturating_mul(SEARCH_VISIT_MULTIPLIER)
        .clamp(MIN_SEARCH_VISITED_FILES, MAX_SEARCH_VISITED_FILES)
}

pub struct SearchMatch {
    pub base: String,
    pub path: String,
    pub rel: String,
    pub line: usize,
    pub text: String,
}

/// In-process `tz_find` matcher: exact substring, not regex and not case-fold.
pub(crate) fn literal_substring_hit(line: &str, query: &str) -> bool {
    line.contains(query)
}

/// Context lines inlined on each side of a hit, matching FSZero's
/// TARGET_CONTEXT_LINES policy for one-call actionable discovery results.
const TARGET_CONTEXT_LINES: usize = 2;

/// Enclosing-symbol inference mirroring FSZero's `enclosing_symbol()`
/// (FSZero src/core/target_ref.rs): nearest declarator line at or above the
/// hit, declarator head capped at 80 chars; None => the grammar's truthful
/// `(file-scope)` fallback. Kept byte-compatible with FSZero's DECLARATORS.
fn enclosing_symbol(lines: &[String], line_no: usize) -> Option<String> {
    const DECLARATORS: &[&str] = &[
        "fn ",
        "pub fn ",
        "async fn ",
        "struct ",
        "enum ",
        "impl ",
        "trait ",
        "mod ",
        "class ",
        "def ",
        "function ",
        "type ",
        "const ",
        "static ",
    ];
    for line in lines[..line_no.min(lines.len())].iter().rev() {
        let trimmed = line.trim();
        if DECLARATORS.iter().any(|d| trimmed.starts_with(d)) {
            let head = trimmed.trim_end_matches(['{', ' ']);
            return Some(head.chars().take(80).collect());
        }
    }
    None
}

/// FSZero snap-to-file hit rendering (FSZero docs/design/target-ref-grammar.md):
/// every distinct target window becomes one `HIT <path>#L<start>-L<end>
/// kind=<kind> sym=<sym>` header plus an inlined `| <line-no>: <text>` window
/// covering the matched line and TARGET_CONTEXT_LINES on each side, so agents
/// snap to file:line without a second discovery call. Byte-identical windows
/// within one file are emitted once (5irj): adjacent matches whose context
/// windows overlap or clamp to the same range render one HIT record while
/// every matching line stays visible. Distinct windows and distinct enclosing
/// symbols remain distinct. Each file is read once for all of its hits;
/// unreadable files fall back to the matched line only.
/// Emit one hit's context window, or skip if this (start, stop, kind, sym,
/// fallback_text) key was already written for the file.
fn emit_hit_window<'a>(
    out: &mut String,
    emitted: &mut Vec<(usize, usize, &'a str, String, Option<String>)>,
    file_lines: &Option<Vec<String>>,
    hit: &SearchMatch,
    kind: &'a str,
) {
    let line = hit.line.max(1);
    let (start, stop) = match file_lines {
        Some(lines) if !lines.is_empty() => (
            line.saturating_sub(TARGET_CONTEXT_LINES).max(1),
            (line + TARGET_CONTEXT_LINES).min(lines.len()),
        ),
        _ => (line, line),
    };
    let fallback_text: Option<String> = match file_lines {
        Some(lines) if !lines.is_empty() => None,
        _ => Some(hit.text.clone()),
    };
    // 631q: carry the inferred enclosing symbol when the file is
    // readable; unreadable/binary files keep (file-scope).
    let sym = match file_lines {
        Some(lines) if !lines.is_empty() => {
            enclosing_symbol(lines, line).unwrap_or_else(|| "(file-scope)".to_string())
        }
        _ => "(file-scope)".to_string(),
    };
    if emitted
        .iter()
        .any(|(e_start, e_stop, e_kind, e_sym, e_fallback)| {
            *e_start == start
                && *e_stop == stop
                && *e_kind == kind
                && *e_sym == sym
                && *e_fallback == fallback_text
        })
    {
        return;
    }
    emitted.push((start, stop, kind, sym.clone(), fallback_text));
    out.push_str(&format!(
        "HIT {}#L{}-L{} kind={} sym={}\n",
        hit.path, start, stop, kind, sym
    ));
    match file_lines {
        Some(lines) if !lines.is_empty() => {
            for no in start..=stop {
                let text = lines.get(no - 1).map(String::as_str).unwrap_or("");
                out.push_str(&format!("| {}: {}\n", no, text));
            }
        }
        _ => out.push_str(&format!("| {}: {}\n", line, hit.text)),
    }
}

pub(crate) fn hit_search_output(matches: &[SearchMatch], kind: &str) -> String {
    let mut out = String::new();
    let mut idx = 0;
    while idx < matches.len() {
        let m = &matches[idx];
        let mut end = idx + 1;
        while end < matches.len() && matches[end].path == m.path {
            end += 1;
        }
        let file_lines: Option<Vec<String>> = std::fs::read_to_string(&m.path)
            .ok()
            .map(|content| content.lines().map(str::to_string).collect());
        // 5irj: stable per-file dedupe key (start, stop, kind, sym,
        // fallback_text). kind is uniform for the whole call, so comparing it
        // is a no-op, but it keeps the key explicit and future-proof if a
        // mixed-kind call ever lands. fallback_text is None for readable files
        // and Some(hit.text) for unreadable ones, so same path/line/kind/sym
        // records whose emitted `| line: text` differs stay distinct.
        let mut emitted: Vec<(usize, usize, &str, String, Option<String>)> = Vec::new();
        for hit in &matches[idx..end] {
            emit_hit_window(&mut out, &mut emitted, &file_lines, hit, kind);
        }
        idx = end;
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

pub(crate) fn flat_search_output(matches: &[SearchMatch]) -> String {
    matches
        .iter()
        .map(|m| format!("{}:{}:{}", m.path, m.line, m.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lossless compact projection of glob matches as an indented prefix trie.
///
/// Root and component labels are JSON strings. A trailing `/` marks a
/// directory component and two spaces encode one level. This keeps whitespace,
/// newlines, separator-like characters, and Unicode unambiguous while emitting
/// each shared directory prefix only once. Roots are sorted and deduplicated,
/// and overlapping paths bind to the most-specific root, so caller ordering
/// cannot change the bytes. Paths outside every declared root remain full JSON
/// strings after an explicit marker.
pub(crate) fn grouped_path_output(paths: &[PathBuf], roots: &[PathBuf]) -> String {
    let mut canonical_roots = roots.to_vec();
    canonical_roots.sort_by(|left, right| {
        display_path(left)
            .cmp(&display_path(right))
            .then_with(|| left.cmp(right))
    });
    canonical_roots.dedup();

    let mut sections: Vec<Vec<Vec<String>>> = vec![Vec::new(); canonical_roots.len()];
    let mut leftovers: Vec<String> = Vec::new();
    for path in paths {
        let mut selected: Option<(usize, usize, Vec<String>)> = None;
        for (idx, root) in canonical_roots.iter().enumerate() {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let components = rel
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(value) => {
                        Some(value.to_string_lossy().into_owned())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if components.is_empty() {
                continue;
            }
            let specificity = root.components().count();
            let replace = match &selected {
                Some((_, best_specificity, _)) => specificity > *best_specificity,
                None => true,
            };
            if replace {
                selected = Some((idx, specificity, components));
            }
        }
        if let Some((idx, _, components)) = selected {
            sections[idx].push(components);
        } else {
            leftovers.push(display_path(path));
        }
    }

    let mut lines = Vec::new();
    for (root, mut rows) in canonical_roots.iter().zip(sections) {
        if rows.is_empty() {
            continue;
        }
        rows.sort();
        lines.push(format!(
            "# root: {}",
            serde_json::to_string(&display_path(root)).expect("path display is serializable")
        ));
        let mut previous_dirs: Vec<String> = Vec::new();
        for components in rows {
            let (dirs, file) = components.split_at(components.len() - 1);
            let shared = dirs
                .iter()
                .zip(&previous_dirs)
                .take_while(|(left, right)| left == right)
                .count();
            for (depth, component) in dirs.iter().enumerate().skip(shared) {
                let label = serde_json::to_string(component)
                    .expect("path component display is serializable");
                lines.push(format!("{}{label}/", "  ".repeat(depth)));
            }
            let label =
                serde_json::to_string(&file[0]).expect("path component display is serializable");
            lines.push(format!("{}{label}", "  ".repeat(dirs.len())));
            previous_dirs = dirs.to_vec();
        }
    }
    if !leftovers.is_empty() {
        leftovers.sort();
        lines.push("# outside-roots".to_string());
        lines.extend(
            leftovers
                .into_iter()
                .map(|path| serde_json::to_string(&path).expect("path display is serializable")),
        );
    }
    lines.join("\n")
}

pub(crate) fn grouped_tree_output(
    entries: &[TreeEntry],
    spans: &[(String, usize)],
    with_headers: bool,
) -> String {
    let mut lines = Vec::new();
    for (idx, (root, start)) in spans.iter().enumerate() {
        let end = spans.get(idx + 1).map_or(entries.len(), |next| next.1);
        if *start == end {
            continue;
        }
        if with_headers {
            lines.push(format!("# root: {root}"));
        }
        for entry in &entries[*start..end] {
            let suffix = if entry.dir { "/" } else { "" };
            lines.push(format!(
                "{}{}{}",
                "  ".repeat(entry.depth),
                entry.name,
                suffix
            ));
        }
    }
    lines.join("\n")
}

/// Echo at most one short line of the caller's query in a zero-hit note:
/// multi-line queries can never match (search is per-line) and long queries
/// would make the note cost O(query) for a 0-token payload. Mirrors the
/// label compaction capsule headers already apply.
pub(crate) fn zero_hit_label(query: &str) -> String {
    const MAX_LABEL_CHARS: usize = 48;
    let first_line = query.lines().next().unwrap_or("");
    let truncated: String = first_line.chars().take(MAX_LABEL_CHARS).collect();
    if truncated.chars().count() < query.chars().count() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Conservative regex-shape check for guidance only. Does not change
/// grep/find semantics (tokenzero-j456). Avoids treating `fn alpha()` as regex.
pub(crate) fn looks_like_regex(pattern: &str) -> bool {
    pattern.contains('[')
        || pattern.contains(']')
        || pattern.contains('|')
        || pattern.contains('^')
        || pattern.contains('$')
        || pattern.contains("(?")
        || pattern.contains(".*")
        || pattern.contains(".+")
        || pattern.contains("\\d")
        || pattern.contains("\\w")
        || pattern.contains("\\s")
        || pattern.contains("\\b")
        || has_regex_repeat(pattern)
}

fn has_regex_repeat(pattern: &str) -> bool {
    pattern
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'{' && pair[1].is_ascii_digit())
}

/// One extra agent-facing line after a zero-hit or truncated note.
pub(crate) fn guidance_hint(
    tool: &str,
    query: &str,
    truncated: bool,
    unreadable: bool,
) -> Option<&'static str> {
    if unreadable {
        return Some("scan skipped unreadable paths; a complete miss was not proven");
    }
    if truncated {
        return Some("results truncated; narrow the path or raise max_files");
    }
    match tool {
        "grep" if looks_like_regex(query) => Some("no regex match; try find for a literal"),
        "find" if looks_like_regex(query) => Some("find is literal; use grep for regex"),
        "glob" => Some("no matching paths; try a broader glob or a different root"),
        "tree" => Some("empty tree; check the root, depth, or hidden-file filter"),
        _ => None,
    }
}

pub(crate) fn with_guidance(
    note: String,
    tool: &str,
    query: &str,
    truncated: bool,
    unreadable: bool,
) -> String {
    match guidance_hint(tool, query, truncated, unreadable) {
        Some(hint) => format!("{note}\n# {hint}"),
        None => note,
    }
}

/// Append a truncated-scan hint to a non-empty result envelope.
pub(crate) fn apply_truncated_hint(response: &mut ToolResponse, mode: Mode) {
    if matches!(mode, Mode::Passthrough) {
        return;
    }
    let Some(visible) = response.visible.as_mut() else {
        return;
    };
    if visible.text.contains("results truncated; narrow the path") {
        return;
    }
    visible
        .text
        .push_str("\n# results truncated; narrow the path or raise max_files");
    if let Some(accounting) = response.accounting.as_mut() {
        accounting.visible_tokens = count_tokens(&visible.text);
    }
}

/// Remaining visible-token budget plus an explicit exhausted flag so agents
/// can tell a complete miss from a scan that stopped on max_files/visit/wall.
pub(crate) fn attach_budget_signal(
    response: &mut ToolResponse,
    max_visible_tokens: usize,
    exhausted: bool,
) {
    let used = response
        .accounting
        .as_ref()
        .map(|accounting| accounting.visible_tokens)
        .unwrap_or(0);
    if exhausted {
        response.remaining_budget_tokens = Some(0);
        response.budget_exhausted = Some(true);
    } else {
        response.remaining_budget_tokens = Some(max_visible_tokens.saturating_sub(used) as u64);
        response.budget_exhausted = Some(false);
    }
}

/// Zero-hit + truncated is not "not found". Name the reason so a harness can
/// retry with a larger budget instead of accepting an empty answer.
pub(crate) fn mark_budget_exhausted_miss(response: &mut ToolResponse) {
    if response.diagnostic.is_some() {
        return;
    }
    response.diagnostic = Some(tokenzero_core::Diagnostic {
        code: "budget_exhausted".into(),
        message: "scan stopped on a budget before a complete miss could be proven".into(),
        repair: Some("raise max_files, narrow the path, or retry with a larger budget".into()),
    });
}

/// Zero-hit after permission/IO skips is not "not found". Distinct from
/// `budget_exhausted` so a harness cannot treat the miss as a complete scan.
pub(crate) fn mark_unreadable_miss(response: &mut ToolResponse) {
    if response.diagnostic.is_some() {
        return;
    }
    response.diagnostic = Some(tokenzero_core::Diagnostic {
        code: "scan_unreadable".into(),
        message: "scan skipped unreadable paths before a complete miss could be proven".into(),
        repair: Some("fix directory permissions or narrow the path to readable trees".into()),
    });
}

/// Empty search-family results otherwise render as a bare `refs:` footer with
/// no signal that the call succeeded and found nothing. Replace the empty
/// visible text with a one-line zero-hit note and account for its cost.
/// Passthrough keeps its verbatim-payload contract; non-empty text (e.g.
/// exact-mode ref lines) is never displaced.
pub(crate) fn apply_zero_hit_note(response: &mut ToolResponse, mode: Mode, note: String) {
    if matches!(mode, Mode::Passthrough) {
        return;
    }
    let Some(visible) = response.visible.as_mut() else {
        return;
    };
    if !visible.text.trim().is_empty() {
        return;
    }
    let note_tokens = count_tokens(&note);
    visible.text = note;
    if let Some(accounting) = response.accounting.as_mut() {
        accounting.visible_tokens = note_tokens;
    }
}

/// Merge extra telemetry keys into the response without clobbering an
/// existing telemetry object (degraded-storage and search-backend markers
/// must survive a dedup/diff serve).
pub(crate) fn merge_telemetry(response: &mut ToolResponse, extra: Value) {
    let Value::Object(extra) = extra else {
        return;
    };
    match response.telemetry.as_mut() {
        Some(Value::Object(existing)) => existing.extend(extra),
        _ => response.telemetry = Some(Value::Object(extra)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_search(
    base: &Path,
    current: &Path,
    query: &str,
    max_results: usize,
    max_visited_files: usize,
    depth: usize,
    stats: &mut SearchStats,
    matches: &mut Vec<SearchMatch>,
) {
    if stats.truncated_by_wall {
        return;
    }
    if matches.len() >= max_results {
        stats.truncated_by_results = true;
        return;
    }
    if stats.visited_files >= max_visited_files {
        stats.truncated_by_visit = true;
        return;
    }
    if depth == 0 {
        return;
    }
    if current.is_file() {
        stats.visited_files += 1;
        if crate::wall::check_active_wall_deadline_every(
            stats.visited_files,
            crate::wall::WALL_CHECK_EVERY_N,
        )
        .is_some()
        {
            stats.truncated_by_wall = true;
            return;
        }
        match fs::read(current) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let before = matches.len();
                let path_display = current.display().to_string();
                let rel_display = current
                    .strip_prefix(base)
                    .ok()
                    .filter(|rel| !rel.as_os_str().is_empty())
                    .map(|rel| rel.display().to_string())
                    .unwrap_or_else(|| path_display.clone());
                for (idx, line) in text.lines().enumerate() {
                    if literal_substring_hit(line, query) {
                        if matches.len() >= max_results {
                            stats.truncated_by_results = true;
                            break;
                        }
                        matches.push(SearchMatch {
                            base: base.display().to_string(),
                            path: path_display.clone(),
                            rel: rel_display.clone(),
                            line: idx + 1,
                            text: line.to_string(),
                        });
                        stats.matched_lines += 1;
                    }
                }
                if matches.len() > before {
                    stats.matched_files += 1;
                }
            }
            Err(_) => stats.unreadable_entries = stats.unreadable_entries.saturating_add(1),
        }
        return;
    }
    let Some(entries) = sorted_entries(current, &mut stats.unreadable_entries) else {
        return;
    };
    for path in entries {
        if should_skip(&path, false) || is_symlink(&path) {
            continue;
        }
        collect_search(
            base,
            &path,
            query,
            max_results,
            max_visited_files,
            depth - 1,
            stats,
            matches,
        );
        if stats.truncated_by_results || stats.truncated_by_visit || stats.truncated_by_wall {
            break;
        }
    }
}

#[derive(Debug)]
pub(crate) enum RgFailure {
    /// rg rejected the pattern (regex parse error); a tool error, not a
    /// fallback, because the internal scanner's substring semantics would
    /// silently return different results.
    InvalidPattern(String),
    /// rg is missing, failed to spawn, or exited with an unexpected status;
    /// the caller falls back to the internal scanner.
    Unavailable(String),
}

/// Hard cap passed to rg. Telemetry must report this; a search-perf claim
/// that omits `search_threads` is running serial (A27 concurrent-off).
pub(crate) const RG_SEARCH_THREADS: u32 = 1;

/// rg finished the tree. `Incomplete` is exit 2 with empty stderr: under
/// `--no-messages` that is IO/permission, not a regex parse (parse still
/// writes stderr). Keep matches; do not call it a complete miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RgExit {
    Complete,
    Incomplete,
}

/// Classify rg's process status without matching English stderr needles.
///
/// grep exit 2 with non-empty stderr is `InvalidPattern` (regex parse still
/// prints under `--no-messages`). Empty-stderr exit 2 is `Incomplete` IO.
pub(crate) fn classify_rg_exit(
    tool: &str,
    code: Option<i32>,
    stderr: &str,
) -> Result<RgExit, RgFailure> {
    match code {
        Some(0) | Some(1) => Ok(RgExit::Complete),
        Some(2) if tool == "grep" && !stderr.is_empty() => {
            Err(RgFailure::InvalidPattern(stderr.to_string()))
        }
        Some(2) => Ok(RgExit::Incomplete),
        other => Err(RgFailure::Unavailable(format!(
            "rg exited with {other:?}: {}",
            preview(stderr)
        ))),
    }
}

/// Portable rg discovery: env → PATH → well-known layouts (wqw.3).
pub fn find_rg_in_path() -> Option<PathBuf> {
    crate::binary_resolve::resolve_rg_binary()
        .ok()
        .map(|resolved| resolved.path)
}

/// Poll interval for the unbounded rg exit wait.
const RG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// Bounded final wait for the tree sweep after the root exited.
const RG_FINAL_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn spawn_rg_output_reader(
    mut reader: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

/// Run ripgrep per root and map its `path:line:text` output onto the same
/// `SearchMatch` rows the internal scanner produces. `find` keeps substring
/// semantics via `--fixed-strings`; `grep` passes the pattern as a regex.
pub(crate) fn rg_search(
    rg: &Path,
    tool: &str,
    query: &str,
    roots: &[PathBuf],
    max_results: usize,
) -> Result<(Vec<SearchMatch>, SearchStats), RgFailure> {
    let mut matches: Vec<SearchMatch> = Vec::new();
    let mut stats = SearchStats {
        search_threads: RG_SEARCH_THREADS,
        ..SearchStats::default()
    };
    for (root_idx, root) in roots.iter().enumerate() {
        if crate::wall::check_active_wall_deadline_every(root_idx, 1).is_some() {
            stats.truncated_by_wall = true;
            break;
        }
        if matches.len() >= max_results {
            stats.truncated_by_results = true;
            break;
        }
        let mut command = std::process::Command::new(rg);
        let thread_cap = RG_SEARCH_THREADS.to_string();
        command.args([
            "--line-number",
            "--no-heading",
            "--color=never",
            // Empties IO/permission stderr so exit 2 + empty stderr is scan
            // IO, not an invalid regex. Regex parse still writes stderr.
            "--no-messages",
            "--hidden",
            "--no-ignore",
            "--with-filename",
            // Multi-tenant hosts may run many TokenZero sessions. Cap rg's
            // internal fanout so one find cannot saturate the machine; the
            // machine-wide analysis permit then bounds how many such searches
            // run at once. `search_threads` telemetry must match this argv.
            "--threads",
            thread_cap.as_str(),
        ]);
        // Mirror the internal scanner's skip list (`should_skip` with hidden
        // entries excluded): `!.*` also keeps the `.tokenzero` recovery cache
        // out of results.
        for skip in ["!.*", "!target", "!__pycache__"] {
            command.args(["--glob", skip]);
        }
        if tool == "find" {
            command.arg("--fixed-strings");
        }
        command
            .arg("--")
            .arg(query)
            .arg(root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // Hub-owned spawn: rg runs single-threaded with no subprocess tree of
        // its own, but cancellation still signals through the exact owned
        // handle under the TokenZero engine binding — never a numeric pid.
        let _dispatch_child_scope = crate::engine_shell::dispatch_child_scope();
        let (verified, pipes) = zero_process::VerifiedChild::spawn_tree_with_pipes(
            command,
            tokenzero_runtime::PROCESS_OWNER_SESSION,
            tokenzero_runtime::PROCESS_GENERATION,
        )
        .map_err(|err| RgFailure::Unavailable(format!("rg spawn failed: {err}")))?;
        crate::engine_shell::publish_dispatch_child(&verified);
        // Register the child so raw-worker v2 cancellation can stop a long
        // search (pid is observation evidence only).
        crate::shell_hooks::note_child(Some(verified.child_id()), None, "running");
        let stdout_reader =
            spawn_rg_output_reader(pipes.stdout.expect("rg stdout pipe configured above"));
        let stderr_reader =
            spawn_rg_output_reader(pipes.stderr.expect("rg stderr pipe configured above"));
        // Mirror wait_with_output: unbounded run, bounded teardown. Cancel and
        // session death signal the owned handle, so the wait ends inside the
        // declared bound.
        loop {
            if verified.wait_for_exit(RG_POLL_INTERVAL) {
                break;
            }
        }
        let status = if let Some(status) = verified.terminal_status() {
            Ok(status)
        } else {
            verified
                .wait(
                    tokenzero_runtime::PROCESS_OWNER_SESSION,
                    tokenzero_runtime::PROCESS_GENERATION,
                    RG_FINAL_WAIT_TIMEOUT,
                    tokenzero_runtime::SHELL_TEARDOWN_GRACE,
                )
                .map_err(|error| RgFailure::Unavailable(format!("rg teardown failed: {error}")))
        };
        if status.is_err() {
            let _ = verified.signal_graceful_for(
                tokenzero_runtime::PROCESS_OWNER_SESSION,
                tokenzero_runtime::PROCESS_GENERATION,
                tokenzero_runtime::SHELL_TEARDOWN_GRACE,
            );
            let _ = verified.revoke();
        }
        // Join both readers before surfacing either reader or teardown error.
        // This prevents one failed join from detaching the other pipe reader.
        let stdout = stdout_reader
            .join()
            .map_err(|_| RgFailure::Unavailable("rg stdout reader panicked".to_string()))
            .and_then(|result| {
                result
                    .map_err(|err| RgFailure::Unavailable(format!("rg stdout read failed: {err}")))
            });
        let stderr = stderr_reader
            .join()
            .map_err(|_| RgFailure::Unavailable("rg stderr reader panicked".to_string()))
            .and_then(|result| {
                result
                    .map_err(|err| RgFailure::Unavailable(format!("rg stderr read failed: {err}")))
            });
        crate::engine_shell::clear_dispatch_child();
        let status = status?;
        let stdout = stdout?;
        let stderr = stderr?;
        let output = std::process::Output {
            status,
            stdout,
            stderr,
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        match classify_rg_exit(tool, output.status.code(), stderr.trim()) {
            Ok(RgExit::Complete) if output.status.code() == Some(1) => continue,
            Ok(RgExit::Complete) => {}
            Ok(RgExit::Incomplete) => {
                stats.unreadable_entries = stats.unreadable_entries.saturating_add(1);
            }
            Err(err) => return Err(err),
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let base = root.display().to_string();
        let mut root_matches: Vec<SearchMatch> = stdout
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let parsed = parse_rg_line(line, &base);
                if parsed.is_none() {
                    stats.unparsed_rows += 1;
                }
                parsed
            })
            .collect();
        // rg's parallel traversal emits files in nondeterministic order; sort
        // component-wise to match the internal scanner's sorted DFS so both
        // backends produce byte-identical flat output (and the same prefix
        // under truncation).
        root_matches.sort_by(|a, b| {
            Path::new(&a.path)
                .cmp(Path::new(&b.path))
                .then(a.line.cmp(&b.line))
        });
        for row in root_matches {
            if matches.len() >= max_results {
                stats.truncated_by_results = true;
                break;
            }
            matches.push(row);
        }
        if stats.truncated_by_results {
            break;
        }
    }
    stats.matched_lines = matches.len();
    let mut paths: Vec<&str> = matches.iter().map(|m| m.path.as_str()).collect();
    paths.dedup();
    stats.matched_files = paths.len();
    // rg does not report how many files it scanned; the matched-file count is
    // the only honest lower bound available for visited_files.
    stats.visited_files = stats.matched_files;
    Ok((matches, stats))
}

/// Parse one `path:line:text` row of rg output. The known root prefix is
/// stripped before splitting so Windows drive colons (and roots that contain
/// `:`) never confuse the parse; only the relative remainder is examined.
pub fn parse_rg_line(line: &str, base: &str) -> Option<SearchMatch> {
    let rest = line.strip_prefix(base)?;
    if let Some(tail) = rest.strip_prefix(':') {
        // Root is the matched file itself: "<base>:<line>:<text>"; rel falls
        // back to the full path exactly like collect_search.
        let mut fields = tail.splitn(2, ':');
        let line_number = fields.next()?.parse::<usize>().ok()?;
        let text = fields.next().unwrap_or("");
        return Some(SearchMatch {
            base: base.to_string(),
            path: base.to_string(),
            rel: base.to_string(),
            line: line_number,
            text: text.to_string(),
        });
    }
    let tail = rest.strip_prefix(std::path::MAIN_SEPARATOR).unwrap_or(rest);
    // A relative path may itself contain `:` (legal on unix), making rg's
    // text format ambiguous. Scan `:<digits>:` boundaries left to right and
    // prefer the first whose prefix exists as a file under the root — the
    // only reliable disambiguator; fall back to the first parseable
    // boundary when nothing verifies (deleted-mid-search files).
    let (rel_end, line_number, text_start) = find_rg_field_boundary(tail, Path::new(base))?;
    let rel = &tail[..rel_end];
    let path_end = line.len() - tail.len() + rel.len();
    Some(SearchMatch {
        base: base.to_string(),
        path: line[..path_end].to_string(),
        rel: rel.to_string(),
        line: line_number,
        text: tail[text_start..].to_string(),
    })
}

fn find_rg_field_boundary(tail: &str, root: &Path) -> Option<(usize, usize, usize)> {
    let mut search_from = 0;
    let mut chosen = None;
    while let Some(offset) = tail[search_from..].find(':') {
        let rel_end = search_from + offset;
        let after = &tail[rel_end + 1..];
        if let Some((second, line)) = after
            .find(':')
            .and_then(|second| after[..second].parse().ok().map(|line| (second, line)))
        {
            let candidate = (rel_end, line, rel_end + second + 2);
            chosen.get_or_insert(candidate);
            if root.join(&tail[..rel_end]).is_file() {
                return Some(candidate);
            }
        }
        search_from = rel_end + 1;
    }
    chosen
}

pub(crate) struct TreeEntry {
    pub(crate) rel: String,
    pub(crate) name: String,
    pub(crate) depth: usize,
    pub(crate) dir: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_tree(
    root: &Path,
    current: &Path,
    depth: usize,
    include_hidden: bool,
    max_files: usize,
    level: usize,
    rows: &mut Vec<TreeEntry>,
    unreadable: &mut usize,
) {
    if rows.len() >= max_files || depth == 0 {
        return;
    }
    let Some(entries) = sorted_entries(current, unreadable) else {
        return;
    };
    for path in entries {
        if rows.len() >= max_files || should_skip(&path, include_hidden) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| rel.display().to_string());
        let dir = path.is_dir();
        rows.push(TreeEntry {
            rel: rel.display().to_string(),
            name,
            depth: level,
            dir,
        });
        if dir {
            collect_tree(
                root,
                &path,
                depth - 1,
                include_hidden,
                max_files,
                level + 1,
                rows,
                unreadable,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_glob(
    root: &Path,
    current: &Path,
    matcher: &GlobMatcher,
    pattern_has_separator: bool,
    include_hidden: bool,
    max_files: usize,
    depth: usize,
    rows: &mut Vec<PathBuf>,
    unreadable: &mut usize,
) {
    if rows.len() >= max_files || depth == 0 {
        return;
    }
    if current.is_file() {
        if glob_matches(root, current, matcher, pattern_has_separator) {
            rows.push(current.to_path_buf());
        }
        return;
    }
    let Some(entries) = sorted_entries(current, unreadable) else {
        return;
    };
    for path in entries {
        if rows.len() >= max_files || should_skip(&path, include_hidden) || is_symlink(&path) {
            continue;
        }
        if path.is_dir() {
            collect_glob(
                root,
                &path,
                matcher,
                pattern_has_separator,
                include_hidden,
                max_files,
                depth - 1,
                rows,
                unreadable,
            );
        } else if glob_matches(root, &path, matcher, pattern_has_separator) {
            rows.push(path);
        }
    }
}

pub(crate) fn display_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

pub(crate) fn glob_matches(
    root: &Path,
    path: &Path,
    matcher: &GlobMatcher,
    pattern_has_separator: bool,
) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    matcher.is_match(rel)
        || matcher.is_match(path)
        || (!pattern_has_separator
            && path
                .file_name()
                .is_some_and(|file_name| matcher.is_match(Path::new(file_name))))
}

pub(crate) fn should_skip(path: &Path, include_hidden: bool) -> bool {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    matches!(name, ".git" | "target" | ".venv" | "__pycache__")
        || (!include_hidden && name.starts_with('.'))
}

#[cfg(test)]
mod classify_rg_exit_tests {
    use super::{
        classify_rg_exit, collect_search, sorted_entries, RgExit, RgFailure, SearchStats,
        MAX_WALK_DEPTH,
    };

    #[test]
    fn grep_exit_2_empty_stderr_is_incomplete_io_not_invalid_pattern() {
        match classify_rg_exit("grep", Some(2), "") {
            Ok(RgExit::Incomplete) => {}
            other => panic!("empty-stderr exit 2 is --no-messages IO, got {other:?}"),
        }
    }

    #[test]
    fn grep_exit_2_with_stderr_is_invalid_pattern() {
        match classify_rg_exit("grep", Some(2), "rg: regex parse error") {
            Err(RgFailure::InvalidPattern(message)) => {
                assert!(
                    message.contains("regex parse error"),
                    "non-empty stderr is the parse diagnostic: {message}"
                );
            }
            other => panic!("expected InvalidPattern, got {other:?}"),
        }
    }

    #[test]
    fn find_exit_2_is_incomplete_not_substring_fallback() {
        match classify_rg_exit("find", Some(2), "") {
            Ok(RgExit::Incomplete) => {}
            other => panic!("find --no-messages exit 2 is Incomplete, got {other:?}"),
        }
        match classify_rg_exit("find", Some(2), "regex parse error") {
            Ok(RgExit::Incomplete) => {}
            other => panic!("find must not treat stderr as InvalidPattern, got {other:?}"),
        }
    }

    #[test]
    fn grep_no_match_is_complete() {
        assert_eq!(
            classify_rg_exit("grep", Some(1), "").unwrap(),
            RgExit::Complete
        );
        assert_eq!(
            classify_rg_exit("grep", Some(0), "").unwrap(),
            RgExit::Complete
        );
    }

    #[test]
    fn sorted_entries_on_a_file_counts_unreadable_instead_of_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").expect("write");
        let mut unreadable = 0;
        assert!(
            sorted_entries(&file, &mut unreadable).is_none(),
            "read_dir on a file must not look like an empty directory"
        );
        assert!(
            unreadable >= 1,
            "unreadable counter must move; got {unreadable}"
        );
    }

    #[test]
    fn collect_search_missing_root_is_unreadable_not_complete_miss() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        let mut stats = SearchStats::default();
        let mut matches = Vec::new();
        collect_search(
            &missing,
            &missing,
            "needle",
            10,
            100,
            MAX_WALK_DEPTH,
            &mut stats,
            &mut matches,
        );
        assert!(matches.is_empty());
        assert!(
            stats.unreadable_entries >= 1,
            "missing root must not render as a complete 0-match scan; unreadable={}",
            stats.unreadable_entries
        );
    }
}
