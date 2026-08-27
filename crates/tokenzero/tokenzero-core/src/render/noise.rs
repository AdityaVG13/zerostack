use crate::*;

pub(crate) fn safe_auto_success(input: &ShellRenderInput<'_>, status: &CommandStatus) -> bool {
    input.mode.effective_policy() == Mode::Auto
        && status.command_success
        && !input.timed_out
        && status.failed_segment.is_none()
        && status.pipeline_masking_warning.is_none()
}

pub(crate) fn should_compact_tiny_shell(
    input: &ShellRenderInput<'_>,
    policy: &PolicyDecision,
    status: &CommandStatus,
) -> bool {
    safe_auto_success(input, status)
        && input.exit_code == Some(0)
        && policy.policy == "passthrough"
        && input.stderr.trim().is_empty()
        && input.stdout.len() <= 512
        && input.stdout.lines().count() <= 8
        && count_tokens(input.stdout) <= 48
}

pub(crate) fn compact_shell_view(stdout: &str) -> String {
    let trimmed = stdout.trim_end();
    if trimmed.is_empty() {
        "ok".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn should_compact_repo_inventory_shell(
    input: &ShellRenderInput<'_>,
    policy: &PolicyDecision,
    status: &CommandStatus,
) -> bool {
    safe_auto_success(input, status)
        && input.exit_code == Some(0)
        && policy.policy == "structured"
        && is_repo_inventory_command(input.command)
        && input.stderr.trim().is_empty()
        && input.combined_ref.is_some()
        && count_tokens(input.stdout) <= 160
        && input.stdout.lines().count() <= 40
}

pub(crate) fn compact_repo_inventory_view(command: &str, output: &str) -> String {
    let stats = inventory_stats(output, 3, looks_like_inventory_file_path);
    let mut out = String::new();
    out.push_str("repo_inventory\n");
    out.push_str(&format!("files_seen: {}\n", stats.files));
    if stats.dirs > 0 {
        out.push_str(&format!("dirs_seen: {}\n", stats.dirs));
    }
    if stats.other > 0 {
        out.push_str(&format!("other_entries_seen: {}\n", stats.other));
    }
    if !stats.paths.is_empty() {
        out.push_str("sample_paths:\n");
        for file in stats.paths {
            out.push_str(&format!("- {}\n", compact_inventory_path(file)));
        }
    } else if !command.trim().is_empty() {
        out.push_str("sample_paths: none\n");
    }
    out
}

pub(crate) fn compact_repo_inventory_shell_capsule(
    input: &ShellRenderInput<'_>,
    body: &str,
) -> String {
    let mut visible = String::new();
    visible.push_str(body.trim_end());
    if let Some(combined_ref) = input.combined_ref {
        visible.push_str(&format!("\ncombined_ref: {combined_ref}"));
    }
    visible
}

pub(crate) fn looks_like_inventory_file_path(path: &str) -> bool {
    path.contains('/')
        || path.contains('\\')
        || path
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.contains('.'))
}

pub(crate) fn compact_inventory_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized = trimmed.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() <= 2 {
        normalized
    } else {
        parts[parts.len().saturating_sub(2)..].join("/")
    }
}

/// Token budget for the visible body of a verified-success command. Raw bytes
/// always remain recoverable through the exact refs, so success output spends
/// at most this many visible tokens (criticals are exempt and always kept).
pub(crate) const SHELL_SUCCESS_SUMMARY_TOKENS: usize = 200;

pub(crate) fn shell_success_summary_budget(max_visible_tokens: usize) -> usize {
    if max_visible_tokens == 0 {
        SHELL_SUCCESS_SUMMARY_TOKENS
    } else {
        SHELL_SUCCESS_SUMMARY_TOKENS.min(max_visible_tokens)
    }
}

