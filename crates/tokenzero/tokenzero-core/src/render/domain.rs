use crate::shell_parse::split_shell_segments;
use crate::*;

#[derive(Default)]
pub(crate) struct InventoryStats<'a> {
    pub files: usize,
    pub dirs: usize,
    /// Entries matching neither the dir nor the file predicate. They are still
    /// real listing entries and must never vanish without a trace.
    pub other: usize,
    pub line_counts: Vec<&'a str>,
    pub paths: Vec<&'a str>,
}

pub(crate) fn inventory_stats<'a>(
    output: &'a str,
    sample_limit: usize,
    is_file: impl Fn(&str) -> bool,
) -> InventoryStats<'a> {
    let mut stats = InventoryStats::default();
    for line in output.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("===") || line.starts_with("---") {
            continue;
        }
        if line.ends_with('/') {
            stats.dirs += 1;
        } else {
            if is_file(line) {
                stats.files += 1;
            } else {
                // A bare `ls` prints relative names with no trailing slash, so a
                // dotless entry like `src`, `target` or `README` satisfies neither
                // arm. Dropping it made the view claim a smaller listing than the
                // command actually produced, with no marker that anything was
                // omitted, so the caller silently read a truncated directory.
                stats.other += 1;
            }
            if stats.paths.len() < sample_limit {
                stats.paths.push(line);
            }
        }
        if line
            .split_whitespace()
            .next()
            .is_some_and(|value| value.parse::<usize>().is_ok())
        {
            stats.line_counts.push(line);
        }
    }
    stats
}

pub fn is_repo_inventory_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let inventory_shape =
        (lower.contains("find ") || lower.contains(" tree") || lower.starts_with("tree"))
            && (lower.contains("echo") || lower.contains("wc -l") || lower.contains("sort"))
            || lower.contains("find . -type f")
            || lower.contains("get-childitem")
            || lower.contains("gci ")
            || segment_is_bare_ls(&lower);
    inventory_shape && all_segments_inventory_safe(command)
}

fn segment_is_bare_ls(lower: &str) -> bool {
    split_shell_segments(lower)
        .iter()
        .any(|segment| segment == "ls" || segment.starts_with("ls -") || segment.starts_with("ls "))
}

/// Commands whose output is a listing of paths, or a filter over one.
///
/// `cat` is deliberately absent. It emits FILE CONTENT, not paths, and the
/// inventory view keeps any line containing `/` or `.` as a "path". A command
/// like `ls dir && cat report.json` was therefore classified as inventory and
/// the JSON body was shredded into sample_paths, one entry per line, e.g.
/// `"contract_version": "1.0",` and `"name": "ctx.step",`.
/// The content was unreadable in the visible output and had to be recovered
/// through the ref. Keeping `cat` out means such a pipeline falls through to
/// normal rendering, which is what the caller wanted.
const INVENTORY_COMMANDS: &[&str] = &[
    "ls",
    "find",
    "tree",
    "dir",
    "gci",
    "get-childitem",
    "sort",
    "wc",
    "head",
    "tail",
    "uniq",
    "cut",
    "echo",
    "sort-object",
    "select-object",
    "where-object",
    "measure-object",
];

fn all_segments_inventory_safe(command: &str) -> bool {
    split_shell_segments(command).iter().all(|segment| {
        split_shell_words(segment)
            .first()
            .map(|word| shell_command_basename(word))
            .is_some_and(|first| INVENTORY_COMMANDS.contains(&first.as_str()))
    })
}

pub fn repo_inventory_view(command: &str, output: &str) -> String {
    let stats = inventory_stats(output, 20, |line| line.contains('/') || line.contains('.'));
    let mut out = String::new();
    out.push_str("repo_inventory:\n");
    out.push_str(&format!("command: {command}\n"));
    out.push_str(&format!(
        "files_seen: {}\ndirs_seen: {}\n",
        stats.files, stats.dirs
    ));
    // Only emitted when nonzero, so a cleanly classified listing renders exactly
    // as before and only an otherwise-lossy one grows a line.
    if stats.other > 0 {
        out.push_str(&format!("other_entries_seen: {}\n", stats.other));
    }
    for (label, values, limit) in [
        ("linecount_summary", &stats.line_counts, 12),
        ("sample_paths", &stats.paths, 20),
    ] {
        if values.is_empty() {
            continue;
        }
        out.push_str(label);
        out.push_str(":\n");
        for value in values.iter().take(limit) {
            out.push_str(&format!("- {value}\n"));
        }
    }
    out
}

