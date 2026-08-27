use crate::*;

pub(crate) fn failed_segment(cmd: &str, out: &str, err: &str, code: Option<i32>) -> Option<String> {
    if is_search_no_match(cmd, out, err, code) || is_expected_false_exit(cmd, out, err, code) {
        return None;
    }
    if looks_env_invocation_failure(cmd, out, err, code) {
        return Some(cmd.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(s) = masked_or_failure_segment(cmd, out, err, code)
        .or_else(|| masked_pipeline_failure_segment(cmd, out, err, code))
    {
        return Some(s);
    }
    let segments = split_shell_segments(cmd);
    let combined = format!("{out}\n{err}").to_ascii_lowercase();
    let fail_out = if code == Some(0) {
        err.to_ascii_lowercase()
    } else {
        combined.clone()
    };
    // With exit_code 0 only pipelines, sequences, and or-lists can hide an upstream
    // status; an and-list that reached the end reported every status it produced. So
    // reporting a failed segment there would contradict the exit code.
    if code == Some(0) && !can_mask_upstream_status(cmd) {
        return None;
    }
    let not_found = not_found_actors(&fail_out);
    if let Some(s) = segments.iter().find(|s| {
        is_explicit_false_segment(s)
            || is_cd_failure_segment(s, code, &fail_out)
            || is_command_not_found_segment(s, &not_found)
    }) {
        return Some((*s).clone());
    }
    code.is_some_and(|c| c != 0)
        .then(|| {
            if let Some(s) = empty_pipeline_search_failure_segment(cmd, out, err, code) {
                return Some(s);
            }
            if let Some(s) = evidence_named_segment(&segments, out, err) {
                return Some(s);
            }
            if looks_diagnostic(&combined)
                && let Some(s) = diagnostic_attributed_segment(&segments, out, err)
            {
                return Some(s);
            }
            segments.last().cloned().filter(|v| !v.is_empty())
        })
        .flatten()
}

pub(crate) fn can_mask_upstream_status(command: &str) -> bool {
    shell_operator_features(command)
        .iter()
        .any(|f| matches!(*f, "pipeline" | "sequence" | "or-list"))
}

/// A leading `!` negates a segment's exit status, so its own diagnostic output is
/// expected rather than evidence of a failure elsewhere in the chain.
pub(crate) fn strip_segment_negation(segment: &str) -> (&str, bool) {
    let trimmed = segment.trim();
    if let Some(rest) = trimmed.strip_prefix('!') {
        let rest = rest.trim_start();
        if !rest.is_empty() && !rest.starts_with('=') {
            return (rest, true);
        }
    }
    (trimmed, false)
}

fn segment_command_name(segment: &str) -> Option<String> {
    let (bare, _) = strip_segment_negation(segment);
    split_shell_words(&shell_analysis_command(bare))
        .first()
        .map(|word| shell_command_basename(word).to_ascii_lowercase())
        .filter(|name| !name.is_empty())
}

/// Command names named by a "not found" diagnostic, so the segment that could not
/// be executed is attributed instead of an earlier segment that ran fine.
fn not_found_actors(failure_output: &str) -> Vec<String> {
    const NOISE: &str = "sh bash zsh command not found such file or directory no";
    failure_output
        .lines()
        .filter(|line| line.contains("not found"))
        .flat_map(|line| line.split([':', ' ', '\t', '`', '\'', '"']))
        .map(|token| shell_command_basename(token.trim()).to_ascii_lowercase())
        .filter(|token| !token.is_empty() && !contains_any_ws(token, NOISE))
        .collect()
}

fn is_cd_failure_segment(segment: &str, exit_code: Option<i32>, failure_output: &str) -> bool {
    segment.to_ascii_lowercase().starts_with("cd ")
        && exit_code.is_some_and(|code| code != 0)
        && contains_any(failure_output, "can't cd|no such file|not a directory")
}

fn is_command_not_found_segment(segment: &str, not_found_actors: &[String]) -> bool {
    !not_found_actors.is_empty()
        && segment_command_name(segment).is_some_and(|name| not_found_actors.contains(&name))
}

fn empty_pipeline_search_failure_segment(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> Option<String> {
    let features = shell_operator_features(command);
    if exit_code != Some(1)
        || !stdout.trim().is_empty()
        || !stderr.trim().is_empty()
        || !features.contains(&"pipeline")
        || features
            .iter()
            .any(|feature| matches!(*feature, "and-list" | "or-list"))
    {
        return None;
    }
    split_shell_segments(command)
        .into_iter()
        .rev()
        .find(|segment| {
            let words = split_shell_words(&shell_analysis_command(segment));
            let is_search = words
                .first()
                .map(|word| shell_command_basename(word))
                .is_some_and(|name| is_search_command(&name));
            let suppresses_output = words.iter().skip(1).any(|word| {
                matches!(word.as_str(), "--quiet" | "--silent")
                    || word
                        .strip_prefix('-')
                        .is_some_and(|flags| flags.contains('q') || flags.contains('s'))
            });
            is_search && !suppresses_output
        })
}

/// The segment whose own command name prefixes an error line, e.g. `tail: cannot
/// open` in `cargo test | tail -5` or `npm ERR!` in a pipefail chain. This is the
/// strongest available signal for which segment produced the controlling status.
fn evidence_named_segment(segments: &[String], stdout: &str, stderr: &str) -> Option<String> {
    let scanned = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let names: Vec<_> = segments
        .iter()
        .map(|segment| segment_command_name(segment))
        .collect();
    for line in scanned.lines() {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else {
            continue;
        };
        // An actor prefix is `name: message` or `name ERR! message`; a bare word
        // that merely contains "error" is message text, not an attribution.
        let prefixed = first.ends_with(':')
            || words
                .next()
                .is_some_and(|second| second.to_ascii_uppercase().starts_with("ERR"));
        let actor = shell_command_basename(first.trim_end_matches(':')).to_ascii_lowercase();
        if actor.is_empty() || !prefixed {
            continue;
        }
        if let Some(index) = names
            .iter()
            .position(|name| name.as_deref() == Some(actor.as_str()))
        {
            return Some(segments[index].clone());
        }
    }
    None
}

/// Families the failure output itself points at, used to break ties when several
/// segments of an and-list or sequence belong to a diagnostic-producing family.
fn output_indicated_family(stdout: &str, stderr: &str) -> Option<&'static str> {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if contains_any(
        &combined,
        "test failed|test result: failed|tests failed|assertion failed|panicked at|failures:",
    ) {
        return Some("test");
    }
    if contains_any(&combined, "could not compile|error[e") {
        return Some("build");
    }
    None
}

fn diagnostic_attributed_segment(
    segments: &[String],
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    let negated = |segment: &String| strip_segment_negation(segment).1;
    let bare = |segment: &String| strip_segment_negation(segment).0.to_string();
    if let Some(family) = output_indicated_family(stdout, stderr) {
        for want_negated in [true, false] {
            if let Some(segment) = segments.iter().find(|segment| {
                negated(segment) == want_negated
                    && shell_family(&bare(segment), stdout, stderr) == family
            }) {
                return Some(segment.clone());
            }
        }
    }
    for want_negated in [true, false] {
        if let Some(segment) = segments.iter().find(|segment| {
            negated(segment) == want_negated
                && is_diagnostic_failure_segment(&bare(segment), stdout, stderr)
        }) {
            return Some(segment.clone());
        }
    }
    None
}

pub(crate) fn is_diagnostic_failure_segment(segment: &str, stdout: &str, stderr: &str) -> bool {
    is_one_of(
        &shell_family(segment, stdout, stderr),
        "test build lint python-test go-test",
    ) || segment.contains("--check")
}

pub(crate) fn masked_or_failure_segment(
    cmd: &str,
    out: &str,
    err: &str,
    code: Option<i32>,
) -> Option<String> {
    if code != Some(0) || is_masked_expected_false_or(cmd, out, err, code) {
        return None;
    }
    // Only the final element of the `||` left-hand side supplies its exit status, so
    // attribution must land there rather than on an earlier `&&`/`;` element.
    let lhs = first_or_list_lhs(cmd)?;
    let elements = split_shell_list_elements(&lhs);
    let s = elements.last()?.trim();
    if s.is_empty() || is_expected_false_segment(s, out, err) {
        return None;
    }
    let segments = split_shell_segments(s);
    if let Some(named) = evidence_named_segment(&segments, out, err) {
        return Some(named);
    }
    looks_masked_failure_evidence(out, err, Some(s)).then(|| s.to_string())
}

pub(crate) fn masked_pipeline_failure_segment(
    cmd: &str,
    out: &str,
    err: &str,
    code: Option<i32>,
) -> Option<String> {
    if code != Some(0)
        || is_masked_expected_false_pipeline(cmd, out, err, code)
        || !shell_operator_features(cmd).contains(&"pipeline")
    {
        return None;
    }
    // Only the last list element decides the reported status, so a masked pipeline
    // failure must be attributed inside it and never to an earlier `&&`/`;` segment.
    let element = split_shell_list_elements(cmd).into_iter().next_back()?;
    let segments = split_shell_segments(&element);
    let named = evidence_named_segment(&segments, out, err);
    if named.is_none()
        && !looks_masked_failure_evidence(out, err, segments.first().map(String::as_str))
    {
        return None;
    }
    named.or_else(|| segments.into_iter().find(|s| !s.is_empty()))
}

pub(crate) fn masking_warning(
    cmd: &str,
    out: &str,
    err: &str,
    code: Option<i32>,
) -> Option<String> {
    if (is_repo_inventory_command(cmd) && code == Some(0) && err.trim().is_empty())
        || looks_env_invocation_failure(cmd, out, err, code)
        || is_masked_expected_false_or(cmd, out, err, code)
        || is_masked_expected_false_pipeline(cmd, out, err, code)
        || !shell_operator_features(cmd)
            .iter()
            .any(|f| matches!(*f, "pipeline" | "sequence" | "or-list"))
    {
        return None;
    }
    let should = if code == Some(0) {
        split_shell_segments(cmd)
            .iter()
            .any(|s| is_explicit_false_segment(s))
            || looks_masked_failure_evidence(out, err, first_nonempty_shell_segment(cmd).as_deref())
            // Exit 0 plus a failed_segment is pipeline_masked: keep the warning
            // so callers can inspect refs or rerun with pipefail.
            || failed_segment(cmd, out, err, code).is_some()
    } else {
        let comb = format!("{out}\n{err}").to_ascii_lowercase();
        split_shell_segments(cmd)
            .iter()
            .any(|s| is_explicit_false_segment(s))
            || contains_any(
                &comb,
                "not found|no such file|permission denied|unrecognized option|invalid option|usage:|error",
            )
    };
    should.then(|| {
        "compound or pipeline syntax can mask upstream failure; inspect refs or rerun with pipefail".to_string()
    })
}

pub(crate) fn pipeline_rerun_command(command: &str, warning: Option<&String>) -> Option<String> {
    if cfg!(windows) || warning.is_none() || !shell_operator_features(command).contains(&"pipeline")
    {
        return None;
    }
    let cmd = shell_analysis_command(command);
    (!cmd.trim().is_empty()).then(|| {
        format!(
            "bash -o pipefail -c {}",
            shell_display_arg(cmd.trim(), "posix")
        )
    })
}

pub(crate) fn first_or_list_lhs(command: &str) -> Option<String> {
    let command = shell_analysis_command(command);
    let mut cursor = QuoteCursor::new(&command);
    while let Some((idx, ch, next)) = cursor.next_unquoted() {
        if (ch, next) == ('|', Some('|')) {
            return Some(command[..idx].trim().to_string());
        }
    }
    None
}

pub(crate) fn first_nonempty_shell_segment(command: &str) -> Option<String> {
    split_shell_segments(command)
        .into_iter()
        .find(|s| !s.is_empty())
}

fn line_has_structured_masked_failure_evidence(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let low = t.to_ascii_lowercase();
    low.starts_with("error:")
        || low.starts_with("error[")
        || low.starts_with("warning:")
        || low.starts_with("fatal:")
        || contains_any(
            &low,
            "panic|traceback|command not found|no such file or directory|permission denied|assertion failed|unrecognized option|invalid option",
        )
}

const SEARCH_DIAG_PREFIXES: &str = "error:|warning:|fatal:|panic|traceback|rg:|grep:|ripgrep:";
const SEARCH_DIAG_NEEDLES: &str = "regex parse error|unrecognized option|invalid option|permission denied|no such file or directory";

fn search_stdout_line_is_diagnostic(line: &str) -> bool {
    let low = line.trim_start().to_ascii_lowercase();
    starts_with_any(&low, SEARCH_DIAG_PREFIXES) || contains_any(&low, SEARCH_DIAG_NEEDLES)
}

pub(crate) fn looks_masked_failure_evidence(out: &str, err: &str, head: Option<&str>) -> bool {
    !err.trim().is_empty() && err.lines().any(line_has_structured_masked_failure_evidence)
        || head.is_some_and(|h| {
            split_shell_words(&shell_analysis_command(h))
                .first()
                .is_some_and(|w| is_search_command(w))
                && !out.trim().is_empty()
                && out.lines().any(search_stdout_line_is_diagnostic)
        })
        || !out.trim().is_empty() && out.lines().any(line_has_structured_masked_failure_evidence)
}

pub(crate) fn shell_syntax_summary_for_status(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> String {
    let features = if looks_env_invocation_failure(command, stdout, stderr, exit_code) {
        raw_shell_operator_features(command)
    } else {
        shell_operator_features(command)
    };
    if features.is_empty() {
        "argv/simple".to_string()
    } else {
        features.join(",")
    }
}

pub(crate) fn shell_operator_features(command: &str) -> Vec<&'static str> {
    raw_shell_operator_features(&shell_analysis_command(command))
}

struct QuoteCursor<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    quote: Option<char>,
    escaped: bool,
}

