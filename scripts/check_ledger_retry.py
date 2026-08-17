#!/usr/bin/env python3
"""Fail if a negative-evidence ledger uses a forbidden retry phrase.

Checks the three durable ledgers:

  docs/progress/perf-negative-results.md
  docs/progress/conformance-negative-results.md
  docs/progress/surface-deferrals.md

Rules:
  * Every ``### YYYY-MM-DD`` entry must declare date, hypothesis, result,
    evidence, and a retry_condition_predicate.
  * ``result`` must be REJECTED, DEFERRED, or CLOSED.
  * The predicate must match one of the eight vocabulary forms.
  * Forbidden phrases (later / TBD / maybe / ...) are illegal in entry
    bodies. The vocabulary / forbidden-phrases section is skipped.

Run: python3 scripts/check_ledger_retry.py
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

LEDGERS = (
    Path("docs/progress/perf-negative-results.md"),
    Path("docs/progress/conformance-negative-results.md"),
    Path("docs/progress/surface-deferrals.md"),
)

ALLOWED_RESULTS = frozenset({"REJECTED", "DEFERRED", "CLOSED"})

ENTRY_RE = re.compile(r"^###\s+(\d{4}-\d{2}-\d{2})\s+")
FIELD_RE = re.compile(
    r"^-\s+\*\*(date|hypothesis|result|evidence|retry_condition_predicate):\*\*\s+(.*)$"
)
RESULT_HEADING_RE = re.compile(
    r"^###\s+\d{4}-\d{2}-\d{2}\s+--\s+\S+\s+--\s+(REJECTED|DEFERRED|CLOSED)\s*$"
)

# Form openings from RETRY-CONDITION-VOCABULARY.md (forms 1-8).
FORM_PREFIXES = (
    "Retry only if a profiler attributes",  # form 1
    "Reconsider only inside the broader",  # form 2
    "Worth reconsidering when",  # form 3
    "Not worth retrying as a standalone patch",  # form 4
    "Do not retry from a cold read",  # form 5
    "Retry condition not applicable",  # form 6
    "Retry only if this workload class exhibits measurable",  # form 7
    "Blocked until",  # form 8
)

# Word-boundary phrases. "later" is the load-bearing anti-vocabulary.
FORBIDDEN = (
    r"later",
    r"in the future",
    r"down the road",
    r"if it seems important",
    r"we should revisit",
    r"tracked elsewhere",
    r"TBD",
    r"TODO",
    r"FIXME",
    r"maybe",
    r"eventually",
    r"when we have time",
    r"if circumstances change",
    r"future work",
    r"might be worth trying",
    r"someone should look at this",
    r"interesting direction",
    r"worth exploring",
)
FORBIDDEN_RE = re.compile(
    r"\b(?:" + "|".join(FORBIDDEN) + r")\b",
    re.IGNORECASE,
)

SKIP_HEADING_RE = re.compile(
    r"^#{2,3}\s+(Retry-Condition Predicate Vocabulary|Forbidden phrases)\b",
    re.IGNORECASE,
)
NEXT_H2_RE = re.compile(r"^##\s+")


def strip_quotes(text: str) -> str:
    text = text.strip()
    if len(text) >= 2 and text[0] == text[-1] and text[0] in {'"', "'"}:
        return text[1:-1].strip()
    return text


def in_skip_section(heading_stack: str) -> bool:
    return heading_stack in {"vocab", "forbidden"}


def parse_heading_state(line: str, state: str) -> str:
    if SKIP_HEADING_RE.match(line):
        if re.search(r"Forbidden", line, re.IGNORECASE):
            return "forbidden"
        return "vocab"
    if state in {"vocab", "forbidden"} and NEXT_H2_RE.match(line):
        return "body"
    return state


def matches_form(predicate: str) -> bool:
    return any(predicate.startswith(prefix) for prefix in FORM_PREFIXES)


def check_file(path: Path) -> list[str]:
    errors: list[str] = []
    if not path.is_file():
        return [f"{path}: missing"]

    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    state = "body"
    entry_start: int | None = None
    fields: dict[str, str] = {}
    heading = ""

    def flush(end_line: int) -> None:
        nonlocal entry_start, fields, heading
        if entry_start is None:
            return
        loc = f"{path}:{entry_start}"
        if not RESULT_HEADING_RE.match(heading):
            errors.append(
                f"{loc}: heading must be "
                f"'### YYYY-MM-DD -- slug -- REJECTED|DEFERRED|CLOSED'"
            )
        missing = [
            key
            for key in (
                "date",
                "hypothesis",
                "result",
                "evidence",
                "retry_condition_predicate",
            )
            if key not in fields
        ]
        if missing:
            errors.append(f"{loc}: missing fields: {', '.join(missing)}")
        result = fields.get("result", "").strip().strip("`")
        if result and result not in ALLOWED_RESULTS:
            errors.append(
                f"{loc}: result must be REJECTED / DEFERRED / CLOSED, got {result!r}"
            )
        pred = strip_quotes(fields.get("retry_condition_predicate", ""))
        if pred:
            if FORBIDDEN_RE.search(pred):
                errors.append(f"{loc}: forbidden phrase in retry_condition_predicate")
            if not matches_form(pred):
                errors.append(
                    f"{loc}: retry_condition_predicate is not one of the 8 forms"
                )
        entry_start = None
        fields = {}
        heading = ""

    for idx, raw in enumerate(lines, start=1):
        line = raw.rstrip()
        state = parse_heading_state(line, state)
        if in_skip_section(state):
            continue
        if ENTRY_RE.match(line):
            flush(idx)
            entry_start = idx
            heading = line
            continue
        if entry_start is not None:
            if line.startswith("## ") and not line.startswith("### "):
                flush(idx)
                continue
            match = FIELD_RE.match(line)
            if match:
                fields[match.group(1)] = match.group(2)
            if FORBIDDEN_RE.search(line):
                errors.append(f"{path}:{idx}: forbidden phrase in entry body")
        elif FORBIDDEN_RE.search(line) and not line.startswith(">"):
            # Preamble / cass-mine / tables outside the vocab skip zone.
            errors.append(f"{path}:{idx}: forbidden phrase outside vocabulary section")

    flush(len(lines) + 1)

    if entry_start is None and not any(
        ENTRY_RE.match(line) for line in lines
    ):
        errors.append(f"{path}: no ### YYYY-MM-DD entries")

    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="Ledger files (default: the three docs/progress ledgers)",
    )
    args = parser.parse_args(argv)
    targets = args.paths or [REPO_ROOT / rel for rel in LEDGERS]
    errors: list[str] = []
    for path in targets:
        resolved = path if path.is_absolute() else REPO_ROOT / path
        errors.extend(check_file(resolved))
    if errors:
        for err in errors:
            print(f"ledger-retry check failed: {err}", file=sys.stderr)
        return 1
    print(f"ledger-retry ok: files={len(targets)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
