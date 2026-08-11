#!/usr/bin/env python3
"""Fail if a tracked file gains an absolute host path.

The hub is public, and the other three repos are converging on the same
gate (zerostack-9g4). Host paths in committed artifacts are cosmetic
leakage, not a functional defect, but they are noise in a public repo and
make evidence artifacts needlessly machine-specific.

Allowlist policy (line-scoped for docs):

- Whole-file allowlist ONLY for scripts that *define* the host-path patterns
  they scan/rewrite (this file and scrub_beads_export.py).
- AGENTS.md is NOT whole-file allowlisted. Only lines that document the
  privacy-check recipe (the rg pattern strings themselves) are skipped.
  A real `/Users/<name>/...` path elsewhere in AGENTS.md fails the gate.
- CLAUDE.md is not allowlisted at all.

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

DEFAULT_REPO = Path(__file__).resolve().parent.parent
# Compatibility hook for thin engine adapters that override the imported
# module's repository root before calling main().
REPO = DEFAULT_REPO

# Absolute host path shapes. Windows C:\Users\ is covered by the Users
# pattern after forward-slash normalization in json.
HOST_PATH = re.compile(r"/Users/[A-Za-z]|/home/[A-Za-z]|C:[\\/]Users[\\/]")

# Whole-file allowlist: only files that define the scan/rewrite patterns.
ALLOWLIST_FILES: dict[str, str] = {
    "scripts/check_no_host_paths.py": "defines the host-path pattern it scans for",
    "scripts/scrub_beads_export.py": "documents the host-path shapes it rewrites",
}

# Line-scoped allowlist: host-path lines are OK only when they match a doc pattern.
# Real username paths (e.g. ${HOME}/...) must not match these.
ALLOWLIST_LINE_RES: dict[str, list[re.Pattern[str]]] = {
    "AGENTS.md": [
        # Privacy check recipe listing scan strings (not a personal path).
        re.compile(r"rg -n ['\"].*/Users/\|/home/\|"),
        # Prose that names the scan strings without a username segment.
        re.compile(r"/Users/\|/home/\|BEGIN"),
        re.compile(r"names /Users/ and /home/ as the strings"),
    ],
    "crates/zero-codemode/tests/locate_manifest.rs": [
        re.compile(
            r'^\s*let ephemeral = "/home/u/\.local/state/fnm_multishells/1347_1785364489620/bin";$'
        ),
    ],
    "crates/zero-codemode/tests/node_resolution.rs": [
        re.compile(
            r'^\s*const MULTISHELL: &str = "/home/u/\.local/state/fnm_multishells/1347_1785364489620/bin/node";$'
        ),
        re.compile(r'^\s*Some\("/home/u"\),$'),
        re.compile(r'^\s*let env = env_with\(.*Some\("/home/u"\).*$'),
        re.compile(
            r'^\s*"/home/u/(?:\.local/share/fnm/aliases/default/bin/node|\.volta/bin/node|\.local/bin/node)",$'
        ),
        re.compile(
            r'^\s*let multishell_dir = "/home/u/\.local/state/fnm_multishells/1347_1785364489620/bin";$'
        ),
        re.compile(
            r'^\s*"/home/u/\.local/share/fnm/node-versions/v24\.14\.1/installation/bin/node"$'
        ),
    ],
}


def tracked_files(repo: Path | None = None) -> list[str]:
    repo = REPO if repo is None else repo
    out = subprocess.run(
        ["git", "ls-files"], cwd=repo, capture_output=True, text=True, check=True
    )
    return [line for line in out.stdout.splitlines() if line.strip()]


def line_allowlisted(rel: str, line: str) -> bool:
    patterns = ALLOWLIST_LINE_RES.get(rel)
    if not patterns:
        return False
    return any(p.search(line) for p in patterns)


def first_offender(rel: str, repo: Path | None = None) -> str | None:
    """Return first host-path hit for a tracked path, or None if clean."""
    repo = REPO if repo is None else repo
    path = repo / rel
    if not path.is_file():
        return None
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    if not HOST_PATH.search(text) or rel in ALLOWLIST_FILES:
        return None
    for i, line in enumerate(text.splitlines(), 1):
        if HOST_PATH.search(line) and not line_allowlisted(rel, line):
            return f"{rel}:{i}: {line.strip()[:120]}"
    return None


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) > 1:
        print("usage: check_no_host_paths.py [repository-root]", file=sys.stderr)
        return 2
    repo = Path(args[0]).resolve() if args else REPO
    if not (repo / ".git").exists():
        print(f"repository root is not a Git checkout: {repo}", file=sys.stderr)
        return 2
    files = tracked_files(repo)
    offenders = [hit for rel in files if (hit := first_offender(rel, repo))]
    if not offenders:
        print(f"no host paths in {len(files)} tracked file(s)")
        return 0
    print("host paths found in tracked files:", file=sys.stderr)
    for o in offenders:
        print(f"  {o}", file=sys.stderr)
    print(
        "\nIf a path is legitimate documentation of the scan pattern, add a "
        "line regex under ALLOWLIST_LINE_RES in scripts/check_no_host_paths.py. "
        "Do not whole-file allowlist agent docs.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
