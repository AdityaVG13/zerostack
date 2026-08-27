#!/usr/bin/env python3
"""Parallel-ingest speedup on a 50k corpus (fszero-lj3).

Cold-indexes a deterministic 50k-file corpus with the ingest pool capped at
1 (serial baseline), 2, 4, 8, and all cores (FSZERO_INGEST_THREADS), and
reports the parallel_ingest phase wall plus derived speedup. Every run
verifies files_walked == N and incremental == false. Output order is
deterministic regardless of thread count (rayon ordered collect) — verified
separately by the test suite.

Outputs benchmarks/ingest-scaling.json and benchmarks/ingest-scaling.md.

Usage: python3 benchmarks/ingest_scaling.py [--files 50000] [--threads 1,2,4,8,0] [--runs 20]
(0 = unset the cap: all cores)
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MIN_MEASURED_RUNS = 20


def percentiles(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)

    def at(fraction: float) -> float:
        rank = (len(ordered) - 1) * fraction
        lower = int(rank)
        upper = min(lower + 1, len(ordered) - 1)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {"p50": at(0.50), "p95": at(0.95), "p99": at(0.99)}


def fszero_bin() -> str:
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def hardware() -> tuple[str, int]:
    cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
    cores = int(subprocess.check_output(["sysctl", "-n", "hw.ncpu"], text=True).strip())
    return cpu, cores


def cold_run(corpus: Path, n_files: int, threads: int) -> dict:
    for store in (".fszero", ".zerostack", ".asgrep"):
        shutil.rmtree(corpus / store, ignore_errors=True)
    env = os.environ.copy()
    env.update({
        "FSZERO_ROOT": str(corpus),
        "FSZERO_STARTUP_INDEX": "1",
        "FSZERO_INDEX_PHASES": "1",
        "FSZERO_INDEX_MAX_FILES": str(n_files + 1000),
    })
    env.pop("FSZERO_INGEST_THREADS", None)
    if threads > 0:
        env["FSZERO_INGEST_THREADS"] = str(threads)
    r = subprocess.run(
        [fszero_bin(), "codemode", "return{ok:true}", "--root", str(corpus)],
        capture_output=True, text=True, timeout=3600, cwd=corpus, env=env,
    )
    if r.returncode != 0:
        raise SystemExit(f"INTEGRITY: cold run failed (threads={threads})")
    line = next(
        (l for l in r.stderr.splitlines() if l.startswith('{"index_phases_ms"')), None
    )
    if line is None:
        raise SystemExit("INTEGRITY: no phase JSON")
    data = json.loads(line)
    if data["counts"]["files_walked"] != n_files or data["counts"]["incremental"]:
        raise SystemExit("INTEGRITY: wrong file count or warm run")
    return data


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--files", type=int, default=50000)
    ap.add_argument("--threads", default="1,2,4,8,0")
    ap.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    args = ap.parse_args()
    if args.runs < MIN_MEASURED_RUNS:
        ap.error(f"--runs must be at least {MIN_MEASURED_RUNS}")
    thread_caps = [int(t) for t in args.threads.split(",")]

    cpu, cores = hardware()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    result: dict = {
        "hardware": f"{cpu} / {cores} cores",
        "cores": cores,
        "git_commit": commit,
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "files": args.files,
        "runs_per_config": args.runs,
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_runs": 0,
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered raw run",
        },
        "seed": 42,
        "configs": [],
    }
    with tempfile.TemporaryDirectory(prefix="fszero_ingest_") as tmp:
        corpus = Path(tmp) / "corpus"
        print(f"=== generating {args.files}-file corpus ...", flush=True)
        subprocess.run(
            ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
             "--files", str(args.files), "--out", str(corpus), "--seed", "42"],
            check=True,
        )
        for t in thread_caps:
            runs = [cold_run(corpus, args.files, t) for _ in range(args.runs)]
            ingest_values = [r["index_phases_ms"]["parallel_ingest"] for r in runs]
            total_values = [r["total_ms"] for r in runs]
            ingest = statistics.median(ingest_values)
            total = statistics.median(total_values)
            label = t if t > 0 else cores
            result["configs"].append({
                "threads": label,
                "median_ingest_ms": ingest,
                "median_total_ms": total,
                "ingest_ms": percentiles(ingest_values),
                "total_ms": percentiles(total_values),
                "runs": runs,
            })
            print(f"  threads={label}: ingest {ingest:.0f} ms, total {total:.0f} ms", flush=True)

    serial = result["configs"][0]["median_ingest_ms"]
    lines = [
        "# Parallel-ingest speedup at 50k files (fszero-lj3)",
        "",
        "Generated by `benchmarks/ingest_scaling.py` — do not hand-edit numbers.",
        f"Hardware: {result['hardware']}. Commit: `{commit[:12]}`. Date: {result['date']}.",
        f"Corpus: {args.files} deterministic synthetic files (seed 42),"
        f" {args.runs} cold runs per config; p50 shown, p95/p99 retained in JSON. Ingest = read + utf8 +"
        " tree-sitter parse phase (rayon pool); DB writes stay sequential and"
        " are reported in total.",
        "",
        "| ingest threads | ingest p50 ms | speedup vs serial | total p50 ms |",
        "| --: | --: | --: | --: |",
    ]
    for c in result["configs"]:
        lines.append(
            f"| {c['threads']} | {c['median_ingest_ms']:.0f} |"
            f" {serial / max(c['median_ingest_ms'], 1e-9):.2f}x |"
            f" {c['median_total_ms']:.0f} |"
        )
    lines += [
        "",
        "Reproduce: `python3 benchmarks/ingest_scaling.py` (requires the release"
        " binary). Raw runs in `ingest-scaling.json`.",
        "",
        "## DEFINE: success metrics",
        "",
        "### Product gate (enforced by runner)",
        "",
        "Integrity only — process exits non-zero on miss:",
        "",
        "- Cold run completes (`ack` path); failure → `INTEGRITY: cold run failed`.",
        "- Phase JSON present; missing → `INTEGRITY: no phase JSON`.",
        "- `counts.files_walked == N` and `counts.incremental == false` → else",
        "  `INTEGRITY: wrong file count or warm run`.",
        "",
        "No performance threshold is checked in `benchmarks/ingest_scaling.py`.",
        "",
        "### Latency / speedup (observational — not a product budget)",
        "",
        "| Metric | Meaning | Gate |",
        "|---|---|---|",
        "| ingest p50 ms vs thread count | Parallel ingest phase wall (median) | **Measurement-only** |",
        "| speedup vs serial | `serial_ingest_p50 / ingest_p50` at each thread cap | **Measurement-only** |",
        "| total p50 ms | End-to-end cold wall including sequential DB | **Measurement-only** |",
        "| p95/p99 in JSON | When `--runs` ≥ statistical profile minimum | **Measurement-only** |",
        "",
        "Published tables are fingerprints for profiling/research, not CI/release pass-fail.",
        "A product floor needs a separate decision + runner exit gate.",
        "",
        "## Non-goals",
        "",
        "- Absolute wall budgets across hosts (host class varies).",
        "- Replacing cold-index 100k wall scenarios (see other bench beads).",
        "",
    ]
    (ROOT / "benchmarks" / "ingest-scaling.json").write_text(json.dumps(result, indent=2) + "\n")
    (ROOT / "benchmarks" / "ingest-scaling.md").write_text("\n".join(lines))
    print("written: benchmarks/ingest-scaling.json, benchmarks/ingest-scaling.md")


if __name__ == "__main__":
    main()
