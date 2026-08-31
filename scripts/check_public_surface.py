#!/usr/bin/env python3
"""Verify the canonical public ZeroKernel documentation."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {"read", "find", "edit", "apply", "run", "state"}
PUBLIC_FILES = (
    "README.md",
    "docs/README.md",
    "docs/architecture.md",
    "docs/components.md",
    "docs/build.md",
    "contracts/README.md",
    "docs/files/architecture.md",
    "docs/files/filesystem-contract.md",
    "docs/structure/architecture.md",
    "docs/structure/query-contract.md",
    "docs/tokens/development.md",
)
FORBIDDEN = (
    "zsx",
    "zero_execute",
    "zero_wait",
    "zero.fs.",
    "zero.graph.",
    "zero.token.",
    "ZMP",
    "fszero-engine",
    "fszero-test-support",
    "graphzero-pack",
    "graphzero-scip",
    "graphzero-semantic",
    "graphzero-test-support",
    "graphzero-why",
    "tokenzero-engine",
    "tokenzero-filters",
    "tokenzero-recovery",
    "tokenzero-runtime",
    "tokenzero-test-support",
    "operation-abi-schemas.json",
    "contracts/filesystem.json",
)


def fail(message: str) -> None:
    raise SystemExit(f"public-surface audit failed: {message}")


def main() -> None:
    surface_text = "\n".join(
        (ROOT / path).read_text(encoding="utf-8")
        for path in ("README.md", "docs/architecture.md")
    )
    public_text = "\n".join(
        (ROOT / path).read_text(encoding="utf-8") for path in PUBLIC_FILES
    )

    found = set(re.findall(r"`z\.(read|find|edit|apply|run|state)`", surface_text))
    if found != EXPECTED:
        fail(f"expected six operations {sorted(EXPECTED)}, found {sorted(found)}")

    for token in FORBIDDEN:
        if token in public_text:
            fail(f"retired or private token remains: {token}")

    print("OK: canonical six-operation public surface")


if __name__ == "__main__":
    main()
