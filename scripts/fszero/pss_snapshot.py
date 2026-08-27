#!/usr/bin/env python3
"""Snapshot process memory: peak RSS and PSS (proportional set size) when available.

Linux: reads /proc/<pid>/smaps_rollup (Pss) and /proc/<pid>/status (VmRSS).
macOS/Darwin: RSS via `ps`; PSS is not available as a portable single field --
returns pss_bytes=null with an explicit reason (do not invent fake PSS from RSS).

Usage:
  uv run python scripts/pss_snapshot.py --pid $$
  uv run python scripts/pss_snapshot.py --pid 1234 --json

Gate policy: never silently substitute RSS for PSS in published claims.
See docs/profiling.md (RSS vs PSS).
"""
from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys
from pathlib import Path


def linux_snapshot(pid: int) -> dict[str, object]:
    status_path = Path(f"/proc/{pid}/status")
    rollup_path = Path(f"/proc/{pid}/smaps_rollup")
    rss_bytes = None
    rss_reason = None
    if status_path.is_file():
        for line in status_path.read_text(errors="replace").splitlines():
            if line.startswith("VmRSS:"):
                # VmRSS:   12345 kB
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    rss_bytes = int(parts[1]) * 1024
                break
        if rss_bytes is None:
            rss_reason = "VmRSS_missing"
    else:
        rss_reason = "proc_status_missing"

    pss_bytes = None
    pss_reason = None
    if rollup_path.is_file():
        for line in rollup_path.read_text(errors="replace").splitlines():
            if line.startswith("Pss:"):
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    # smaps_rollup Pss is in kB
                    pss_bytes = int(parts[1]) * 1024
                break
        if pss_bytes is None:
            pss_reason = "Pss_field_missing"
    else:
        pss_reason = "smaps_rollup_unavailable"

    return {
        "pid": pid,
        "platform": "Linux",
        "rss_bytes": rss_bytes,
        "rss_status": "available" if rss_bytes is not None else "unavailable",
        "rss_reason": rss_reason,
        "pss_bytes": pss_bytes,
        "pss_status": "available" if pss_bytes is not None else "unavailable",
        "pss_reason": pss_reason,
        "source": {
            "rss": f"/proc/{pid}/status:VmRSS",
            "pss": f"/proc/{pid}/smaps_rollup:Pss",
        },
    }


def darwin_snapshot(pid: int) -> dict[str, object]:
    rss_bytes = None
    rss_reason = None
    try:
        run = subprocess.run(
            ["ps", "-o", "rss=", "-p", str(pid)],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        if run.returncode == 0 and run.stdout.strip().isdigit():
            # ps rss is kilobytes
            rss_bytes = int(run.stdout.strip()) * 1024
        else:
            rss_reason = f"ps_exit_{run.returncode}" if run.returncode else "ps_empty"
    except FileNotFoundError:
        rss_reason = "ps_not_found"
    except subprocess.TimeoutExpired:
        rss_reason = "ps_timeout"

    return {
        "pid": pid,
        "platform": "Darwin",
        "rss_bytes": rss_bytes,
        "rss_status": "available" if rss_bytes is not None else "unavailable",
        "rss_reason": rss_reason,
        # Honest null: macOS has phys_footprint / footprint via task_info and
        # Instruments, but no portable PSS equivalent of smaps_rollup.
        "pss_bytes": None,
        "pss_status": "unavailable",
        "pss_reason": "darwin_no_smaps_pss; use Instruments Allocations / phys_footprint (see docs/profiling.md)",
        "source": {
            "rss": "ps -o rss=",
            "pss": None,
        },
    }


def snapshot(pid: int) -> dict[str, object]:
    system = platform.system()
    if system == "Linux":
        return linux_snapshot(pid)
    if system == "Darwin":
        return darwin_snapshot(pid)
    return {
        "pid": pid,
        "platform": system,
        "rss_bytes": None,
        "rss_status": "unavailable",
        "rss_reason": f"unsupported_platform:{system}",
        "pss_bytes": None,
        "pss_status": "unavailable",
        "pss_reason": f"unsupported_platform:{system}",
        "source": {"rss": None, "pss": None},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True, help="target process id")
    parser.add_argument("--json", action="store_true", help="print JSON (default)")
    args = parser.parse_args()
    doc = snapshot(args.pid)
    json.dump(doc, fp=sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    if doc.get("rss_status") == "unavailable" and doc.get("pss_status") == "unavailable":
        sys.exit(2)


if __name__ == "__main__":
    main()
