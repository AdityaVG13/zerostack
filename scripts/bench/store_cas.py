#!/usr/bin/env python3
"""Thin runner for the store CAS / atomic_write bench-shaped test.

Does not replace benchmarks/savings-bench-v1.json. Invokes the harness
test that already uses measure_with_teardown and emits JSON v3 fields.

    python3 scripts/bench/store_cas.py
    rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack" cargo test -p zerostack-harness store_cas -- --test-threads=1
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--via-rch",
        action="store_true",
        help="wrap the cargo test in rch exec with the pinned target dir",
    )
    args = parser.parse_args()
    cargo = [
        "cargo",
        "test",
        "-p",
        "zerostack-harness",
        "store_cas",
        "--",
        "--test-threads=1",
    ]
    if args.via_rch:
        base = os.environ.get("RCH_TARGET_BASE") or os.environ.get("TMPDIR") or "/tmp"
        target = str(Path(base) / "rch_target_zerostack")
        cmd = [
            "rch",
            "exec",
            "--",
            "env",
            f"CARGO_TARGET_DIR={target}",
            *cargo,
        ]
    else:
        cmd = cargo
    print(" ".join(cmd))
    raise SystemExit(
        subprocess.call(cmd, cwd=REPO_ROOT, env=os.environ.copy())
    )


if __name__ == "__main__":
    main()
