#!/usr/bin/env python3
"""Fail-closed checker for the SPEC-GZ verifier ledger.

Validates ``docs/spec-tags.md`` against the declared oracle harness
``crates/graphzero-query/tests/oracle_harness.rs``. The checker is static and
never executes any command named in the Markdown.

Schema (Markdown table, exactly five cells per row):

    | ID | Requirement | Source | Verifier | Status |

- ``ID``: ``SPEC-GZ-NNN``, stable and unique.
- ``Requirement``: nonempty prose.
- ``Source``: ``<path>::<symbol>``; the file must exist and the symbol text
  must appear in it.
- ``Verifier``: ``<path>::<test_fn>``; the path must be the declared oracle
  harness and ``test_fn`` must be a ``#[test]`` function in it. Verifier refs
  are unique: each harness test maps to exactly one ledger row.
- ``Status``: exactly ``VERIFIED``. A ``MISSING`` row or any unknown status
  fails because the ledger forbids prose-only and MISSING rows.

The checker also fails when the ledger is empty or when any ``#[test]``
function in the declared harness is absent from the ledger (orphan verifier).

Exit codes: 0 = valid, 1 = ledger/harness validation failures, 2 = I/O or
argument error.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

#: Declared oracle harness that every Verifier reference must point at.
ORACLE_HARNESS = Path("crates/graphzero-query/tests/oracle_harness.rs")
#: Ledger location relative to the repository root.
DEFAULT_LEDGER = Path("docs/spec-tags.md")
SPEC_ID = re.compile(r"^SPEC-GZ-\d{3}$")
VALID_STATUS = {"VERIFIED"}
DISALLOWED_STATUS = {"MISSING"}


class LedgerError(Exception):
    """Unreadable or structurally unusable ledger/harness input."""


def load_ledger_rows(path: Path) -> list[tuple[int, list[str]]]:
    """Parse the Markdown table into ``(line_no, cells)`` data rows.

    The header row and the ``|---|`` separator row are skipped. Every other
    line starting with ``|`` is a data row and must have exactly five cells.
    """
    if not path.is_file():
        raise LedgerError(f"missing ledger: {path}")
    lines = path.read_text(encoding="utf-8").splitlines()
    rows: list[tuple[int, list[str]]] = []
    ledger_active = False
    for idx, line in enumerate(lines, 1):
        stripped = line.strip()
        if not stripped.startswith("|"):
            if ledger_active:
                break  # the ledger table ended
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if not ledger_active:
            if len(cells) == 5 and cells[0] == "ID" and cells[-1] == "Status":
                ledger_active = True
            continue  # documentation/schema tables are not ledger rows
        if set("".join(cells)) <= {"-", ":"}:
            continue  # separator row inside the ledger table
        if len(cells) != 5:
            raise LedgerError(
                f"{path}:{idx}: malformed row: expected 5 columns, got {len(cells)}"
            )
        rows.append((idx, cells))
    if not ledger_active:
        raise LedgerError(
            f"{path}: no ledger table with header 'ID | Requirement | Source | Verifier | Status'"
        )
    return rows


def parse_ref(ref: str, what: str, line_no: int, path: Path) -> tuple[str, str]:
    """Split ``<path>::<symbol>``; the symbol may itself contain ``::``."""
    file_part, sep, symbol = ref.rpartition("::")
    if not sep or not file_part or not symbol:
        raise LedgerError(
            f"{path}:{line_no}: malformed {what} reference {ref!r}; "
            "expected '<file path>::<symbol>'"
        )
    return file_part, symbol


def resolve_ref(base: Path, ref: str) -> Path:
    """Resolve a ledger file reference against the ledger's directory."""
    candidate = Path(ref)
    return candidate if candidate.is_absolute() else base / candidate


def harness_tests(path: Path) -> set[str]:
    """Return the set of ``#[test]`` function names in a Rust file."""
    if not path.is_file():
        raise LedgerError(f"missing oracle harness: {path}")
    tests: set[str] = set()
    lines = path.read_text(encoding="utf-8").splitlines()
    for i, line in enumerate(lines):
        if line.strip() != "#[test]":
            continue
        j = i + 1
        while j < len(lines):
            candidate = lines[j].strip()
            if not candidate or candidate.startswith("//"):
                j += 1
                continue
            if candidate.startswith("fn "):
                name = candidate[3:].split("(", 1)[0].strip()
                if name:
                    tests.add(name)
            break
    return tests


