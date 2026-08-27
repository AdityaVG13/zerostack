#!/usr/bin/env python3
"""ZeroRef v1 interoperability conformance gate.

This script is the release/CI gate referenced by bead fszero-c6q.7.
It blocks broad FSZero interoperability claims until the retained three-binary
matrix evidence exists on macOS, Linux, and Windows.

Run from the repo root:
    python3 scripts/zeroref_conformance_gate.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE_FILE = ROOT / "docs" / "contracts" / "zeroref-conformance-evidence.json"
CAPABILITY_FIXTURES = ROOT / "docs" / "contracts" / "zeroref-capability-fixtures.json"
README = ROOT / "README.md"
DOCS = [
    README,
    ROOT / "docs" / "architecture.md",
    ROOT / "docs" / "zerostack.md",
    ROOT / "docs" / "codemode.md",
    ROOT / "docs" / "mcp.md",
]

BROAD_PHRASES = [
    "any scheme resolves anywhere",
    "all schemes resolve",
    "resolves across all engines",
    "universal interoperability",
    "any ref works anywhere",
    "every scheme works everywhere",
]
REQUIRED_OSES = ("macos", "linux", "windows")
CELL_STATUSES = {"pass", "fail", "skip"}


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def audit_docs() -> list[str]:
    findings: list[str] = []
    for doc in DOCS:
        if not doc.exists():
            if doc == README:
                findings.append(f"{doc.name}: missing")
            continue
        text = doc.read_text(encoding="utf-8")
        for line_no, line in enumerate(text.splitlines(), start=1):
            lower = line.lower()
            for phrase in BROAD_PHRASES:
                if phrase in lower:
                    if "<!--" not in line and "evidence" not in lower:
                        findings.append(
                            f"{doc.name}:{line_no}: unmarked broad claim {phrase!r}"
                        )
    return findings


def audit_capability_fixtures() -> list[str]:
    findings: list[str] = []
    if not CAPABILITY_FIXTURES.exists():
        findings.append("capability fixtures missing")
        return findings
    data = load_json(CAPABILITY_FIXTURES)
    peers = data.get("peers", [])
    enabled = [p for p in peers if p.get("name") == "compatible_enabled"]
    disabled = [p for p in peers if p.get("name") == "compatible_but_peer_disabled"]
    if not enabled:
        findings.append("missing compatible_enabled capability fixture")
    if not disabled:
        findings.append("missing compatible_but_peer_disabled capability fixture")
    return findings


def summarize_platform(cells: list[dict[str, Any]]) -> dict[str, Any]:
    """Count executed/pass cells explicitly; skips are never execution."""
    statuses = [cell.get("status") for cell in cells]
    passed = statuses.count("pass")
    failed = statuses.count("fail")
    skipped = statuses.count("skip")
    executed = passed + failed
    if not cells:
        status = "missing"
    elif failed:
        status = "red"
    elif passed == len(cells):
        status = "green"
    elif skipped == len(cells):
        status = "not-run"
    else:
        status = "incomplete"
    return {
        "total": len(cells),
        "executed": executed,
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
        "status": status,
    }


def audit_matrix(matrix: Any) -> list[str]:
    findings: list[str] = []
    if not isinstance(matrix, dict):
        return ["matrix is missing or is not an object"]
    rows = matrix.get("rows")
    if not isinstance(rows, list):
        return ["matrix.rows is missing or is not an array"]

    rows_by_os: dict[str, list[dict[str, Any]]] = {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            findings.append(f"matrix.rows[{index}] is not an object")
            continue
        os_name = row.get("os")
        if os_name not in REQUIRED_OSES:
            findings.append(f"matrix.rows[{index}] has unknown os {os_name!r}")
            continue
        if os_name in rows_by_os:
            findings.append(f"matrix has duplicate {os_name} rows")
            continue
        cells = row.get("cells")
        if not isinstance(cells, list) or not cells:
            findings.append(f"matrix {os_name} row has no cells")
            rows_by_os[os_name] = []
            continue
        valid_cells: list[dict[str, Any]] = []
        for cell_index, cell in enumerate(cells):
            if not isinstance(cell, dict):
                findings.append(f"matrix {os_name} cell {cell_index} is not an object")
                continue
            status = cell.get("status")
            if status not in CELL_STATUSES:
                findings.append(
                    f"matrix {os_name} cell {cell_index} has invalid status {status!r}"
                )
            if status == "skip" and not str(cell.get("skip_reason", "")).strip():
                findings.append(
                    f"matrix {os_name} cell {cell_index} skips without a reason"
                )
            valid_cells.append(cell)
        rows_by_os[os_name] = valid_cells

    platform_status = {
        os_name: summarize_platform(rows_by_os.get(os_name, []))
        for os_name in REQUIRED_OSES
    }
    for os_name in REQUIRED_OSES:
        if os_name not in rows_by_os:
            findings.append(f"matrix missing required {os_name} row")

    verdicts = {
        os_name: summary["status"] for os_name, summary in platform_status.items()
    }
    if any(status == "red" for status in verdicts.values()):
        derived_status = "red"
    elif all(status == "green" for status in verdicts.values()):
        derived_status = "green"
    elif (
        verdicts["macos"] == "green"
        and verdicts["linux"] == "not-run"
        and verdicts["windows"] == "not-run"
    ):
        derived_status = "macos-only"
    else:
        derived_status = "incomplete"

    if matrix.get("platform_status") != platform_status:
        findings.append(
            "matrix platform_status does not match cells: "
            f"declared={matrix.get('platform_status')!r} derived={platform_status!r}"
        )
    if matrix.get("status") != derived_status:
        findings.append(
            f"matrix status {matrix.get('status')!r} does not match derived "
            f"status {derived_status!r}"
        )
    if derived_status != "green":
        findings.append(
            f"matrix status is {derived_status!r}; release requires non-skip "
            "green rows for macOS, Linux, and Windows"
        )
    return findings


def audit_evidence() -> list[str]:
    findings: list[str] = []
    if not EVIDENCE_FILE.exists():
        findings.append(
            f"{EVIDENCE_FILE.name}: missing — three-binary matrix evidence not yet retained"
        )
        return findings
    data = load_json(EVIDENCE_FILE)
    findings.extend(audit_matrix(data.get("matrix")))
    for required in ["descriptor_tests", "docs_audit", "timestamp"]:
        if required not in data:
            findings.append(f"evidence file missing field {required}")
    return findings


def synthetic_matrix(declared_status: str, statuses: dict[str, str]) -> dict[str, Any]:
    cells_by_os = {
        os_name: [
            {
                "writer": "fszero",
                "reader": "fszero",
                "status": status,
                **(
                    {"skip_reason": "synthetic platform unavailable"}
                    if status == "skip"
                    else {}
                ),
            }
        ]
        for os_name, status in statuses.items()
    }
    derived_platform = {
        os_name: summarize_platform(cells_by_os.get(os_name, []))
        for os_name in REQUIRED_OSES
    }
    return {
        "status": declared_status,
        "platform_status": derived_platform,
        "rows": [
            {"os": os_name, "cells": cells_by_os[os_name]}
            for os_name in REQUIRED_OSES
            if os_name in cells_by_os
        ],
    }


def self_test() -> int:
    green = synthetic_matrix("green", {os_name: "pass" for os_name in REQUIRED_OSES})
    assert audit_matrix(green) == []

    limited = synthetic_matrix(
        "macos-only", {"macos": "pass", "linux": "skip", "windows": "skip"}
    )
    limited_findings = audit_matrix(limited)
    assert len(limited_findings) == 1
    assert "release requires non-skip green rows" in limited_findings[0]

    dishonest = synthetic_matrix(
        "green", {"macos": "pass", "linux": "skip", "windows": "skip"}
    )
    dishonest_findings = audit_matrix(dishonest)
    assert any(
        "does not match derived status 'macos-only'" in f for f in dishonest_findings
    )
    assert any("release requires non-skip green rows" in f for f in dishonest_findings)

    missing = synthetic_matrix("green", {"macos": "pass"})
    missing_findings = audit_matrix(missing)
    assert any("missing required linux row" in f for f in missing_findings)
    assert any("missing required windows row" in f for f in missing_findings)

    print("ZeroRef v1 conformance gate self-test: OK (skip-as-green rejected)")
    return 0


def main() -> int:
    args = sys.argv[1:]
    if args == ["--self-test"]:
        return self_test()
    if args:
        print("usage: zeroref_conformance_gate.py [--self-test]", file=sys.stderr)
        return 2

    findings = []
    findings.extend(audit_docs())
    findings.extend(audit_capability_fixtures())
    findings.extend(audit_evidence())

    if findings:
        print("ZeroRef v1 conformance gate: BLOCKED", file=sys.stderr)
        for f in findings:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print("ZeroRef v1 conformance gate: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