impl<'a> QuoteCursor<'a> {
    fn new(command: &'a str) -> Self {
        Self {
            chars: command.char_indices().peekable(),
            quote: None,
            escaped: false,
        }
    }

    fn next_unquoted(&mut self) -> Option<(usize, char, Option<char>)> {
        while let Some((idx, ch)) = self.chars.next() {
            if self.escaped {
                self.escaped = false;
            } else if self.quote != Some('\'') && ch == '\\' {
                self.escaped = true;
            } else if Some(ch) == self.quote {
                self.quote = None;
            } else if self.quote.is_none() && (ch == '\'' || ch == '"') {
                self.quote = Some(ch);
            } else if self.quote.is_none() {
                return Some((idx, ch, self.chars.peek().map(|(_, n)| *n)));
            }
        }
        None
    }
}

pub(crate) fn raw_shell_operator_features(command: &str) -> Vec<&'static str> {
    let mut features = Vec::new();
    let mut cursor = QuoteCursor::new(command);
    while let Some((_, ch, next)) = cursor.next_unquoted() {
        let f = match (ch, next) {
            ('&', Some('&')) => {
                cursor.chars.next();
                "and-list"
            }
            ('|', Some('|')) => {
                cursor.chars.next();
                "or-list"
            }
            ('|', _) => "pipeline",
            (';', _) => "sequence",
            ('>' | '<', _) => "redirect",
            ('$', Some('(')) => {
                cursor.chars.next();
                "subshell"
            }
            ('`', _) => "subshell",
            _ => continue,
        };
        if !features.contains(&f) {
            features.push(f);
        }
    }
    features
}

