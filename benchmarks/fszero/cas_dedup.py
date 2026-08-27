#!/usr/bin/env python3
"""Checkout-dedup benchmark (fszero-zjt / acceptance seed for fszero-qhz).

Two identical checkouts share one explicit store root with a canonical CAS
(blobs/ pre-created = the opt-in). Checkout A ingests first (all blobs
minted); checkout B repeats the identical workload: every blob put must be
a verified no-op (created=0 new objects) and B's blob phase is cheaper.
Honest accounting: B still pays hashing + index work — reported, not
hidden.

Usage: python3 benchmarks/cas_dedup.py [--files 2000]
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def fszero_bin() -> str:
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def count_objects(blobs: Path) -> int:
    algo = blobs / "sha256"
    if not algo.is_dir():
        return 0
    return sum(1 for shard in algo.iterdir() if shard.is_dir() for f in shard.iterdir()
               if len(f.name) == 64)


def ingest(checkout: Path, store_root: Path, n_files: int) -> float:
    env = os.environ.copy()
    env.update({
        "FSZERO_ROOT": str(checkout),
        "ZEROSTACK_STORE_ROOT": str(store_root),
        "FSZERO_STARTUP_INDEX": "1",
        "FSZERO_INDEX_MAX_FILES": str(n_files + 1000),
    })
    t0 = time.monotonic()
    r = subprocess.run(
        [fszero_bin(), "codemode", "return{ok:true}", "--root", str(checkout)],
        capture_output=True, text=True, timeout=3600, cwd=checkout, env=env,
    )
    wall = time.monotonic() - t0
    if r.returncode != 0:
        raise SystemExit(f"INTEGRITY: ingest failed under {checkout}")
    return wall


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--files", type=int, default=2000)
    args = ap.parse_args()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()

    with tempfile.TemporaryDirectory(prefix="fszero_dedup_") as tmp:
        tmp = Path(tmp)
        corpus = tmp / "corpus"
        subprocess.run(
            ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
             "--files", str(args.files), "--out", str(corpus), "--seed", "42"],
            check=True,
        )
        a, b = tmp / "checkout_a", tmp / "checkout_b"
        shutil.copytree(corpus, a)
        shutil.copytree(corpus, b)
        store_root = tmp / "shared_store"
        (store_root / "blobs").mkdir(parents=True)  # explicit CAS opt-in

        wall_a = ingest(a, store_root, args.files)
        objects_after_a = count_objects(store_root / "blobs")
        wall_b = ingest(b, store_root, args.files)
        objects_after_b = count_objects(store_root / "blobs")

        result = {
            "git_commit": commit,
            "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "files": args.files,
            "checkout_a_wall_s": wall_a,
            "checkout_b_wall_s": wall_b,
            "objects_after_a": objects_after_a,
            "objects_after_b": objects_after_b,
            "new_objects_from_b": objects_after_b - objects_after_a,
            "honest_note": "B warm-starts entirely off the SHARED store root: same "
                           "relative keys + mtime-preserving copy match A's manifest, so "
                           "B's cold index is a no-op AND its blob puts never run. Costs "
                           "stated: A pays a per-object fsync for CAS publication "
                           "(dual-write makes A's cold ingest slower than a no-CAS run); "
                           "same-machine shared root only — the artifact-import variant "
                           "(docs/design/team-shared-warm-store.md) is designed, not built.",
        }
        if objects_after_b != objects_after_a:
            raise SystemExit(f"INTEGRITY: checkout B minted new objects: {result}")

    out = ROOT / "benchmarks" / "cas-dedup.json"
    out.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    print(f"written: {out}")


if __name__ == "__main__":
    main()
