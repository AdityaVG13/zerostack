#!/usr/bin/env python3
"""Fail CI when external GitHub Actions are not pinned to full commit SHAs."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOTS = (Path(".github/workflows"), Path(".github/actions"))
SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
USES_RE = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)")


def is_external_action(ref: str) -> bool:
    return not (
        ref.startswith("./")
        or ref.startswith("../")
        or ref.startswith("docker://")
        or ref.startswith(".github/")
    )


def check_file(path: Path) -> list[str]:
    errors: list[str] = []
    for line_no, line in enumerate(path.read_text().splitlines(), 1):
        match = USES_RE.match(line)
        if not match:
            continue
        ref = match.group(1).strip('"\'')
        if not is_external_action(ref):
            continue
        if "@" not in ref:
            errors.append(f"{path}:{line_no}: external action lacks @sha: {ref}")
            continue
        _, version = ref.rsplit("@", 1)
        if not SHA_RE.fullmatch(version):
            errors.append(f"{path}:{line_no}: external action must use full 40-char SHA, not {version!r}: {ref}")
    return errors


def main() -> int:
    files = sorted(
        p
        for root in ROOTS
        if root.exists()
        for p in root.rglob("*")
        if p.suffix in {".yml", ".yaml"}
    )
    errors = [err for path in files for err in check_file(path)]
    if errors:
        print("Unpinned external GitHub Actions detected:", file=sys.stderr)
        for err in errors:
            print(f"  {err}", file=sys.stderr)
        print("Pin external actions to immutable 40-character commit SHAs. Local ./ actions are allowed.", file=sys.stderr)
        return 1
    print(f"checked {len(files)} workflow/action files; all external actions are SHA-pinned")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