/// Top-level list elements, split on `&&`, `||`, and `;` but NOT on `|`: a
/// pipeline is one element whose internal stages share a single exit status.
pub(crate) fn split_shell_list_elements(command: &str) -> Vec<String> {
    let command = shell_analysis_command(command);
    let mut elements = Vec::new();
    let (mut start, mut cursor) = (0, QuoteCursor::new(&command));
    while let Some((idx, ch, next)) = cursor.next_unquoted() {
        let len = match (ch, next) {
            ('&', Some('&')) | ('|', Some('|')) => {
                cursor.chars.next();
                2
            }
            ('|', _) => continue,
            (';', _) => 1,
            _ => continue,
        };
        let element = command[start..idx].trim();
        if !element.is_empty() {
            elements.push(element.to_string());
        }
        start = idx + len;
    }
    let element = command[start..].trim();
    if !element.is_empty() {
        elements.push(element.to_string());
    }
    elements
}

pub(crate) fn split_shell_segments(command: &str) -> Vec<String> {
    let command = shell_analysis_command(command);
    let mut segments = Vec::new();
    let (mut start, mut cursor) = (0, QuoteCursor::new(&command));
    while let Some((idx, ch, next)) = cursor.next_unquoted() {
        let len = match (ch, next) {
            ('&', Some('&')) | ('|', Some('|')) => {
                cursor.chars.next();
                2
            }
            ('|' | ';', _) => 1,
            _ => continue,
        };
        let seg = command[start..idx].trim();
        if !seg.is_empty() {
            segments.push(seg.to_string());
        }
        start = idx + len;
    }
    let seg = command[start..].trim();
    if !seg.is_empty() {
        segments.push(seg.to_string());
    }
    segments
}

