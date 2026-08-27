#!/usr/bin/env python3
"""Drive the search-scale gold bigram bakeoff (graphzero-aluu).

Focused cargo test only — never a full workspace suite.
Does not flip GRAPHZERO_SEARCH_BIGRAM default-on.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "benchmarks" / "search_bigram_bakeoff" / "report.json"
GEN = ROOT / "benchmarks" / "gold" / "search" / "gen_corpus.py"
LOCK = Path("/tmp/zerostack-swarm-locks/graphzero.lock")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--skip-gen-check", action="store_true")
    ap.add_argument("--rounds", type=int, default=30)
    args = ap.parse_args()

    LOCK.parent.mkdir(parents=True, exist_ok=True)
    if not args.skip_gen_check:
        subprocess.check_call([sys.executable, str(GEN), "--check"], cwd=ROOT)

    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = env.get("CARGO_BUILD_JOBS", "2")
    env["GRAPHZERO_SEARCH_GOLD_ROUNDS"] = str(args.rounds)
    # Ensure default-off for the test process start.
    env.pop("GRAPHZERO_SEARCH_BIGRAM", None)

    cargo = [
        "cargo",
        "test",
        "-p",
        "graphzero-query",
        "--test",
        "search_bigram_gold_bakeoff",
        "--release",
        "--",
        "--nocapture",
        "--test-threads=1",
    ]
    # Do NOT nest flock here: callers already hold graphzero.lock (outer flock).
    # Nested flock on the same lock deadlocks (non-reentrant).
    print("+", " ".join(cargo), flush=True)
    rc = subprocess.call(cargo, cwd=ROOT, env=env)
    if rc != 0:
        return rc
    if not REPORT.is_file():
        print(f"missing report: {REPORT}", file=sys.stderr)
        return 2
    report = json.loads(REPORT.read_text())
    assert report.get("default_on") is False
    assert report.get("bead") in {
        "graphzero-aluu",
        "graphzero-md9k",
        "graphzero-2tee",
    }
    assert any(
        "search_bigram_spike_result.json" in str(x)
        for x in report.get("not_gold_inputs", [])
    )
    print(
        json.dumps(
            {
                "ok": True,
                "success_rate": report.get("success_rate"),
                "p95_improve_pct": report.get("warm", {}).get("p95_improve_pct"),
                "mem_ratio": report.get("memory", {}).get("ratio"),
                "symbols": report.get("corpus", {}).get("symbol_count"),
                "default_on": report.get("default_on"),
                "report": str(REPORT.relative_to(ROOT)),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
