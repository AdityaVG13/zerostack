#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

pub const PULSE_SCHEMA_VERSION: &str = "tokenzero.pulse";

macro_rules! string_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident, $as_vis:vis as_str {
        $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)?
    }) => {
        $(#[$meta])*
        $vis enum $name { $($(#[$variant_meta])* $variant),+ }

        impl $name {
            $as_vis fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum Mode, as_str {
        #[default] Auto => "auto",
        Passthrough => "passthrough",
        Diagnostic => "diagnostic",
        Structured => "structured",
        Dedupe => "dedupe",
        DiffAware => "diff-aware",
        Exact => "exact",
    }
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const MAP: &[(&[&str], Mode)] = &[
            (&["auto"], Mode::Auto),
            (&["passthrough"], Mode::Passthrough),
            (&["diagnostic"], Mode::Diagnostic),
            (&["structured"], Mode::Structured),
            (&["dedupe"], Mode::Dedupe),
            (&["diff-aware", "diff_aware", "diffaware"], Mode::DiffAware),
            (&["exact"], Mode::Exact),
        ];
        MAP.iter()
            .find(|(aliases, _)| aliases.contains(&s))
            .map(|(_, m)| *m)
            .ok_or_else(|| format!("unsupported mode: {s}"))
    }
}

string_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ContentType, as_str {
        Code => "code",
        ShellOutput => "shell_output",
        SearchResult => "search_result",
        Tree => "tree",
        Diff => "diff",
        JsonConfig => "json_config",
        Markdown => "markdown",
        Logs => "logs",
        Unknown => "unknown",
    }
}

/// Returns true if `haystack` starts with any pipe-delimited prefix.
pub(crate) fn starts_with_any(h: &str, p: &str) -> bool {
    p.split('|').any(|n| h.starts_with(n))
}

/// Returns true if `haystack` contains any pipe-delimited needle.
pub(crate) fn contains_any(h: &str, p: &str) -> bool {
    p.split('|').any(|n| h.contains(n))
}

/// Returns true if `haystack` contains any whitespace-delimited needle.
pub(crate) fn contains_any_ws(h: &str, n: &str) -> bool {
    n.split_whitespace().any(|w| h.contains(w))
}

const DIAGNOSTIC_KEYWORDS: &str = "error|warning|failed|failure|panic|traceback|exception|assertion|expected|actual|not ok|prompt|enter ";
const DIFF_LINE_PREFIXES: &str = "diff --git|index |--- |+++ |@@|rename |deleted file|new file|+|-";
const SECRET_TOKEN_PREFIXES: &str = "sk-|sk-proj-|ghp_|github_pat_|AKIA|glpat-|xoxb-|xoxp-";

fn looks_critical_line(line: &str) -> bool {
    contains_any(&line.to_ascii_lowercase(), DIAGNOSTIC_KEYWORDS)
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossySpan {
    pub description: String,
    pub reason: String,
    pub recovery_may_be_needed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capsule {
    pub text: String,
    pub raw_tokens: usize,
    pub visible_tokens: usize,
    pub omitted_lines: usize,
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected_anchors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lossy_spans: Vec<LossySpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lossy_policy_id: Option<String>,
}

impl Capsule {
    /// Enforce RACC omission rule: transformed bytes require an exact selector,
    /// a visible protected anchor, or an explicit lossy declaration.
    pub fn validate_omission_rule(&self, original: &str) -> Result<(), String> {
        let original = original.trim_end();
        if original.is_empty() || self.text.contains(original) {
            return Ok(());
        }
        if self
            .protected_anchors
            .iter()
            .any(|anchor| !anchor.is_empty() && self.text.contains(&format!("[[anchor:{anchor}]]")))
        {
            return Ok(());
        }
        if self
            .exact_refs
            .iter()
            .any(|reference| exact_ref_has_selector(reference) && self.text.contains(reference))
        {
            return Ok(());
        }
        let lossy_declared = self
            .lossy_policy_id
            .as_ref()
            .is_some_and(|id| !id.is_empty())
            && !self.lossy_spans.is_empty()
            && self.lossy_spans.iter().all(|span| {
                !span.description.is_empty()
                    && !span.reason.is_empty()
                    && span.recovery_may_be_needed
            })
            && self.text.contains("mode=lossy")
            && self.text.contains("lossy_policy_id=");
        lossy_declared.then_some(()).ok_or_else(|| {
            "capsule omitted bytes without a protected anchor, exact z://blob selector, or explicit lossy declaration".to_string()
        })
    }
}

fn exact_recovery_scheme(reference: &str) -> bool {
    let base = reference
        .split_once('#')
        .map_or(reference, |(base, _)| base);
    base.starts_with("z://blob/")
}

fn exact_ref_has_selector(reference: &str) -> bool {
    let Some((base, selector)) = reference.split_once('#') else {
        return false;
    };
    if !exact_recovery_scheme(base) || selector.is_empty() {
        return false;
    }
    // Same published grammar expand accepts: `#Bstart-end`, `#Bstart+len`,
    // `#Bn`, `#Lstart-end`, `#Lstart-Lend`, `#Ln`. A recovery cue the
    // expander cannot parse is not a selector.
    if let Some(bytes) = selector.strip_prefix('B') {
        if let Some((start, end)) = bytes.split_once('-') {
            return start.parse::<usize>().is_ok() && end.parse::<usize>().is_ok();
        }
        if let Some((start, len)) = bytes.split_once('+') {
            return start.parse::<usize>().is_ok() && len.parse::<usize>().is_ok();
        }
        return bytes.parse::<usize>().is_ok();
    }
    if let Some(lines) = selector.strip_prefix('L') {
        if let Some((start, end)) = lines.split_once('-') {
            return start.parse::<usize>().is_ok()
                && end.trim_start_matches('L').parse::<usize>().is_ok();
        }
        return lines.parse::<usize>().is_ok();
    }
    selector
        .strip_prefix("symbol=")
        .is_some_and(|symbol| !symbol.is_empty())
}

fn exact_recovery_ref(reference: &str, byte_len: usize) -> Option<String> {
    exact_recovery_scheme(reference).then(|| {
        if reference.contains('#') {
            reference.to_string()
        } else {
            format!("{reference}#B0-{byte_len}")
        }
    })
}

fn validated_capsule(capsule: Capsule, original: &str) -> Result<Capsule, String> {
    capsule.validate_omission_rule(original)?;
    Ok(capsule)
}

fn finalize_capsule_omission(
    mut capsule: Capsule,
    original: &str,
    max_visible_tokens: usize,
    exact_ref: Option<String>,
) -> Result<Capsule, String> {
    let original_trimmed = original.trim_end();
    let omitted = !original_trimmed.is_empty() && !capsule.text.contains(original_trimmed);
    if omitted {
        if let Some(reference) = exact_ref.filter(|value| exact_ref_has_selector(value)) {
            // validate_omission_rule requires the selector to be present in the VISIBLE TEXT, not merely
            // recorded in exact_refs: a ref a caller cannot see is a ref it cannot expand, so recording it
            // alone would satisfy the struct while still stranding the omitted bytes.
            if !capsule.text.contains(&reference) {
                capsule.text.push('\n');
                capsule.text.push_str(&format!(
                    "... omitted by visible budget; expand {reference} for the full output ..."
                ));
                capsule.visible_tokens = count_tokens(&capsule.text);
            }
            capsule.exact_refs.push(reference);
        } else {
            let mut declared = capsule.text.clone();
            if !declared.contains("mode=lossy") {
                declared.push('\n');
                declared.push_str(VISIBLE_BUDGET_LOSSY_DECLARATION);
            }
            let declared_tokens = count_tokens(&declared);
            let raw_full_tokens = count_tokens(original_trimmed);
            if declared_tokens >= raw_full_tokens {
                // Inflation guard: a lossy declaration plus summary that costs more than
                // the raw payload is not a compression. Exact mode is not exempt — hiding
                // one token behind a 10-token stub is still worse than the raw payload.
                capsule.text = original_trimmed.to_string();
                capsule.visible_tokens = raw_full_tokens;
                capsule.omitted_lines = 0;
            } else {
                capsule.lossy_policy_id = Some("tokenzero.visible-compression".to_string());
                capsule.lossy_spans.push(LossySpan {
                    description: "bytes omitted from the visible capsule".to_string(),
                    reason: "visible token budget or selected compression policy".to_string(),
                    recovery_may_be_needed: true,
                });
                capsule.text = enforce_token_budget_with_ref(&declared, max_visible_tokens, None);
                capsule.visible_tokens = count_tokens(&capsule.text);
            }
        }
    }
    validated_capsule(apply_never_worse_passthrough(capsule, original), original)
}

/// A capsule whose visible cost exceeds the raw payload is not a save.
/// Emit the payload itself so callers cannot report spent>raw as compression.
fn apply_never_worse_passthrough(mut capsule: Capsule, original: &str) -> Capsule {
    let raw_text = original.trim_end();
    let raw_count = count_tokens(raw_text);
    if raw_count < capsule.visible_tokens {
        capsule.text = raw_text.to_string();
        capsule.visible_tokens = raw_count;
        capsule.omitted_lines = 0;
        capsule.exact_refs.clear();
        capsule.lossy_spans.clear();
        capsule.lossy_policy_id = None;
        capsule.mode = Mode::Passthrough;
    }
    capsule
}

pub fn make_capsule(
    text: &str,
    mode: Mode,
    max_visible_tokens: usize,
    label: Option<&str>,
) -> Result<Capsule, String> {
    let raw_tokens = count_tokens(text);
    make_capsule_with_raw_tokens(text, raw_tokens, mode, max_visible_tokens, label)
}

pub fn make_capsule_with_raw_tokens(
    text: &str,
    raw_tokens: usize,
    mode: Mode,
    max_visible_tokens: usize,
    label: Option<&str>,
) -> Result<Capsule, String> {
    make_capsule_with_recovery_ref(text, raw_tokens, mode, max_visible_tokens, label, None)
}

/// Adds an inline exact-ref recovery cue to a token-budgeted capsule.
pub fn make_capsule_with_recovery_ref(
    text: &str,
    raw_tokens: usize,
    mode: Mode,
    max_tokens: usize,
    label: Option<&str>,
    recovery_ref: Option<&str>,
) -> Result<Capsule, String> {
    let prefix = capsule_prefix(label, max_tokens, raw_tokens);
    let exact_ref = recovery_ref.and_then(|reference| exact_recovery_ref(reference, text.len()));
    let policy = mode;
    let mut visible = match policy {
        Mode::Exact => format!("{prefix}[exact payload stored; use expand for raw bytes]"),
        Mode::Passthrough => format!("{prefix}{}", text.trim_end()),
        Mode::Diagnostic => match error_block(text, 3) {
            b if b.trim().is_empty() => summarize_lines(text, 8, 6, &prefix),
            b => format!("{prefix}{}", b.trim_end()),
        },
        Mode::Structured => summarize_lines(text, 24, 16, &prefix),
        Mode::Dedupe => format!("{prefix}{}", dedupe_lines(text, 8).trim_end()),
        Mode::DiffAware => format!("{prefix}{}", diff_summary(text, 120).trim_end()),
        Mode::Auto if max_tokens == 0 || raw_tokens <= max_tokens => {
            format!("{prefix}{}", text.trim_end())
        }
        Mode::Auto => summarize_lines(text, 18, 12, &prefix),
    };
    if policy != Mode::Passthrough {
        visible = enforce_token_budget_with_ref(&visible, max_tokens, exact_ref.as_deref());
    }
    let mut visible_tokens = count_tokens(&visible);
    let mut mode = mode;
    if visible_tokens > raw_tokens {
        let fallback = text.trim_end().to_string();
        let fallback_tokens = count_tokens(&fallback);
        if fallback_tokens < visible_tokens {
            visible_tokens = fallback_tokens;
            visible = fallback;
            mode = Mode::Passthrough;
        }
    }
    finalize_capsule_omission(
        Capsule {
            visible_tokens,
            raw_tokens,
            omitted_lines: text.lines().count().saturating_sub(visible.lines().count()),
            text: visible,
            mode,
            protected_anchors: Vec::new(),
            exact_refs: Vec::new(),
            lossy_spans: Vec::new(),
            lossy_policy_id: None,
        },
        text,
        max_tokens,
        exact_ref,
    )
}

/// Creates a domain-aware summary with byte-exact recovery via `recovery_ref`.
pub fn make_capsule_content_aware(
    text: &str,
    raw_tokens: usize,
    content_type: ContentType,
    max_visible_tokens: usize,
    label: Option<&str>,
    recovery_ref: Option<&str>,
    aggressive: bool,
) -> Result<Capsule, String> {
    if !aggressive && (max_visible_tokens == 0 || raw_tokens <= max_visible_tokens) {
        return make_capsule_with_recovery_ref(
            text,
            raw_tokens,
            Mode::Auto,
            max_visible_tokens,
            label,
            recovery_ref,
        );
    }
    let prefix = capsule_prefix(label, max_visible_tokens, raw_tokens);
    let exact_ref = recovery_ref.and_then(|reference| exact_recovery_ref(reference, text.len()));
    let budget = if aggressive {
        max_visible_tokens / 3
    } else {
        max_visible_tokens
    };
    let visible = match content_type {
        ContentType::Code => summarize_code(text, budget, &prefix),
        ContentType::Logs | ContentType::ShellOutput => summarize_logs(text, budget, &prefix),
        ContentType::JsonConfig => summarize_json(text, budget, &prefix),
        ContentType::Diff => summarize_lines(text, 12, 8, &prefix),
        ContentType::SearchResult => summarize_lines(text, 20, 5, &prefix),
        _ => summarize_lines(text, 18, 12, &prefix),
    };
    let visible = enforce_token_budget_with_ref(&visible, max_visible_tokens, exact_ref.as_deref());
    let visible_tokens = count_tokens(&visible);
    finalize_capsule_omission(
        Capsule {
            omitted_lines: text.lines().count().saturating_sub(visible.lines().count()),
            text: visible,
            raw_tokens,
            visible_tokens,
            mode: if aggressive { Mode::Exact } else { Mode::Auto },
            protected_anchors: Vec::new(),
            exact_refs: Vec::new(),
            lossy_spans: Vec::new(),
            lossy_policy_id: None,
        },
        text,
        max_visible_tokens,
        exact_ref,
    )
}

/// Summarize code: show first N lines (imports/signatures) + last M lines.
const CODE_SIG_PREFIXES: &str =
    "pub |fn |struct |enum |impl |trait |class |def |function |export |import |use |#[";

fn push_labeled_lines(out: &mut String, label: &str, lines: &[&str], limit: usize) {
    if lines.is_empty() {
        return;
    }
    out.push_str(label);
    for line in lines.iter().take(limit) {
        out.push_str(line);
        out.push('\n');
    }
}

fn summarize_code(text: &str, budget_tokens: usize, prefix: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total <= 30 {
        return format!("{prefix}{}", text.trim_end());
    }
    let sigs: Vec<&str> = lines
        .iter()
        .take(total.min(80))
        .filter(|l| starts_with_any(l.trim(), CODE_SIG_PREFIXES))
        .copied()
        .collect();
    let head = 8.min(total);
    let tail = 6.min(total.saturating_sub(head));
    let mut out = format!("{prefix}{}", lines[..head].join("\n"));
    push_labeled_lines(
        &mut out,
        "\n\n# declarations/signatures:\n",
        &sigs,
        budget_tokens / 8,
    );
    out.push_str(&omitted_lines_marker(total.saturating_sub(head + tail)));
    out + &lines[total - tail..].join("\n")
}

/// Summarize logs: prioritize errors/warnings, then head+tail.
const LOG_ERROR_NEEDLES: &str = "error fatal panic failed traceback";

fn summarize_logs(text: &str, budget_tokens: usize, prefix: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let (mut errs, mut warns) = (Vec::new(), Vec::new());
    for l in &lines {
        let low = l.to_ascii_lowercase();
        if contains_any_ws(&low, LOG_ERROR_NEEDLES) {
            errs.push(*l);
        } else if low.contains("warn") {
            warns.push(*l);
        }
    }
    let mut out = prefix.to_string();
    let limit = budget_tokens / 6;
    push_labeled_lines(
        &mut out,
        &format!("# {} error(s):\n", errs.len()),
        &errs,
        limit,
    );
    push_labeled_lines(
        &mut out,
        &format!("# {} warning(s):\n", warns.len()),
        &warns,
        limit / 2,
    );
    if errs.is_empty() && warns.is_empty() {
        let head = 6.min(lines.len());
        let tail = 4.min(lines.len().saturating_sub(head));
        out.push_str(&lines[..head].join("\n"));
        if lines.len() > head + tail {
            out.push_str(&format!(
                "\n... omitted {} lines ...\n",
                lines.len().saturating_sub(head + tail)
            ));
        }
        if tail > 0 {
            out.push_str(&lines[lines.len() - tail..].join("\n"));
        }
    } else {
        out.push_str(&format!(
            "# {} total lines; exact ref available",
            lines.len()
        ));
    }
    out
}

/// Summarize JSON: show schema shape (keys, types, array lengths).
fn summarize_json(text: &str, _budget_tokens: usize, prefix: &str) -> String {
    let mut out = prefix.to_string();
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(serde_json::Value::Object(map)) => {
            out.push_str(&format!("json_object: {} keys\n", map.len()));
            for (key, val) in map.iter().take(25) {
                let kind = match val {
                    serde_json::Value::String(s) if s.len() > 100 => "string(long)",
                    serde_json::Value::Array(a) if a.is_empty() => "array(0)",
                    serde_json::Value::Object(o) if o.is_empty() => "object(0)",
                    other => json_kind(other),
                };
                out.push_str(&format!("  {key}: {kind}\n"));
            }
        }
        Ok(serde_json::Value::Array(items)) => {
            out.push_str(&format!("json_array: {} items\n", items.len()));
            if let Some(first) = items.first() {
                let sample: String = serde_json::to_string(first)
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect();
                out.push_str(&format!("  sample: {sample}\n"));
            }
        }
        _ => return summarize_lines(text, 12, 8, prefix),
    }
    out + "# exact ref available for full content"
}

pub fn summarize_lines(text: &str, head: usize, tail: usize, prefix: &str) -> String {
    let lines: Vec<_> = text.lines().collect();
    // Saturate: `head = usize::MAX` must keep the whole text, not wrap and panic
    // on `lines[..head]`.
    if lines.len() <= head.saturating_add(tail).saturating_add(3) {
        return format!("{prefix}{}", text.trim_end());
    }
    format!(
        "{prefix}{}\n\n... omitted {} lines; exact ref available ...\n\n{}",
        lines[..head].join("\n"),
        lines.len().saturating_sub(head.saturating_add(tail)),
        lines[lines.len() - tail..].join("\n"),
    )
}

fn capsule_prefix(label: Option<&str>, max_visible_tokens: usize, raw_tokens: usize) -> String {
    let Some(label) = label else {
        return String::new();
    };
    let full = format!("# {label}\n");
    if max_visible_tokens == 0 {
        return full;
    }
    let budget = max_visible_tokens.saturating_sub(raw_tokens).max(4);
    if count_tokens(&full) <= budget {
        return full;
    }
    let compact = format!("# {}\n", compact_label(label));
    if count_tokens(&compact) <= budget || count_tokens(&compact) < count_tokens(&full) {
        compact
    } else {
        "# source\n".to_string()
    }
}

fn compact_label(label: &str) -> String {
    if label.contains(['\\', '/'])
        && let Some(name) = Path::new(label).file_name().and_then(|name| name.to_str())
    {
        return format!(".../{name}");
    }
    let mut chars = label.chars();
    let head: String = chars.by_ref().take(48).collect();
    chars
        .next()
        .map_or_else(|| label.to_string(), |_| format!("{head}..."))
}

/// Selects information-dense lines within a soft budget while always retaining criticals.
pub fn summarize_tokens(text: &str, max_tokens: usize, prefix: &str) -> String {
    if max_tokens == 0 {
        return format!("{prefix}{}", text.trim_end());
    }
    let lines: Vec<&str> = text.lines().collect();
    if count_tokens(text) <= max_tokens || lines.len() <= 4 {
        return format!("{prefix}{}", text.trim_end());
    }
    let n = lines.len();
    let line_tokens: Vec<usize> = lines.iter().map(|l| count_tokens(l)).collect();
    let mut order: Vec<usize> = (0..n).collect();
    let scores: Vec<u32> = (0..n)
        .map(|idx| {
            let line = lines[idx];
            if looks_critical_line(line) {
                100
            } else if line.trim().is_empty() {
                0
            } else {
                let mut s = if idx < 3 || idx + 3 >= n { 60 } else { 0 };
                if line_information_density(line) {
                    s += 30;
                }
                s.max(1)
            }
        })
        .collect();
    order.sort_by(|a, b| scores[*b].cmp(&scores[*a]).then(a.cmp(b)));
    let mut selected = vec![false; n];
    let mut spent = 0usize;
    for &idx in &order {
        let cost = line_tokens[idx];
        if scores[idx] >= 100 || (scores[idx] != 0 && spent + cost + 13 <= max_tokens) {
            selected[idx] = true;
            spent = if scores[idx] >= 100 {
                spent.saturating_add(cost)
            } else {
                spent + cost
            };
        }
    }
    for idx in 1..n.saturating_sub(1) {
        if !selected[idx] && selected[idx - 1] && selected[idx + 1] && line_tokens[idx] <= 13 {
            selected[idx] = true;
        }
    }
    if !selected.iter().any(|v| *v) {
        return summarize_lines(text, 8, 6, prefix);
    }
    let mut out = prefix.to_string();
    let mut omitted = 0;
    for idx in 0..n {
        if !selected[idx] {
            omitted += 1;
            continue;
        }
        if omitted > 0 {
            push_summary_line(
                &mut out,
                &format!("... +{omitted} lines; exact ref available ..."),
            );
            omitted = 0;
        }
        push_summary_line(&mut out, lines[idx]);
    }
    if omitted > 0 {
        push_summary_line(
            &mut out,
            &format!("... +{omitted} lines; exact ref available ..."),
        );
    }
    out
}

fn push_summary_line(out: &mut String, line: &str) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
}

/// Detects artifact identifiers, paths, line references, numbers, or hashes.
fn line_information_density(line: &str) -> bool {
    let (digits, paths) = line.chars().fold((0, 0), |(d, p), c| {
        (
            d + usize::from(c.is_ascii_digit()),
            p + usize::from(c == '/' || c == '\\'),
        )
    });
    digits >= 3 || paths >= 2 || line.contains(".rs:") || line.contains(".py:")
}

/// Shell-only dedupe also collapses digit-varying runs while preserving critical lines.
pub fn dedupe_lines_impl(text: &str, context: usize, structural: bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let norm = structural.then(|| {
        lines
            .iter()
            .map(|l| normalize_digit_runs(l))
            .collect::<Vec<_>>()
    });
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if structural && looks_critical_line(line) {
            out.push(line.to_string());
            idx += 1;
            continue;
        }
        let exact = lines[idx..]
            .iter()
            .take_while(|candidate| **candidate == line)
            .count();
        if exact >= 3 {
            out.push(line.to_string());
            out.push(format!("... repeated {} more times ...", exact - 1));
            idx += exact;
            continue;
        }
        if let Some(n) = norm.as_ref() {
            let similar = (idx..lines.len())
                .take_while(|&i| !looks_critical_line(lines[i]) && n[i] == n[idx])
                .count();
            if similar >= 4 {
                out.push(line.to_string());
                out.push(format!(
                    "... {} similar lines collapsed (digits vary); exact ref available ...",
                    similar - 1
                ));
                idx += similar;
                continue;
            }
        }
        out.extend(lines[idx..idx + exact].iter().map(|l| l.to_string()));
        idx += exact;
    }
    compact_head_tail(out, context)
}

fn compact_head_tail(out: Vec<String>, context: usize) -> String {
    // Saturate: `context = usize::MAX` must keep the whole buffer, not wrap
    // `context * 2` and panic on `out[..context]`.
    if out.len() <= context.saturating_mul(2).saturating_add(20) {
        return out.join("\n");
    }
    format!(
        "{}\n... omitted {} lines; exact ref available ...\n{}",
        out[..context].join("\n"),
        out.len().saturating_sub(context.saturating_mul(2)),
        out[out.len() - context..].join("\n")
    )
}

fn normalize_digit_runs(line: &str) -> String {
    let (mut out, mut in_d) = (String::with_capacity(line.len()), false);
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_d {
                out.push('#');
                in_d = true;
            }
        } else {
            in_d = false;
            out.push(c);
        }
    }
    out
}

