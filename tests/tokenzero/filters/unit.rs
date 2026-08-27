use tokenzero_filters::*;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Assert a command was NOT rewritten (applied=false, command unchanged).
fn assert_not_rewritten(result: &RewriteResult) {
    assert!(
        !result.applied,
        "expected no rewrite for '{}'",
        result.command
    );
    assert_eq!(
        result.rewritten_command, result.command,
        "rewrite must leave '{}' unchanged",
        result.command
    );
}

/// Assert a command WAS rewritten to the expected output (applied=true).
fn assert_rewritten_to(result: &RewriteResult, expected: &str) {
    assert!(result.applied, "expected rewrite for '{}'", result.command);
    assert_eq!(
        result.rewritten_command, expected,
        "'{}' rewrite mismatch",
        result.command
    );
}

// ── Discovery ────────────────────────────────────────────────────────────────

const EXPECTED_FAMILIES: &[&str] = &[
    "read", "search", "tree", "git", "test", "build", "docker", "kubectl", "package", "config",
];

#[test]
fn discovers_launch_critical_families() {
    let report = discover();

    // Report-level readiness. Classic MCP is not a workspace bin; env-parse
    // plus `tokenzero` on PATH is not dispatch.
    assert!(report.install_ready, "install_ready must be true");
    assert!(
        !report.mcp_ready,
        "mcp_ready must not be true without a tokenzero-mcp binary"
    );
    assert!(report.shell_ready, "shell_ready must be true");
    if cfg!(windows) {
        assert_eq!(
            report.os_warnings,
            vec!["verify PowerShell and cmd quoting with the OS matrix before launch"]
        );
    } else {
        assert!(
            report.os_warnings.is_empty(),
            "no OS warnings expected on this platform"
        );
    }

    // Every discovered filter must be structurally valid.
    assert_eq!(
        report.supported_filters.len(),
        EXPECTED_FAMILIES.len(),
        "filter count must match EXPECTED_FAMILIES"
    );
    for f in &report.supported_filters {
        assert!(
            !f.commands.is_empty(),
            "family '{}' must list at least one command",
            f.family
        );
    }
    assert!(
        report
            .supported_filters
            .iter()
            .filter(|f| f.family != "config")
            .all(|f| f.supported),
        "non-config families in FILTER_SPECS must have at least one rewrite"
    );
    let read = report
        .supported_filters
        .iter()
        .find(|f| f.family == "read")
        .expect("read family");
    assert!(
        read.exact_refs,
        "read/cat rewrite is the family that produces tokenzero refs"
    );

    // Every expected family must be present.
    let families: Vec<_> = report
        .supported_filters
        .iter()
        .map(|f| f.family.as_str())
        .collect();
    for &family in EXPECTED_FAMILIES {
        assert!(
            families.contains(&family),
            "family '{family}' must be present"
        );
    }
}

// ── Compound / shell-operator commands ───────────────────────────────────────

#[test]
fn compound_commands_are_left_unmodified() {
    // Benign pipes, sequences, logical operators, substitutions, and
    // arithmetic expansions remain conservatively unvouched compounds.
    for command in [
        "cat foo.txt | grep bar",
        "cargo test --workspace 2>&1 | tail -40",
        "ls -la; git status",
        "make build && make test",
        "grep -r needle . || true",
        "echo \"today is $(date)\"",
        "echo \"`uname -a`\"",
        "cat $((1+1)).txt",
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
        assert_eq!(r.reason, "compound command left unmodified");
        assert!(!r.safe, "compounds are never vouched: {command}");
    }

    // A mutation in any later command position must surface the mutation
    // classification rather than being hidden behind the generic compound reason.
    for command in [
        "git status\nrm -rf /tmp/x",
        "git status\rrm -rf /tmp/x",
        "cat foo $(rm -rf /tmp/x)",
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
        assert!(
            r.reason.contains("destructive mutation"),
            "{command}: {}",
            r.reason
        );
        assert!(
            !r.safe,
            "mutation-bearing compound must be unsafe: {command}"
        );
    }
}
#[test]
fn quoted_operators_do_not_count_as_compound() {
    let r = rewrite_command("cat 'a|b.txt'", "safe", true);
    assert_rewritten_to(&r, "tokenzero read 'a|b.txt'");
    assert!(r.safe);
    assert_eq!(r.family, "read");
}

#[test]
fn shell_parser_helpers_recover_an_empty_command_list() {
    let mut commands = Vec::new();
    let nested: Vec<char> = "rm x)".chars().collect();
    push_nested(&mut commands, &nested, 0, ')');
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].nested_commands, ["rm x"]);

    commands.clear();
    let mut word = "cat".to_owned();
    flush_shell_word(&mut commands, &mut word);
    assert!(word.is_empty());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].words, ["cat"]);
}

