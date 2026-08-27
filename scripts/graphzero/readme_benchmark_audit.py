#!/usr/bin/env python3
"""Fail when README benchmark claims drift from benchmarks/latency/results.json.

The README annotates benchmark claims with comments of the form:

    <!-- claim:benchmarks/latency/results.json#orient.orient_symbol_p50_ms -->

This audit resolves each JSON path and requires the README line containing the
claim to include the current artifact value. Numeric values may use the same
precision as the README table; ISO timestamps may use just the date portion.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

CLAIM_RE = re.compile(r"<!--\s*claim:benchmarks/latency/results\.json#([A-Za-z0-9_.-]+)\s*-->")


def resolve_path(data: Any, dotted: str) -> Any:
    cur = data
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            raise KeyError(dotted)
        cur = cur[part]
    return cur


def value_spellings(value: Any) -> set[str]:
    if isinstance(value, bool):
        return {str(value).lower(), str(value)}
    if isinstance(value, int):
        return {str(value)}
    if isinstance(value, float):
        spellings = {str(value), f"{value:.3f}", f"{value:.6f}"}
        spellings.add(f"{value:.3f}".rstrip("0").rstrip("."))
        spellings.add(f"{value:.6f}".rstrip("0").rstrip("."))
        return spellings
    if isinstance(value, str):
        spellings = {value}
        if re.match(r"^\d{4}-\d{2}-\d{2}T", value):
            spellings.add(value[:10])
        return spellings
    return {json.dumps(value, sort_keys=True, separators=(",", ":"))}


def audit(readme: Path, results_path: Path) -> int:
    data = json.loads(results_path.read_text(encoding="utf-8"))
    failures: list[str] = []
    claim_count = 0
    for line_no, line in enumerate(readme.read_text(encoding="utf-8").splitlines(), start=1):
        for match in CLAIM_RE.finditer(line):
            claim_count += 1
            dotted = match.group(1)
            try:
                value = resolve_path(data, dotted)
            except KeyError:
                failures.append(f"{readme}:{line_no}: missing benchmarks/latency/results.json path {dotted}")
                continue
            spellings = value_spellings(value)
            if not any(spelling and spelling in line for spelling in spellings):
                expected = ", ".join(sorted(spellings))
                failures.append(
                    f"{readme}:{line_no}: claim {dotted} not reflected on line; expected one of: {expected}"
                )
    if claim_count == 0:
        failures.append(f"{readme}: no benchmarks/latency/results.json claim markers found")
    for failure in failures:
        print(failure)
    if failures:
        print(f"FAIL: {len(failures)} README benchmark claim drift finding(s)")
        return 1
    print(f"OK: {claim_count} README benchmark claim(s) match {results_path}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--readme", type=Path, default=Path("README.md"))
    parser.add_argument("--results", type=Path, default=Path("benchmarks/latency/results.json"))
    args = parser.parse_args()
    return audit(args.readme, args.results)


if __name__ == "__main__":
    sys.exit(main())
