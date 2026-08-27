#!/usr/bin/env python3
"""Measure per-CLI-call store-open cost for tiny and warm repository roots.

This is the dedicated regenerating runner for benchmarks/store-open.json.
The durable 100k/1m RecoveryStore reopen gate remains benchmarks/store_open.py.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
MIN_MEASURED_RUNS = 20
DEFAULT_OUTPUT = ROOT / "benchmarks" / "store-open.json"


def fszero_bin() -> str:
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def local_store(root: Path) -> tuple[str, int]:
    candidates = (
        (".zerostack/fszero/store.sqlite3", root / ".zerostack/fszero/store.sqlite3"),
        (".fszero/store.sqlite3", root / ".fszero/store.sqlite3"),
    )
    for label, path in candidates:
        if path.is_file():
            return label, path.stat().st_size
    return "none", 0


def child_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    # This scenario compares the two roots' local stores. Ambient shared-store
    # pins would silently turn it into a different workload.
    for key in (
        "ZEROSTACK_STORE_ROOT",
        "ZERO_STACK_STORE_ROOT",
        "FSZERO_SHARED_STORE",
        "ZEROSTACK_SHARED_STORE",
        "FSZERO_ALLOW_EPHEMERAL",
    ):
        env.pop(key, None)
    env["FSZERO_ROOT"] = str(root)
    return env


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    rank = (len(ordered) - 1) * fraction
    lower = int(rank)
    upper = min(lower + 1, len(ordered) - 1)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)


def time_calls(root: Path, runs: int) -> dict[str, Any]:
    env = child_env(root)
    command = [fszero_bin(), "codemode", "return{ok:true}", "--root", str(root)]
    prime = subprocess.run(command, capture_output=True, env=env, check=False)
    if prime.returncode != 0:
        raise SystemExit(f"INTEGRITY: prime call failed for {root.name!r}")

    samples_ms: list[float] = []
    for _ in range(runs):
        started = time.perf_counter_ns()
        run = subprocess.run(command, capture_output=True, env=env, check=False)
        samples_ms.append((time.perf_counter_ns() - started) / 1_000_000)
        if run.returncode != 0:
            raise SystemExit(f"INTEGRITY: measured call failed for {root.name!r}")
    return {
        "p50_ms": statistics.median(samples_ms),
        "p95_ms": percentile(samples_ms, 0.95),
        "p99_ms": percentile(samples_ms, 0.99),
        "min_ms": min(samples_ms),
        "max_ms": max(samples_ms),
        "runs": samples_ms,
    }


def git_value(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.runs < MIN_MEASURED_RUNS:
        parser.error(f"--runs must be at least {MIN_MEASURED_RUNS}")

    result: dict[str, Any] = {
        "schema_version": 1,
        "scenario": "cli_store_open_tiny_vs_warm_repo",
        "status": "observational_only",
        "success_budget": None,
        "git_commit": git_value("rev-parse", "HEAD"),
        "git_dirty": bool(git_value("status", "--porcelain", "--untracked-files=no")),
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "hardware": f"{platform.system()} {platform.machine()} {platform.processor() or 'unknown'}",
        "measurement": "fresh process wall time from perf_counter_ns; one unmeasured prime per root",
        "runs_per_root": args.runs,
        "warmup_runs_per_root": 1,
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_runs_per_root": 1,
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered raw run",
        },
    }
    with tempfile.TemporaryDirectory(prefix="fszero_tiny_") as tmp:
        tiny = Path(tmp) / "root"
        tiny.mkdir()
        (tiny / "a.txt").write_text("x\n")
        result["tiny_root"] = time_calls(tiny, args.runs)

    result["repo_root"] = time_calls(ROOT, args.runs)
    store_layout, store_bytes = local_store(ROOT)
    result["repo_store_layout"] = store_layout
    result["repo_store_bytes"] = store_bytes
    delta = result["repo_root"]["p50_ms"] - result["tiny_root"]["p50_ms"]
    result["repo_minus_tiny_p50_ms"] = delta
    result["repo_minus_tiny_p95_ms"] = (
        result["repo_root"]["p95_ms"] - result["tiny_root"]["p95_ms"]
    )
    result["repo_minus_tiny_p99_ms"] = (
        result["repo_root"]["p99_ms"] - result["tiny_root"]["p99_ms"]
    )
    # Compatibility field retained for the historical artifact and citations.
    result["store_size_dependent_ms_p50"] = delta

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(
        f"tiny p50 {result['tiny_root']['p50_ms']:.1f} ms | repo p50 "
        f"{result['repo_root']['p50_ms']:.1f} ms | repo-minus-tiny "
        f"{delta:.1f} ms ({store_bytes} byte local store)"
    )
    print(f"written: {args.output}")


if __name__ == "__main__":
    main()
