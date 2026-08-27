#!/usr/bin/env python3
"""Apply canonical host-path policy to the FSZero checkout."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from zerostack_testkit_adapter import ENGINE_ROOT, load_hub_script

_hub = load_hub_script("check_no_host_paths.py", "_fszero_hub_host_paths")
REPO = ENGINE_ROOT
_hub.REPO = REPO

HOST_PATH = _hub.HOST_PATH
ALLOWLIST_FILES = _hub.ALLOWLIST_FILES
ALLOWLIST_LINE_RES = _hub.ALLOWLIST_LINE_RES
SKIP_PREFIXES = (".beads/backup/",)
tracked_files = _hub.tracked_files
line_allowlisted = _hub.line_allowlisted
_hub_first_offender = _hub.first_offender


def first_offender(rel: str, repo: Path | None = None) -> str | None:
    """Apply FSZero's opaque Beads-backup exclusion before shared policy."""
    if any(rel == prefix.rstrip("/") or rel.startswith(prefix) for prefix in SKIP_PREFIXES):
        return None
    return _hub_first_offender(rel, repo)


_hub.first_offender = first_offender
main = _hub.main


if __name__ == "__main__":
    raise SystemExit(main())
