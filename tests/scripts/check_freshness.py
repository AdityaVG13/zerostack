#!/usr/bin/env python3
"""Validate an explicit local conformance report index; never create evidence."""
from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path, PurePath
from typing import Any

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
FIELDS = ("engine", "surface", "semantic_contract_digest", "operation_registry_digest", "git_revision", "timestamp", "bin")


def load(path: Path, errors: list[str]) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        errors.append(f"{path.name}: cannot read valid JSON: {exc}")
        return None


def timestamp(value: object, label: str, errors: list[str]) -> datetime | None:
    if not isinstance(value, str):
        errors.append(f"{label}: timestamp must be a string")
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        errors.append(f"{label}: timestamp must be RFC 3339")
        return None
    if parsed.tzinfo is None:
        errors.append(f"{label}: timestamp must include a timezone")
        return None
    return parsed.astimezone(timezone.utc)


def basename(value: object) -> bool:
    return (
        isinstance(value, str)
        and bool(value)
        and not PurePath(value).is_absolute()
        and PurePath(value).name == value
        and "/" not in value
        and "\\" not in value
    )


def compare_index_fields(entry: dict[str, Any], report: dict[str, Any], label: str, name: str, errors: list[str]) -> None:
    for field in FIELDS:
        left, right = entry.get(field), report.get(field)
        if not isinstance(left, str) or not left:
            errors.append(f"{label}.{field}: must be a non-empty string")
        if left != right:
            errors.append(f"{name}: {field} does not match index")


def validate_digests(report: dict[str, Any], name: str, errors: list[str]) -> None:
    for field in ("semantic_contract_digest", "operation_registry_digest"):
        value = report.get(field)
        if not isinstance(value, str) or not HEX64.fullmatch(value):
            errors.append(f"{name}: {field} must be 64 lowercase hex characters")


def validate_timestamp(report: dict[str, Any], name: str, current: datetime, max_age_days: int, errors: list[str]) -> None:
    stamp = timestamp(report.get("timestamp"), name, errors)
    if stamp is None:
        return
    if stamp > current + timedelta(minutes=5):
        errors.append(f"{name}: timestamp is in the future")
    elif current - stamp > timedelta(days=max_age_days):
        errors.append(f"{name}: report is stale (older than {max_age_days} days)")


def validate_report(entry: dict[str, Any], report: dict[str, Any], label: str, name: str, current: datetime, max_age_days: int, errors: list[str]) -> None:
    compare_index_fields(entry, report, label, name, errors)
    validate_digests(report, name, errors)
    revision = report.get("git_revision")
    if not isinstance(revision, str) or not HEX40.fullmatch(revision):
        errors.append(f"{name}: git_revision must be 40 lowercase hex characters")
    if not basename(report.get("bin")):
        errors.append(f"{name}: bin must be basename-only")
    validate_timestamp(report, name, current, max_age_days, errors)
    if report.get("passed") is not True:
        errors.append(f"{name}: passed must be true")
    if report.get("completion_status") != "complete":
        errors.append(f"{name}: completion_status must be 'complete'")


def report_name(entry: dict[str, Any], label: str, index_path: Path, indexed: set[str], errors: list[str]) -> str | None:
    value = entry.get("report")
    if not basename(value) or not str(value).endswith(".json"):
        errors.append(f"{label}: report must be a basename-only .json filename")
        return None
    name = str(value)
    if name == index_path.name or name in indexed:
        errors.append(f"{label}: duplicate or self-referencing report {name!r}")
        return None
    indexed.add(name)
    return name


def validate_entry(entry: object, number: int, index_path: Path, indexed: set[str], current: datetime, max_age_days: int, errors: list[str]) -> None:
    label = f"reports[{number}]"
    if not isinstance(entry, dict):
        errors.append(f"{label}: entry must be an object")
        return
    name = report_name(entry, label, index_path, indexed, errors)
    if name is None:
        return
    report = load(index_path.parent / name, errors)
    if not isinstance(report, dict):
        if report is not None:
            errors.append(f"{name}: report must be an object")
        return
    validate_report(entry, report, label, name, current, max_age_days, errors)


def validate(index_path: Path, max_age_days: int = 30, now: datetime | None = None) -> list[str]:
    errors: list[str] = []
    index = load(index_path, errors)
    if not isinstance(index, dict):
        if index is not None:
            errors.append(f"{index_path.name}: index must be an object")
        return errors
    entries = index.get("reports")
    if not isinstance(entries, list) or not entries:
        return errors + [f"{index_path.name}: reports must be a non-empty array"]
    indexed: set[str] = set()
    current = (now or datetime.now(timezone.utc)).astimezone(timezone.utc)
    for number, entry in enumerate(entries):
        validate_entry(entry, number, index_path, indexed, current, max_age_days, errors)
    extras = {path.name for path in index_path.parent.glob("*.json")} - indexed - {index_path.name}
    errors.extend(f"{name}: report is not indexed" for name in sorted(extras))
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("index", type=Path, help="explicit local report index JSON")
    parser.add_argument("--max-age-days", type=int, default=30)
    args = parser.parse_args(argv)
    if args.max_age_days < 0:
        parser.error("--max-age-days must be non-negative")
    errors = validate(args.index, args.max_age_days)
    if errors:
        for error in errors:
            print(f"freshness: {error}", file=sys.stderr)
        return 1
    print(f"local conformance evidence valid: {args.index}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
