# Security scan contract

Runtime trust boundaries, implemented mitigations, and residual risks are documented in [threat_model.md](threat_model.md).

## Local command

Run `./scripts/security_scan.sh` from any directory in the checkout. The command detects and runs each locally installed scanner:

- `cargo-audit` checks the resolved Rust dependency graph against the RustSec advisory database.
- `cargo-deny check` applies dependency advisory, license, ban, and source policy from the repository configuration.
- `gitleaks detect --source <repo> --no-banner --redact` scans the checkout without printing candidate secret values.
- The built-in secret-keyword ledger check runs when both `python3` and `git` are present. It scans tracked regular text files for private-key block headers, AWS access-key IDs, GitHub personal access tokens, Slack tokens, and literal `api_key` assignments, then classifies every hit via `scripts/secret_keyword_ledger.tsv`.

The command prints `[RUN]`, `[PASS]`, `[FAIL]`, or `[SKIP]` for every scanner. A missing optional scanner is explicitly reported and does not by itself fail a local run because the fallback remains available. Exit status is `0` when every scanner that ran is clean, `1` when a scanner reports findings, and `2` when no scanner can run. A scanner's nonzero status is preserved as a failed scan in the summary; investigate its output to distinguish findings from an operational failure.

The command does not install tools, update dependencies, or remediate findings. Report findings separately and fix them through normal reviewed changes.

## CI policy and cadence

### What runs automatically (slim PR gates)

`.github/workflows/ci.yml` runs on every `pull_request` (and `workflow_dispatch`) with:

- privacy / beads export scrub (`scripts/check_no_host_paths.py`, `scripts/scrub_beads_export.py --check`);
- GitHub Actions SHA-pin policy (`scripts/check_action_pins.py`);
- `cargo deny check` (advisories, licenses, bans, sources) via a SHA-pinned `cargo-deny` install.

These are the merge-gate security honesty checks. They do **not** yet invoke the full `./scripts/security_scan.sh` suite (no `cargo-audit` / `gitleaks` / secret-keyword ledger on every PR).

### What remains manual

Heavy Rust CI (`.github/workflows/rust-ci.yml`), development-contract, and a full local `./scripts/security_scan.sh` run remain `workflow_dispatch` / operator-driven to conserve Actions budget. When running the full scanner locally or on a dispatched runner:

- install pinned versions of `cargo-audit`, `cargo-deny`, and `gitleaks` when available;
- invoke `./scripts/security_scan.sh`;
- treat any nonzero exit as a failed security check.

Recommended operator cadence for the full scan (not yet scheduled in Actions):

- before release cuts;
- after dependency pin bumps; and
- weekly when capacity allows, so newly published advisories are noticed even without source changes.

Scanner upgrades require normal dependency/tooling review. Findings are evidence to triage, not permission for the scan script to rewrite source or lockfiles.

## Secret-keyword ledger governance

`scripts/check_secret_keyword_ledger.py` enforces `scripts/secret_keyword_ledger.tsv`. Every tracked keyword hit must be classified as `fixture`, `doc`, `code`, or `config` in a tab-separated row of pattern identifier, repository-relative path, 1-based line number, class, and the sha256 digest of the exact matched line bytes. Matched content is never recorded. Line numbers and digests are taken from current `git grep -n -z` output, so an unrelated edit that shifts a listed line makes the row stale until it is updated, and editing the matched line at an existing key makes the digest mismatch until the row is updated.

The checker fails closed on: a live hit with no ledger row (new unclassified hit), a ledger row with no live hit (deleted or stale entry), a live digest mismatch at the same (pattern, path, line) key (content changed), a duplicate row, an unknown pattern id, an invalid class, a non-sha256 digest, and any row classified `real-risk` (real secrets must be removed, never suppressed).

The checker enforces the class vocabulary and fail-closed rules above; no separate prose-adjacency requirement is enforced. Keep the class honest: `code` covers production source that merely references keyword-shaped text, `fixture`/`doc`/`config` cover the other non-secret categories, and `real-risk` is always a failure so real secrets cannot be suppressed. Remove a row in the same change that removes or rewrites its fixture. Audit all rows during the weekly security run and at least quarterly as part of security maintenance.

The current audit found no tracked lines matching the focused patterns, so the ledger contains no rows. Keyword-only redaction logic is not a private-key block and is intentionally outside the pattern set rather than classified.

`scripts/security_scan_allowlist.txt` is retained only as a deprecated historical artifact and is no longer read.


## Cargo-deny waivers and follow-ups

The license allowlist in `deny.toml` is evidence-based and admits only permissive identifiers present (or recently present) in the resolved graph: 0BSD, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-1-Clause, BSD-2-Clause, BSD-3-Clause, CC0-1.0, ISC, MIT, Unicode-3.0, Unlicense, and Zlib. It deliberately does not admit LGPL-2.1-or-later, MPL-2.0, or `LicenseRef-MIT-OpenAI-Anthropic-Rider`. Unmatched allowances (for example BSD-2-Clause when no current crate uses it) are warnings, not failures.

The four informational unsoundness advisories are temporarily ignored by exact advisory ID in `deny.toml`, not globally downgraded:

- `RUSTSEC-2026-0190`, `anyhow 1.0.102`: undefined behavior requires adding error context and then calling `Error::downcast_mut`. Repository source contains no `downcast_mut` call. Upgrade when an advisory-clearing release is available.
- `RUSTSEC-2026-0008`, `git2 0.19.0`: undefined behavior requires dereferencing a newly empty/default `git2::Buf`. Repository source contains no `git2::Buf`, `Buf::new`, or `Buf::default` call. Upgrade `git2` as a dependency follow-up.
- `RUSTSEC-2026-0183`, `git2 0.19.0`: undefined behavior requires `Remote::list()` on a remote advertising no references. Repository source contains no `Remote::list` or `.list()` call. Upgrade to `git2 >= 0.21.0` in a dependency-review change.
- `RUSTSEC-2026-0184`, `git2 0.19.0`: undefined behavior requires a buffer-created blame hunk through `Blame::blame_buffer`. Repository source contains no `blame_buffer` call. Upgrade to `git2 >= 0.21.0` in the same dependency-review change.

These are path-specific, temporary waivers: remove each ignore as soon as its resolved crate version is outside the advisory range, and re-triage immediately if one of the named APIs is introduced. `git2` currently arrives through test-support/dev dependency paths; that reduces production exposure but does not remove the upgrade obligation.

The allowed git origins are the revision-pinned `zero-codemode` and `AdityaVG13/zerostack` repositories documented in `dependency_pins.md`; all other unknown git sources remain denied. Duplicate-version findings remain warnings and are reported, not suppressed or version-bumped by this policy change.

### Verification record (2026-07-29)

`cargo deny check` on main after the slim PR-gate work reported `advisories ok, bans ok, licenses ok, sources ok` (exit 0), with only unmatched-allowance warnings for BSD-2-Clause and for `fastmcp_rust` when that optional feature is not in the default resolve set. Historical 2026-07-11 triage notes about six nonpermissive license rejections are superseded by the current lockfile/policy; re-open follow-ups only if those packages reappear.
