#!/usr/bin/env python3
"""Fail-closed secret-keyword ledger checker for tracked GraphZero files.

Every current tracked keyword hit must be listed in the ledger as

    pattern-id<TAB>repository-relative path<TAB>line number<TAB>class<TAB>sha256

with a class of ``fixture``, ``doc``, ``code``, or ``config``. Matched content
is never recorded; the row key is (pattern id, path, 1-based line number) and
the final column is the sha256 digest of the exact matched line bytes as
emitted by ``git grep -n -z`` (the digest includes the line terminator).

The checker fails when:

- a live hit has no ledger row (new unclassified hit);
- a ledger row matches no live hit (deleted or stale entry);
- the live sha256 for an existing (pattern, path, line) key differs from the
  recorded digest (content changed at the same location);
- a ledger row is duplicated;
- a row uses an unknown pattern id, an invalid class, or a non-sha256 digest;
- any row is classified ``real-risk`` (real secrets must be removed, never
  suppressed).

Live hits come from ``git grep -n -z`` over tracked files (byte-oriented: each
match is ``path NUL line-number NUL matched-line``), so ``.git``, runtime
``.beads`` state, build outputs, and generated artifacts are excluded by
construction and again explicitly. The ledger, this checker, its test
fixtures, and the former allowlist are excluded explicitly so their own
synthetic text never counts as a hit. Filenames containing colons parse
correctly because only NUL separates fields. Paths containing tabs or line
breaks cannot be represented safely in TSV and fail closed. Malformed output,
unsafe paths, and git spawn failures raise a redacted :class:`LedgerError` that
never contains matched content or git stderr.

Exit codes: 0 = valid, 1 = ledger/hit validation failures, 2 = I/O or
argument error.
"""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path

#: Canonical focused secret-keyword patterns (same set as the former
#: security_scan.sh fallback). Keep in sync with the ledger column values.
PATTERNS: dict[str, str] = {
    "private-key": r"-----BEGIN( [A-Z0-9]+)? PRIVATE KEY-----",
    "aws-access-key": r"AKIA[A-Z0-9]{16}",
    "github-pat": r"ghp_[A-Za-z0-9]{36}",
    "slack-token": r"xox[bap]-[A-Za-z0-9-]{10,}",
    "api-key-literal": r"[Aa][Pp][Ii]_[Kk][Ee][Yy][[:space:]]*=[[:space:]]*[\"']?[A-Za-z0-9_./+=:-]{8,}",
}

#: Permitted ledger classes. ``real-risk`` is deliberately absent: it fails.
VALID_CLASSES = {"fixture", "doc", "code", "config"}
REAL_RISK = "real-risk"

#: Length of a lowercase hex sha256 digest.
DIGEST_LEN = 64
_HEX = frozenset("0123456789abcdef")

#: Ledger location relative to the repository root.
DEFAULT_LEDGER = Path("scripts/secret_keyword_ledger.tsv")

#: Explicit self-exclusion: these files intentionally carry synthetic keyword
#: text that must never count as a live hit.
SELF_EXCLUDES = (
    ":(exclude)scripts/check_secret_keyword_ledger.py",
    ":(exclude)scripts/test_check_secret_keyword_ledger.py",
    ":(exclude)scripts/secret_keyword_ledger.tsv",
    ":(exclude)scripts/security_scan_allowlist.txt",
)

#: Runtime/generated paths excluded explicitly. None are tracked today, but
#: the excludes keep the check correct if any are ever committed.
RUNTIME_EXCLUDES = (
    ":(exclude).git/**",
    ":(exclude).beads/**",
    ":(exclude).pi-subagents/**",
    ":(exclude)target/**",
    ":(exclude).rch-target*/**",
)


class LedgerError(Exception):
    """Operational failure (I/O, git spawn, or malformed scanner output)."""


def line_digest(content: bytes) -> str:
    """Lowercase sha256 hex of the exact matched line bytes."""
    return hashlib.sha256(content).hexdigest()


