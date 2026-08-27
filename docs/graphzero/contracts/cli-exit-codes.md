# GraphZero CLI exit-code contract

Centralized contract for every `graphzero` subcommand (source of truth for
docs, agent JSON, and harnesses).

## General commands

| Code | Meaning |
|------|---------|
| 0 | success |
| 1 | runtime/domain failure — machine-readable diagnostics on stderr (JSON when `--json`/`GRAPHZERO_JSON=1`/agent mode), human hint otherwise |
| 2 | usage / clap parse failure (missing required args, unknown verb) — stderr; JSON in agent mode |

Stream contract: success payloads on stdout; errors/diagnostics on stderr. Never exit 0 with an error body on stdout.

`graphzero capabilities` publishes this table under `exit_codes` and `stdout_stderr`.

## `graphzero zeroref-fixture` (conformance surface)

Per-class stable codes aligned with the ZeroRef v1 error registry
(docs/contracts/zeroref-fixture-cli.md):

| Code | Class |
|------|-------|
| 0 | success |
| 1 | other |
| 2 | malformed |
| 3 | unsupported |
| 4 | range_out_of_bounds |
| 5 | not_utf8 |
| 6 | missing |
| 7 | io |
| 8 | digest_mismatch |
| 9 | policy_denied |
| 10 | incompatible_version |
| 11 | legacy_ambiguity |

Changing any mapping is a breaking contract change: update this file, the
fixture CLI doc, and the self-tests in the same commit.
