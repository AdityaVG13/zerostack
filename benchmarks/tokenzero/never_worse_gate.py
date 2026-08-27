#!/usr/bin/env python3
"""Fail closed when benchmark output exceeds its same-task raw baseline."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

SCHEMA_VERSION = "never-worse/v1"
# Envelope-inclusive captured stdout is not a never-worse denominator for
# million-line edit/expand rows (tokenzero-4bhr / tokenzero-l1q6). Bakeoff
# still uses captured-stdout-bytes/v1; million-line-nav uses visible payload.
SURFACE_CAPTURED_STDOUT = "captured-stdout-bytes/v1"
SURFACE_VISIBLE_PAYLOAD = "visible-payload-bytes/v1"
ALLOWED_SURFACES = {SURFACE_CAPTURED_STDOUT, SURFACE_VISIBLE_PAYLOAD}
UNIT_ID = "estimator:bytes-ceil-div4/v1"
SUITE_SURFACES = {
    "million-line-nav": SURFACE_VISIBLE_PAYLOAD,
    "competitor-bakeoff": SURFACE_CAPTURED_STDOUT,
}
MILLION_LINE_TASKS = (
    "read_50_lines",
    "grep_expand",
    "tree_glob_read",
    "grep_expand_edit_verify",
    "recall",
)
_TASK_RE = re.compile(r"[A-Za-z0-9_.:-]+")


class ReceiptError(ValueError):
    pass


@dataclass(frozen=True)
class Row:
    task: str
    candidate_bytes: int
    raw_bytes: int
    candidate_units: int
    raw_units: int


def _nonnegative(raw: str, field: str, line: int) -> int:
    try:
        value = int(raw)
    except ValueError as error:
        raise ReceiptError(f"line {line}: {field} must be an integer") from error
    if value < 0:
        raise ReceiptError(f"line {line}: {field} must be nonnegative")
    return value


def parse_receipt(path: Path) -> tuple[str, str, list[Row]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ReceiptError(f"cannot read receipt {path}: {error}") from error
    if len(lines) < 6:
        raise ReceiptError("receipt is missing metadata, header, or task rows")
    expected_metadata = [
        ("schema_version", SCHEMA_VERSION),
        ("suite", None),
        ("surface_id", None),
        ("unit_id", None),
    ]
    metadata: dict[str, str] = {}
    for line_number, ((expected_key, expected_value), line) in enumerate(
        zip(expected_metadata, lines[:4], strict=True), start=1
    ):
        fields = line.split("\t")
        if len(fields) != 2 or fields[0] != expected_key or not fields[1]:
            raise ReceiptError(f"line {line_number}: expected {expected_key}<TAB>value")
        if expected_value is not None and fields[1] != expected_value:
            raise ReceiptError(
                f"line {line_number}: {expected_key} mismatch: {fields[1]!r} != {expected_value!r}"
            )
        metadata[fields[0]] = fields[1]
    surface = metadata["surface_id"]
    if surface not in ALLOWED_SURFACES:
        raise ReceiptError(
            f"surface_id {surface!r} is not a never-worse denominator "
            f"(allowed: {sorted(ALLOWED_SURFACES)})"
        )
    if "Q99" in metadata["unit_id"].upper() or "Q99" in metadata["suite"].upper():
        raise ReceiptError(
            "Q99-Input is not a TokenZero product unit; "
            f"receipts must use {UNIT_ID} (tokenzero-5wfr)"
        )
    if metadata["unit_id"] != UNIT_ID:
        raise ReceiptError(
            f"unit_id mismatch: {metadata['unit_id']!r} != {UNIT_ID!r}"
        )
    required_surface = SUITE_SURFACES.get(metadata["suite"])
    if required_surface is not None and surface != required_surface:
        raise ReceiptError(
            f"suite {metadata['suite']!r} must use surface {required_surface!r}, "
            f"got {surface!r}"
        )
    expected_header = "task\tcandidate_bytes\traw_bytes\tcandidate_units\traw_units"
    if lines[4] != expected_header:
        raise ReceiptError(f"line 5: expected header {expected_header!r}")

    rows: list[Row] = []
    seen: set[str] = set()
    for line_number, line in enumerate(lines[5:], start=6):
        fields = line.split("\t")
        if len(fields) != 5:
            raise ReceiptError(
                f"line {line_number}: expected exactly 5 tab-separated fields"
            )
        task = fields[0]
        if _TASK_RE.fullmatch(task) is None:
            raise ReceiptError(f"line {line_number}: invalid task id {task!r}")
        if task in seen:
            raise ReceiptError(f"line {line_number}: duplicate task {task!r}")
        seen.add(task)
        candidate_bytes = _nonnegative(fields[1], "candidate_bytes", line_number)
        raw_bytes = _nonnegative(fields[2], "raw_bytes", line_number)
        candidate_units = _nonnegative(fields[3], "candidate_units", line_number)
        raw_units = _nonnegative(fields[4], "raw_units", line_number)
        expected_candidate = (candidate_bytes + 3) // 4
        expected_raw = (raw_bytes + 3) // 4
        if candidate_units != expected_candidate or raw_units != expected_raw:
            raise ReceiptError(
                f"line {line_number}: {UNIT_ID} count mismatch for measured bytes"
            )
        if candidate_bytes == 0 and raw_bytes > 0:
            raise ReceiptError(
                f"line {line_number}: empty candidate with nonempty raw is not a never-worse measurement"
            )
        rows.append(Row(task, candidate_bytes, raw_bytes, candidate_units, raw_units))
    if not rows:
        raise ReceiptError("receipt has no task rows")
    if metadata["suite"] == "million-line-nav":
        required = set(MILLION_LINE_TASKS)
        if seen != required:
            raise ReceiptError(
                "million-line-nav receipt must contain exactly "
                f"{list(MILLION_LINE_TASKS)}, got {sorted(seen)}"
            )
    return metadata["suite"], metadata["surface_id"], rows


def render(suite: str, surface: str, rows: list[Row]) -> tuple[str, bool]:
    passed = all(row.candidate_units <= row.raw_units for row in rows)
    output = [
        "## Never-worse estimated-token budget assertion",
        "",
        f"Suite: `{suite}`. Surface: `{surface}`. Unit: `{UNIT_ID}`. This is a heuristic estimate, not Q99.",
        "",
        "| task | TokenZero bytes | raw-cli bytes | TokenZero est_tokens | raw-cli est_tokens | delta | result |",
        "|---|---:|---:|---:|---:|---:|---|",
    ]
    for row in rows:
        delta = row.raw_units - row.candidate_units
        result = "PASS" if delta >= 0 else "FAIL"
        output.append(
            f"| `{row.task}` | {row.candidate_bytes} | {row.raw_bytes} | "
            f"{row.candidate_units} | {row.raw_units} | {delta} | **{result}** |"
        )
    verdict = "PASS" if passed else "FAIL"
    output.extend(
        [
            "",
            f"> **Result: {verdict}** -- every TokenZero row must be <= its same-task raw-cli baseline in `{UNIT_ID}` units.",
        ]
    )
    return "\n".join(output), passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("receipt", type=Path)
    args = parser.parse_args()
    try:
        suite, surface, rows = parse_receipt(args.receipt)
    except ReceiptError as error:
        print(f"never-worse gate: invalid receipt: {error}", file=sys.stderr)
        return 2
    rendered, passed = render(suite, surface, rows)
    print(rendered)
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
