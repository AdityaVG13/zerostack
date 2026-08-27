#!/usr/bin/env python3
"""Apply canonical host-path policy to the TokenZero checkout."""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from zerostack_testkit_adapter import ENGINE_ROOT, load_hub_script

_hub = load_hub_script("check_no_host_paths.py", "_tokenzero_hub_host_paths")
REPO = ENGINE_ROOT
_hub.REPO = REPO

HOST_PATH = _hub.HOST_PATH
ALLOWLIST_FILES = _hub.ALLOWLIST_FILES
ALLOWLIST_LINE_RES = _hub.ALLOWLIST_LINE_RES
ALLOWLIST_LINE_RES.update(
    {
        "tests/tokenzero/install/unit/package_audit/tar.rs": [
            re.compile(r"(?:C:/Users/example|/home/example)/"),
        ],
        "crates/tokenzero/tokenzero-recovery/benches/perf_hotspots/baseline-shell.sample.txt": [
            re.compile(r"/Users/(?:USER|\*)/"),
        ],
        "crates/tokenzero/tokenzero-recovery/src/embedded_store_tests.rs": [
            re.compile(r"C:\\Users\\x\\proj"),
        ],
        "docs/tokenzero/benchmarks.md": [
            re.compile(r"${TOKENZERO_ROOT}/target/release/tokenzero"),
        ],
        "docs/tokenzero/install.md": [
            re.compile(r"/Users/you/AI/tokenzero/target/release/tokenzero"),
        ],
    }
)
tracked_files = _hub.tracked_files
line_allowlisted = _hub.line_allowlisted
first_offender = _hub.first_offender
main = _hub.main


if __name__ == "__main__":
    raise SystemExit(main())