pub fn structured_shell_view(command: &str, stdout: &str, stderr: &str) -> String {
    let combined = format!("{stdout}\n{stderr}");
    if is_repo_inventory_command(command) {
        // stdout only: an inventory is a stdout concept, and folding stderr in
        // put diagnostics into sample_paths as though they were paths, e.g.
        //   - ls: /nope: No such file or directory
        // sitting beside real entries. A consumer reading sample_paths as paths
        // cannot tell the difference.
        return repo_inventory_view(command, stdout);
    }
    if is_search_shell_command(command) {
        return search_shell_view(command, stdout, stderr);
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        let mut out = String::from("json_summary:\n");
        match value {
            serde_json::Value::Object(map) => {
                out.push_str(&format!("type: object\nkeys: {}\n", map.len()));
                for (key, value) in map.iter().take(20) {
                    out.push_str(&format!("- {key}: {}\n", json_kind(value)));
                }
            }
            serde_json::Value::Array(items) => {
                out.push_str(&format!("type: array\nitems: {}\n", items.len()));
                for item in items.iter().take(20) {
                    if is_abnormal_json(item) {
                        out.push_str(&format!("- abnormal: {}\n", compact_json(item)));
                    }
                }
            }
            other => out.push_str(&format!("type: {}\n", json_kind(&other))),
        }
        return out;
    }
    if looks_status_table(&combined) {
        let mut out = String::from("status_summary:\n");
        for line in combined.lines().take(80) {
            let lower = line.to_ascii_lowercase();
            if [
                "error",
                "failed",
                "crash",
                "pending",
                "terminating",
                "unhealthy",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
                || line.starts_with("NAME")
            {
                out.push_str(line);
                out.push('\n');
            }
        }
        if out.lines().count() > 1 {
            return out;
        }
    }
    if let Some((tool, count)) = requested_output_lines(command) {
        let take = count.min(MAX_HONORED_HEAD_LINES);
        return match tool {
            "tail" => summarize_lines(&combined, 0, take, ""),
            _ => summarize_lines(&combined, take, 0, ""),
        };
    }
    summarize_lines(&combined, 40, 16, "")
}

/// True only when EVERY top-level segment is a search command or a pure
/// line filter. `search_shell_view` labels all stdout lines as matches, so a
/// mixed command like `grep X; ls Y` must never take the search view: the
/// ls output would be presented as grep matches.
pub fn is_search_shell_command(command: &str) -> bool {
    let segments = split_shell_segments(command);
    let mut any_search = false;
    for segment in &segments {
        let Some(first) = split_shell_words(segment)
            .first()
            .map(|word| shell_command_basename(word))
        else {
            return false;
        };
        if is_search_command(&first) {
            any_search = true;
        } else if !SEARCH_FILTERS.contains(&first.as_str()) {
            return false;
        }
    }
    any_search
}

const SEARCH_FILTERS: &[&str] = &[
    "head", "tail", "sort", "uniq", "wc", "cut", "tr", "cat", "tee",
];

pub(crate) fn is_search_command(command: &str) -> bool {
    const COMMANDS: &[&str] = &["rg", "grep", "egrep", "fgrep", "ag", "ack", "findstr"];
    COMMANDS.contains(&shell_command_basename(command).as_str())
}

pub(crate) fn shell_command_basename(command: &str) -> String {
    let leaf = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let stem = leaf
        .rsplit_once('.')
        .and_then(|(stem, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "exe" | "cmd" | "bat" | "com"
            )
            .then_some(stem)
        })
        .unwrap_or(leaf);
    stem.to_ascii_lowercase()
}

pub(crate) fn is_search_no_match(
    command: &str,
    _stdout: &str,
    _stderr: &str,
    exit_code: Option<i32>,
) -> bool {
    // rg/grep exit 1 is "no match" (or a partial run with some hits plus a
    // permission warning). Empty-stream was too strict: any stderr line
    // turned a search into policy: diagnostic and buried stdout.
    exit_code == Some(1) && is_search_shell_command(command)
}

