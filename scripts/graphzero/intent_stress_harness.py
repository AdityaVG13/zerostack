#!/usr/bin/env python3
"""Opt-in agent intent stress harness (graphzero-cf4r.1 / R-024).

Replays a JSONL corpus against graphzero + gzero binaries and classifies
outcomes as silent_fail | useless_error | useful_hint | inferred_and_acted.

Thresholds (fail closed when exceeded):
  silent_fail          > 0 on critical_path containing gzero/snap or gzero
  useless_error        > --useless-error-max (default: 0 for fixture corpus)
  missing binaries     exit 0 with skip notice unless --require-binaries

Skip: when a requested binary is absent, that entry is skipped (exit 0 overall
unless --require-binaries). Documented so CI can opt in without blocking
interactive use.

Usage:
  scripts/intent_stress_harness.py --help
  scripts/intent_stress_harness.py \\
      --binary target/debug/graphzero \\
      --gzero target/debug/gzero \\
      --corpus tests/agent_intent/intent_inference_corpus.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


CLASSIFICATIONS = (
    "silent_fail",
    "useless_error",
    "useful_hint",
    "inferred_and_acted",
)


def classify(entry: dict[str, Any], proc: subprocess.CompletedProcess[str]) -> str:
    expected = entry.get("expect", "useful_hint")
    combined = (proc.stdout or "") + (proc.stderr or "")
    must = entry.get("must_contain") or []
    want_exit = entry.get("exit_code")
    if want_exit is not None and proc.returncode != int(want_exit):
        # Wrong exit with no useful text → silent or useless
        if not combined.strip():
            return "silent_fail"
        if any(tok in combined for tok in must):
            return "useful_hint"
        return "useless_error"
    if not combined.strip() and proc.returncode != 0:
        return "silent_fail"
    if must and not all(tok in combined for tok in must):
        # Non-empty but missing required pedagogy tokens
        if proc.returncode == 0 and expected == "inferred_and_acted":
            return "useless_error"
        return "useless_error" if combined.strip() else "silent_fail"
    if expected in CLASSIFICATIONS:
        return expected
    return "useful_hint"


def run_entry(
    entry: dict[str, Any],
    binaries: dict[str, Path],
) -> tuple[str, str]:
    name = entry.get("binary", "gzero")
    path = binaries.get(name)
    if path is None or not path.is_file():
        return "skip", f"missing binary {name}"
    argv = [str(path)] + list(entry.get("argv") or [])
    try:
        proc = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            timeout=float(entry.get("timeout_sec", 15)),
            env={**os.environ, "NO_COLOR": "1"},
        )
    except subprocess.TimeoutExpired:
        return "silent_fail", "timeout"
    got = classify(entry, proc)
    detail = f"exit={proc.returncode} class={got}"
    return got, detail


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Intent stress harness for graphzero/gzero (R-024).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
thresholds:
  silent_fail > 0 on critical gzero/snap paths  → fail
  useless_error > --useless-error-max           → fail (default 0)

skip:
  Absent binaries skip those entries (exit 0) unless --require-binaries.

classifications:
  silent_fail | useless_error | useful_hint | inferred_and_acted
""".strip(),
    )
    parser.add_argument(
        "--binary",
        default="target/debug/graphzero",
        help="Path to graphzero binary (default: target/debug/graphzero)",
    )
    parser.add_argument(
        "--gzero",
        default="target/debug/gzero",
        help="Path to gzero binary (default: target/debug/gzero)",
    )
    parser.add_argument(
        "--corpus",
        default="tests/agent_intent/intent_inference_corpus.jsonl",
        help="JSONL corpus path",
    )
    parser.add_argument(
        "--useless-error-max",
        type=int,
        default=0,
        help="Max allowed useless_error count before non-zero exit (default: 0)",
    )
    parser.add_argument(
        "--require-binaries",
        action="store_true",
        help="Fail if graphzero or gzero binary is missing",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable summary JSON on stdout",
    )
    args = parser.parse_args()

    corpus_path = Path(args.corpus)
    if not corpus_path.is_file():
        print(f"error: corpus not found: {corpus_path}", file=sys.stderr)
        return 2

    binaries = {
        "graphzero": Path(args.binary),
        "gzero": Path(args.gzero),
    }
    if args.require_binaries:
        missing = [n for n, p in binaries.items() if not p.is_file()]
        if missing:
            print(f"error: missing binaries: {missing}", file=sys.stderr)
            return 2

    counts: Counter[str] = Counter()
    critical_silent = 0
    rows: list[dict[str, Any]] = []
    with corpus_path.open() as fh:
        for line_no, line in enumerate(fh, 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            entry = json.loads(line)
            got, detail = run_entry(entry, binaries)
            counts[got] += 1
            critical = str(entry.get("critical_path") or "")
            if got == "silent_fail" and (
                "gzero/snap" in critical or critical in ("gzero", "gzero/snap")
            ):
                critical_silent += 1
            rows.append(
                {
                    "id": entry.get("id", f"line-{line_no}"),
                    "class": got,
                    "detail": detail,
                    "critical_path": critical,
                }
            )

    summary = {
        "corpus": str(corpus_path),
        "counts": dict(counts),
        "critical_silent_fail": critical_silent,
        "useless_error_max": args.useless_error_max,
        "entries": rows,
    }
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(
            "intent_stress:",
            " ".join(f"{k}={counts.get(k, 0)}" for k in CLASSIFICATIONS),
            f"skip={counts.get('skip', 0)}",
            f"critical_silent={critical_silent}",
        )
        for row in rows:
            print(f"  {row['id']}: {row['class']} ({row['detail']})")

    if critical_silent > 0:
        print(
            f"FAIL: silent_fail={critical_silent} on gzero/snap critical paths",
            file=sys.stderr,
        )
        return 1
    if counts.get("useless_error", 0) > args.useless_error_max:
        print(
            f"FAIL: useless_error={counts['useless_error']} > max {args.useless_error_max}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
