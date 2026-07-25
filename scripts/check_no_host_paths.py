#!/usr/bin/env python3
"""Fail if a tracked file gains an absolute host path.

The hub is public, and the other three repos are converging on the same
gate (zerostack-9g4). Host paths in committed artifacts are cosmetic
leakage, not a functional defect, but they are noise in a public repo and
make evidence artifacts needlessly machine-specific.

Allowlist:
  AGENTS.md, CLAUDE.md  - the privacy-check pattern itself names /Users/
                          and /home/ as the strings it scans for.

The beads exports are NO LONGER allowlisted. br stamps source_repo_path with
an absolute path and has no config knob to stop it, so scripts/scrub_beads_export.py
rewrites it before the export is staged (zerostack-sg3). Blanket-allowlisting
the file meant the gate could not see a real leak in a bead description either,
which is the more sensitive content of the two.

Run: python3 scripts/check_no_host_paths.py
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Absolute host path shapes. Windows C:\Users\ is covered by the Users
# pattern after forward-slash normalization in json.
HOST_PATH = re.compile(r'/Users/[A-Za-z]|/home/[A-Za-z]|C:[\\/]Users[\\/]')

# file:reason pairs that are legitimate and must not be flagged.
ALLOWLIST: dict[str, str] = {
    "AGENTS.md": "names /Users/ and /home/ as the strings the privacy check scans for",
    "CLAUDE.md": "same privacy-check pattern as AGENTS.md",
    "scripts/check_no_host_paths.py": "defines the host-path pattern it scans for",
    "scripts/scrub_beads_export.py": "documents the host-path shapes it rewrites",
}


def tracked_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files"], cwd=REPO, capture_output=True, text=True, check=True
    )
    return [line for line in out.stdout.splitlines() if line.strip()]


def main() -> int:
    offenders: list[str] = []
    for rel in tracked_files():
        reason = ALLOWLIST.get(rel)
        path = REPO / rel
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if not HOST_PATH.search(text):
            continue
        if reason is not None:
            continue
        for i, line in enumerate(text.splitlines(), 1):
            if HOST_PATH.search(line):
                offenders.append(f"{rel}:{i}: {line.strip()[:120]}")
                break

    if offenders:
        print("host paths found in tracked files:", file=sys.stderr)
        for o in offenders:
            print(f"  {o}", file=sys.stderr)
        print(
            "\nIf a path is legitimate, add the file to ALLOWLIST in "
            "scripts/check_no_host_paths.py with a reason.",
            file=sys.stderr,
        )
        return 1

    count = len(tracked_files())
    print(f"no host paths in {count} tracked file(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
