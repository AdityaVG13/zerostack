#!/usr/bin/env python3
"""Non-Pi reference adapter for the shared-suite runner.

It validates canonical catalog/artifact inputs and the registered-test gate. It
is deliberately independent of Rust, engine crates, and vendor harnesses.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

TESTS_ROOT = Path(__file__).resolve().parents[1]


def load_json(path: Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("all",), required=True)
    args = parser.parse_args(argv)

    catalog = load_json(TESTS_ROOT / "catalog.json")
    if not isinstance(catalog, dict) or catalog.get("version") != "zerostack.shared-suite.catalog.v1":
        raise SystemExit("invalid shared-suite catalog")
    cases = catalog.get("cases")
    if not isinstance(cases, list) or not cases:
        raise SystemExit("catalog has no cases")
    for case in cases:
        if not isinstance(case, dict) or not case.get("id") or not case.get("promise"):
            raise SystemExit("catalog case lacks id/promise")
        raw_origin = str(case.get("origin", ""))
        origin_name = raw_origin.split("::", 1)[0]
        if origin_name.startswith("tests/"):
            origin = TESTS_ROOT / "tests" / origin_name.removeprefix("tests/")
            if not origin.is_file():
                origin = TESTS_ROOT / origin_name.removeprefix("tests/")
        elif origin_name.startswith("crates/"):
            origin = TESTS_ROOT.parent / origin_name
        else:
            origin = TESTS_ROOT / origin_name
        if not origin.is_file():
            raise SystemExit(f"catalog origin missing: {case.get('origin')}")

    checks = [
        [sys.executable, str(TESTS_ROOT / "scripts" / "check_budget.py"), "--self-test"],
        [sys.executable, str(TESTS_ROOT / "scripts" / "check_surface_substrate.py")],
    ]
    for command in checks:
        result = subprocess.run(
            command,
            cwd=TESTS_ROOT,
            check=False,
            text=True,
            capture_output=True,
        )
        if result.returncode != 0:
            print(result.stdout, end="")
            print(result.stderr, end="", file=sys.stderr)
            return result.returncode
    print(json.dumps({"adapter": "reference", "suite": args.suite, "status": "pass", "cases": len(cases)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
