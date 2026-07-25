#!/usr/bin/env python3
"""Fail when the committed conformance attestations have gone stale.

The engines live in repositories this one does not control, so a report rots
without anything here changing. See reports/ATTESTATION.md for the contract.

Exit 0 when every namespace has a fresh, passing, revision-pinned attestation.
Exit 1 with one line per violation otherwise.
"""

from __future__ import annotations

import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

REQUIRED_NS = ("fz", "tz", "gz")
MAX_REPORT_AGE_DAYS = 30
FULL_HEX_40 = re.compile(r"^[0-9a-f]{40}$")

REQUIRED_FIELDS = (
    "ns",
    "report",
    "engine_repo",
    "engine_rev",
    "engine_rev_committed_at",
    "generated_at",
    "contract_version",
    "passed",
)

REPORTS_DIR = Path(__file__).resolve().parent.parent / "reports"
INDEX = REPORTS_DIR / "attestation.json"


def parse_rfc3339(value: str) -> datetime | None:
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, AttributeError):
        return None
    # A naive timestamp is ambiguous, and comparing it against an aware "now"
    # raises. Treat it as UTC rather than crashing the gate.
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=timezone.utc)
    return parsed


def main() -> int:
    problems: list[str] = []

    if not INDEX.is_file():
        print(f"FAIL: no attestation index at {INDEX}", file=sys.stderr)
        print(
            "  The conformance suite has never been tied to an engine revision.",
            file=sys.stderr,
        )
        return 1

    try:
        entries = json.loads(INDEX.read_text())
    except json.JSONDecodeError as exc:
        print(f"FAIL: {INDEX.name} is not valid JSON: {exc}", file=sys.stderr)
        return 1

    if not isinstance(entries, list):
        print(f"FAIL: {INDEX.name} must be a JSON array", file=sys.stderr)
        return 1

    now = datetime.now(timezone.utc)
    newest: dict[str, tuple[datetime, dict]] = {}

    for index, entry in enumerate(entries):
        label = f"{INDEX.name}[{index}]"
        if not isinstance(entry, dict):
            problems.append(f"{label}: entry is not an object")
            continue

        missing = [f for f in REQUIRED_FIELDS if f not in entry]
        if missing:
            problems.append(f"{label}: missing required field(s): {', '.join(missing)}")
            continue

        ns = entry["ns"]
        if ns not in REQUIRED_NS:
            problems.append(f"{label}: unknown ns {ns!r}")
            continue

        rev = entry["engine_rev"]
        if not isinstance(rev, str) or not FULL_HEX_40.match(rev):
            problems.append(
                f"{label}: engine_rev must be 40 lowercase hex, got {rev!r}; "
                "a report that names no revision attests to nothing"
            )
            continue

        report_path = REPORTS_DIR / entry["report"]
        if not report_path.is_file():
            problems.append(f"{label}: report file {entry['report']} does not exist")
            continue

        # An absolute path from the authoring machine is a leak, not evidence.
        try:
            report = json.loads(report_path.read_text())
        except json.JSONDecodeError as exc:
            problems.append(f"{label}: {entry['report']} is not valid JSON: {exc}")
            continue
        binary = report.get("substrate_binary") or report.get("bin") or ""
        if binary.startswith("/") or binary.startswith("~"):
            problems.append(
                f"{label}: {entry['report']} records an absolute substrate path "
                f"({binary!r}); record the basename only"
            )

        generated = parse_rfc3339(entry["generated_at"])
        if generated is None:
            problems.append(
                f"{label}: generated_at {entry['generated_at']!r} is not RFC 3339"
            )
            continue

        previous = newest.get(ns)
        if previous is None or generated > previous[0]:
            newest[ns] = (generated, entry)

    for ns in REQUIRED_NS:
        current = newest.get(ns)
        if current is None:
            problems.append(
                f"ns {ns}: no usable attestation; the hub has no independent "
                "evidence that this engine conforms"
            )
            continue

        generated, entry = current
        age_days = (now - generated).days
        if age_days > MAX_REPORT_AGE_DAYS:
            problems.append(
                f"ns {ns}: newest attestation is {age_days}d old "
                f"(limit {MAX_REPORT_AGE_DAYS}d), engine_rev {entry['engine_rev'][:12]}; "
                "re-run the suite against current engine HEAD"
            )
        if not entry["passed"]:
            problems.append(
                f"ns {ns}: newest attestation records passed=false for "
                f"engine_rev {entry['engine_rev'][:12]}; fix the engine or "
                "reopen the bead, do not leave a failing attestation committed"
            )

    if problems:
        print("conformance freshness: FAIL", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "\nSee conformance/reports/ATTESTATION.md for the contract.",
            file=sys.stderr,
        )
        return 1

    for ns in REQUIRED_NS:
        generated, entry = newest[ns]
        print(
            f"conformance freshness: {ns} ok "
            f"(rev {entry['engine_rev'][:12]}, {(now - generated).days}d old)"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