/// Success-noise compaction preconditions: verified success, no timeout, no
/// masked pipeline hazard, and an exact combined ref to recover raw bytes.
pub(crate) fn should_compact_success_noise(
    input: &ShellRenderInput<'_>,
    status: &CommandStatus,
) -> bool {
    safe_auto_success(input, status) && input.combined_ref.is_some()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuccessFamily {
    Cargo,
    Pytest,
    NpmInstall,
    GitTransfer,
}

pub(crate) fn success_noise_families(command: &str) -> Vec<SuccessFamily> {
    let mut families = Vec::new();
    for segment in split_shell_segments(command) {
        let words = split_shell_words(&segment);
        let first = words
            .first()
            .map(|word| shell_command_basename(word))
            .unwrap_or_default();
        let family = match first.as_str() {
            "cargo" | "rustc" | "rustup" => Some(SuccessFamily::Cargo),
            "pytest" => Some(SuccessFamily::Pytest),
            "python" | "python3" => segment
                .contains("-m pytest")
                .then_some(SuccessFamily::Pytest),
            "npm" | "pnpm" | "yarn" => {
                let second = words.get(1).map(String::as_str).unwrap_or_default();
                matches!(
                    second,
                    "install" | "ci" | "i" | "add" | "update" | "upgrade" | "audit" | "dedupe"
                )
                .then_some(SuccessFamily::NpmInstall)
            }
            "git" => {
                let sub = git_subcommand_index(&words)
                    .and_then(|index| words.get(index))
                    .map(String::as_str)
                    .unwrap_or_default();
                matches!(
                    sub,
                    "clone" | "fetch" | "pull" | "push" | "gc" | "submodule"
                )
                .then_some(SuccessFamily::GitTransfer)
            }
            _ => None,
        };
        if let Some(family) = family
            && !families.contains(&family)
        {
            families.push(family);
        }
    }
    families
}

#[derive(Default)]
struct NoiseTally {
    compiled: usize,
    fresh: usize,
    downloaded: usize,
    bookkeeping: usize,
    tests_ok: usize,
    pytest_passed: usize,
    git_progress: usize,
    finished_in: Option<String>,
    summary_lines: Vec<String>,
    last_progress: std::collections::BTreeMap<String, String>,
}

impl NoiseTally {
    fn collapsed(&self) -> usize {
        self.compiled
            + self.fresh
            + self.downloaded
            + self.bookkeeping
            + self.tests_ok
            + self.pytest_passed
            + self.git_progress
    }
}

impl SuccessFamily {
    fn tool_label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Pytest => "pytest",
            Self::NpmInstall => "npm",
            Self::GitTransfer => "git",
        }
    }

    /// Pass markers win over `looks_critical_line` so names like
    /// `warning_handling_works` still collapse.
    fn absorb_pass(self, tally: &mut NoiseTally, trimmed: &str) -> bool {
        match self {
            Self::Cargo if is_cargo_test_ok_line(trimmed) => {
                tally.tests_ok += 1;
                true
            }
            Self::Pytest if is_pytest_pass_marker(trimmed) => {
                tally.pytest_passed += 1;
                true
            }
            _ => false,
        }
    }

    fn absorb(self, tally: &mut NoiseTally, trimmed: &str, line: &str) -> bool {
        match self {
            Self::Cargo => {
                if let Some(rest) = trimmed.strip_prefix("Finished ") {
                    tally.finished_in = rest.rsplit_once(" in ").map(|(_, t)| t.to_string());
                    true
                } else if starts_with_any(trimmed, "Compiling |Checking |Documenting ") {
                    tally.compiled += 1;
                    true
                } else if trimmed.starts_with("Fresh ") {
                    tally.fresh += 1;
                    true
                } else if starts_with_any(trimmed, "Downloaded |Downloading ") {
                    tally.downloaded += 1;
                    true
                } else if starts_with_any(
                    trimmed,
                    "Updating |Locking |Adding |Removing |Installing |Blocking |Building |Running |Doc-tests ",
                ) || trimmed.starts_with("running ") && trimmed.ends_with("tests")
                    || trimmed == "running 1 test"
                {
                    tally.bookkeeping += 1;
                    true
                } else if trimmed.starts_with("test result:") {
                    tally.summary_lines.push(line.to_string());
                    true
                } else {
                    false
                }
            }
            Self::Pytest => {
                if is_pytest_summary_line(trimmed) {
                    tally.summary_lines.push(
                        trimmed
                            .trim_matches(|c: char| c == '=' || c == ' ')
                            .to_string(),
                    );
                    true
                } else if is_pytest_noise_line(trimmed) {
                    if trimmed.ends_with("PASSED")
                        || trimmed.contains(" PASSED ")
                        || trimmed.contains("::")
                    {
                        tally.pytest_passed += 1;
                    } else {
                        tally.bookkeeping += 1;
                    }
                    true
                } else {
                    false
                }
            }
            Self::NpmInstall => {
                if is_npm_summary_line(trimmed) {
                    tally.summary_lines.push(trimmed.to_string());
                    true
                } else if is_npm_noise_line(trimmed) {
                    tally.bookkeeping += 1;
                    true
                } else {
                    false
                }
            }
            Self::GitTransfer => {
                let Some(prefix) = git_progress_prefix(trimmed) else {
                    return false;
                };
                tally.git_progress += 1;
                tally
                    .last_progress
                    .insert(prefix.to_string(), line.to_string());
                true
            }
        }
    }
}

