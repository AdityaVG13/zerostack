#!/usr/bin/env python3
"""ZeroRef v1 interoperability conformance gate.

This script is the release/CI gate referenced by bead graphzero-zeroref-v1-shared-cas-1ghi.8.
It blocks broad interoperability claims until the retained three-binary matrix
evidence exists on macOS, Linux, and Windows.

Run from the repo root:
    python3 scripts/zeroref_conformance_gate.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE_FILE = ROOT / "docs" / "contracts" / "zeroref-conformance-evidence.json"
CAPABILITY_FIXTURES = ROOT / "docs" / "contracts" / "zeroref-capability-fixtures.json"
README = ROOT / "README.md"
ADR = ROOT / "docs" / "adr" / "002-zeroref-v1.md"

BROAD_PHRASES = [
    "any scheme resolves anywhere",
    "all schemes resolve",
    "resolves across all engines",
    "universal interoperability",
    "any ref works anywhere",
    "every scheme works everywhere",
]
REQUIRED_ENGINES = {"fszero", "graphzero", "tokenzero"}
REQUIRED_OS = {"macos", "linux", "windows"}
HEX = re.compile(r"^[0-9a-f]+$")


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def audit_docs() -> list[str]:
    findings: list[str] = []
    for doc in [README, ADR]:
        if not doc.exists():
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


def valid_hex(value: Any, length: int) -> bool:
    return isinstance(value, str) and len(value) == length and HEX.fullmatch(value) is not None


def audit_evidence() -> list[str]:
    findings: list[str] = []
    if not EVIDENCE_FILE.exists():
        findings.append(
            f"{EVIDENCE_FILE.name}: missing -- three-binary matrix evidence not yet retained"
        )
        return findings
    data = load_json(EVIDENCE_FILE)
    if data.get("schema") != "zeroref-conformance-evidence/v1":
        findings.append("evidence schema must be 'zeroref-conformance-evidence/v1'")
    for required in ["descriptor_tests", "docs_audit", "timestamp"]:
        if required not in data:
            findings.append(f"evidence file missing field {required}")

    matrix = data.get("matrix")
    if not isinstance(matrix, dict):
        findings.append("evidence matrix must be an object")
        return findings
    status = matrix.get("status")
    if status != "green":
        findings.append(
            f"matrix status is {status!r}; release requires green with macOS/Linux/Windows rows"
        )

    rows = matrix.get("rows")
    if not isinstance(rows, list):
        findings.append("matrix.rows must be an array")
        rows = []
    rows_by_os: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("os"), str):
            findings.append("every matrix row must be an object with an os string")
            continue
        rows_by_os.setdefault(row["os"], []).append(row)
    for os_name in sorted(REQUIRED_OS):
        os_rows = rows_by_os.get(os_name, [])
        if len(os_rows) != 1:
            findings.append(f"matrix requires exactly one {os_name} row, found {len(os_rows)}")
            continue
        cells = os_rows[0].get("cells")
        if not isinstance(cells, list) or not cells:
            findings.append(f"matrix {os_name} row has no cells")
            continue
        non_pass = [cell for cell in cells if not isinstance(cell, dict) or cell.get("status") != "pass"]
        if non_pass:
            findings.append(f"matrix {os_name} row has {len(non_pass)} non-pass cells")
        pairs = {
            (cell.get("writer"), cell.get("reader"))
            for cell in cells
            if isinstance(cell, dict)
        }
        required_pairs = {(writer, reader) for writer in REQUIRED_ENGINES for reader in REQUIRED_ENGINES}
        missing_pairs = sorted(required_pairs - pairs)
        if missing_pairs:
            findings.append(f"matrix {os_name} row misses writer/reader pairs: {missing_pairs}")
        bad_hashes = [
            cell
            for cell in cells
            if isinstance(cell, dict)
            and cell.get("status") == "pass"
            and (
                not valid_hex(cell.get("expected_hash"), 64)
                or cell.get("actual_hash") != cell.get("expected_hash")
            )
        ]
        if bad_hashes:
            findings.append(f"matrix {os_name} row has {len(bad_hashes)} pass cells without byte-exact hashes")

    binaries = matrix.get("sibling_shas")
    if not isinstance(binaries, list):
        findings.append("matrix.sibling_shas must be an array")
        binaries = []
    binary_keys = {
        (entry.get("os"), entry.get("engine"))
        for entry in binaries
        if isinstance(entry, dict)
        and valid_hex(entry.get("commit"), 40)
        and valid_hex(entry.get("sha256"), 64)
    }
    required_binary_keys = {(os_name, engine) for os_name in REQUIRED_OS for engine in REQUIRED_ENGINES}
    missing_binaries = sorted(required_binary_keys - binary_keys)
    if missing_binaries:
        findings.append(f"matrix misses pinned native binaries: {missing_binaries}")
    return findings


def main() -> int:
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