/// P04-001: cat rewrites must preserve shell-denoted argument spans
/// (expansions, globs, tilde, comments) instead of re-quoting them.
#[test]
fn cat_rewrite_preserves_shell_argument_semantics() {
    for (command, expected) in [
        ("cat $FILE", "tokenzero read $FILE"),
        ("cat *.rs", "tokenzero read *.rs"),
        ("cat ~/secret", "tokenzero read ~/secret"),
        (
            "cat visible # ignored words",
            "tokenzero read visible # ignored words",
        ),
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_rewritten_to(&r, expected);
        assert!(r.safe, "{command}");
        let again = rewrite_command(&r.rewritten_command, "safe", true);
        assert_eq!(
            again.rewritten_command, r.rewritten_command,
            "idempotent: {command}"
        );
    }
}

// ── Destructive / unsafe ─────────────────────────────────────────────────────

/// Covers all `unsafe_reason` categories: destructive first-words, git
/// mutations, sed/perl in-place edits, find with side effects, docker/kubectl
/// mutations, package mutations, network commands, dispatchers, and remote
/// execution.  The previous `destructive_commands_are_unmodified` test (which
/// tested only `git push` + `rm`) was a pure subset and has been deleted.
#[test]
fn expanded_destructive_commands_are_flagged() {
    for (command, expected_reason_fragment) in [
        // Destructive first-words
        ("rm -rf /tmp/x", "destructive"),
        ("shred -u secrets.txt", "destructive"),
        ("truncate -s 0 log.txt", "destructive"),
        ("mkfs.ext4 /dev/sda1", "destructive"),
        ("mount /dev/sda1 /mnt", "destructive"),
        ("rsync --delete src/ dst/", "destructive"),
        // In-place file edits
        ("sed -i s/a/b/ file.txt", "in-place file edit"),
        ("perl -pi -e s/a/b/ file.txt", "in-place file edit"),
        // find with side effects
        ("find . -name '*.tmp' -delete", "find with side effects"),
        (
            "find . -name '*.log' -exec rm {} +",
            "find with side effects",
        ),
        // Git mutations
        ("git push origin main", "git mutation"),
        ("git restore .", "git mutation"),
        ("git stash drop", "git mutation"),
        ("git tag v1.0.0", "git mutation"),
        (
            "git remote add origin https://example.com/repo.git",
            "git mutation",
        ),
        // Docker mutations
        ("docker run --rm image", "docker mutation"),
        ("docker compose up -d", "docker mutation"),
        ("docker cp file container:/tmp/file", "docker mutation"),
        ("docker import image.tar repo:tag", "docker mutation"),
        // kubectl mutations
        ("kubectl exec -it pod -- sh", "kubectl mutation"),
        ("kubectl cp file pod:/tmp/file", "kubectl mutation"),
        // Package mutations
        ("cargo add serde", "package"),
        ("npm uninstall left-pad", "package"),
        ("npm ci", "package"),
        ("uv pip install requests", "package"),
        // Dispatchers
        ("xargs rm -rf /tmp/foo", "dispatcher"),
        ("eval ls", "dispatcher"),
        ("sudo ls", "dispatcher"),
        ("npx some-package", "dispatcher"),
        // Remote execution
        ("ssh host uptime", "remote execution"),
        ("scp file host:/tmp/", "remote execution"),
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
        assert!(!r.safe, "{command}");
        assert!(
            r.reason.contains(expected_reason_fragment),
            "{command}: expected '{expected_reason_fragment}' in reason, got '{}'",
            r.reason
        );
    }
}

#[test]
fn unknown_families_are_not_vouched() {
    let r = rewrite_command("frobnicate --all", "safe", true);
    assert_not_rewritten(&r);
    assert_eq!(r.reason, "unsupported command family");
    assert!(!r.safe);
    assert_eq!(r.family, "unknown");
}

#[test]
fn disabled_mode_reports_honest_safety() {
    // Destructive: detected as unsafe before the disabled-mode path,
    // so probe.safe=false propagates through.
    let dangerous = rewrite_command("rm -rf /tmp/x", "off", false);
    assert_eq!(dangerous.reason, "disabled");
    assert!(
        !dangerous.safe,
        "unsafe command stays unsafe even in disabled mode"
    );

    // Benign read command: safe even when rewrites are disabled.
    let benign = rewrite_command("cat README.md", "off", false);
    assert_eq!(benign.reason, "disabled");
    assert!(benign.safe, "read command stays safe even in disabled mode");
    assert_eq!(benign.family, "read");

    // Git mutation: unsafe even in disabled mode.
    let git_mut = rewrite_command("git push origin main", "off", false);
    assert_eq!(git_mut.reason, "disabled");
    assert!(
        !git_mut.safe,
        "git mutation stays unsafe even in disabled mode"
    );
}

// ── Read-only / passthrough ──────────────────────────────────────────────────

#[test]
fn read_only_finds_and_passthroughs_stay_vouched() {
    // Passthrough commands: vouched AND unchanged.
    for (command, expected_family) in [
        ("head -n 5 foo.txt", "read"),
        ("find . -name '*.rs'", "tree"),
        ("git status", "git"),
        ("git diff", "git"),
        ("docker ps", "docker"),
        ("kubectl get pods", "kubectl"),
    ] {
        let r = rewrite_command(command, "safe", true);
        assert!(r.safe, "{command} must be vouched");
        assert_eq!(r.family, expected_family, "{command} family mismatch");
        assert_eq!(r.rewritten_command, command, "{command} must be unchanged");
    }

    // cat is rewritten to tokenzero read but still vouched.
    let r = rewrite_command("cat README.md", "safe", true);
    assert!(r.safe, "cat must be vouched");
    assert_eq!(r.family, "read");
    assert_rewritten_to(&r, "tokenzero read README.md");
}

