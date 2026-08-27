#!/usr/bin/env python3
"""Deterministic real-task reading-set replay benchmark."""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
TASKS = HERE / "tasks.jsonl"
REPORT = HERE / "report.json"
MIN_SAVINGS_MULTIPLE = 2.0


def load_tasks() -> list[dict]:
    rows: list[dict] = []
    for line_no, line in enumerate(TASKS.read_text().splitlines(), 1):
        if not line.strip():
            continue
        row = json.loads(line)
        row["_line"] = line_no
        rows.append(row)
    if not rows:
        raise SystemExit("tasks.jsonl is empty")
    return rows


def expand_paths(row: dict, key: str) -> list[str]:
    paths = set(row.get(key, []))
    glob_key = key.replace("files", "globs")
    for pattern in row.get(glob_key, []):
        matches = sorted(
            p.relative_to(ROOT).as_posix()
            for p in ROOT.glob(pattern)
            if p.is_file()
        )
        if not matches:
            raise FileNotFoundError(f"glob {pattern} matched no files for {row['id']}")
        paths.update(matches)
    return sorted(paths)


def file_bytes(paths: Iterable[str]) -> int:
    total = 0
    for rel in paths:
        path = ROOT / rel
        if not path.is_file():
            raise FileNotFoundError(f"missing benchmark path: {rel}")
        total += path.stat().st_size
    return total


def estimated_tokens(byte_count: int) -> int:
    return math.ceil(byte_count / 4)


def measure_row(row: dict) -> dict:
    guided_list = expand_paths(row, "graph_guided_files")
    unguided_list = expand_paths(row, "unguided_candidate_files")
    guided_files = set(guided_list)
    unguided_files = set(unguided_list)
    required = set(row["success_files"])
    if not required <= guided_files:
        raise AssertionError(f"{row['id']} graph-guided set misses {sorted(required - guided_files)}")
    if not required <= unguided_files:
        raise AssertionError(f"{row['id']} unguided set misses {sorted(required - unguided_files)}")
    guided_bytes = file_bytes(guided_list)
    unguided_bytes = file_bytes(unguided_list)
    if guided_bytes <= 0 or unguided_bytes <= 0:
        raise AssertionError(f"{row['id']} produced non-positive byte counts")
    return {
        "id": row["id"],
        "repo": row["repo"],
        "change": row["change"],
        "target_symbol": row["target_symbol"],
        "success_files": row["success_files"],
        "graph_guided": {
            "files_read": len(guided_files),
            "bytes_read": guided_bytes,
            "estimated_tokens": estimated_tokens(guided_bytes),
            "success": True,
        },
        "unguided": {
            "files_read": len(unguided_files),
            "bytes_read": unguided_bytes,
            "estimated_tokens": estimated_tokens(unguided_bytes),
            "success": True,
        },
        "byte_savings_multiple": round(unguided_bytes / guided_bytes, 2),
        "outcome": row["outcome"],
    }


def build_report() -> dict:
    rows = [measure_row(row) for row in load_tasks()]
    guided_total = sum(row["graph_guided"]["bytes_read"] for row in rows)
    unguided_total = sum(row["unguided"]["bytes_read"] for row in rows)
    return {
        "schema_version": 1,
        "methodology": "deterministic replay of completed GraphZero change tasks; unguided reads broad repo/module candidates expanded from committed globs; graph_guided reads committed reading-set closure; success requires all success_files present; estimated_tokens=ceil(bytes/4)",
        "task_count": len(rows),
        "sample_accounting": {
            "total_samples": len(rows),
            "dropped_count": 0,
            "losses": [
                row["id"]
                for row in rows
                if not (row["graph_guided"]["success"] and row["unguided"]["success"])
            ],
        },
        "graph_guided_successes": len(rows),
        "unguided_successes": len(rows),
        "graph_guided_bytes": guided_total,
        "unguided_bytes": unguided_total,
        "byte_savings_multiple": round(unguided_total / guided_total, 2),
        "graph_guided_estimated_tokens": estimated_tokens(guided_total),
        "unguided_estimated_tokens": estimated_tokens(unguided_total),
        "rows": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true", help="rewrite report.json")
    parser.add_argument("--check", action="store_true", help="verify report.json and gates")
    args = parser.parse_args()
    report = build_report()
    if report["byte_savings_multiple"] < MIN_SAVINGS_MULTIPLE:
        raise SystemExit(f"savings {report['byte_savings_multiple']}x below {MIN_SAVINGS_MULTIPLE}x gate")
    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.write:
        REPORT.write_text(text)
    elif args.check:
        if not REPORT.is_file():
            raise SystemExit("report.json missing; run with --write")
        if REPORT.read_text() != text:
            raise SystemExit("report.json is stale; run with --write and review diff")
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
