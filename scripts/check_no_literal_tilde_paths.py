#!/usr/bin/env python3
"""Reject repository paths containing a literal ``~`` component."""

from __future__ import annotations

import sys
from pathlib import Path

EXCLUDED_DIRS: frozenset[str] = frozenset({".git", ".mypy_cache", ".nox", ".pytest_cache", ".ruff_cache", ".tox", ".venv", "__pycache__", "node_modules", "target", "venv"})

def literal_tilde_paths(root: Path) -> list[Path]:
    """Return offending paths without traversing symlinks or tool caches."""
    offenders: list[Path] = []
    pending: list[Path] = [root]
    while pending:
        directory = pending.pop()
        try:
            children = sorted(directory.iterdir(), key=lambda child: child.name)
        except OSError as error:
            raise OSError(f"cannot scan {directory}: {error}") from error
        for child in children:
            if child.name == "~":
                offenders.append(child.relative_to(root))
                continue
            if child.name in EXCLUDED_DIRS or child.is_symlink():
                continue
            if child.is_dir():
                pending.append(child)
    return sorted(offenders, key=lambda path: path.as_posix())

def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    root = Path(args[0]).resolve() if args else Path(__file__).resolve().parent.parent
    if not root.is_dir():
        print(f"error: repository root is not a directory: {root}", file=sys.stderr)
        return 2
    try:
        offenders = literal_tilde_paths(root)
    except OSError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if offenders:
        print("literal '~' path components are forbidden:", file=sys.stderr)
        for path in offenders:
            print(f"  {path.as_posix()}", file=sys.stderr)
        return 1
    print("No literal '~' path components found.")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
