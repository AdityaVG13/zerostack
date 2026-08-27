#!/usr/bin/env python3
"""Measure open-time WAL compaction on the GraphZero self-repo through RCH."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["PATH"] = f"/opt/homebrew/bin:{env['PATH']}"
    env["RCH_VISIBILITY"] = "verbose"
    command = [
        "rch",
        "exec",
        "--",
        "cargo",
        "run",
        "--release",
        "--quiet",
        "-p",
        "graphzero-store",
        "--example",
        "wal_open_compaction_bench",
        "--",
        str(root),
    ]
    completed = subprocess.run(
        command,
        cwd=root,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    transcript = f"{completed.stdout}\n{completed.stderr}"
    if completed.returncode != 0:
        print(transcript, file=sys.stderr)
        return completed.returncode
    if "[RCH] remote" not in transcript or "spark" not in transcript:
        print("benchmark refused: build did not prove remote Spark execution", file=sys.stderr)
        print(transcript, file=sys.stderr)
        return 1

    prefix = "WAL_OPEN_COMPACTION_RESULT="
    result_line = next(
        (line for line in transcript.splitlines() if line.startswith(prefix)),
        None,
    )
    if result_line is None:
        print("benchmark refused: JSON result missing", file=sys.stderr)
        return 1
    result = json.loads(result_line.removeprefix(prefix))
    # The command is recorded into a committed artifact; the repo argument is
    # the host checkout path, so record it repo-relative.
    result["command"] = " ".join("." if arg == str(root) else arg for arg in command)
    result["rch_proof"] = next(
        line for line in transcript.splitlines() if "[RCH] remote" in line
    )
    output = root / "bench" / "wal_open_compaction.json"
    output.write_text(json.dumps(result, indent=2) + "\n")
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