pub(crate) fn shell_analysis_command(command: &str) -> String {
    shell_analysis_command_from_words(&split_shell_words(command))
        .unwrap_or_else(|| command.to_string())
}

pub(crate) fn shell_analysis_command_from_words(words: &[String]) -> Option<String> {
    let first = words
        .first()
        .map(|w| shell_command_basename(w))
        .unwrap_or_default();
    match first.as_str() {
        "sh" | "bash" | "zsh" => shell_c_command_argument(words),
        "cmd" => cmd_command_argument(words),
        "powershell" | "pwsh" => powershell_command_argument(words),
        "env" => env_split_string_analysis_command(words).or_else(|| {
            env_wrapped_command_index(words)
                .and_then(|idx| shell_analysis_command_from_words(&words[idx..]))
        }),
        _ => None,
    }
}

pub(crate) fn cmd_command_argument(words: &[String]) -> Option<String> {
    let mut idx = 1;
    while idx < words.len() {
        let w = &words[idx];
        let low = w.to_ascii_lowercase();
        if matches!(low.as_str(), "/c" | "/k") {
            return shell_command_tail(words, idx + 1, "cmd");
        }
        if (low.starts_with("/c") || low.starts_with("/k")) && low.len() > 2 {
            return Some(w[2..].trim().to_string()).filter(|c| !c.is_empty());
        }
        if low.starts_with('/') {
            idx += 1;
        } else {
            return None;
        }
    }
    None
}

