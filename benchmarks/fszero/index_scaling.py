#!/usr/bin/env python3
"""Cold-index scaling curve: where does build_index degrade superlinearly?

For each corpus size, generates a deterministic synthetic corpus
(gen_corpus.py), then runs COLD index builds (stores cleared each run,
FSZERO_STARTUP_INDEX=1) with per-phase attribution (FSZERO_INDEX_PHASES=1).
Every run hard-verifies files_walked == N and incremental == false — a
silently capped or warm run aborts the benchmark rather than publishing a
lie (docs/benchmark-integrity.md).

Outputs benchmarks/index-scaling.json (all runs + provenance) and
benchmarks/index-scaling.md (medians, per-phase log-log slopes, ranked
superlinear hazards) — the markdown is generated from the measured data.

Usage: python3 benchmarks/index_scaling.py [--sizes 1000,5000,...] [--runs 20]
"""
from __future__ import annotations

import argparse
import json
import math
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_SIZES = [1000, 5000, 10000, 25000, 50000, 100000]
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
    import os
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def git_provenance() -> dict[str, object]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "-uno", "--", ".",
         ":(exclude)benchmarks/index-scaling.json", ":(exclude)benchmarks/index-scaling.md"],
        cwd=ROOT, text=True,
    )
    return {"git_commit": commit, "git_dirty": bool(status.strip())}


def hardware() -> str:
    try:
        cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        ram = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True).strip())
        return f"{cpu} / {ram // (1024 ** 3)} GB"
    except Exception:
        return "unknown"


def resource_profiler() -> tuple[str, str, str] | None:
    """Return a supported /usr/bin/time mode, or an explicit no-profiler fallback."""
    time_bin = Path("/usr/bin/time")
    if not time_bin.is_file():
        return None
    if sys.platform == "darwin":
        return str(time_bin), "-l", "darwin-time-l"
    probe = subprocess.run(
        [str(time_bin), "-v", "/usr/bin/true"], capture_output=True, text=True
    )
    if probe.returncode == 0:
        return str(time_bin), "-v", "gnu-time-v"
    return None


def parse_resources(stderr: str, profiler: tuple[str, str, str] | None) -> dict[str, object]:
    """Parse only documented time(1) measurements; never estimate resources."""
    resources: dict[str, object] = {
        "profiler": profiler[2] if profiler else "unavailable",
        "user_cpu_ms": None,
        "system_cpu_ms": None,
        "cpu_total_ms": None,
        "peak_rss_bytes": None,
    }
    if profiler is None:
        return resources

    if profiler[2] == "darwin-time-l":
        user = re.search(r"([0-9]+(?:\.[0-9]+)?)\s+user\b", stderr)
        system = re.search(r"([0-9]+(?:\.[0-9]+)?)\s+sys\b", stderr)
        rss = re.search(r"([0-9]+)\s+maximum resident set size", stderr)
        rss_bytes = int(rss.group(1)) if rss else None
    else:
        user = re.search(r"^\s*User time \(seconds\):\s*([0-9]+(?:\.[0-9]+)?)\s*$", stderr, re.M)
        system = re.search(r"^\s*System time \(seconds\):\s*([0-9]+(?:\.[0-9]+)?)\s*$", stderr, re.M)
        rss = re.search(r"^\s*Maximum resident set size \(kbytes\):\s*([0-9]+)\s*$", stderr, re.M)
        rss_bytes = int(rss.group(1)) * 1024 if rss else None

    user_ms = float(user.group(1)) * 1000 if user else None
    system_ms = float(system.group(1)) * 1000 if system else None
    resources.update({
        "user_cpu_ms": user_ms,
        "system_cpu_ms": system_ms,
        "cpu_total_ms": user_ms + system_ms if user_ms is not None and system_ms is not None else None,
        "peak_rss_bytes": rss_bytes,
    })
    return resources


