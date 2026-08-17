#!/usr/bin/env python3
"""Pass-over-pass bench-history ratchet.

Compares a current JSON v3-shaped report against the local
`.bench-history/<bench>.latest.json` self-oracle (gitignored).

Gate thresholds (any one breach fails):
  primary score   -3%
  geomean         -5%
  per-category    -10%
  p90             -15%
  throughput      -5%

If only one score exists, that score is gated. Missing metrics are skipped,
not invented. Within-noise is not a win. cv_pct null or >5 is noise.

This script never claims the port is faster than a reference.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_HISTORY = REPO_ROOT / ".bench-history"
DEFAULT_BENCH = "savings-bench"
SCHEMA_V3 = "zerostack.comprehensive-bench-report.v3"

PRIMARY_MAX = 0.03
GEOMEAN_MAX = 0.05
CATEGORY_MAX = 0.10
P90_MAX = 0.15
THROUGHPUT_MAX = 0.05
CV_NOISE_PCT = 5.0
GITIGNORE_BAN = ".bench-history/"


def fail(message: str) -> None:
    print(f"bench-history check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_json(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot parse {path}: {exc}")
    if not isinstance(data, dict):
        fail(f"{path} is not a JSON object")
    return data


def as_float(value: object) -> float | None:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, (int, float)):
        number = float(value)
        if math.isfinite(number):
            return number
    return None


def score_direction(report: dict[str, Any], default: str = "lower_is_better") -> str:
    summary = report.get("summary")
    if isinstance(summary, dict):
        direction = summary.get("primary_score_direction")
        if direction in ("lower_is_better", "higher_is_better"):
            return direction
    return default


def extract_primary(report: dict[str, Any]) -> float | None:
    summary = report.get("summary")
    if not isinstance(summary, dict):
        return None
    weighted = summary.get("per_category_weighted")
    if isinstance(weighted, dict):
        score = as_float(weighted.get("score"))
        if score is not None:
            return score
    return as_float(summary.get("primary_score"))


def extract_named(report: dict[str, Any], *keys: str) -> float | None:
    summary = report.get("summary")
    if not isinstance(summary, dict):
        return None
    for key in keys:
        value = as_float(summary.get(key))
        if value is not None:
            return value
    return None


def extract_categories(report: dict[str, Any]) -> dict[str, float]:
    raw = report.get("categories")
    if not isinstance(raw, dict):
        return {}
    out: dict[str, float] = {}
    for name, body in raw.items():
        if isinstance(body, dict):
            score = as_float(body.get("score"))
        else:
            score = as_float(body)
        if score is not None:
            out[str(name)] = score
    return out


def regression_fraction(
    baseline: float, current: float, *, higher_is_better: bool
) -> float:
    if baseline == 0.0:
        fail("baseline score is 0; cannot compute a regression fraction")
    if higher_is_better:
        return (baseline - current) / abs(baseline)
    return (current - baseline) / abs(baseline)


def cv_is_noise(report: dict[str, Any]) -> tuple[bool, str]:
    cv = report.get("cv_pct")
    if cv is None:
        return True, "cv_pct is null (unknown; not a win)"
    number = as_float(cv)
    if number is None:
        return True, f"cv_pct is not numeric: {cv!r}"
    if number > CV_NOISE_PCT:
        return True, f"cv_pct={number:.3f} > {CV_NOISE_PCT} (noise)"
    return False, f"cv_pct={number:.3f}"


def check_gitignore(root: Path) -> None:
    path = root / ".gitignore"
    if not path.is_file():
        return
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = raw.strip()
        if stripped.startswith("#") or not stripped:
            continue
        if stripped == GITIGNORE_BAN or stripped == "/.bench-history/":
            return


def validate_seed(report: dict[str, Any], path: Path) -> None:
    schema = report.get("schema_version")
    if schema != SCHEMA_V3:
        fail(f"{path.name}: schema_version is {schema!r}, expected {SCHEMA_V3!r}")
    if "cv_pct" not in report:
        fail(f"{path.name}: missing cv_pct (may be JSON null, must be present)")
    if extract_primary(report) is None:
        fail(f"{path.name}: no primary_score / per_category_weighted.score")
    gate = report.get("ci_regression_gate")
    if not isinstance(gate, dict):
        fail(f"{path.name}: missing ci_regression_gate")


def gate_one(
    name: str,
    baseline: float | None,
    current: float | None,
    threshold: float,
    *,
    higher_is_better: bool,
    failures: list[str],
    notes: list[str],
) -> None:
    if baseline is None and current is None:
        notes.append(f"{name}: absent in both files; not gated")
        return
    if baseline is None or current is None:
        notes.append(f"{name}: present on only one side; not invented, not gated")
        return
    drop = regression_fraction(baseline, current, higher_is_better=higher_is_better)
    pct = drop * 100.0
    if drop > threshold + 1e-15:
        failures.append(
            f"{name}: regression {pct:.3f}% exceeds {threshold * 100:.1f}% "
            f"(baseline={baseline} current={current})"
        )
        return
    if drop < -1e-15:
        notes.append(
            f"{name}: moved {(-pct):.3f}% in the improvement direction "
            "(within-noise is not a win)"
        )
        return
    notes.append(f"{name}: unchanged")


def compare(
    baseline: dict[str, Any],
    current: dict[str, Any],
    *,
    quiet: bool = False,
) -> int:
    failures: list[str] = []
    notes: list[str] = []
    direction = score_direction(baseline)
    lower_is_better = direction == "lower_is_better"

    gate_one(
        "primary_score",
        extract_primary(baseline),
        extract_primary(current),
        PRIMARY_MAX,
        higher_is_better=not lower_is_better,
        failures=failures,
        notes=notes,
    )
    gate_one(
        "geomean",
        extract_named(baseline, "geomean_ratio", "geomean"),
        extract_named(current, "geomean_ratio", "geomean"),
        GEOMEAN_MAX,
        higher_is_better=False,
        failures=failures,
        notes=notes,
    )
    gate_one(
        "p90",
        extract_named(baseline, "p90_ratio", "p90"),
        extract_named(current, "p90_ratio", "p90"),
        P90_MAX,
        higher_is_better=False,
        failures=failures,
        notes=notes,
    )
    gate_one(
        "throughput",
        extract_named(baseline, "throughput"),
        extract_named(current, "throughput"),
        THROUGHPUT_MAX,
        higher_is_better=True,
        failures=failures,
        notes=notes,
    )

    base_cats = extract_categories(baseline)
    cur_cats = extract_categories(current)
    names = sorted(set(base_cats) | set(cur_cats))
    if not names:
        notes.append("per-category: no category scores; not gated")
    for name in names:
        gate_one(
            f"category:{name}",
            base_cats.get(name),
            cur_cats.get(name),
            CATEGORY_MAX,
            higher_is_better=not lower_is_better,
            failures=failures,
            notes=notes,
        )

    noise, noise_note = cv_is_noise(current)
    notes.append(noise_note)
    if not failures and noise:
        notes.append("verdict: pass (no regression). within-noise / unknown-cv is not a win")
    elif not failures:
        notes.append("verdict: pass (no regression). still not a keep without profile-first evidence")

    if not quiet:
        for note in notes:
            print(f"bench-history: {note}")
        if failures:
            for item in failures:
                print(f"bench-history REGRESSION: {item}", file=sys.stderr)
    if failures:
        return 1
    return 0


def run_self_test() -> None:
    seed = {
        "schema_version": SCHEMA_V3,
        "cv_pct": None,
        "summary": {
            "primary_score": 0.044,
            "primary_score_direction": "lower_is_better",
            "per_category_weighted": {"score": 0.044, "weights": {"exact_tokens": 1.0}},
            "geomean_ratio": None,
            "p90_ratio": None,
            "throughput": None,
        },
        "categories": {"exact_tokens": {"score": 0.044, "direction": "lower_is_better"}},
        "ci_regression_gate": {},
    }
    same = json.loads(json.dumps(seed))
    if compare(seed, same, quiet=True) != 0:
        fail("self-test: identical reports must pass")
    worse = json.loads(json.dumps(seed))
    worse["summary"]["primary_score"] = 0.044 * 1.04
    worse["summary"]["per_category_weighted"]["score"] = 0.044 * 1.04
    worse["categories"]["exact_tokens"]["score"] = 0.044 * 1.04
    if compare(seed, worse, quiet=True) == 0:
        fail("self-test: +4% primary regression must fail")
    print("bench-history self-test ok")


def latest_path(history: Path, bench: str) -> Path:
    return history / f"{bench}.latest.json"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history-dir", type=Path, default=DEFAULT_HISTORY)
    parser.add_argument("--bench", default=DEFAULT_BENCH)
    parser.add_argument(
        "--current",
        type=Path,
        help="current report JSON; defaults to the committed latest (self-check)",
    )
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()

    if args.self_test:
        run_self_test()
        return

    root = args.root.resolve()
    check_gitignore(root)
    history = args.history_dir
    if not history.is_absolute():
        history = root / history
    path = latest_path(history, args.bench)
    if not path.is_file():
        print(f"bench-history: skip (no local baseline at {path})")
        return
    baseline = load_json(path)
    validate_seed(baseline, path)
    current_path = args.current if args.current is not None else path
    if not current_path.is_absolute():
        current_path = root / current_path
    current = load_json(current_path)
    validate_seed(current, current_path)
    code = compare(baseline, current)
    if code != 0:
        raise SystemExit(code)
    print(
        f"bench-history ok: bench={args.bench} "
        f"baseline={path.relative_to(root)} "
        f"current={current_path.relative_to(root) if current_path.is_relative_to(root) else current_path}"
    )


if __name__ == "__main__":
    main()
