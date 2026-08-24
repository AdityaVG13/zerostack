#!/usr/bin/env python3
"""Verify the canonical public ZeroKernel documentation and benchmark artifact."""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {"read", "find", "edit", "apply", "run", "state"}
FORBIDDEN = ("zsx", "zero_execute", "zero_wait", "zero.fs.", "zero.graph.", "zero.token.", "ZMP")


def fail(message: str) -> None:
    raise SystemExit(f"public-surface audit failed: {message}")


def main() -> None:
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    specification = (ROOT / "docs/zero-kernel.md").read_text(encoding="utf-8")
    public_text = readme + "\n" + specification

    found = set(re.findall(r"`z\.(read|find|edit|apply|run|state)`", public_text))
    if found != EXPECTED:
        fail(f"expected six operations {sorted(EXPECTED)}, found {sorted(found)}")

    for token in FORBIDDEN:
        if token in public_text:
            fail(f"retired or private token remains: {token}")

    artifact_path = ROOT / "benchmarks/zero-kernel-reference.json"
    try:
        artifact = json.loads(artifact_path.read_text(encoding="utf-8"))  # ubs:ignore — JSONDecodeError is handled below.
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read benchmark artifact: {error}")
    if artifact.get("schema") != "zerokernel.reference-benchmark.v1":
        fail("unexpected benchmark schema")
    method = artifact.get("method", {})
    if method.get("runs_per_operation", 0) < 20:
        fail("benchmark sample floor is below 20")
    if method.get("dropped_samples") != 0:
        fail("benchmark artifact dropped samples")

    print("OK: canonical six-operation public surface and benchmark artifact")


if __name__ == "__main__":
    main()