def valid_resources(resources: dict[str, object]) -> bool:
    values = (
        resources["user_cpu_ms"],
        resources["system_cpu_ms"],
        resources["cpu_total_ms"],
        resources["peak_rss_bytes"],
    )
    if any(not isinstance(value, (int, float)) or not math.isfinite(value) for value in values):
        return False
    return (
        resources["user_cpu_ms"] >= 0
        and resources["system_cpu_ms"] >= 0
        and resources["cpu_total_ms"] > 0
        and resources["peak_rss_bytes"] > 0
    )


def median_resource(values: list[object]) -> float | None:
    if any(value is None for value in values):
        return None
    return statistics.median(values)


def cold_run(corpus: Path, n_files: int) -> dict:
    """One cold index build; returns the parsed phase JSON. Aborts on any
    integrity violation (wrong file count, warm run, failed ack)."""
    import os

    for store in (".fszero", ".zerostack", ".asgrep"):
        shutil.rmtree(corpus / store, ignore_errors=True)
    env = os.environ.copy()
    env.update({
        "FSZERO_ROOT": str(corpus),
        "FSZERO_STARTUP_INDEX": "1",
        "FSZERO_INDEX_PHASES": "1",
        "FSZERO_INDEX_MAX_FILES": str(n_files + 1000),
        "ZEROSTACK_STORE_ROOT": str(corpus / ".zerostack"),
    })
    # Trivial plan: FSZERO_STARTUP_INDEX makes session init run build_index
    # regardless of plan content, and a no-op plan cannot fail on corpus
    # shape (the `explore` recipe reads files a synthetic tree lacks).
    profiler = resource_profiler()
    command = [fszero_bin(), "codemode", "return{ok:true}", "--root", str(corpus)]
    if profiler is not None:
        command = [profiler[0], profiler[1], *command]
    r = subprocess.run(
        command, capture_output=True, text=True, timeout=3600, cwd=corpus, env=env,
    )
    ack = r.stdout.strip().splitlines()[0] if r.stdout.strip() else ""
    if r.returncode != 0 or ack.startswith("X0"):
        raise SystemExit(f"INTEGRITY: cold run failed (ack={ack}) at N={n_files}")
    phase_line = next(
        (line for line in r.stderr.splitlines() if line.startswith('{"index_phases_ms"')), None
    )
    if phase_line is None:
        raise SystemExit(f"INTEGRITY: no phase JSON on stderr at N={n_files}")
    data = json.loads(phase_line)
    counts = data["counts"]
    if counts["files_walked"] != n_files:
        raise SystemExit(
            f"INTEGRITY: files_walked={counts['files_walked']} != N={n_files} "
            "(silent cap or corpus mismatch) — refusing to publish"
        )
    if counts["incremental"]:
        raise SystemExit(f"INTEGRITY: run at N={n_files} was warm, not cold")
    resources = parse_resources(r.stderr, profiler)
    if profiler is not None and not valid_resources(resources):
        raise SystemExit(
            f"INTEGRITY: profiler {profiler[2]} produced invalid resource data at N={n_files}"
        )
    data["resources"] = resources
    return data


def slope(n0: int, t0: float, n1: int, t1: float) -> float:
    """log-log slope between two points; 1.0 = linear, >1 superlinear."""
    if n0 == n1 or t0 <= 0 or t1 <= 0:
        return float("nan")
    return (math.log(t1) - math.log(t0)) / (math.log(n1) - math.log(n0))


