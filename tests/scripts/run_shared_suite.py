#!/usr/bin/env python3
"""Run the canonical shared suite against reference or explicitly located binaries.

The runner is intentionally transport-neutral: the Rust conformance binary owns
the selected raw-worker, planner, or MCP framing, while the adapter descriptor
supplies only the executable, namespace, and surface.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

TESTS_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = TESTS_ROOT.parent
DESCRIPTORS = TESTS_ROOT / "adapters" / "descriptors.json"


def load_descriptors() -> dict:
    value = json.loads(DESCRIPTORS.read_text(encoding="utf-8"))
    if value.get("version") != "zerostack.shared-suite.adapters.v1":
        raise ValueError("unsupported adapter descriptor version")
    adapters = value.get("adapters")
    if not isinstance(adapters, list) or not adapters:
        raise ValueError("adapter descriptors must be non-empty")
    return {item["id"]: item for item in adapters}


def command_for(adapter: str, descriptor: dict, binary: str | None) -> list[str]:
    if adapter == "reference":
        return [sys.executable, str(TESTS_ROOT / "scripts" / "reference_adapter.py")]
    env_name = descriptor.get("binary_env")
    selected = binary or (os.environ.get(env_name) if env_name else None) or descriptor.get("binary")
    if not selected:
        raise ValueError(f"--{adapter}-bin or descriptor binary is required")
    return [selected]


def run_one(adapter: str, descriptor: dict, binary: str | None, reports: Path) -> int:
    command = command_for(adapter, descriptor, binary)
    namespace = descriptor.get("namespace", "reference")
    if adapter == "reference":
        return subprocess.call(command + ["--suite", "all"], cwd=TESTS_ROOT)
    conformance = os.environ.get(
        "ZEROSTACK_CONFORMANCE_BIN", "zerostack-shared-conformance"
    )
    probe = [
        conformance,
        "--ns",
        namespace,
        "--bin",
        str(Path(command[0]).resolve()),
        "--surface",
        descriptor.get("surface", "codemode"),
        "--reports-dir",
        str(reports / adapter),
    ]
    print("$", " ".join(probe))
    return subprocess.call(probe, cwd=REPO_ROOT)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("adapter", nargs="?", help="adapter id, or omit with --all")
    parser.add_argument("--all", action="store_true", help="run every configured engine and reference adapter")
    parser.add_argument("--reports-dir", type=Path, default=TESTS_ROOT / "reports")
    parser.add_argument("--fszero-bin")
    parser.add_argument("--graphzero-bin")
    parser.add_argument("--tokenzero-bin")
    parser.add_argument("--reference", action="store_true", help="run only the non-Pi reference adapter")
    args = parser.parse_args(argv)
    descriptors = load_descriptors()
    paths = {
        "fszero": args.fszero_bin,
        "graphzero": args.graphzero_bin,
        "tokenzero": args.tokenzero_bin,
    }
    if args.all:
        selected = ["fszero", "graphzero", "tokenzero", "reference"]
    elif args.reference:
        selected = ["reference"]
    elif args.adapter:
        selected = [args.adapter]
    else:
        parser.error("choose an adapter or --all")
    unknown = [item for item in selected if item not in descriptors]
    if unknown:
        parser.error(f"unknown adapter(s): {', '.join(unknown)}")
    missing = [
        item
        for item in selected
        if item != "reference"
        and not paths.get(item)
        and not descriptors[item].get("binary")
        and not (descriptors[item].get("binary_env") and os.environ.get(descriptors[item]["binary_env"]))
    ]
    if missing:
        parser.error(f"missing explicit binary for: {', '.join(missing)}")
    status = 0
    for adapter in selected:
        status = max(status, run_one(adapter, descriptors[adapter], paths.get(adapter), args.reports_dir))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