def parse_grep_z(data: bytes, pattern_id: str) -> list[tuple[str, str, int, str]]:
    """Parse ``git grep -n -z`` bytes into (pattern_id, path, line, digest).

    Each match is emitted as ``path NUL line-number NUL matched-line newline``
    with no separator between records (the newline is the record terminator,
    appended by git when the source line lacks one). The digest covers the
    exact emitted line bytes, including that newline. Malformed records and
    non-UTF8 paths raise a redacted :class:`LedgerError` that never includes
    the matched content.
    """
    if not data:
        return []
    hits: list[tuple[str, str, int, str]] = []
    i = 0
    n = len(data)
    try:
        while i < n:
            j = data.index(b"\0", i)
            path_b = data[i:j]
            i = j + 1
            j = data.index(b"\0", i)
            lineno_b = data[i:j]
            i = j + 1
            j = data.index(b"\n", i)
            line_b = data[i:j]
            i = j + 1
            try:
                path = path_b.decode("utf-8")
            except UnicodeDecodeError:
                raise LedgerError(
                    f"git grep output for {pattern_id} contains a non-UTF8 path"
                ) from None
            if any(separator in path for separator in ("\t", "\r", "\n")):
                raise LedgerError(
                    f"git grep output for {pattern_id} contains a path "
                    "that cannot be represented safely in TSV"
                )
            try:
                line_no = int(lineno_b)
            except ValueError:
                raise LedgerError(
                    f"malformed git grep output for {pattern_id}"
                ) from None
            # The emitted line includes its newline terminator.
            hits.append((pattern_id, path, line_no, line_digest(line_b + b"\n")))
    except ValueError:
        raise LedgerError(f"malformed git grep output for {pattern_id}") from None
    return hits


def git_grep_hits_for(
    repo_root: Path, pattern_id: str, regex: str
) -> list[tuple[str, str, int, str]]:
    """Return (pattern_id, relative path, line number, line sha256) live hits."""
    args = ["git", "grep", "-n", "-I", "-z", "-E", "-e", regex, "--", "."]
    args += list(SELF_EXCLUDES) + list(RUNTIME_EXCLUDES)
    try:
        proc = subprocess.run(args, cwd=repo_root, capture_output=True)
    except OSError:
        raise LedgerError(f"git grep failed for {pattern_id}: cannot spawn git") from None
    if proc.returncode not in (0, 1):
        raise LedgerError(f"git grep failed rc={proc.returncode} for {pattern_id}")
    return parse_grep_z(proc.stdout, pattern_id)


def collect_live_hits(repo_root: Path) -> list[tuple[str, str, int, str]]:
    """Collect every (pattern_id, path, line, digest) live hit."""
    hits: list[tuple[str, str, int, str]] = []
    for pattern_id, regex in PATTERNS.items():
        hits.extend(git_grep_hits_for(repo_root, pattern_id, regex))
    return hits


