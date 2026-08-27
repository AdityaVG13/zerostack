#!/usr/bin/env python3
"""World-fork latency gate (fszero-ap9).

Measures per-fork cost of `zero.fs.world("fork")` on a 23k-file and a
100k-file corpus. Fork is copy-on-write by construction (no tree scan, no
file reads, no store writes at fork time), so the measured p50 must be
under 10 ms on BOTH sizes and must not grow with repo size.

Method: one CLI invocation runs a codemode plan with K forks; another runs
1 fork. Per-fork cost = (wall_K - wall_1) / (K - 1), which subtracts
process startup and store-open cost. This is an honest UPPER bound: it
includes per-op codemode step accounting, not just fork_world() itself
(which is a pure in-memory insert). p50/p95/p99 over --runs runs. The gate
FAILS (exit 1) if p50 per-fork >= 10 ms on any size.

Outputs benchmarks/world-fork.json and benchmarks/world-fork.md.

Usage: python3 benchmarks/world_fork.py [--sizes 23000,100000] [--forks 200] [--runs 20]
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MIN_MEASURED_RUNS = 20


def percentiles(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)

    def at(fraction: float) -> float:
        rank = (len(ordered) - 1) * fraction
        lower = int(rank)
        upper = min(lower + 1, len(ordered) - 1)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {"p50": at(0.50), "p95": at(0.95), "p99": at(0.99)}


def fszero_bin() -> str:
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def git_provenance() -> dict[str, object]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "-uno", "--", ".",
         ":(exclude)benchmarks/world-fork.json", ":(exclude)benchmarks/world-fork.md"],
        cwd=ROOT, text=True,
    )
    return {"git_commit": commit, "git_dirty": bool(status.strip())}


def hardware() -> str:
    try:
        cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        cores = subprocess.check_output(["sysctl", "-n", "hw.ncpu"], text=True).strip()
        return f"{cpu} / {cores} cores"
    except Exception:
        return "unknown"


def fork_plan(k: int) -> str:
    return (
        f"let n=0;for(let i=0;i<{k};i++){{const r=await zero.fs.world('fork');n++;}}"
        "return{forks:n};"
    )


def timed_run(corpus: Path, k: int) -> float:
    env = os.environ.copy()
    env["FSZERO_ROOT"] = str(corpus)
    t0 = time.monotonic()
    r = subprocess.run(
        [fszero_bin(), "codemode", fork_plan(k), "--root", str(corpus)],
        capture_output=True, text=True, timeout=600, cwd=corpus, env=env,
    )
    wall = time.monotonic() - t0
    ack = r.stdout.strip().splitlines()[0] if r.stdout.strip() else ""
    if r.returncode != 0 or ack.startswith("X0"):
        raise SystemExit(f"INTEGRITY: fork plan failed (ack={ack}, k={k})")
    return wall


def measure(corpus: Path, forks: int, runs: int) -> dict:
    per_fork_ms = []
    walls = []
    for _ in range(runs):
        wall_1 = timed_run(corpus, 1)
        wall_k = timed_run(corpus, forks)
        per_fork_ms.append((wall_k - wall_1) * 1000.0 / (forks - 1))
        walls.append({"wall_1_s": wall_1, "wall_k_s": wall_k})
    return {
        "forks_per_run": forks,
        "median_per_fork_ms": statistics.median(per_fork_ms),
        "per_fork_ms_percentiles": percentiles(per_fork_ms),
        "per_fork_ms": per_fork_ms,
        "walls": walls,
    }


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--sizes", default="23000,100000")
    ap.add_argument("--forks", type=int, default=200)
    ap.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    args = ap.parse_args()
    if args.runs < MIN_MEASURED_RUNS:
        ap.error(f"--runs must be at least {MIN_MEASURED_RUNS}")
    sizes = [int(s) for s in args.sizes.split(",")]

    result: dict = {
        "hardware": hardware(),
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        **git_provenance(),
        "runs_per_size": args.runs,
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_runs": 0,
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered raw run",
        },
        "seed": 42,
        "gate_ms": 10.0,
        "sizes": [],
    }
    for n in sizes:
        with tempfile.TemporaryDirectory(prefix=f"fszero_fork_{n}_") as tmp:
            corpus = Path(tmp) / "corpus"
            print(f"=== N={n}: generating corpus ...", flush=True)
            subprocess.run(
                ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
                 "--files", str(n), "--out", str(corpus), "--seed", "42"],
                check=True,
            )
            m = measure(corpus, args.forks, args.runs)
            m["files"] = n
            result["sizes"].append(m)
            print(f"  N={n}: median per-fork {m['median_per_fork_ms']:.3f} ms", flush=True)

    lines = [
        "# World-fork latency gate (fszero-ap9)",
        "",
        "Generated by `benchmarks/world_fork.py` — do not hand-edit numbers.",
        f"Hardware: {result['hardware']}. Commit: `{result['git_commit'][:12]}`"
        f" (dirty={str(result['git_dirty']).lower()}). Date: {result['date']}.",
        f"Method: per-fork = (wall of {args.forks}-fork plan - wall of 1-fork plan)"
        f" / {args.forks - 1}, p50 shown over {args.runs} runs; p95/p99 are in JSON; corpora are"
        " deterministic synthetic trees (`gen_corpus.py`, seed 42). Fork is"
        " copy-on-write: no tree scan, no file reads, no store writes.",
        "",
        "| files | p50 per-fork ms | gate (<10ms) |",
        "| --: | --: | :-- |",
    ]
    failed = False
    for m in result["sizes"]:
        ok = m["median_per_fork_ms"] < result["gate_ms"]
        failed = failed or not ok
        lines.append(
            f"| {m['files']} | {m['median_per_fork_ms']:.3f} | {'PASS' if ok else 'FAIL'} |"
        )
    if len(result["sizes"]) >= 2:
        lo, hi = result["sizes"][0], result["sizes"][-1]
        lines += [
            "",
            f"Size independence: {hi['files']}/{lo['files']} files ="
            f" {hi['files'] / lo['files']:.1f}x repo, per-fork ratio"
            f" {hi['median_per_fork_ms'] / max(lo['median_per_fork_ms'], 1e-9):.2f}x.",
        ]
    lines += [
        "",
        "Reproduce: `python3 benchmarks/world_fork.py` (requires the release-perf"
        " binary; `./scripts/profile_build.sh -p fs-zero --bin fszero`). Raw runs in `world-fork.json`.",
        "",
    ]
    (ROOT / "benchmarks" / "world-fork.json").write_text(json.dumps(result, indent=2) + "\n")
    (ROOT / "benchmarks" / "world-fork.md").write_text("\n".join(lines))
    print("written: benchmarks/world-fork.json, benchmarks/world-fork.md")
    if failed:
        raise SystemExit("GATE FAILED: per-fork p50 >= 10 ms")


if __name__ == "__main__":
    main()
