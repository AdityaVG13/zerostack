#!/usr/bin/env python3
"""Fail-closed performance receipt ratchet for GraphZero.

Read-only comparator: it never writes, never mutates ``benchmarks/rebaseline/
history.jsonl``, and never produces a benchmark. It compares one current
receipt against one committed baseline and enforces:

* exact identity checks (full metric-key set, corpus name, cargo profile,
  host class, and isolation) before any latency comparison, plus an exact
  numeric-metric-set match so a status-only/numeric shape change cannot
  silently skip relative comparison;
* a minimum sample count per latency metric;
* absolute p50/p95 budgets from ``benchmarks/latency/latency_gate.json`` for gated
  metrics (``orient_symbol``, ``blast``);
* a relative-regression policy: current p50/p95 must not exceed the baseline
  p50/p95 scaled by ``relative_max_multiple`` (per-metric from the gate, or
  the baseline policy default when a metric has no gate entry). It allows any
  slowdown up to that multiple; it fails only when the multiple is exceeded.

Malformed metric, policy, or gate data (non-object blocks, non-finite or
negative p50/p95, non-integer ``runs``, non-positive ``min_samples``, relative
multiples below 1, non-numeric absolute limits) fails closed with exit code 2.

Exit codes: 0 = pass, 1 = ratchet failure (identity mismatch, numeric shape
change, insufficient samples, or absolute/relative regression), 2 = usage or
malformed input.

Example:
    uv run python scripts/bench_ratchet.py
    uv run python scripts/bench_ratchet.py --current /tmp/current.json
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

DEFAULT_BASELINE = Path("benchmarks/latency/.bench-history/baseline.json")
DEFAULT_CURRENT = Path("benchmarks/rebaseline/latest.json")
DEFAULT_LATENCY_GATE = Path("benchmarks/latency/latency_gate.json")
SCHEMA_VERSION = 1
#: Fallback relative-regression multiple when the gate has no entry for a metric.
DEFAULT_RELATIVE_MULTIPLE = 1.5
EXIT_PASS = 0
EXIT_FAIL = 1
EXIT_USAGE = 2


class RatchetInputError(Exception):
    """Malformed input or unsupported schema; maps to exit code 2."""


class RatchetFailure(Exception):
    """A gate/identity check failed; maps to exit code 1."""


def load_json(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise RatchetInputError(f"missing file: {path}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise RatchetInputError(f"malformed JSON in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise RatchetInputError(f"{path}: expected a JSON object, got {type(data).__name__}")
    return data


def require_fields(path: Path, data: dict[str, Any], fields: list[tuple[str, str]]) -> None:
    """Validate that every dotted field exists; raises RatchetInputError otherwise."""
    for dotted, what in fields:
        node: Any = data
        for part in dotted.split("."):
            if not isinstance(node, dict) or part not in node:
                raise RatchetInputError(f"{path}: missing required field {dotted} ({what})")
            node = node[part]


def validate_finite_number(
    path: Path,
    label: str,
    value: Any,
    *,
    nonnegative: bool = True,
    minimum: float | None = None,
) -> None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RatchetInputError(
            f"{path}: {label} must be a number, got {type(value).__name__} ({value!r})"
        )
    if not math.isfinite(value):
        raise RatchetInputError(f"{path}: {label} must be finite, got {value!r}")
    if nonnegative and value < 0:
        raise RatchetInputError(f"{path}: {label} must be nonnegative, got {value!r}")
    if minimum is not None and value < minimum:
        raise RatchetInputError(f"{path}: {label} must be >= {minimum}, got {value!r}")


def validate_metrics(path: Path, metrics: Any) -> None:
    if not isinstance(metrics, dict):
        raise RatchetInputError(f"{path}: metrics must be an object, got {type(metrics).__name__}")
    for name, value in metrics.items():
        if not isinstance(value, dict):
            raise RatchetInputError(
                f"{path}: metrics.{name} must be an object, got {type(value).__name__}"
            )
        has_p50 = "p50_ms" in value
        has_p95 = "p95_ms" in value
        if has_p50 != has_p95:
            raise RatchetInputError(
                f"{path}: metrics.{name} must define both p50_ms and p95_ms or neither"
            )
        if has_p50:
            validate_finite_number(path, f"metrics.{name}.p50_ms", value["p50_ms"])
            validate_finite_number(path, f"metrics.{name}.p95_ms", value["p95_ms"])
            runs = value.get("runs")
            if isinstance(runs, bool) or not isinstance(runs, int):
                raise RatchetInputError(
                    f"{path}: metrics.{name}.runs must be an integer, got "
                    f"{type(runs).__name__ if runs is not None else 'None'}"
                )


def validate_gate(path: Path, thresholds: Any) -> None:
    if not isinstance(thresholds, dict):
        raise RatchetInputError(
            f"{path}: thresholds must be an object, got {type(thresholds).__name__}"
        )
    for name, entry in thresholds.items():
        if not isinstance(entry, dict):
            raise RatchetInputError(
                f"{path}: thresholds.{name} must be an object, got {type(entry).__name__}"
            )
        if "p50_max_ms" in entry:
            validate_finite_number(path, f"thresholds.{name}.p50_max_ms", entry["p50_max_ms"])
        if "p95_max_ms" in entry:
            validate_finite_number(path, f"thresholds.{name}.p95_max_ms", entry["p95_max_ms"])
        if "relative_max_multiple" in entry:
            validate_finite_number(
                path,
                f"thresholds.{name}.relative_max_multiple",
                entry["relative_max_multiple"],
                nonnegative=False,
                minimum=1.0,
            )


def validate_policy(path: Path, policy: Any) -> None:
    if not isinstance(policy, dict):
        raise RatchetInputError(f"{path}: policy must be an object, got {type(policy).__name__}")
    if "min_samples" in policy:
        ms = policy["min_samples"]
        if isinstance(ms, bool) or not isinstance(ms, int) or ms < 1:
            raise RatchetInputError(
                f"{path}: policy.min_samples must be a positive integer, got {ms!r}"
            )
    if "relative_max_multiple_default" in policy:
        validate_finite_number(
            path,
            "policy.relative_max_multiple_default",
            policy["relative_max_multiple_default"],
            nonnegative=False,
            minimum=1.0,
        )


def nested_get(data: dict[str, Any], dotted: str) -> Any:
    node: Any = data
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            return None
        node = node[part]
    return node


def check_identity(
    what: str,
    baseline: dict[str, Any],
    current: dict[str, Any],
) -> None:
    for dotted, label in [
        ("corpus.name", "corpus"),
        ("measurement_environment.profile", "cargo profile"),
        ("measurement_environment.host_class", "host class"),
        ("measurement_environment.isolation", "isolation"),
    ]:
        b = nested_get(baseline, dotted)
        c = nested_get(current, dotted)
        if b != c:
            raise RatchetFailure(
                f"{what}: {label} mismatch (baseline={b!r} current={c!r}); "
                "refusing to compare receipts from different conditions"
            )
    baseline_metrics = set(baseline.get("metrics", {}))
    current_metrics = set(current.get("metrics", {}))
    if baseline_metrics != current_metrics:
        raise RatchetFailure(
            f"{what}: scenario/metric set mismatch "
            f"(baseline-only={sorted(baseline_metrics - current_metrics)} "
            f"current-only={sorted(current_metrics - baseline_metrics)})"
        )


def numeric_metrics(metrics: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Latency metrics carrying numeric p50_ms/p95_ms; status-only entries skipped."""
    return {
        name: value
        for name, value in metrics.items()
        if isinstance(value, dict) and "p50_ms" in value and "p95_ms" in value
    }