def load_ledger(
    ledger_path: Path,
) -> tuple[dict[tuple[str, str, int], tuple[str, str]], list[str]]:
    """Parse the 5-column TSV ledger into {row_key: (class, sha256)}.

    Structural violations are returned as error strings; a missing ledger file
    raises :class:`LedgerError`.
    """
    if not ledger_path.is_file():
        raise LedgerError(f"missing ledger: {ledger_path}")
    try:
        ledger_text = ledger_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        raise LedgerError(f"cannot read ledger: {ledger_path}") from None
    rows: dict[tuple[str, str, int], tuple[str, str]] = {}
    errors: list[str] = []
    for file_line, raw in enumerate(ledger_text.splitlines(), start=1):
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parts = stripped.split("\t")
        if len(parts) != 5:
            errors.append(
                f"{ledger_path}:{file_line}: expected 5 tab-separated fields, "
                f"got {len(parts)}"
            )
            continue
        pattern_id, rel_path, line_text, cls, digest = parts
        if pattern_id not in PATTERNS:
            errors.append(f"{ledger_path}:{file_line}: unknown pattern id {pattern_id!r}")
            continue
        try:
            line_no = int(line_text)
        except ValueError:
            errors.append(
                f"{ledger_path}:{file_line}: line number must be an integer, got {line_text!r}"
            )
            continue
        if cls not in VALID_CLASSES and cls != REAL_RISK:
            errors.append(
                f"{ledger_path}:{file_line}: invalid class {cls!r} "
                f"(expected one of {sorted(VALID_CLASSES)})"
            )
            continue
        if cls == REAL_RISK:
            errors.append(
                f"{ledger_path}:{file_line}: class {REAL_RISK} is forbidden; "
                "real secrets must be removed, never suppressed"
            )
            continue
        if (
            len(digest) != DIGEST_LEN
            or digest != digest.lower()
            or any(c not in _HEX for c in digest)
        ):
            errors.append(
                f"{ledger_path}:{file_line}: digest must be {DIGEST_LEN} lowercase "
                "hex sha256 characters"
            )
            continue
        key = (pattern_id, rel_path, line_no)
        if key in rows:
            errors.append(
                f"{ledger_path}:{file_line}: duplicate entry for {pattern_id} {rel_path}:{line_no}"
            )
            continue
        rows[key] = (cls, digest)
    return rows, errors


def run_checks(repo_root: Path, ledger_path: Path) -> list[str]:
    """Return every validation error; an empty list means the check passed.

    Operational failures (missing ledger, unable to spawn ``git``, nonzero
    ``git grep`` exit, malformed output) raise :class:`LedgerError`, which
    ``main`` maps to exit code 2; validation failures are returned as a list
    and map to exit 1.
    """
    rows, errors = load_ledger(ledger_path)
    live = collect_live_hits(repo_root)
    live_by_key: dict[tuple[str, str, int], str] = {}
    for pattern_id, rel_path, line_no, digest in live:
        live_by_key[(pattern_id, rel_path, line_no)] = digest
    live_keys = set(live_by_key)
    ledger_keys = set(rows)
    for pattern_id, rel_path, line_no in sorted(live_keys - ledger_keys):
        live_digest = live_by_key[(pattern_id, rel_path, line_no)]
        errors.append(
            f"unclassified hit: {pattern_id} {rel_path}:{line_no} "
            f"sha256={live_digest} "
            "(add a ledger row with class fixture/doc/code/config)"
        )
    for pattern_id, rel_path, line_no in sorted(ledger_keys - live_keys):
        errors.append(
            f"stale or deleted ledger entry: {pattern_id} {rel_path}:{line_no} "
            "no longer matches a tracked file"
        )
    for pattern_id, rel_path, line_no in sorted(live_keys & ledger_keys):
        _cls, ledger_digest = rows[(pattern_id, rel_path, line_no)]
        live_digest = live_by_key[(pattern_id, rel_path, line_no)]
        if live_digest != ledger_digest:
            errors.append(
                f"content changed at key: {pattern_id} {rel_path}:{line_no} "
                f"sha256={live_digest} "
                "(matched line bytes no longer match the recorded sha256; "
                "update the ledger row)"
            )
    return errors


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail-closed secret-keyword classification ledger checker."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=None,
        help="repository root (default: parent of this script)",
    )
    parser.add_argument(
        "--ledger",
        type=Path,
        default=None,
        help="ledger path (default: <root>/scripts/secret_keyword_ledger.tsv)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = (args.root or Path(__file__).resolve().parents[2]).resolve()
    ledger = (args.ledger or root / DEFAULT_LEDGER).resolve()
    try:
        errors = run_checks(root, ledger)
    except LedgerError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    if errors:
        print(f"secret-keyword ledger check failed with {len(errors)} error(s)", file=sys.stderr)
        return 1
    print(f"secret-keyword ledger check passed ({ledger})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