const POWERSHELL_VAL_OPTIONS: &str = "-configurationname -executionpolicy -inputformat -outputformat -settingsfile -version -windowstyle -workingdirectory";

pub(crate) fn powershell_command_argument(words: &[String]) -> Option<String> {
    let is_val_opt = |o: &str| contains_any_ws(o, POWERSHELL_VAL_OPTIONS);
    let is_inline_val = |o: &str| o.split_once(':').is_some_and(|(opt, _)| is_val_opt(opt));
    let mut idx = 1;
    while idx < words.len() {
        let w = &words[idx];
        let low = w.to_ascii_lowercase();
        if matches!(low.as_str(), "-command" | "-c") {
            return shell_command_tail(words, idx + 1, "powershell");
        }
        if low.starts_with("-command:") {
            return Some(w["-command:".len()..].trim().to_string()).filter(|c| !c.is_empty());
        }
        if matches!(
            low.as_str(),
            "-encodedcommand" | "-enc" | "-e" | "-file" | "-f"
        ) {
            return None;
        }
        if is_inline_val(&low) || (low.starts_with('-') && !is_val_opt(&low)) {
            idx += 1;
        } else if is_val_opt(&low) {
            idx += 2;
        } else {
            return None;
        }
    }
    None
}

pub(crate) fn shell_command_tail(words: &[String], start: usize, style: &str) -> Option<String> {
    let tail = words.get(start..)?;
    (!tail.is_empty()).then(|| {
        if tail.len() == 1 {
            tail[0].clone()
        } else if !tail.first().is_some_and(|w| is_search_command(w)) {
            tail.join(" ")
        } else {
            tail.iter()
                .map(|w| shell_display_arg(w, style))
                .collect::<Vec<_>>()
                .join(" ")
        }
    })
}

pub(crate) fn env_split_string_analysis_command(words: &[String]) -> Option<String> {
    let split_words = env_split_string_words(words)?;
    if split_words.is_empty() {
        return None;
    }
    let mut env_words = vec!["env".to_string()];
    env_words.extend(split_words);
    shell_analysis_command_from_words(&env_words[env_wrapped_command_index(&env_words)?..])
}

fn advance_env_option(words: &[String], index: &mut usize) -> bool {
    let word = words[*index].as_str();
    if matches!(
        word,
        "-u" | "--unset" | "-C" | "--chdir" | "--argv0" | "-S" | "--split-string"
    ) {
        *index += 2;
        true
    } else if matches!(
        word,
        "-i" | "--ignore-environment" | "-0" | "--null" | "--debug"
    ) || ["--unset=", "--chdir=", "--argv0=", "--split-string="]
        .iter()
        .any(|p| word.starts_with(p))
        || (!word.starts_with('-') && word.split_once('=').is_some_and(|(k, _)| !k.is_empty()))
    {
        *index += 1;
        true
    } else {
        false
    }
}

