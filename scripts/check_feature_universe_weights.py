#!/usr/bin/env python3
"""Validate the product FeatureUniverse matrix and its normalized weights."""

from __future__ import annotations

import math
import sys
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any, cast

MATRIX_PATH = Path(__file__).resolve().parents[1] / "conformance/contracts/supported_surface_matrix.toml"
ALLOWED_STATUSES = ("present", "partial", "missing", "excluded")
WEIGHT_TOLERANCE = 1e-9

FeatureRow = dict[str, Any]


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


def duplicate_values(values: list[str]) -> list[str]:
    return sorted(value for value, count in Counter(values).items() if count > 1)


def parse_declarations(matrix: dict[str, Any]) -> list[str]:
    declared = matrix.get("declared_feature_ids")
    if not isinstance(declared, list) or not declared:
        fail("declared_feature_ids must be a non-empty array")
    if not all(nonempty_string(feature_id) for feature_id in declared):
        fail("declared_feature_ids must contain only non-empty strings")
    feature_ids = cast(list[str], declared)
    duplicates = duplicate_values(feature_ids)
    if duplicates:
        fail(f"duplicate declared feature IDs: {duplicates}")
    return feature_ids


def parse_feature_tables(matrix: dict[str, Any]) -> list[object]:
    features = matrix.get("feature")
    if not isinstance(features, list):
        fail("feature must be an array of tables")
    return cast(list[object], features)


def parse_feature_id(row: FeatureRow, index: int, known_ids: set[str]) -> str:
    feature_id = row.get("id")
    if not nonempty_string(feature_id):
        fail(f"feature row {index} has a missing or empty id")
    parsed_id = cast(str, feature_id)
    if parsed_id in known_ids:
        fail(f"duplicate feature ID: {parsed_id}")
    return parsed_id


def parse_status(row: FeatureRow, feature_id: str) -> str:
    status = row.get("status")
    if status not in ALLOWED_STATUSES:
        fail(f"{feature_id}: unknown status {status!r}")
    return cast(str, status)


def parse_weight(row: FeatureRow, feature_id: str) -> float:
    weight = row.get("weight")
    if isinstance(weight, bool) or not isinstance(weight, (int, float)):
        fail(f"{feature_id}: weight must be a number")
    numeric_weight = float(weight)
    if not math.isfinite(numeric_weight) or numeric_weight <= 0.0:
        fail(f"{feature_id}: weight must be finite and greater than zero")
    return numeric_weight


def validate_evidence(row: FeatureRow, feature_id: str, status: str) -> None:
    evidence = row.get("evidence")
    valid = (
        isinstance(evidence, list)
        and bool(evidence)
        and all(nonempty_string(item) for item in evidence)
    )
    if not valid:
        fail(f"{feature_id}: {status} requires non-empty evidence")


def validate_rationale(row: FeatureRow, feature_id: str, status: str) -> None:
    if not nonempty_string(row.get("rationale")):
        fail(f"{feature_id}: {status} requires a non-empty rationale")


def validate_support(row: FeatureRow, feature_id: str, status: str) -> None:
    if status in ("present", "partial"):
        validate_evidence(row, feature_id, status)
        return
    validate_rationale(row, feature_id, status)


def parse_feature_row(value: object, index: int, known_ids: set[str]) -> tuple[str, str, float]:
    if not isinstance(value, dict):
        fail(f"feature row {index} is not a table")
    row = cast(FeatureRow, value)
    feature_id = parse_feature_id(row, index, known_ids)
    status = parse_status(row, feature_id)
    weight = parse_weight(row, feature_id)
    validate_support(row, feature_id, status)
    return feature_id, status, weight


def account_features(features: list[object]) -> tuple[set[str], Counter[str], list[float]]:
    row_ids: set[str] = set()
    statuses: Counter[str] = Counter()
    weights: list[float] = []
    for index, value in enumerate(features):
        feature_id, status, weight = parse_feature_row(value, index, row_ids)
        row_ids.add(feature_id)
        statuses[status] += 1
        weights.append(weight)
    return row_ids, statuses, weights


def validate_feature_ids(declared_ids: list[str], row_ids: set[str]) -> None:
    missing = sorted(set(declared_ids) - row_ids)
    unexpected = sorted(row_ids - set(declared_ids))
    if missing:
        fail(f"declared feature IDs missing rows: {missing}")
    if unexpected:
        fail(f"feature rows absent from declared_feature_ids: {unexpected}")


def validate_weight_sum(weights: list[float]) -> float:
    weight_sum = math.fsum(weights)
    if abs(weight_sum - 1.0) > WEIGHT_TOLERANCE:
        fail(f"weight sum is {weight_sum:.12f}, expected 1.0 +/- {WEIGHT_TOLERANCE}")
    return weight_sum


def validate() -> tuple[int, Counter[str], float]:
    matrix = load_matrix(MATRIX_PATH)
    declared_ids = parse_declarations(matrix)
    features = parse_feature_tables(matrix)
    row_ids, statuses, weights = account_features(features)
    validate_feature_ids(declared_ids, row_ids)
    weight_sum = validate_weight_sum(weights)
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
