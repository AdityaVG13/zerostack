#!/usr/bin/env python3
"""Print Rust files over an LOC threshold (advisory budget helper).

Read-only: walks the repo for *.rs, skips target/ and .git/, prints offenders.
Does not compile anything or invoke cargo.
"""

from __future__ import annotations

import argparse
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--threshold",
        type=int,
        default=500,
        help="Report files with more than this many lines (default: 500)",
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="Repository root (default: parent of scripts/)",
    )
    args = parser.parse_args()
    root: Path = args.root.resolve()
    skip = {"target", ".git"}
    rows: list[tuple[int, str, str]] = []
    for path in root.rglob("*.rs"):
        if any(part in skip for part in path.parts):
            continue
        try:
            with path.open(errors="ignore") as fh:
                n = sum(1 for _ in fh)
        except OSError:
            continue
        if n <= args.threshold:
            continue
        rel = path.relative_to(root)
        if rel.parts and rel.parts[0] == "crates" and len(rel.parts) > 1:
            owner = rel.parts[1]
        elif rel.parts and rel.parts[0] == "tests":
            owner = "tests"
        else:
            owner = "other"
        rows.append((n, owner, str(rel)))
    rows.sort(reverse=True)
    print(f"threshold={args.threshold} offenders={len(rows)} root={root}")
    for n, owner, rel in rows:
        print(f"{n:5d}  {owner:28s}  {rel}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