pub(crate) fn env_split_string_words(words: &[String]) -> Option<Vec<String>> {
    let mut idx = 1;
    while idx < words.len() {
        let w = &words[idx];
        if w == "--" {
            return None;
        }
        if matches!(w.as_str(), "-S" | "--split-string") {
            return words.get(idx + 1).map(|v| split_shell_words(v));
        }
        if let Some(v) = w.strip_prefix("--split-string=") {
            return Some(split_shell_words(v));
        }
        if !advance_env_option(words, &mut idx) {
            return None;
        }
    }
    None
}

pub(crate) fn env_wrapped_command_index(words: &[String]) -> Option<usize> {
    let mut idx = 1;
    while idx < words.len() {
        let w = &words[idx];
        if w == "--" {
            return (idx + 1 < words.len()).then_some(idx + 1);
        }
        if advance_env_option(words, &mut idx) {
            continue;
        }
        return (!w.starts_with('-')).then_some(idx);
    }
    None
}

pub(crate) fn looks_env_invocation_failure(
    cmd: &str,
    out: &str,
    err: &str,
    code: Option<i32>,
) -> bool {
    if code == Some(0) {
        return false;
    }
    let words = split_shell_words(cmd);
    if !words
        .first()
        .is_some_and(|w| shell_command_basename(w) == "env")
    {
        return false;
    }
    if !words
        .iter()
        .any(|w| matches!(w.as_str(), "-C" | "--chdir") || w.starts_with("--chdir="))
    {
        return false;
    }
    let comb = format!("{out}\n{err}").to_ascii_lowercase();
    comb.contains("env:") && contains_any(&comb, "cannot change directory|not a directory|chdir")
}

pub(crate) fn shell_c_command_argument(words: &[String]) -> Option<String> {
    let has_c = |w: &str| {
        w.strip_prefix('-')
            .is_some_and(|f| !f.is_empty() && !f.starts_with('-') && f.chars().any(|ch| ch == 'c'))
    };
    let mut idx = 1;
    while idx < words.len() {
        let w = &words[idx];
        if matches!(w.as_str(), "-c" | "--command") {
            return words.get(idx + 1).cloned();
        }
        if let Some(cmd) = w.strip_prefix("--command=") {
            return Some(cmd.to_string());
        }
        if matches!(w.as_str(), "-o" | "+o" | "-O" | "+O") {
            idx += 2;
        } else if has_c(w) {
            return words.get(idx + 1).cloned();
        } else if w == "--" || (!w.starts_with('-') && !w.starts_with('+')) {
            return None;
        } else {
            idx += 1;
        }
    }
    None
}

pub(crate) fn split_shell_words(command: &str) -> Vec<String> {
    let (mut words, mut cur, mut quote) = (Vec::new(), String::new(), None);
    for ch in command.chars() {
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch.is_whitespace() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

const DIAG_KEYWORDS: &str = "error|warning|failed|failure|panic|traceback|exception|assertion|expected|actual|not ok|prompt|enter ";

pub(crate) fn looks_diagnostic(text: &str) -> bool {
    text.lines().any(looks_critical_line)
}

pub(crate) fn looks_critical_line(line: &str) -> bool {
    contains_any(&line.to_ascii_lowercase(), DIAG_KEYWORDS)
}

pub(crate) fn repeated_line_count(text: &str) -> usize {
    let (mut prev, mut repeats) = ("", 0);
    for line in text.lines() {
        if line == prev && !line.trim().is_empty() {
            repeats += 1;
        }
        prev = line;
    }
    repeats
}

pub(crate) fn looks_status_table(text: &str) -> bool {
    text.lines().any(|line| {
        let upper = line.to_ascii_uppercase();
        upper.contains("STATUS") && (upper.contains("NAME") || upper.contains("READY"))
    })
}

pub(crate) fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub(crate) fn compact_json(value: &serde_json::Value) -> String {
    let mut text = serde_json::to_string(value).unwrap_or_default();
    if text.len() > 240 {
        text.truncate(240);
        text += "...";
    }
    text
}

pub(crate) fn is_abnormal_json(value: &serde_json::Value) -> bool {
    contains_any_ws(
        &value.to_string().to_ascii_lowercase(),
        "error failed unhealthy pending crash warning",
    )
}

