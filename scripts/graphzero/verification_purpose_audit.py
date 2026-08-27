#!/usr/bin/env python3
"""Audit the committed verification-purpose ledger against the repository inventory."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / ".verification-purpose" / "ledger.json"
ALLOWED_PURPOSES = {"program-durable", "session-only", "drop"}
REQUIRED_FIELDS = {
    "artifact_path",
    "purpose",
    "downstream_failure_class",
    "loc",
    "runtime_cost",
    "decision",
    "no_op_reason",
}


def repo_path(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def inventory() -> set[str]:
    paths: set[str] = set()
    crates = ROOT / "crates"
    if crates.exists():
        paths.update(repo_path(p) for p in crates.glob("*/tests/**/*") if p.is_file())
    action = ROOT / ".github" / "actions" / "graphzero-verify"
    if action.exists():
        paths.update(repo_path(p) for p in action.glob("**/*") if p.is_file())
    workflows = ROOT / ".github" / "workflows"
    if workflows.exists():
        paths.update(repo_path(p) for p in workflows.glob("*.yml") if p.is_file())
    for base in ("bench", "benchmarks"):
        root = ROOT / base
        if root.exists():
            paths.update(repo_path(p) for p in root.glob("**/*") if p.is_file())
    scripts = ROOT / "scripts"
    if scripts.exists():
        for p in scripts.glob("**/*"):
            if not p.is_file():
                continue
            rel = repo_path(p)
            if (
                "benchmark" in p.name
                or p.name.startswith("readme_")
                or rel.startswith("scripts/perf/")
                or rel == "scripts/verification_purpose_audit.py"
            ):
                paths.add(rel)
    return paths


def fail(message: str) -> int:
    print(f"ERROR: {message}", file=sys.stderr)
    return 1


def main() -> int:
    tracked = subprocess.run(  # ubs:ignore — return code determines whether the local ledger is public.
        ["git", "ls-files", "--error-unmatch", ".verification-purpose/ledger.json"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if tracked.returncode != 0:
        print("SKIP: verification-purpose ledger is local and untracked")
        return 0
    if not LEDGER.exists():
        return fail("tracked verification-purpose ledger is missing")
    try:
        doc = json.loads(LEDGER.read_text())  # ubs:ignore — JSONDecodeError is converted below.
    except (OSError, json.JSONDecodeError) as error:
        return fail(f"invalid local ledger: {error}")
    if doc.get("schema") != "graphzero.verification-purpose.v1":
        return fail("unexpected ledger schema")
    rows = doc.get("rows")
    if not isinstance(rows, list):
        return fail("ledger rows must be a list")

    seen: set[str] = set()
    errors: list[str] = []
    for idx, row in enumerate(rows, start=1):
        if not isinstance(row, dict):
            errors.append(f"row {idx}: not an object")
            continue
        missing_fields = REQUIRED_FIELDS - row.keys()
        if missing_fields:
            errors.append(f"row {idx}: missing fields {sorted(missing_fields)}")
        path = row.get("artifact_path")
        if not isinstance(path, str) or not path:
            errors.append(f"row {idx}: artifact_path must be non-empty string")
            continue
        if path in seen:
            errors.append(f"duplicate ledger row: {path}")
        seen.add(path)
        purpose = row.get("purpose")
        if purpose not in ALLOWED_PURPOSES:
            errors.append(f"{path}: invalid purpose {purpose!r}")
        if purpose == "drop" and row.get("decision") != "remove":
            errors.append(f"{path}: drop rows must use decision=remove")
        for field in ("downstream_failure_class", "runtime_cost", "decision", "no_op_reason"):
            if not isinstance(row.get(field), str) or not row[field].strip():
                errors.append(f"{path}: {field} must be non-empty")
        actual = ROOT / path
        if actual.exists():
            actual_loc = sum(1 for _ in actual.open("rb"))
            if row.get("loc") != actual_loc:
                errors.append(f"{path}: loc {row.get('loc')} != {actual_loc}")
        elif purpose != "drop":
            errors.append(f"{path}: missing file must be purpose=drop")

    expected = inventory()
    missing = expected - seen
    extra_live = {p for p in seen - expected if (ROOT / p).exists()}
    if missing:
        errors.append("missing inventory rows: " + ", ".join(sorted(missing)))
    if extra_live:
        errors.append("live rows outside inventory: " + ", ".join(sorted(extra_live)))

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(f"OK: {len(rows)} ledger rows cover {len(expected)} verification artifacts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
