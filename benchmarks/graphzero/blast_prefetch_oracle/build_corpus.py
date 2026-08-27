#!/usr/bin/env python3
"""Capture the blast-as-prefetch replay corpus from the GraphZero self-repo.

Each replay event is one real commit that changed both source and test files.
Faults are the test files the commit had to touch; graph candidates are the
blast-radius break sites captured from the live `graphzero blast` index and
resolved to the test files that exercise them.

The capture requires a built `graphzero` binary and a warm index. The scoring
side (`run.py`) consumes only the committed corpus, so metrics are reproducible
without re-running the graph.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = Path(__file__).resolve().parent / "corpus.jsonl"
GZ = ROOT / "target" / "release" / "graphzero"

FN_DEF = re.compile(r"\bfn\s+([a-zA-Z_][a-zA-Z0-9_]*)")
IDENT = re.compile(r"[a-zA-Z_][a-zA-Z0-9_]{3,}")

MAX_SYMBOLS_PER_EVENT = 8
MAX_CANDIDATES_PER_EVENT = 64


def sh(args: list[str]) -> str:
    return subprocess.run(args, cwd=ROOT, capture_output=True, text=True).stdout


def is_test_file(path: str) -> bool:
    return path.endswith(".rs") and "/tests/" in path


def test_file_index() -> dict[str, set[str]]:
    """Map identifier -> test files mentioning it (test-coverage resolution)."""
    index: collections.defaultdict[str, set[str]] = collections.defaultdict(set)
    for rel in sh(["git", "ls-files", "*.rs"]).split():
        if not is_test_file(rel):
            continue
        try:
            text = (ROOT / rel).read_text(encoding="utf8", errors="ignore")
        except OSError:
            continue
        for ident in set(IDENT.findall(text)):
            index[ident].add(rel)
    return index


def touched_symbols(commit: str, src_files: list[str]) -> list[str]:
    diff = sh(["git", "show", "--format=", "-U0", commit, "--", *src_files])
    syms: set[str] = set()
    for line in diff.splitlines():
        if line.startswith(("+++", "---")):
            continue
        if line.startswith(("+", "-", "@@")):
            syms.update(FN_DEF.findall(line))
    return sorted(syms)


def blast_capsule(symbol: str, cache: dict[str, dict | None]) -> dict | None:
    if symbol in cache:
        return cache[symbol]
    capsule = None
    raw = sh([str(GZ), "blast", "--intent", f"change signature of {symbol}", "--json"])
    try:
        ref = json.loads(raw)["data"]["raw"]
        capsule = json.loads(sh([str(GZ), "expand", ref]))
    except (ValueError, KeyError, TypeError):
        capsule = None
    cache[symbol] = capsule
    return capsule


def graph_candidates(symbols: list[str], sym2test: dict[str, set[str]],
                     cache: dict[str, dict | None]) -> list[dict]:
    """Blast break sites resolved to candidate test files.

    callers    = break sites in this event resolving to the test file
    proximity  = max confidence / (1 + hop) over those break sites
    """
    callers: collections.Counter[str] = collections.Counter()
    proximity: dict[str, float] = {}
    for symbol in symbols:
        capsule = blast_capsule(symbol, cache)
        if not capsule:
            continue
        for site in capsule.get("break_sites", []):
            weight = float(site.get("confidence", 0.5)) / (1.0 + int(site.get("hop", 1)))
            if weight <= 0.0:
                # zero-confidence break sites carry no ranking signal
                continue
            for path in sym2test.get(site["symbol"], ()):
                callers[path] += 1
                proximity[path] = max(proximity.get(path, 0.0), weight)
    ranked = sorted(
        callers,
        key=lambda p: (-callers[p] * proximity[p], p),
    )[:MAX_CANDIDATES_PER_EVENT]
    return [
        {"path": p, "callers": callers[p], "proximity": round(proximity[p], 6)}
        for p in ranked
    ]


def build(limit: int, scan: int) -> list[dict]:
    sym2test = test_file_index()
    cache: dict[str, dict | None] = {}
    events: list[dict] = []
    for commit in sh(["git", "log", "--format=%H", "-n", str(scan)]).split():
        files = [f for f in sh(["git", "show", "--name-only", "--format=", commit]).split()
                 if f.endswith(".rs")]
        faults = sorted(f for f in files if is_test_file(f))
        src_files = [f for f in files if not is_test_file(f)]
        if not faults or not src_files:
            continue
        symbols = touched_symbols(commit, src_files)[:MAX_SYMBOLS_PER_EVENT]
        if not symbols:
            continue
        candidates = graph_candidates(symbols, sym2test, cache)
        if not candidates:
            continue
        date = sh(["git", "show", "-s", "--format=%cI", commit]).strip()
        events.append({
            "event_id": commit[:12],
            "commit": commit,
            "date": date,
            "touched_symbols": symbols,
            "faults": faults,
            "graph_candidates": candidates,
        })
        if len(events) >= limit:
            break
    events.reverse()  # chronological replay order
    return events


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=70)
    ap.add_argument("--scan", type=int, default=400)
    args = ap.parse_args()
    if not GZ.exists():
        raise SystemExit(f"missing {GZ}; build with cargo build --release first")
    events = build(args.limit, args.scan)
    if len(events) < 50:
        raise SystemExit(f"corpus too small: {len(events)} events (need >=50)")
    payload = "".join(json.dumps(e, sort_keys=True) + "\n" for e in events)
    OUT.write_text(payload, encoding="utf8")
    digest = hashlib.sha256(payload.encode()).hexdigest()
    print(f"wrote {len(events)} events to {OUT} sha256={digest}")


if __name__ == "__main__":
    main()
