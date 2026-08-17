#!/usr/bin/env python3
"""Emit and verify the FeatureUniverse coverage dashboard.

Partial NEVER rounds up to success. Excluded still counts as debt for a
strict-100% claim (`strict_coverage`). Family labels are organizational;
the scoring category is the one global hub universe.

The committed artifact lives at
`conformance/contracts/feature_coverage_dashboard.json` because
`conformance/reports/` is gitignored (engine runtime evidence).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
import tomllib
from collections import defaultdict
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
MATRIX_PATH = REPO_ROOT / "conformance/contracts/supported_surface_matrix.toml"
DASHBOARD_PATH = REPO_ROOT / "conformance/contracts/feature_coverage_dashboard.json"
CATALOG_PATH = REPO_ROOT / "conformance/contracts/invariant_catalog.toml"
SCHEMA_VERSION = "zerostack.feature_coverage_dashboard.v1"
PARTIAL_CONTRIBUTION = 0.5
REQUIRED_INVARIANTS = (
    "INV-ENGINE-IDENTITY",
    "INV-BOTH-ERROR",
    "INV-GOLDEN-CHECKSUMS",
    "INV-BENCH-HISTORY",
    "INV-CRASH-ORACLE",
    "INV-WEIGHT-SUM",
)

# Import the weight loader so dashboard and weight gate share one matrix parse.
sys.path.insert(0, str(REPO_ROOT / "scripts"))
from check_feature_universe_weights import (  # noqa: E402
    ALLOWED_STATUSES,
    validate,
)


def fail(message: str) -> None:
    print(f"feature-coverage-dashboard failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def truncate_score(value: float) -> float:
    """Integer-truncation to 6 decimal places (not f64 round)."""
    return math.trunc(value * 1_000_000.0) / 1_000_000.0


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        fail(f"cannot parse {path}: {exc}")
    if not isinstance(data, dict):
        fail(f"{path} is not a TOML table")
    return data


def contribution(status: str) -> float:
    if status == "present":
        return 1.0
    if status == "partial":
        return PARTIAL_CONTRIBUTION
    return 0.0


def family_verdict(present: int, partial: int, missing: int, excluded: int) -> str:
    """Partial never rounds up. All-excluded is none (coverage debt)."""
    if partial > 0 or missing > 0:
        if present > 0 or partial > 0:
            return "partial"
        return "none"
    if present > 0 and excluded == 0:
        return "full"
    if present > 0 and excluded > 0:
        return "partial"
    return "none"


def build_dashboard(matrix: dict[str, Any]) -> dict[str, Any]:
    features = matrix.get("feature")
    if not isinstance(features, list):
        fail("feature must be an array of tables")

    histogram = {status: 0 for status in ALLOWED_STATUSES}
    family_rows: dict[str, list[dict[str, Any]]] = defaultdict(list)
    weighted_success = 0.0
    in_scope_weight = 0.0
    all_weight = 0.0

    for raw in features:
        if not isinstance(raw, dict):
            fail("feature row is not a table")
        feature_id = str(raw.get("id", ""))
        family = str(raw.get("family", ""))
        status = str(raw.get("status", ""))
        weight = raw.get("weight")
        if status not in ALLOWED_STATUSES:
            fail(f"{feature_id}: unknown status {status!r}")
        if isinstance(weight, bool) or not isinstance(weight, (int, float)):
            fail(f"{feature_id}: weight must be a number")
        numeric_weight = float(weight)
        histogram[status] += 1
        family_rows[family].append(raw)
        all_weight += numeric_weight
        weighted_success += numeric_weight * contribution(status)
        if status != "excluded":
            in_scope_weight += numeric_weight

    families: list[dict[str, Any]] = []
    for family in sorted(family_rows):
        rows = family_rows[family]
        counts = {status: 0 for status in ALLOWED_STATUSES}
        family_success = 0.0
        family_weight = 0.0
        family_in_scope = 0.0
        for raw in rows:
            status = str(raw["status"])
            weight = float(raw["weight"])
            counts[status] += 1
            family_weight += weight
            family_success += weight * contribution(status)
            if status != "excluded":
                family_in_scope += weight
        present, partial, missing, excluded = (
            counts["present"],
            counts["partial"],
            counts["missing"],
            counts["excluded"],
        )
        verdict = family_verdict(present, partial, missing, excluded)
        if verdict == "full" and (partial > 0 or missing > 0 or excluded > 0):
            fail(f"{family}: partial/missing/excluded rounded up to full")
        if present + partial + missing + excluded != len(rows):
            fail(f"{family}: status counts do not cover rows")
        family_effective = (
            family_success / family_in_scope if family_in_scope > 0.0 else 0.0
        )
        family_strict = family_success / family_weight if family_weight > 0.0 else 0.0
        families.append(
            {
                "family": family,
                "feature_count": len(rows),
                "present": present,
                "partial": partial,
                "missing": missing,
                "excluded": excluded,
                "weighted_score": truncate_score(family_effective),
                "strict_coverage": truncate_score(family_strict),
                "verdict": verdict,
                "strict_100": present == len(rows) and partial == 0 and missing == 0 and excluded == 0,
            }
        )

    effective = weighted_success / in_scope_weight if in_scope_weight > 0.0 else 0.0
    strict = weighted_success / all_weight if all_weight > 0.0 else 0.0
    missing = histogram["missing"]
    partial = histogram["partial"]
    excluded = histogram["excluded"]
    present = histogram["present"]
    strict_100 = missing == 0 and partial == 0 and excluded == 0
    if missing > 0 or not strict_100:
        release_gate = "red"
    elif partial > 0:
        release_gate = "yellow"
    else:
        release_gate = "green"

    return {
        "schema_version": SCHEMA_VERSION,
        "source": "conformance/contracts/supported_surface_matrix.toml",
        "catalog": "conformance/contracts/invariant_catalog.toml",
        "weight_policy": str(matrix.get("weight_policy", "")),
        "feature_count": len(features),
        "status_histogram": {
            "present": present,
            "partial": partial,
            "missing": missing,
            "excluded": excluded,
        },
        "families": families,
        "global": {
            "effective_coverage": truncate_score(effective),
            "strict_coverage": truncate_score(strict),
            "partial_contribution": PARTIAL_CONTRIBUTION,
            "partial_never_rounds_up": True,
            "excluded_is_strict_debt": True,
            "strict_100_certifiable": strict_100,
            "release_gate_verdict": release_gate,
        },
    }


def validate_catalog() -> list[str]:
    if not CATALOG_PATH.is_file():
        fail(f"missing invariant catalog: {CATALOG_PATH.relative_to(REPO_ROOT)}")
    catalog = load_toml(CATALOG_PATH)
    invariants = catalog.get("invariant")
    if not isinstance(invariants, list) or not invariants:
        fail("invariant catalog has no [[invariant]] rows")
    found: list[str] = []
    for raw in invariants:
        if not isinstance(raw, dict):
            fail("catalog invariant is not a table")
        inv_id = str(raw.get("id", ""))
        if not inv_id:
            fail("catalog invariant missing id")
        found.append(inv_id)
        artifacts = raw.get("artifact")
        if not isinstance(artifacts, list) or not artifacts:
            fail(f"{inv_id}: requires at least one artifact")
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                fail(f"{inv_id}: artifact is not a table")
            rel = artifact.get("path")
            if not isinstance(rel, str) or not rel:
                fail(f"{inv_id}: artifact path missing")
            target = REPO_ROOT / rel
            if not target.is_file():
                fail(f"{inv_id}: artifact path does not exist: {rel}")
            expected = artifact.get("hash")
            if isinstance(expected, str) and expected:
                actual = sha256_file(target)
                if actual != expected:
                    fail(f"{inv_id}: hash drift for {rel} (got {actual})")
    missing = [inv_id for inv_id in REQUIRED_INVARIANTS if inv_id not in found]
    if missing:
        fail(f"catalog missing required invariants: {missing}")
    return found


def dump_json(data: dict[str, Any]) -> bytes:
    return (
        json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    ).encode("utf-8")


def write_dashboard(data: dict[str, Any]) -> None:
    DASHBOARD_PATH.parent.mkdir(parents=True, exist_ok=True)
    DASHBOARD_PATH.write_bytes(dump_json(data))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write",
        action="store_true",
        help="rewrite the committed dashboard JSON",
    )
    args = parser.parse_args()

    feature_count, statuses, weight_sum = validate()
    matrix = load_toml(MATRIX_PATH)
    dashboard = build_dashboard(matrix)
    catalog_ids = validate_catalog()

    payload = dump_json(dashboard)
    if args.write:
        write_dashboard(dashboard)
    elif not DASHBOARD_PATH.is_file():
        fail(
            f"missing {DASHBOARD_PATH.relative_to(REPO_ROOT)}; "
            "run with --write to create it"
        )
    else:
        committed = DASHBOARD_PATH.read_bytes()
        if committed != payload:
            fail(
                "committed dashboard drifted from the live matrix; "
                "run python3 scripts/check_feature_coverage_dashboard.py --write"
            )

    hist = dashboard["status_histogram"]
    glob = dashboard["global"]
    accounting = " ".join(f"{status}={hist[status]}" for status in ALLOWED_STATUSES)
    print(
        f"feature-coverage-dashboard ok: features={feature_count} {accounting} "
        f"weight_sum={weight_sum:.12f} "
        f"effective={glob['effective_coverage']:.6f} "
        f"strict={glob['strict_coverage']:.6f} "
        f"gate={glob['release_gate_verdict']} "
        f"families={len(dashboard['families'])} "
        f"catalog={len(catalog_ids)} "
        f"statuses={dict(statuses)}"
    )
    if glob["strict_100_certifiable"] and (
        hist["missing"] or hist["partial"] or hist["excluded"]
    ):
        fail("strict_100_certifiable is true while missing/partial/excluded remain")
    if glob["release_gate_verdict"] == "green" and not glob["strict_100_certifiable"]:
        fail("release gate is green without a strict-100% claim")


if __name__ == "__main__":
    main()
