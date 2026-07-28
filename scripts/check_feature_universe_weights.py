#!/usr/bin/env python3
"""Validate the product FeatureUniverse matrix and its normalized weights."""

from __future__ import annotations

import math
import sys
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any

MATRIX_PATH = Path(__file__).resolve().parents[1] / "docs/contracts/supported_surface_matrix.toml"
ALLOWED_STATUSES = ("present", "partial", "missing", "excluded")
WEIGHT_TOLERANCE = 1e-9


def fail(message: str) -> None:
    print(f"feature-universe check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def nonempty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def load_matrix(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        fail(f"cannot parse {path}: {exc}")
    return data


def validate() -> tuple[int, Counter[str], float]:
    matrix = load_matrix(MATRIX_PATH)
    declared = matrix.get("declared_feature_ids")
    features = matrix.get("feature")

    if not isinstance(declared, list) or not declared:
        fail("declared_feature_ids must be a non-empty array")
    if not all(nonempty_string(feature_id) for feature_id in declared):
        fail("declared_feature_ids must contain only non-empty strings")
    duplicate_declarations = sorted(
        feature_id for feature_id, count in Counter(declared).items() if count > 1
    )
    if duplicate_declarations:
        fail(f"duplicate declared feature IDs: {duplicate_declarations}")

    if not isinstance(features, list):
        fail("feature must be an array of tables")

    rows_by_id: dict[str, dict[str, Any]] = {}
    weights: list[float] = []
    statuses: Counter[str] = Counter()
    for index, row in enumerate(features):
        if not isinstance(row, dict):
            fail(f"feature row {index} is not a table")
        feature_id = row.get("id")
        if not nonempty_string(feature_id):
            fail(f"feature row {index} has a missing or empty id")
        if feature_id in rows_by_id:
            fail(f"duplicate feature ID: {feature_id}")
        rows_by_id[feature_id] = row

        status = row.get("status")
        if status not in ALLOWED_STATUSES:
            fail(f"{feature_id}: unknown status {status!r}")
        statuses[status] += 1

        weight = row.get("weight")
        if isinstance(weight, bool) or not isinstance(weight, (int, float)):
            fail(f"{feature_id}: weight must be a number")
        numeric_weight = float(weight)
        if not math.isfinite(numeric_weight) or numeric_weight <= 0.0:
            fail(f"{feature_id}: weight must be finite and greater than zero")
        weights.append(numeric_weight)

        if status in ("present", "partial"):
            evidence = row.get("evidence")
            if (
                not isinstance(evidence, list)
                or not evidence
                or not all(nonempty_string(item) for item in evidence)
            ):
                fail(f"{feature_id}: {status} requires non-empty evidence")
        else:
            if not nonempty_string(row.get("rationale")):
                fail(f"{feature_id}: {status} requires a non-empty rationale")

    declared_ids = set(declared)
    row_ids = set(rows_by_id)
    missing = sorted(declared_ids - row_ids)
    unexpected = sorted(row_ids - declared_ids)
    if missing:
        fail(f"declared feature IDs missing rows: {missing}")
    if unexpected:
        fail(f"feature rows absent from declared_feature_ids: {unexpected}")

    weight_sum = math.fsum(weights)
    if abs(weight_sum - 1.0) > WEIGHT_TOLERANCE:
        fail(f"weight sum is {weight_sum:.12f}, expected 1.0 +/- {WEIGHT_TOLERANCE}")

    return len(features), statuses, weight_sum


def main() -> None:
    feature_count, statuses, weight_sum = validate()
    accounting = " ".join(
        f"{status}={statuses[status]}" for status in ALLOWED_STATUSES
    )
    print(
        f"feature-universe ok: features={feature_count} {accounting} "
        f"weight_sum={weight_sum:.12f}"
    )


if __name__ == "__main__":
    main()