#[test]
fn argument_payloads_are_never_classified_as_intent() {
    for command in [
        r#"br create --description "will write and remove things""#,
        r#"echo "rm -rf""#,
        r#"printf '%s' "drop table""#,
    ] {
        assert_eq!(
            analyze_shell(command).0,
            None,
            "payload changed policy: {command}"
        );
    }

    // A listed mutation remains a mutation regardless of harmless-looking or
    // dangerous-looking message payload text.
    for command in [
        r#"git commit -m "documentation only""#,
        r#"git commit -m "delete old write path""#,
        r#"git push -o ci.variable="message=read only""#,
    ] {
        assert!(
            analyze_shell(command)
                .0
                .is_some_and(|reason| reason.contains("git mutation")),
            "listed mutation escaped policy: {command}"
        );
    }
}

#[test]
fn mutation_is_detected_at_every_shell_command_position() {
    for command in [
        "echo ok && rm x",
        "echo ok || rm x",
        "echo ok; rm x",
        "echo ok | rm x",
        "echo ok
rm x",
        "$(rm x)",
        "echo $(rm x)",
        "echo \u{60}rm x\u{60}",
        r#"sh -c 'rm x'"#,
        r#"bash -c "echo ok && rm x""#,
        r#"/bin/sh -c 'git push origin main'"#,
    ] {
        assert!(
            analyze_shell(command).0.is_some(),
            "mutation escaped policy: {command}"
        );
    }
}

#[test]
fn quoted_command_text_is_data_but_substitutions_still_execute() {
    for command in [r#"echo "rm -rf""#, r#"printf '%s' 'git push'"#] {
        assert_eq!(
            analyze_shell(command).0,
            None,
            "quoted data changed policy: {command}"
        );
    }
    for command in [r#"echo "$(rm x)""#, "printf '%s' \u{60}git push\u{60}"] {
        assert!(
            analyze_shell(command).0.is_some(),
            "substitution escaped policy: {command}"
        );
    }
}

// ── split_words / has_shell_operators ─────────────────────────────────────────

#[cfg(not(windows))]
#[test]
fn backslash_escaped_quotes_split_correctly() {
    assert_eq!(
        split_words(r#"cat "a\"b.txt""#),
        vec!["cat".to_string(), "a\"b.txt".to_string()]
    );
    // An escaped quote must not flip quote state and hide a real pipe.
    assert!(analyze_shell(r#"echo \" | rm -rf /tmp/x"#).1);
    // An escaped operator is not an operator.
    assert!(!analyze_shell(r"cat foo\;bar.txt").1);
}

// ── Quiet flag injection ─────────────────────────────────────────────────────

#[test]
fn quiet_flags_injected_for_noisy_toolchains() {
    for (command, expected) in [
        ("cargo build --workspace", "cargo build --workspace -q"),
        ("cargo check -p demo", "cargo check -p demo -q"),
        (
            "cargo clippy --all-targets",
            "cargo clippy --all-targets -q",
        ),
        ("cargo test -p demo", "cargo test -p demo -q"),
        (
            "git clone https://example.com/demo.git",
            "git clone https://example.com/demo.git --quiet",
        ),
        ("git fetch origin", "git fetch origin --quiet"),
        ("git pull origin main", "git pull origin main --quiet"),
        ("npm test", "npm test --silent"),
        ("npm run build", "npm run build --silent"),
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_rewritten_to(&r, expected);
        assert!(r.safe, "{command}");
    }
}

#[test]
fn bounded_rewrites_respect_existing_limits() {
    for command in [
        "tree -L 0",
        "tree -L2 src",
        "tree --depth=4 src",
        "git log --max-count=5",
        "git log -n5",
        "git log -n 5",
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
        assert!(r.safe, "{command}");
    }
}

#[test]
fn quiet_injection_respects_explicit_verbosity_and_passthrough_separators() {
    for command in [
        "cargo build -q",
        "cargo test --workspace -- --nocapture",
        "cargo check --verbose",
        "git clone --progress https://example.com/demo.git",
        "git fetch -v origin",
        "npm test --silent",
        "npm run build --loglevel=warn",
        "pnpm test",
        "yarn test",
        "go test ./...",
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
    }
}

#[test]
fn quiet_injection_never_touches_mutations_or_compounds() {
    for command in [
        "git push origin main",
        "npm install left-pad",
        "cargo install ripgrep",
        "cargo build && cargo test",
        "git pull origin main || true",
    ] {
        let r = rewrite_command(command, "safe", true);
        assert_not_rewritten(&r);
    }
}

#[test]
fn quoted_search_argument_is_byte_identical_after_rewrite() {
    let command = r"grep -n 'a\|b' input.txt";
    let result = rewrite_command(command, "safe", true);
    assert_eq!(result.rewritten_command, command);
}