def render_markdown(result: dict) -> str:
    sizes = [s["files"] for s in result["sizes"]]
    med = {s["files"]: s["median_phases_ms"] for s in result["sizes"]}
    totals = {s["files"]: s["median_total_ms"] for s in result["sizes"]}
    resources = {s["files"]: s["median_resources"] for s in result["sizes"]}
    phases = list(med[sizes[0]].keys())

    def resource_cell(value: object, scale: float = 1.0, suffix: str = "") -> str:
        if value is None:
            return "n/a"
        return f"{float(value) / scale:.1f}{suffix}"

    lines = [
        "# Cold-index scaling: superlinear hazards (fszero-xez)",
        "",
        "Generated by `benchmarks/index_scaling.py` — do not hand-edit numbers.",
        f"Hardware: {result['hardware']}. Benchmark commit: `{result['git_commit'][:12]}`"
        f" (dirty={str(result['git_dirty']).lower()}). Date: {result['date']}.",
        f"Corpora: deterministic synthetic rust-like trees (`gen_corpus.py`, seed 42),"
        f" {result['runs_per_size']} cold runs per size; p50 shown, p95/p99 retained in JSON; every run"
        " verified files_walked == N and incremental == false.",
        "",
        "## Wall time per phase (p50 ms)",
        "",
        "| files | " + " | ".join(phases) + " | total |",
        "| --: | " + " | ".join("--:" for _ in phases) + " | --: |",
    ]
    for n in sizes:
        row = [f"{med[n][p]:.0f}" for p in phases]
        lines.append(f"| {n} | " + " | ".join(row) + f" | {totals[n]:.0f} |")

    lines += [
        "",
        "## Child-process resources (p50 per cold run)",
        "",
        "| files | user CPU (ms) | system CPU (ms) | total CPU (ms) | effective CPU utilization (%) | peak RSS (MiB) |",
        "| --: | --: | --: | --: | --: | --: |",
    ]
    for n in sizes:
        resource = resources[n]
        total_cpu = resource["cpu_total_ms"]
        utilization = (100.0 * total_cpu / totals[n]) if total_cpu is not None and totals[n] > 0 else None
        lines.append(
            f"| {n} | {resource_cell(resource['user_cpu_ms'])} | "
            f"{resource_cell(resource['system_cpu_ms'])} | {resource_cell(total_cpu)} | "
            f"{resource_cell(utilization)} | {resource_cell(resource['peak_rss_bytes'], 1024 ** 2)} |"
        )

    profiler = result["resource_profiler"]
    lines += [
        "",
        "### Resource profiling methodology",
        "",
        "Resource rows measure only the release `fszero codemode` child; corpus generation and the Python benchmark harness are outside the timed command.",
        "On Darwin, `/usr/bin/time -l` supplies `user`, `sys`, and `maximum resident set size` (bytes).",
        "On GNU systems, supported `/usr/bin/time -v` supplies user/system seconds and peak RSS in KiB, converted to ms and bytes.",
        "If neither supported format is available, raw resource fields and table cells are `null`/`n/a`; no estimates are synthesized.",
        f"Profiler selected for this run: `{profiler}`. Effective CPU utilization is `(user CPU + system CPU) / wall time`, shown as a percentage.",
        "",
        "## Scaling exponents (log-log slope between consecutive sizes; 1.0 = linear)",
        "",
        "| step | " + " | ".join(phases) + " | total |",
        "| :-- | " + " | ".join("--:" for _ in phases) + " | --: |",
    ]
    for a, b in zip(sizes, sizes[1:]):
        row = [f"{slope(a, med[a][p], b, med[b][p]):.2f}" for p in phases]
        lines.append(
            f"| {a}→{b} | " + " | ".join(row)
            + f" | {slope(a, totals[a], b, totals[b]):.2f} |"
        )

    n_lo, n_hi = sizes[0], sizes[-1]
    all_rows = [
        (
            p,
            slope(n_lo, med[n_lo][p], n_hi, med[n_hi][p]),
            med[n_hi][p],
            100.0 * med[n_hi][p] / totals[n_hi],
        )
        for p in phases
    ]
    # A high exponent on a sub-1% phase is noise, not a hazard: rank the
    # material phases by exponent; list the negligible ones separately.
    material = sorted((r for r in all_rows if r[3] >= 1.0), key=lambda h: h[1], reverse=True)
    negligible = sorted((r for r in all_rows if r[3] < 1.0), key=lambda h: h[1], reverse=True)
    lines += [
        "",
        f"## Ranked hazards (overall exponent {n_lo}→{n_hi}, share of total at {n_hi} files)",
        "",
        "| rank | phase | exponent | ms at max | share |",
        "| --: | :-- | --: | --: | --: |",
    ]
    for i, (p, k, ms, share) in enumerate(material, 1):
        exponent = "n/a" if n_lo == n_hi else f"{k:.2f}"
        lines.append(f"| {i} | {p} | {exponent} | {ms:.0f} | {share:.1f}% |")
    if negligible:
        lines += ["", f"Below the noise floor (<1% of total at {n_hi} files):"]
        for p, k, ms, share in negligible:
            lines.append(f"- {p}: exponent {k:.2f}, {ms:.0f} ms ({share:.1f}%)")
    lines += [
        "",
        f"Total wall {n_lo}→{n_hi}: exponent"
        f" {'n/a' if n_lo == n_hi else f'{slope(n_lo, totals[n_lo], n_hi, totals[n_hi]):.2f}'};"
        f" {totals[n_hi] / 1000:.2f}s at {n_hi} files"
        f" (northstar gate fszero-v5n: 100k under 5s).",
        "",
        "Reproduce: `python3 benchmarks/index_scaling.py` (requires the release-perf"
        " binary; `./scripts/profile_build.sh -p fs-zero --bin fszero`). Raw runs in `index-scaling.json`.",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--sizes", default=",".join(map(str, DEFAULT_SIZES)))
    ap.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    args = ap.parse_args()
    if args.runs < MIN_MEASURED_RUNS:
        ap.error(f"--runs must be at least {MIN_MEASURED_RUNS}")
    sizes = [int(s) for s in args.sizes.split(",")]

    result: dict = {
        "hardware": hardware(),
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        **git_provenance(),
        "runs_per_size": args.runs,
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_runs": 0,
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered raw run",
        },
        "seed": 42,
        "resource_profiler": (resource_profiler() or (None, None, "unavailable"))[2],
        "sizes": [],
    }

    for n in sizes:
        with tempfile.TemporaryDirectory(prefix=f"fszero_scale_{n}_") as tmp:
            corpus = Path(tmp) / "corpus"
            print(f"=== N={n}: generating corpus ...", flush=True)
            subprocess.run(
                ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
                 "--files", str(n), "--out", str(corpus), "--seed", "42"],
                check=True,
            )
            runs = []
            for i in range(args.runs):
                data = cold_run(corpus, n)
                runs.append(data)
                print(f"  run {i + 1}: total {data['total_ms']:.0f} ms", flush=True)
            phases = list(runs[0]["index_phases_ms"].keys())
            result["sizes"].append({
                "files": n,
                "runs_total_ms": [r["total_ms"] for r in runs],
                "median_total_ms": statistics.median(r["total_ms"] for r in runs),
                "total_ms": percentiles([r["total_ms"] for r in runs]),
                "median_phases_ms": {
                    p: statistics.median(r["index_phases_ms"][p] for r in runs) for p in phases
                },
                "phases_ms": {
                    p: percentiles([r["index_phases_ms"][p] for r in runs]) for p in phases
                },
                "all_runs_phases_ms": [r["index_phases_ms"] for r in runs],
                "all_runs_resources": [r["resources"] for r in runs],
                "median_resources": {
                    key: median_resource([r["resources"][key] for r in runs])
                    for key in ("user_cpu_ms", "system_cpu_ms", "cpu_total_ms", "peak_rss_bytes")
                },
            })

    out_json = ROOT / "benchmarks" / "index-scaling.json"
    out_json.write_text(json.dumps(result, indent=2) + "\n")
    out_md = ROOT / "benchmarks" / "index-scaling.md"
    out_md.write_text(render_markdown(result))
    print(f"\nwritten: {out_json}\nwritten: {out_md}")


if __name__ == "__main__":
    main()
