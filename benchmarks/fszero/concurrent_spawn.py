#!/usr/bin/env python3
"""Concurrent-spawn CPU/thermal footprint (fszero-hym).

Measures what N fszero children spawned in parallel over the SAME root cost
in aggregate CPU and wall time, cold and warm. Evidence for the incident
where 4 concurrent children each burned ~30% CPU cold-indexing the same
tree and overheated the host.

Three configs per fan-out N:
  cold-unlocked  stores cleared, FSZERO_INDEX_LOCK=0 (pre-fix behavior:
                 every child runs its own full cold build)
  cold-locked    stores cleared, single-indexer lock on (default): one child
                 pays the cold build, the rest block then take the
                 incremental path off the winner's manifest
  warm           stores intact from the previous cold run

Per-child CPU (ru_utime+ru_stime via os.wait4) is exact; storm wall is
last-child-exit minus spawn. Integrity: every locked/warm child must exit 0
with a non-error ack and emit phase JSON; cold-unlocked child failures are
recorded (that IS the current-behavior data point), never hidden.

Outputs benchmarks/concurrent-spawn.json and benchmarks/concurrent-spawn.md.

Usage: python3 benchmarks/concurrent_spawn.py [--files 10000] [--fanout 1,2,4,8] [--runs 20]
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import shutil
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


def git_provenance() -> dict[str, object]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "-uno", "--", ".",
         ":(exclude)benchmarks/concurrent-spawn.json",
         ":(exclude)benchmarks/concurrent-spawn.md"],
        cwd=ROOT, text=True,
    )
    return {"git_commit": commit, "git_dirty": bool(status.strip())}


def hardware() -> str:
    try:
        cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        cores = subprocess.check_output(["sysctl", "-n", "hw.ncpu"], text=True).strip()
        return f"{cpu} / {cores} cores"
    except Exception:
        return "unknown"


def clear_stores(corpus: Path) -> None:
    for store in (".fszero", ".zerostack", ".asgrep"):
        shutil.rmtree(corpus / store, ignore_errors=True)


def storm(corpus: Path, n_files: int, fanout: int, lock: bool) -> dict:
    """Spawn `fanout` children concurrently; return per-child CPU + storm wall."""
    env = os.environ.copy()
    env.update({
        "FSZERO_ROOT": str(corpus),
        "FSZERO_STARTUP_INDEX": "1",
        "FSZERO_INDEX_PHASES": "1",
        "FSZERO_INDEX_MAX_FILES": str(n_files + 1000),
    })
    if not lock:
        env["FSZERO_INDEX_LOCK"] = "0"
    procs = []
    outs = []
    t0 = time.monotonic()
    for _ in range(fanout):
        fo = tempfile.TemporaryFile()
        fe = tempfile.TemporaryFile()
        p = subprocess.Popen(
            [fszero_bin(), "codemode", "return{ok:true}", "--root", str(corpus)],
            stdout=fo, stderr=fe, cwd=corpus, env=env,
        )
        procs.append(p)
        outs.append((fo, fe))
    children = []
    for p, (fo, fe) in zip(procs, outs):
        _, status, ru = os.wait4(p.pid, 0)
        p.returncode = os.waitstatus_to_exitcode(status)
        fo.seek(0)
        fe.seek(0)
        stdout = fo.read().decode(errors="replace")
        stderr = fe.read().decode(errors="replace")
        fo.close()
        fe.close()
        ack = stdout.strip().splitlines()[0] if stdout.strip() else ""
        phase_line = next(
            (line for line in stderr.splitlines() if line.startswith('{"index_phases_ms"')), None
        )
        phases = json.loads(phase_line) if phase_line else None
        children.append({
            "ok": p.returncode == 0 and not ack.startswith("X0"),
            "cpu_s": ru.ru_utime + ru.ru_stime,
            "incremental": phases["counts"]["incremental"] if phases else None,
            "files_walked": phases["counts"]["files_walked"] if phases else None,
            "index_total_ms": phases["total_ms"] if phases else None,
        })
    wall_s = time.monotonic() - t0
    return {
        "fanout": fanout,
        "lock": lock,
        "wall_s": wall_s,
        "agg_cpu_s": sum(c["cpu_s"] for c in children),
        "cold_children": sum(1 for c in children if c["incremental"] is False),
        "warm_children": sum(1 for c in children if c["incremental"] is True),
        "failed_children": sum(1 for c in children if not c["ok"]),
        "children": children,
    }


def run_config(corpus: Path, n_files: int, fanout: int, mode: str, runs: int) -> dict:
    samples = []
    for _ in range(runs):
        if mode.startswith("cold"):
            clear_stores(corpus)
        s = storm(corpus, n_files, fanout, lock=(mode != "cold-unlocked"))
        if mode != "cold-unlocked" and s["failed_children"]:
            raise SystemExit(f"INTEGRITY: {s['failed_children']} child(ren) failed in {mode} N={fanout}")
        if mode.startswith("cold"):
            cold_ok = [c for c in s["children"] if c["incremental"] is False and c["ok"]]
            if not cold_ok:
                raise SystemExit(f"INTEGRITY: no successful cold child in {mode} N={fanout}")
            if any(c["files_walked"] != n_files for c in cold_ok):
                raise SystemExit(f"INTEGRITY: files_walked != {n_files} in {mode} N={fanout}")
        samples.append(s)
    wall_values = [s["wall_s"] for s in samples]
    cpu_values = [s["agg_cpu_s"] for s in samples]
    return {
        "mode": mode,
        "fanout": fanout,
        "median_wall_s": statistics.median(wall_values),
        "median_agg_cpu_s": statistics.median(cpu_values),
        "wall_s": percentiles(wall_values),
        "aggregate_cpu_s": percentiles(cpu_values),
        "cold_children": samples[-1]["cold_children"],
        "warm_children": samples[-1]["warm_children"],
        "failed_children": max(s["failed_children"] for s in samples),
        "runs": samples,
    }


def render_markdown(result: dict) -> str:
    rows = result["configs"]
    by = {(r["mode"], r["fanout"]): r for r in rows}
    fanouts = sorted({r["fanout"] for r in rows})
    lines = [
        "# Concurrent-spawn CPU footprint (fszero-hym)",
        "",
        "Generated by `benchmarks/concurrent_spawn.py` — do not hand-edit numbers.",
        f"Hardware: {result['hardware']}. Commit: `{result['git_commit'][:12]}`"
        f" (dirty={str(result['git_dirty']).lower()}). Date: {result['date']}.",
        f"Corpus: {result['files']} synthetic files (`gen_corpus.py`, seed 42);"
        f" {result['runs_per_config']} runs per config; p50 shown, p95/p99 retained in JSON.",
        "",
        "N children spawned simultaneously over the same root. `cold-unlocked` is"
        " pre-fix behavior (`FSZERO_INDEX_LOCK=0`): every child runs a full cold"
        " build. `cold-locked` is the shipped single-indexer lock: one cold build,"
        " N-1 children take the incremental path. CPU is summed user+sys across"
        " children (os.wait4 rusage).",
        "",
        "| N | mode | storm wall p50 s | aggregate CPU p50 s | cold | warm | failed |",
        "| --: | :-- | --: | --: | --: | --: | --: |",
    ]
    for n in fanouts:
        for mode in ("cold-unlocked", "cold-locked", "warm"):
            r = by.get((mode, n))
            if not r:
                continue
            lines.append(
                f"| {n} | {mode} | {r['median_wall_s']:.2f} | {r['median_agg_cpu_s']:.2f}"
                f" | {r['cold_children']} | {r['warm_children']} | {r['failed_children']} |"
            )
    n_max = fanouts[-1]
    un = by.get(("cold-unlocked", n_max))
    lk = by.get(("cold-locked", n_max))
    if un and lk and lk["median_agg_cpu_s"] > 0:
        lines += [
            "",
            f"At N={n_max}: single-indexer lock cuts aggregate cold CPU"
            f" {un['median_agg_cpu_s']:.2f}s → {lk['median_agg_cpu_s']:.2f}s"
            f" ({un['median_agg_cpu_s'] / lk['median_agg_cpu_s']:.1f}× less CPU/heat).",
        ]
    lines += [
        "",
        "Reproduce: `python3 benchmarks/concurrent_spawn.py` (requires the release-perf"
        " binary; `./scripts/profile_build.sh -p fs-zero --bin fszero`). Raw runs in `concurrent-spawn.json`.",
        "",
        "## DEFINE: success metrics",
        "",
        "### Product gate (enforced by runner)",
        "",
        "Integrity only — process exits non-zero on miss (`SystemExit` with `INTEGRITY:` prefix):",
        "",
        "- `cold-locked` / `warm`: every child exits 0 with a non-error ack"
        " (`failed_children == 0`); failure → `INTEGRITY: N child(ren) failed in …`.",
        "- Cold modes: at least one successful cold child (`incremental is false`);"
        " missing → `INTEGRITY: no successful cold child …`.",
        "- Cold modes: every successful cold child reports `files_walked == N` for the"
        " generated corpus size; miss → `INTEGRITY: files_walked != N …`.",
        "- `cold-unlocked` child failures are recorded as data (pre-fix multi-indexer"
        " behavior), never hidden, and do **not** fail the runner.",
        "",
        "No CPU-ratio, wall-ratio, or absolute-latency threshold is checked in"
        " `benchmarks/concurrent_spawn.py`.",
        "",
        "### CPU / wall (observational — not a product budget)",
        "",
        "| Metric | Meaning | Gate |",
        "|---|---|---|",
        "| aggregate CPU p50 s (cold-unlocked vs cold-locked) |"
        " Summed user+sys across children; lock win narrative |"
        " **Measurement-only** |",
        "| storm wall p50 s | Last-child-exit minus spawn | **Measurement-only** |",
        "| cold/warm child counts under lock |"
        " Expect ~1 cold + (N-1) warm when lock works |"
        " **Observational shape** (integrity still requires zero failed) |",
        "| unlocked:locked aggregate CPU ratio (e.g. historical ~53× at N=8) |"
        " Thermal / incident evidence fingerprint |"
        " **Measurement-only** — not CI/release pass-fail |",
        "",
        "Published tables (including historical M5 Max rows) are fingerprints for"
        " profiling and incident narrative, not product budgets. A product floor"
        " (e.g. locked N=8 aggregate CPU ≤ k× single cold, or ratio ≥ R) needs a"
        " separate decision + runner exit gate.",
        "",
        "## Non-goals",
        "",
        "- Absolute wall or CPU budgets across hosts (host class and thermal state vary).",
        "- Encoding lock CPU-ratio pass/fail without an explicit product decision.",
        "- Replacing single-process cold-index wall scenarios (see other bench beads).",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--files", type=int, default=10000)
    ap.add_argument("--fanout", default="1,2,4,8")
    ap.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    args = ap.parse_args()
    if args.runs < MIN_MEASURED_RUNS:
        ap.error(f"--runs must be at least {MIN_MEASURED_RUNS}")
    fanouts = [int(s) for s in args.fanout.split(",")]

    result: dict = {
        "hardware": hardware(),
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        **git_provenance(),
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
    with tempfile.TemporaryDirectory(prefix="fszero_spawn_") as tmp:
        corpus = Path(tmp) / "corpus"
        print(f"=== generating {args.files}-file corpus ...", flush=True)
        subprocess.run(
            ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
             "--files", str(args.files), "--out", str(corpus), "--seed", "42"],
            check=True,
        )
        for n in fanouts:
            for mode in ("cold-unlocked", "cold-locked", "warm"):
                r = run_config(corpus, args.files, n, mode, args.runs)
                result["configs"].append(r)
                print(
                    f"  N={n} {mode}: wall {r['median_wall_s']:.2f}s"
                    f" cpu {r['median_agg_cpu_s']:.2f}s cold={r['cold_children']}"
                    f" warm={r['warm_children']} failed={r['failed_children']}",
                    flush=True,
                )

    out_json = ROOT / "benchmarks" / "concurrent-spawn.json"
    out_json.write_text(json.dumps(result, indent=2) + "\n")
    out_md = ROOT / "benchmarks" / "concurrent-spawn.md"
    out_md.write_text(render_markdown(result))
    print(f"\nwritten: {out_json}\nwritten: {out_md}")


if __name__ == "__main__":
    main()
