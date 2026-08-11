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


def git_head(repo: Path) -> str:
    """Resolve the exact current HEAD of a repository (40..=64 lowercase hex)."""
    result = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"cannot resolve git HEAD for {repo}: {result.stderr.strip()}")
    head = result.stdout.strip()
    if len(head) < 40 or len(head) > 64 or not all(c in "0123456789abcdef" for c in head):
        raise ValueError(f"invalid git HEAD {head!r} for {repo}")
    return head


def run_one(
    adapter: str,
    descriptor: dict,
    binary: str | None,
    reports: Path,
    source_head: str | None,
    hub_head: str | None,
) -> int:
    command = command_for(adapter, descriptor, binary)
    namespace = descriptor.get("namespace", "reference")
    if adapter == "reference":
        return subprocess.call(command + ["--suite", "all"], cwd=TESTS_ROOT)
    # Production receipts are immutable: bind the checked engine repository,
    # not the hub checkout that happens to run this script.
    if source_head is None:
        repo_env = descriptor.get("source_repo_env")
        source_repo = os.environ.get(repo_env) if repo_env else None
        if not source_repo:
            raise ValueError(
                f"--{adapter}-source-head is required when {repo_env or 'source_repo_env'} is unset"
            )
        source_head = git_head(Path(source_repo))
    if hub_head is None:
        hub_head = git_head(REPO_ROOT)
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
        "--source-head",
        source_head,
        "--hub-head",
        hub_head,
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
    parser.add_argument(
        "--source-head",
        help="exact source head for one selected engine; forbidden with multi-engine --all",
    )
    parser.add_argument("--fszero-source-head", help="exact FSZero repository head")
    parser.add_argument("--graphzero-source-head", help="exact GraphZero repository head")
    parser.add_argument("--tokenzero-source-head", help="exact TokenZero repository head")
    parser.add_argument(
        "--hub-head",
        help="current hub repository head (default: git HEAD of the shared-suite repo)",
    )
    parser.add_argument("--reference", action="store_true", help="run only the non-Pi reference adapter")
    args = parser.parse_args(argv)
    descriptors = load_descriptors()
    paths = {
        "fszero": args.fszero_bin,
        "graphzero": args.graphzero_bin,
        "tokenzero": args.tokenzero_bin,
    }
    source_heads = {
        "fszero": args.fszero_source_head,
        "graphzero": args.graphzero_source_head,
        "tokenzero": args.tokenzero_source_head,
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
    selected_engines = [item for item in selected if item != "reference"]
    if args.source_head and len(selected_engines) > 1:
        parser.error("--source-head is ambiguous with multiple engines; pass each --*-source-head")
    status = 0
    for adapter in selected:
        status = max(
            status,
            run_one(
                adapter,
                descriptors[adapter],
                paths.get(adapter),
                args.reports_dir,
                source_heads.get(adapter) or args.source_head,
                args.hub_head,
            ),
        )
    return status


if __name__ == "__main__":
    raise SystemExit(main())