/// Render a dense success view for known-noisy toolchains: progress and
/// bookkeeping lines collapse into counts while every critical line (and its
/// indented continuation block) is kept verbatim. Returns `None` when the
/// command is not a recognized family or nothing was recognized as noise.
pub(crate) fn success_noise_view(command: &str, stdout: &str, stderr: &str) -> Option<String> {
    let families = success_noise_families(command);
    if families.is_empty() {
        return None;
    }
    let mut tally = NoiseTally::default();
    let mut kept_lines: Vec<String> = Vec::new();
    let mut other_lines = 0usize;
    let mut kept_other = 0usize;
    let mut in_critical_block = false;

    for raw_line in stdout.lines().chain(stderr.lines()) {
        let line = raw_line.rsplit('\r').next().unwrap_or(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_critical_block = false;
            continue;
        }
        if families
            .iter()
            .copied()
            .any(|family| family.absorb_pass(&mut tally, trimmed))
        {
            in_critical_block = false;
            continue;
        }
        if looks_critical_line(line) {
            in_critical_block = true;
            kept_lines.push(line.to_string());
            continue;
        }
        if families
            .iter()
            .copied()
            .any(|family| family.absorb(&mut tally, trimmed, line))
        {
            in_critical_block = false;
            continue;
        }
        if in_critical_block && is_critical_continuation_line(line) {
            kept_lines.push(line.to_string());
            continue;
        }
        in_critical_block = false;
        other_lines += 1;
        if kept_other < 24 {
            kept_lines.push(line.to_string());
            kept_other += 1;
        }
    }

    if tally.collapsed() == 0 && tally.summary_lines.is_empty() && tally.finished_in.is_none() {
        return None;
    }

    let header_parts: Vec<_> = [
        (tally.compiled, "compiled"),
        (tally.fresh, "fresh"),
        (tally.downloaded, "downloaded"),
        (tally.tests_ok, "tests ok"),
        (tally.pytest_passed, "passed"),
        (tally.git_progress, "progress lines"),
        (tally.bookkeeping, "bookkeeping"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    let tool = families
        .first()
        .map(|family| family.tool_label())
        .unwrap_or("tool");
    let mut out = String::new();
    out.push_str(tool);
    out.push_str(" ok");
    if let Some(time) = tally.finished_in.as_deref() {
        out.push_str(" in ");
        out.push_str(time);
    }
    if !header_parts.is_empty() {
        out.push_str(": ");
        out.push_str(&header_parts.join(", "));
    }
    out.push_str(" [collapsed]");
    for line in tally
        .summary_lines
        .iter()
        .chain(tally.last_progress.values())
        .chain(&kept_lines)
    {
        out.push('\n');
        out.push_str(line);
    }
    if other_lines > kept_other {
        out.push_str(&format!(
            "\n... +{} more lines; exact ref available ...",
            other_lines.saturating_sub(kept_other)
        ));
    }
    Some(out)
}
