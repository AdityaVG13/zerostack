#!/usr/bin/env python3
"""variance_envelope.py — same-host p95 drift check (docs/BUDGETS.md).

Reads hyperfine JSON (`results[0].times` in seconds), summary objects with
`p95_ms`, or trial-vector JSON (`times_ms` / `trials_ms` lists). Compares runs
to each other, or a candidate to an explicit prior baseline.

Usage:
  variance_envelope.py run1.json run2.json [...]
  variance_envelope.py --baseline prior.json candidate.json
  variance_envelope.py --help

Percentiles: nearest-rank ceil(p*N)-1, clamped.

Verdict (max |p95 - median| / median among inputs, or |cand-base|/base):
  ≤  5%  → STABLE (exit 0)
  ≤ 10%  → NOISE / within envelope (exit 0)
  ≤ 20%  → INVESTIGATE (exit 1)
  >  20%  → ESCALATE (exit 1)

Missing extractable p95 → exit 2.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path


def nearest_rank(values: list[float], p: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    idx = max(0, min(math.ceil(p * len(ordered)) - 1, len(ordered) - 1))
    return ordered[idx]


def median_value(values: list[float]) -> float:
    ordered = sorted(values)
    mid = len(ordered) // 2
    if len(ordered) % 2 == 1:
        return ordered[mid]
    return (ordered[mid - 1] + ordered[mid]) / 2.0


def percentiles_ms_from_seconds(times_s: list[float]) -> dict[str, float | None]:
    ms = [float(t) * 1000.0 for t in times_s]
    return {
        "p50_ms": nearest_rank(ms, 0.50),
        "p95_ms": nearest_rank(ms, 0.95),
        "p99_ms": nearest_rank(ms, 0.99),
    }


def percentiles_ms_from_ms(times_ms: list[float]) -> dict[str, float | None]:
    ms = [float(t) for t in times_ms]
    return {
        "p50_ms": nearest_rank(ms, 0.50),
        "p95_ms": nearest_rank(ms, 0.95),
        "p99_ms": nearest_rank(ms, 0.99),
    }


def extract_stats(obj: dict) -> dict[str, float | None] | None:
    """Return p50/p95/p99 ms when extractable."""
    if "p95_ms" in obj:
        return {
            "p50_ms": float(obj["p50_ms"]) if obj.get("p50_ms") is not None else None,
            "p95_ms": float(obj["p95_ms"]),
            "p99_ms": float(obj["p99_ms"]) if obj.get("p99_ms") is not None else None,
        }
    for key in ("times_ms", "trials_ms", "trial_vector_ms"):
        if key in obj and isinstance(obj[key], list) and obj[key]:
            return percentiles_ms_from_ms([float(x) for x in obj[key]])
    results = obj.get("results") or []
    if results:
        times = results[0].get("times") or []
        if times:
            return percentiles_ms_from_seconds([float(t) for t in times])
    # bare list of numbers at top level is not expected; reject
    return None


def load_stats(path: Path) -> dict[str, float | None]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"{path}: expected JSON object")
    stats = extract_stats(data)
    if stats is None or stats.get("p95_ms") is None:
        raise ValueError(f"could not extract p95 from {path}")
    return stats


def drift_fraction(values: list[float], *, baseline: float | None) -> tuple[float, float]:
    """Return (reference, max_relative_drift). reference is baseline or median."""
    if baseline is not None:
        if baseline == 0:
            if all(v == 0 for v in values):
                return 0.0, 0.0
            raise ValueError(
                "baseline p95 is zero but candidate is non-zero; relative drift undefined"
            )
        max_drift = max(abs(v - baseline) / baseline for v in values)
        return baseline, max_drift
    median = median_value(values)
    if median == 0:
        if all(v == 0 for v in values):
            return 0.0, 0.0
        raise ValueError(
            "median p95 is zero but at least one run is non-zero; relative drift undefined"
        )
    max_drift = max(abs(v - median) / median for v in values)
    return median, max_drift


def verdict(max_drift: float) -> tuple[str, int]:
    if max_drift <= 0.05:
        return "STABLE", 0
    if max_drift <= 0.10:
        return "NOISE (within envelope)", 0
    if max_drift <= 0.20:
        return "INVESTIGATE — p95 drift > 10%", 1
    return "ESCALATE — p95 drift > 20%", 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Check same-host p95 drift against docs/BUDGETS.md variance envelope.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="hyperfine / summary / trial-vector JSON files",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=None,
        help="prior accepted baseline JSON; remaining paths are candidates",
    )
    parser.add_argument(
        "--fail-on-investigate",
        action="store_true",
        default=True,
        help=argparse.SUPPRESS,  # default behavior: exit 1 above 10%
    )
    args = parser.parse_args(argv)

    paths: list[Path] = list(args.paths)
    if args.baseline is not None:
        # baseline + candidates
        if not paths:
            parser.error("--baseline requires at least one candidate path")
    elif len(paths) < 2:
        parser.error("need at least two run files, or --baseline plus a candidate")

    try:
        rows: list[tuple[str, dict[str, float | None]]] = []
        baseline_p95: float | None = None
        if args.baseline is not None:
            bstats = load_stats(args.baseline)
            baseline_p95 = float(bstats["p95_ms"])  # type: ignore[arg-type]
            rows.append((str(args.baseline) + " [baseline]", bstats))
        for path in paths:
            rows.append((str(path), load_stats(path)))
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    p95s = [float(stats["p95_ms"]) for _, stats in rows]  # type: ignore[arg-type]
    # When --baseline, drift only over candidates (exclude baseline from max drift set)
    if args.baseline is not None:
        candidate_p95s = p95s[1:]
        try:
            ref, max_drift = drift_fraction(candidate_p95s, baseline=baseline_p95)
        except ValueError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    else:
        try:
            ref, max_drift = drift_fraction(p95s, baseline=None)
        except ValueError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2

    print(f"{'file':50s}  p50_ms    p95_ms    p99_ms")
    for label, stats in rows:
        p50 = stats.get("p50_ms")
        p95 = stats.get("p95_ms")
        p99 = stats.get("p99_ms")
        p50_s = f"{p50:8.2f}" if p50 is not None else "     n/a"
        p95_s = f"{p95:8.2f}" if p95 is not None else "     n/a"
        p99_s = f"{p99:8.2f}" if p99 is not None else "     n/a"
        print(f"{label:50s}  {p50_s}  {p95_s}  {p99_s}")

    ref_label = "Baseline p95" if args.baseline is not None else "Median p95"
    print(f"\n{ref_label}: {ref:.2f} ms")
    print(f"Max drift:     {max_drift * 100:.1f}%")
    label, code = verdict(max_drift)
    print(f"Verdict:       {label}")
    return code


if __name__ == "__main__":
    sys.exit(main())
