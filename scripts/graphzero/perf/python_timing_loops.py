#!/usr/bin/env python3
"""
SIMULATION DO NOT CITE — placeholder timing loops (not product latency).

This script intentionally does NOT measure GraphZero. Sleep stubs and mock sizes
remain only so the file can be run as a local harness sketch. Numbers written by
this file must never appear in README, rebaseline, gates, or claim language.

Real measurement entrypoints (cite these instead):
  - cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture
  - crates/graphzero-test-support/src/gates/snap_export_perf_gate.rs
  - scripts/benchmark_driver.py
  - benchmarks/rebaseline/

Runnable simulation only:
  python3 scripts/perf/python_timing_loops.py --simulate
Without --simulate the process exits non-zero and writes nothing.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

# Repo root via this file's location — never a host-absolute path.
REPO = Path(__file__).resolve().parents[3]

# Shared Hyndman-Fan type 7 percentiles (same as rebaseline/stats.py).
sys.path.insert(0, str(REPO / "benchmarks" / "rebaseline"))
from stats import median as _median, percentile as _percentile  # noqa: E402

SIMULATION_BANNER = "SIMULATION DO NOT CITE"
CITATION_STATUS = "SIMULATION_DO_NOT_CITE"


def time_it(fn, iters: int = 20):
    times = []
    for _ in range(iters):
        t0 = time.perf_counter()
        fn()
        times.append((time.perf_counter() - t0) * 1000)
    med = _median(times)
    p99 = _percentile(times, 0.99)
    return med, p99, min(times), max(times)


def mock_competitor_full_dump(n_syms: int = 50) -> str:
    data = {
        "nodes": [
            {"id": f"n{i}", "label": f"sym_{i}", "file": f"src/m_{i:04}.rs"}
            for i in range(n_syms)
        ],
        "edges": [{"source": f"n{i}", "target": f"n{i-1}"} for i in range(1, n_syms)],
    }
    return json.dumps(data, separators=(",", ":"))


def measure_warm_cold():
    print("=== SIMULATION warm vs cold (sleep stubs; not snap+export) ===")

    def cold_op():
        time.sleep(0.0005)

    def warm_op():
        time.sleep(0.0001)

    c_med, c_p99, _, _ = time_it(cold_op, 30)
    w_med, w_p99, _, _ = time_it(warm_op, 30)
    print(f"Cold med/p99: {c_med:.3f}/{c_p99:.3f} ms [simulation]")
    print(f"Warm med/p99: {w_med:.3f}/{w_p99:.3f} ms [simulation]")
    return {
        "cold_med_ms": c_med,
        "warm_med_ms": w_med,
        "cold_p99_ms": c_p99,
        "warm_p99_ms": w_p99,
        "method": "time.sleep stubs",
    }


def measure_size_ab_vs_mocks():
    print("=== SIMULATION size A/B (mock JSON only; not gz-snap export) ===")
    mock_50 = len(mock_competitor_full_dump(50))
    mock_200 = len(mock_competitor_full_dump(200))
    print(f"Mock full 50: {mock_50}B , 200: {mock_200}B")
    # No hardcoded "gz" sizes pretending to be product measurements.
    return {
        "mock50_bytes": mock_50,
        "mock200_bytes": mock_200,
        "gz_export_bytes": None,
        "note": "GZ sizes omitted; use snap_export_perf_gate for real export size",
    }


def measure_full_loop():
    print("=== SIMULATION full loop (sleep stub; not snap+export+blast) ===")

    def full_loop_op():
        time.sleep(0.0013)

    med, p99, mn, mx = time_it(full_loop_op, 15)
    print(f"Full loop med/p99: {med:.3f}/{p99:.3f} ms [simulation]")
    return {
        "full_loop_med_ms": med,
        "full_loop_p99_ms": p99,
        "min_ms": mn,
        "max_ms": mx,
        "method": "time.sleep stub",
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            f"{SIMULATION_BANNER}. Sleep/mock harness only. "
            "Not product latency. Pass --simulate to run."
        )
    )
    parser.add_argument(
        "--simulate",
        action="store_true",
        help="Acknowledge this is a non-product simulation and run stubs.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Optional path for JSON summary (still marked non-citable).",
    )
    args = parser.parse_args(argv)

    if not args.simulate:
        print(
            f"{SIMULATION_BANNER}\n"
            "Refusing to run without --simulate.\n"
            "This script measures time.sleep / mock JSON only and must not be cited.\n"
            "Product paths:\n"
            "  cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture\n"
            "  scripts/benchmark_driver.py\n"
            "  benchmarks/rebaseline/\n",
            file=sys.stderr,
        )
        return 2

    print(SIMULATION_BANNER)
    print("GraphZero SIMULATION harness (python timing loops) — not product latency")
    print(f"Repo root (from __file__): {REPO}")

    warm_cold = measure_warm_cold()
    sizes = measure_size_ab_vs_mocks()
    loop = measure_full_loop()

    # Never emit always-true product targets. Targets are omitted entirely;
    # simulation_meta documents why nothing here may be claimed.
    summary = {
        "citation_status": CITATION_STATUS,
        "product_latency": False,
        "banner": SIMULATION_BANNER,
        "simulation_meta": {
            "measures": "time.sleep stubs and mock JSON dumps only",
            "real_entrypoints": [
                "cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture",
                "crates/graphzero-test-support/src/gates/snap_export_perf_gate.rs",
                "scripts/benchmark_driver.py",
                "benchmarks/rebaseline/",
            ],
        },
        "warm_cold": warm_cold,
        "size_ab": sizes,
        "full_loop": loop,
        # Explicit: no "targets" map — always-pass gates removed (graphzero-9t7tv).
    }

    out_path = args.out
    if out_path is not None:
        out_path.write_text(json.dumps(summary, indent=2) + "\n")
        print("Simulation summary written to", out_path)
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
