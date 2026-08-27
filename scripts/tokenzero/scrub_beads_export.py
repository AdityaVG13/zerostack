#!/usr/bin/env python3
"""Delegate TokenZero Beads scrubbing to canonical ZeroStack policy."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from zerostack_testkit_adapter import ENGINE_ROOT, load_hub_script

_hub = load_hub_script("scrub_beads_export.py", "_tokenzero_hub_beads_scrub")
HOME_PATH = _hub.HOME_PATH
WIN_HOME_PATH = _hub.WIN_HOME_PATH


def relativize(text: str) -> str:
    """Apply the corrected Windows-first order pending the next hub revision."""
    return HOME_PATH.sub("~", WIN_HOME_PATH.sub("~", text))


_hub.relativize = relativize
scrub_value = _hub.scrub_value
scrub_record = _hub.scrub_record
scrub_file = _hub.scrub_file


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    paths = args.paths or [ENGINE_ROOT / ".beads" / "issues.jsonl"]
    status = 0
    for path in paths:
        status |= scrub_file(path, check_only=args.check)
    return status


if __name__ == "__main__":
    raise SystemExit(main())
