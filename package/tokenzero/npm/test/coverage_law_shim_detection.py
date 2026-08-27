#!/usr/bin/env python3
"""Narrow coverage-law regression for npm PATH shim detection (tokenzero-g3y.16).

Baseline suite must pass. Dropping ``looksLikeNpmShimInvocation`` must fail —
that branch keeps PATH from launching npm-generated wrappers while still
allowing distinct TOKENZERO_BIN / PATH binaries that merely mention the shim
path (P09-001 / CE-P12-01).
"""
from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SRC = ROOT / "package/npm"
NEEDLE = "return looksLikeNpmShimInvocation(normalizedText, normalizedSelf);"
MUTANT = "return false; // mutation: drop PATH npm-shim detection"


def main() -> int:
    with tempfile.TemporaryDirectory() as td:
        dest = Path(td) / "npm"
        shutil.copytree(SRC, dest)
        test = dest / "test/tokenzero-shim.test.js"
        shim = dest / "bin/tokenzero.js"

        baseline = subprocess.run(
            ["node", "--test", str(test)],
            capture_output=True,
            text=True,
        )
        text = shim.read_text(encoding="utf-8")
        if NEEDLE not in text:
            print(
                json.dumps(
                    {
                        "ok": False,
                        "error": "mutation needle missing",
                        "needle": NEEDLE,
                        "baseline_exit": baseline.returncode,
                    },
                    indent=2,
                )
            )
            return 1

        shim.write_text(text.replace(NEEDLE, MUTANT, 1), encoding="utf-8")
        mutant = subprocess.run(
            ["node", "--test", str(test)],
            capture_output=True,
            text=True,
        )
        mutation_survived = mutant.returncode == 0
        ok = baseline.returncode == 0 and not mutation_survived
        result = {
            "law": "tests must kill removal of PATH npm-shim detection",
            "visited_unit": "package/npm/test",
            "needle": NEEDLE,
            "baseline_exit": baseline.returncode,
            "mutant_exit": mutant.returncode,
            "mutation_survived": mutation_survived,
            "ok": ok,
            "baseline_tail": baseline.stdout[-600:],
            "mutant_tail": mutant.stdout[-600:],
        }
        print(json.dumps(result, indent=2))
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