def run(ledger: Path, harness: Path) -> tuple[int, list[str]]:
    """Validate the ledger; returns ``(exit_code, problems)``."""
    problems: list[str] = []
    try:
        rows = load_ledger_rows(ledger)
        if not rows:
            return 1, [f"{ledger}: ledger contains no data rows"]
        declared = harness_tests(harness)
        if not declared:
            return 1, [f"{harness}: no #[test] functions found"]
    except LedgerError as exc:
        return 2, [str(exc)]

    ids: set[str] = set()
    verifiers: set[str] = set()
    ledger_verifiers: set[str] = set()
    for line_no, cells in rows:
        spec_id, requirement, source, verifier, status = cells
        if not spec_id.startswith("SPEC-GZ-"):
            problems.append(
                f"{ledger}:{line_no}: id {spec_id!r} does not use the SPEC-GZ- prefix"
            )
        elif not SPEC_ID.fullmatch(spec_id):
            problems.append(f"{ledger}:{line_no}: malformed id {spec_id!r}")
        elif spec_id in ids:
            problems.append(f"{ledger}:{line_no}: duplicate id {spec_id!r}")
        else:
            ids.add(spec_id)

        if not requirement.strip():
            problems.append(f"{ledger}:{line_no}: empty requirement")

        try:
            source_file, source_symbol = parse_ref(source, "source", line_no, ledger)
            source_path = resolve_ref(ledger.parent, source_file)
            if not source_path.is_file():
                problems.append(
                    f"{ledger}:{line_no}: source file does not exist: {source_file}"
                )
            elif source_symbol not in source_path.read_text(encoding="utf-8"):
                problems.append(
                    f"{ledger}:{line_no}: source symbol {source_symbol!r} not found "
                    f"in {source_file}"
                )
        except LedgerError as exc:
            problems.append(str(exc))

        try:
            verifier_file, verifier_fn = parse_ref(verifier, "verifier", line_no, ledger)
            if resolve_ref(ledger.parent, verifier_file) != harness:
                problems.append(
                    f"{ledger}:{line_no}: verifier file {verifier_file!r} is not the "
                    f"declared oracle harness {harness}"
                )
            else:
                ledger_verifiers.add(verifier_fn)
                if verifier_fn in verifiers:
                    problems.append(
                        f"{ledger}:{line_no}: duplicate verifier {verifier_fn!r}"
                    )
                else:
                    verifiers.add(verifier_fn)
                if verifier_fn not in declared:
                    problems.append(
                        f"{ledger}:{line_no}: verifier test {verifier_fn!r} is not a "
                        f"#[test] function in {harness}"
                    )
        except LedgerError as exc:
            problems.append(str(exc))

        if status not in VALID_STATUS:
            if status in DISALLOWED_STATUS:
                problems.append(
                    f"{ledger}:{line_no}: status {status!r} is forbidden; the ledger "
                    "must not contain MISSING rows"
                )
            else:
                problems.append(
                    f"{ledger}:{line_no}: unknown status {status!r}; expected VERIFIED"
                )

    orphans = sorted(declared - ledger_verifiers)
    for orphan in orphans:
        problems.append(
            f"{harness}: #[test] function {orphan!r} is absent from the ledger "
            "(orphan verifier)"
        )

    return (1 if problems else 0), problems


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail-closed SPEC-GZ verifier ledger checker."
    )
    parser.add_argument(
        "--ledger",
        type=Path,
        default=DEFAULT_LEDGER,
        help=f"ledger to check (default: {DEFAULT_LEDGER})",
    )
    parser.add_argument(
        "--harness",
        type=Path,
        default=ORACLE_HARNESS,
        help=f"declared oracle harness (default: {ORACLE_HARNESS})",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    code, problems = run(args.ledger, args.harness)
    if problems:
        for problem in problems:
            print(f"SPEC-TAGS: {problem}", file=sys.stderr)
    if code == 0:
        print(f"SPEC-TAGS: {args.ledger} valid against {args.harness}")
    return code


if __name__ == "__main__":
    sys.exit(main())