def run_ratchet(
    baseline_path: Path,
    current_path: Path,
    gate_path: Path,
    min_samples: int | None,
    relative_multiple: float | None,
) -> int:
    baseline = load_json(baseline_path)
    current = load_json(current_path)
    gate = load_json(gate_path)

    for path, data in [
        (baseline_path, baseline),
        (current_path, current),
        (gate_path, gate),
    ]:
        if data.get("schema_version") != SCHEMA_VERSION:
            raise RatchetInputError(
                f"{path}: unsupported schema_version={data.get('schema_version')!r} "
                f"(expected {SCHEMA_VERSION})"
            )

    require_fields(
        baseline_path,
        baseline,
        [
            ("corpus.name", "corpus identity"),
            ("measurement_environment.profile", "identity"),
            ("measurement_environment.host_class", "identity"),
            ("measurement_environment.isolation", "identity"),
            ("metrics", "baseline metrics"),
        ],
    )
    require_fields(
        current_path,
        current,
        [
            ("corpus.name", "corpus identity"),
            ("measurement_environment.profile", "identity"),
            ("measurement_environment.host_class", "identity"),
            ("measurement_environment.isolation", "identity"),
            ("metrics", "current metrics"),
        ],
    )
    require_fields(gate_path, gate, [("thresholds", "absolute budgets")])

    validate_metrics(baseline_path, baseline["metrics"])
    validate_metrics(current_path, current["metrics"])
    validate_gate(gate_path, gate["thresholds"])
    if "policy" in baseline:
        validate_policy(baseline_path, baseline["policy"])

    if min_samples is not None:
        if isinstance(min_samples, bool) or not isinstance(min_samples, int) or min_samples < 1:
            raise RatchetInputError(
                f"--min-samples must be a positive integer, got {min_samples!r}"
            )
    if relative_multiple is not None:
        validate_finite_number(
            Path("--relative-multiple"),
            "value",
            relative_multiple,
            nonnegative=False,
            minimum=1.0,
        )

    check_identity("ratchet", baseline, current)

    baseline_metrics = numeric_metrics(baseline["metrics"])
    current_metrics = numeric_metrics(current["metrics"])
    baseline_numeric_keys = set(baseline_metrics)
    current_numeric_keys = set(current_metrics)
    if baseline_numeric_keys != current_numeric_keys:
        raise RatchetFailure(
            "ratchet: numeric metric set mismatch "
            f"(baseline-only={sorted(baseline_numeric_keys - current_numeric_keys)} "
            f"current-only={sorted(current_numeric_keys - baseline_numeric_keys)}); "
            "a metric's numeric/status-only shape changed between receipts"
        )

    policy = baseline.get("policy", {})
    min_samples = min_samples if min_samples is not None else policy.get("min_samples", 10)
    default_multiple = (
        relative_multiple
        if relative_multiple is not None
        else policy.get("relative_max_multiple_default", DEFAULT_RELATIVE_MULTIPLE)
    )

    thresholds = gate["thresholds"]
    problems: list[str] = []
    for name in sorted(current_metrics):
        cur = current_metrics[name]
        base = baseline_metrics[name]
        runs = cur["runs"]
        if runs < min_samples:
            problems.append(
                f"{name}: insufficient samples (runs={runs}, required >= {min_samples})"
            )
            continue
        gate_entry = thresholds.get(name, {})
        p50_max = gate_entry.get("p50_max_ms")
        p95_max = gate_entry.get("p95_max_ms")
        if p50_max is not None and cur["p50_ms"] > p50_max:
            problems.append(
                f"{name}: absolute p50 regression ({cur['p50_ms']:.3f} ms > {p50_max} ms)"
            )
        if p95_max is not None and cur["p95_ms"] > p95_max:
            problems.append(
                f"{name}: absolute p95 regression ({cur['p95_ms']:.3f} ms > {p95_max} ms)"
            )
        multiple = gate_entry.get("relative_max_multiple", default_multiple)
        base_p50 = base["p50_ms"]
        base_p95 = base["p95_ms"]
        if cur["p50_ms"] > base_p50 * multiple:
            problems.append(
                f"{name}: relative p50 regression "
                f"({cur['p50_ms']:.3f} ms > {base_p50} ms * {multiple})"
            )
        if cur["p95_ms"] > base_p95 * multiple:
            problems.append(
                f"{name}: relative p95 regression "
                f"({cur['p95_ms']:.3f} ms > {base_p95} ms * {multiple})"
            )

    status = "pass" if not problems else "fail"
    print(
        json.dumps(
            {
                "status": status,
                "baseline": str(baseline_path),
                "current": str(current_path),
                "gate": str(gate_path),
                "min_samples": min_samples,
                "relative_max_multiple_default": default_multiple,
                "gated_metrics": sorted(current_metrics),
                "problems": problems,
            },
            indent=2,
        )
    )
    return EXIT_PASS if status == "pass" else EXIT_FAIL


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Fail-closed performance receipt ratchet: compare a current receipt "
            "against a committed baseline after exact identity checks. "
            "Output is always a JSON report on stdout."
        ),
        epilog=(
            "Exit codes: 0 pass, 1 ratchet failure, 2 usage/malformed input. "
            "The script is read-only and never mutates history."
        ),
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=DEFAULT_BASELINE,
        help=f"committed baseline receipt (default: {DEFAULT_BASELINE})",
    )
    parser.add_argument(
        "--current",
        type=Path,
        default=DEFAULT_CURRENT,
        help=f"current receipt to check (default: {DEFAULT_CURRENT})",
    )
    parser.add_argument(
        "--latency-gate",
        type=Path,
        default=DEFAULT_LATENCY_GATE,
        help=f"absolute p50/p95 budgets (default: {DEFAULT_LATENCY_GATE})",
    )
    parser.add_argument(
        "--min-samples",
        type=int,
        default=None,
        help="minimum samples per latency metric (default: baseline policy or 10)",
    )
    parser.add_argument(
        "--relative-multiple",
        type=float,
        default=None,
        help="fallback relative-regression multiple (default: baseline policy or 1.5)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        return run_ratchet(
            args.baseline,
            args.current,
            args.latency_gate,
            args.min_samples,
            args.relative_multiple,
        )
    except RatchetInputError as exc:
        print(f"bench_ratchet: input error: {exc}", file=sys.stderr)
        return EXIT_USAGE
    except RatchetFailure as exc:
        print(f"bench_ratchet: failure: {exc}", file=sys.stderr)
        return EXIT_FAIL


if __name__ == "__main__":
    sys.exit(main())