pub(crate) fn is_expected_false_exit(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> bool {
    if exit_code != Some(1) {
        return false;
    }
    if shell_operator_features(command).contains(&"pipeline") {
        return is_expected_false_pipeline_exit(command, stdout, stderr);
    }
    is_expected_false_segment(command, stdout, stderr)
}

fn is_expected_false_pipeline_edge(command: &str, stdout: &str, stderr: &str, first: bool) -> bool {
    let segments = split_shell_segments(command);
    let edge = if first {
        segments.split_first()
    } else {
        segments.split_last()
    };
    edge.is_some_and(|(candidate, others)| {
        is_expected_false_segment(candidate, stdout, stderr)
            && !others
                .iter()
                .any(|segment| is_explicit_false_segment(segment))
    })
}

pub(crate) fn is_expected_false_pipeline_exit(command: &str, stdout: &str, stderr: &str) -> bool {
    is_expected_false_pipeline_edge(command, stdout, stderr, false)
}

pub(crate) fn is_expected_false_segment(command: &str, stdout: &str, stderr: &str) -> bool {
    if !stderr.trim().is_empty() {
        return false;
    }
    let command = shell_analysis_command(command);
    let words = split_shell_words(&command);
    let first = words
        .first()
        .map(|word| shell_command_basename(word))
        .unwrap_or_default();
    match first.as_str() {
        "test" | "[" | "[[" => stdout.trim().is_empty(),
        command if is_search_command(command) => stdout.trim().is_empty(),
        "command" => {
            words.get(1).is_some_and(|word| word == "-v")
                && words.len() >= 3
                && words[2..].iter().all(|word| !word.starts_with('-'))
        }
        "cmp" | "diff" => true,
        "git" => {
            let Some(subcommand_index) = git_subcommand_index(&words) else {
                return false;
            };
            let is_diff = words
                .get(subcommand_index)
                .is_some_and(|word| word == "diff");
            let diff_args = &words[subcommand_index + 1..];
            let asks_for_status = diff_args
                .iter()
                .any(|word| word == "--quiet" || word == "--exit-code");
            let check_mode = diff_args.iter().any(|word| word == "--check");
            is_diff && asks_for_status && !check_mode
        }
        _ => false,
    }
}

pub(crate) fn is_masked_expected_false_or(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> bool {
    exit_code == Some(0)
        && first_or_list_lhs(command)
            .filter(|segment| !segment.is_empty())
            .is_some_and(|segment| is_expected_false_segment(&segment, stdout, stderr))
}

pub(crate) fn is_masked_expected_false_pipeline(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> bool {
    exit_code == Some(0)
        && stderr.trim().is_empty()
        && shell_operator_features(command).contains(&"pipeline")
        && is_expected_false_pipeline_edge(command, stdout, stderr, true)
}

pub(crate) fn is_explicit_false_segment(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    lower == "false" || lower.starts_with("false ")
}

pub(crate) fn git_subcommand_index(words: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < words.len() {
        let word = words[index].as_str();
        if matches!(
            word,
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            index += 2;
        } else if word.starts_with('-') {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

const DEFAULT_SEARCH_SAMPLE_LINES: usize = 80;
const MAX_HONORED_HEAD_LINES: usize = 256;

/// `head -n N` / `tail -n N` / `head -N`. The caller asked for these lines;
/// a 20-line sample hid the overflow at line 218 of a `head -n 200` dump.
pub(crate) fn requested_output_lines(command: &str) -> Option<(&'static str, usize)> {
    let words = split_shell_words(command);
    let first = words.first().map(|word| shell_command_basename(word))?;
    let tool = match first.as_str() {
        "head" => "head",
        "tail" => "tail",
        _ => return None,
    };
    let mut index = 1;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "-n" || word == "--lines" {
            let count = words.get(index + 1)?.trim_end_matches(['k', 'K', 'm', 'M']);
            return count.parse().ok().map(|n| (tool, n));
        }
        if let Some(inline) = word
            .strip_prefix("-n")
            .or_else(|| word.strip_prefix("--lines="))
        {
            if !inline.is_empty() {
                return inline
                    .trim_end_matches(['k', 'K', 'm', 'M'])
                    .parse()
                    .ok()
                    .map(|n| (tool, n));
            }
        }
        if tool == "head" || tool == "tail" {
            if let Some(digits) = word
                .strip_prefix('-')
                .filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
            {
                return digits.parse().ok().map(|n| (tool, n));
            }
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }
        break;
    }
    None
}

/// Largest `-A`/`-B`/`-C` value in a search command, if any.
///
/// A bare `grep pattern file` yields one line per match, so a 20-line sample is
/// a fair summary. `grep -A 30` does not: the caller has explicitly said the
/// surrounding lines ARE the answer, and a 20-line cap can cut off before the
/// first match's context ends, leaving the one thing that was asked for
/// reachable only through a second expand of the ref.
fn requested_context_lines(command: &str) -> Option<usize> {
    let words = split_shell_words(command);
    let mut largest: Option<usize> = None;
    for (index, word) in words.iter().enumerate() {
        let flag = word.trim_start_matches('-');
        let Some(kind) = flag.chars().next().filter(|c| matches!(c, 'A' | 'B' | 'C')) else {
            continue;
        };
        if !word.starts_with('-') || word.starts_with("--") {
            continue;
        }
        // Both -A30 and -A 30 are valid.
        let inline = &flag[kind.len_utf8()..];
        let value = if inline.is_empty() {
            words
                .get(index + 1)
                .and_then(|next| next.parse::<usize>().ok())
        } else {
            inline.parse::<usize>().ok()
        };
        if let Some(value) = value {
            largest = Some(largest.map_or(value, |current: usize| current.max(value)));
        }
    }
    largest
}

pub(crate) fn search_shell_view(command: &str, stdout: &str, stderr: &str) -> String {
    let matches: Vec<_> = stdout
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    let diagnostics: Vec<_> = stderr
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && looks_critical_line(line))
        .collect();

    let mut out = String::from("search_summary:\n");
    out.push_str(&format!("matches_seen: {}\n", matches.len()));
    if !diagnostics.is_empty() {
        out.push_str("diagnostics:\n");
        for line in diagnostics.iter().take(6) {
            out.push_str(&format!("- {line}\n"));
        }
    }
    // Honor an explicit context request: show enough lines to cover the
    // context the caller asked for, rather than a flat sample cap.
    let limit = match requested_context_lines(command) {
        Some(context) => DEFAULT_SEARCH_SAMPLE_LINES.max((context + 1) * 4),
        None => DEFAULT_SEARCH_SAMPLE_LINES,
    };
    if !matches.is_empty() {
        out.push_str("sample_matches:\n");
        for line in matches.iter().take(limit) {
            out.push_str(&format!("- {line}\n"));
        }
        if matches.len() > limit {
            out.push_str(&format!(
                "... omitted {} matches; exact ref available ...\n",
                matches.len() - limit
            ));
        }
    }
    out
}

pub fn diagnostic_shell_view(stdout: &str, stderr: &str, max_visible_tokens: usize) -> String {
    let combined = format!("{stdout}\n{stderr}");
    let critical = critical_lines(&combined, 3);
    let view = if critical.trim().is_empty() {
        summarize_lines(&combined, 16, 12, "")
    } else {
        critical
    };
    enforce_token_budget(&view, max_visible_tokens)
}

/// Preserve the final bytes of both streams when a zero-exit shell pipeline is
/// diagnostically hazardous. The normal diagnostic view prioritizes critical
/// lines, which can otherwise hide the downstream command's stdout and exit marker.
pub fn diagnostic_shell_view_with_tail(
    stdout: &str,
    stderr: &str,
    max_visible_tokens: usize,
) -> String {
    let mut view = String::new();
    for (label, stream) in [("# final stdout:\n", stdout), ("# final stderr:\n", stderr)] {
        let mut tail: Vec<_> = stream
            .lines()
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(12)
            .collect();
        tail.reverse();
        if tail.is_empty() {
            continue;
        }
        if !view.is_empty() {
            view.push('\n');
        }
        view.push_str(label);
        view.push_str(&tail.join("\n"));
        view.push('\n');
    }

    let combined = format!("{stdout}\n{stderr}");
    let critical = critical_lines(&combined, 3);
    if critical
        .lines()
        .any(|line| !line.trim().is_empty() && !view.lines().any(|visible| visible == line))
    {
        if !view.is_empty() {
            view.push('\n');
        }
        view.push_str("# critical diagnostics:\n");
        view.push_str(critical.trim_end());
    }
    if view.trim().is_empty() {
        view = summarize_lines(&combined, 16, 12, "");
    }
    enforce_token_budget(&view, max_visible_tokens)
}