pub fn diff_summary(text: &str, max_lines: usize) -> String {
    let out: Vec<_> = text
        .lines()
        .filter(|l| starts_with_any(l, DIFF_LINE_PREFIXES))
        .take(max_lines.max(1))
        .collect();
    if out.is_empty() {
        summarize_lines(text, 18, 12, "")
    } else {
        out.join("\n")
    }
}

pub fn dedupe_lines(text: &str, context: usize) -> String {
    dedupe_lines_impl(text, context, false)
}

pub fn mask_visible_secrets(text: &str) -> String {
    text.lines()
        .map(mask_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn mask_secret_line(line: &str) -> String {
    let low = line.to_ascii_lowercase();
    // Longer keys first so aws_secret_access_key= is not missed because secret= does not
    // match secret_access. Keep trailing space on "bearer " so the marker matches
    // SECRET_MARKERS and the mask lands after the separator (not glued as "bearer[masked]").
    if let Some((key, pos)) = [
        "aws_secret_access_key=",
        "aws_access_key_id=",
        "token=",
        "password=",
        "secret=",
        "api_key=",
        "apikey=",
        "x-api-key:",
        "api-key:",
        "authorization:",
        "bearer ",
    ]
    .into_iter()
    .find_map(|key| low.find(key).map(|pos| (key, pos)))
    {
        return format!("{}[masked]", &line[..pos + key.len()]);
    }
    line.split_whitespace()
        .map(|word| {
            if starts_with_any(word, SECRET_TOKEN_PREFIXES) {
                "[masked]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn critical_lines(text: &str, radius: usize) -> String {
    keyword_window_view(text, radius, looks_critical_line)
}

pub fn error_block(text: &str, radius: usize) -> String {
    keyword_window_view(text, radius, |line| regex_like_error(&line))
}

/// Keeps radius windows around hits and marks every omitted gap explicitly.
fn omitted_lines_marker(n: usize) -> String {
    format!("... omitted {n} lines; exact ref available ...")
}
fn keyword_window_view(text: &str, radius: usize, is_hit: impl Fn(&str) -> bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = vec![false; lines.len()];
    for (idx, line) in lines.iter().enumerate() {
        if is_hit(line) {
            let (start, end) = (
                idx.saturating_sub(radius),
                idx.saturating_add(radius)
                    .saturating_add(1)
                    .min(lines.len()),
            );
            keep[start..end].fill(true);
        }
    }
    if !keep.iter().any(|&k| k) {
        return String::new();
    }
    let (mut out, mut idx) = (Vec::new(), 0);
    while idx < lines.len() {
        if keep[idx] {
            out.push(lines[idx].to_string());
            idx += 1;
        } else {
            let start = idx;
            while idx < lines.len() && !keep[idx] {
                idx += 1;
            }
            out.push(omitted_lines_marker(idx - start));
        }
    }
    out.join("\n")
}

const ERROR_NEEDLES: &str = "error exception traceback failed assertion panic expected actual";

fn regex_like_error(line: &&str) -> bool {
    contains_any_ws(&line.to_ascii_lowercase(), ERROR_NEEDLES)
}

pub fn line_range(text: &str, start: usize, end: usize) -> String {
    let start = start.max(1);
    let end = end.max(start);
    text.lines()
        .skip(start - 1)
        .take(end - start + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn symbol_block(text: &str, symbol: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(hit) = lines.iter().position(|line| contains_word(line, symbol)) else {
        return String::new();
    };
    let mut start = hit;
    while start > 0 && lines[start - 1].starts_with([' ', '\t']) {
        start -= 1;
    }
    let indent = leading_ws(lines[hit]);
    let mut end = hit + 1;
    while end < lines.len() {
        let line = lines[end];
        if !line.trim().is_empty() && leading_ws(line) <= indent && end > hit + 1 {
            break;
        }
        end += 1;
    }
    lines[start..end].join("\n")
}

fn contains_word(line: &str, symbol: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word == symbol)
}

fn leading_ws(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "go", "java", "c", "cc", "cpp", "h", "hpp",
];

fn looks_like_logs(text: &str) -> bool {
    let s: Vec<&str> = text.lines().take(20).collect();
    s.len() >= 5
        && s.iter()
            .filter(|l| {
                contains_any_ws(&l.to_ascii_uppercase(), "DEBUG INFO WARN ERROR FATAL TRACE")
            })
            .count()
            > s.len() / 3
}
pub fn detect_content_type(text: &str, path: Option<&Path>) -> ContentType {
    if let Some(ext) = path.and_then(|p| p.extension()).and_then(|v| v.to_str()) {
        match ext {
            ext if CODE_EXTENSIONS.contains(&ext) => return ContentType::Code,
            "json" => return ContentType::JsonConfig,
            "md" | "markdown" => return ContentType::Markdown,
            "diff" | "patch" => return ContentType::Diff,
            "log" => return ContentType::Logs,
            _ => {}
        }
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        ContentType::JsonConfig
    } else if trimmed.starts_with("diff --git") || trimmed.starts_with("@@") {
        ContentType::Diff
    } else if text.contains("Traceback") || text.contains("FAILED") || text.contains("error:") {
        ContentType::ShellOutput
    } else if looks_like_logs(text) {
        ContentType::Logs
    } else {
        ContentType::Unknown
    }
}

pub mod decision_view;
pub mod live_pareto;
pub mod model_artifacts;
pub mod output_novelty;
pub mod provider_cache;
pub mod reasoning_state;
pub use live_pareto::{
    EvidenceFreshness, LiveCandidate, LiveEntry, LiveParetoDecision, MetricOrder, ProtectedOutcome,
    VerifierIdentity, decide_live_pareto,
};
pub mod representation_economics;
pub mod token_classes;
mod tokens;

pub use tokens::{
    BYTES_ESTIMATOR_ID, LEXICAL_ESTIMATOR_ID, TokenizerFamily, TokenizerIdPreflightError,
    TokenizerMetadata, UNLABELED_ESTIMATE_TOKENIZER_PREFIX, VISIBLE_BUDGET_LOSSY_DECLARATION,
    active_model_id, active_tokenizer_metadata, count_tokens, count_tokens_for_model,
    count_tokens_tokenizer_id, enforce_token_budget, enforce_token_budget_with_ref,
    pack_to_token_boundary, pack_to_token_boundary_for_model,
    pack_to_token_boundary_for_model_with_char_limit, pack_to_token_boundary_with_char_limit,
    prefix_end_for_kept_lines, preflight_tokenizer_id, savings_ratio, savings_ratio_u64,
    sha256_hex, tokenizer_metadata,
};
