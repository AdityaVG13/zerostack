#!/usr/bin/env python3
"""Delegate FSZero literal-tilde path checks to canonical ZeroStack policy."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from zerostack_testkit_adapter import ENGINE_ROOT, load_hub_script

_hub = load_hub_script("check_no_literal_tilde_paths.py", "_fszero_hub_tilde_paths")
EXCLUDED_DIRS = _hub.EXCLUDED_DIRS
literal_tilde_paths = _hub.literal_tilde_paths


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    return _hub.main(args or [str(ENGINE_ROOT)])


if __name__ == "__main__":
    raise SystemExit(main())
