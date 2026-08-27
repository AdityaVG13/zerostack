#!/usr/bin/env python3
"""Structure gate + optional refresh notes for packaging/man/*.1 (R-IDEA-013)."""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MAN = Path(__file__).resolve().parent

REQUIRED = {
    "fszero.1": [
        r"^\.TH FSZERO 1 ",
        r"^\.SH NAME$",
        r"fszero \\?- ",
        r"^\.SH SYNOPSIS$",
        r"^\.SH DESCRIPTION$",
        r"^\.SH COMMANDS$",
        r"^\.SH EXIT STATUS$",
        r"^\.SH ENVIRONMENT$",
        r"FSZERO_ROOT",
        r"^\.SH SEE ALSO$",
    ],
}

# Mirror of high-value verbs from SHIM_COMMANDS (keep fszero.1 COMMANDS in sync).
EXPECTED_VERBS = [
    "help",
    "doctor",
    "batch",
    "store-gc",
    "telemetry",
    "zeroref-fixture",
    "capabilities",
    "layout",
    "robot-triage",
    "robot-docs",
    "completions",
]


def check() -> int:
    failed = 0
    for name, patterns in REQUIRED.items():
        path = MAN / name
        if not path.is_file():
            print(f"missing {path}", file=sys.stderr)
            failed += 1
            continue
        text = path.read_text()
        for pat in patterns:
            if not re.search(pat, text, re.M):
                print(f"{name}: missing /{pat}/", file=sys.stderr)
                failed += 1
        if name == "fszero.1":
            for v in EXPECTED_VERBS:
                if v not in text:
                    print(f"fszero.1: missing verb {v}", file=sys.stderr)
                    failed += 1
    if failed:
        print(f"generate_fszero_1: FAIL ({failed})", file=sys.stderr)
        return 1
    print("generate_fszero_1: ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="validate man page structure")
    ap.add_argument("--write", action="store_true", help="reserved; pages are hand-authored")
    args = ap.parse_args()
    if args.write:
        print("pages are hand-authored from robot-docs; use --check", file=sys.stderr)
        return 2
    return check()


if __name__ == "__main__":
    raise SystemExit(main())
