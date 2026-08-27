#!/usr/bin/env python3
"""Run multi-size blast scaling curve (graphzero-hkexf) and write benchmarks/latency/scaling_curve.json.

Prefers RCH when available so heavy index work stays off the laptop:
  python3 benchmarks/latency/run_scaling_curve.py
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env.setdefault("CARGO_TARGET_DIR", "/tmp/rch_target_graphzero")
    cargo = [
        "cargo",
        "run",
        "--release",
        "--quiet",
        "-p",
        "graphzero-store",
        "--example",
        "scaling_curve_bench",
    ]
    if shutil.which("rch"):
        env["PATH"] = f"/opt/homebrew/bin:{env.get('PATH', '')}"
        command = ["rch", "exec", "--", *cargo]
    else:
        command = cargo

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

    prefix = "SCALING_CURVE_RESULT="
    result_line = next(
        (line for line in transcript.splitlines() if line.startswith(prefix)),
        None,
    )
    if result_line is None:
        print("scaling curve refused: JSON result missing", file=sys.stderr)
        print(transcript, file=sys.stderr)
        return 1

    result = json.loads(result_line.removeprefix(prefix))
    result["command"] = " ".join(command)
    if "[RCH] remote" in transcript:
        result["rch_proof"] = next(
            line for line in transcript.splitlines() if "[RCH] remote" in line
        )

    # Sanity: every size has p95_ratio, N=1 ratio ~ 1.
    points = result.get("points") or []
    if not points:
        print("scaling curve refused: empty points", file=sys.stderr)
        return 1
    n1 = next(p for p in points if p["files"] == 1)
    ratio = float(n1["blast"]["p95_ratio_vs_n1"])
    if abs(ratio - 1.0) > 1e-6:
        print(f"scaling curve refused: N=1 p95_ratio expected 1.0 got {ratio}", file=sys.stderr)
        return 1

    output = root / "bench" / "scaling_curve.json"
    output.write_text(json.dumps(result, indent=2) + "\n")
    print(output)
    for p in points:
        b = p["blast"]
        print(
            f"n={p['files']:>4}  p50={b['p50_ms']:.3f}ms  "
            f"p95={b['p95_ms']:.3f}ms  ratio={b['p95_ratio_vs_n1']:.3f}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
