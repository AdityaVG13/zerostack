#!/usr/bin/env python3
"""Reproducible validated-store reopen benchmark for Linux.

The Rust perf helper seeds stores through RecoveryStore, validates them once,
then this driver measures subsequent opens with GNU time. JSON is always
written before an acceptance failure.
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
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


def scenario_fingerprint() -> dict:
    command = [
        sys.executable,
        str(ROOT / "scripts" / "env_fingerprint.py"),
        "--root",
        str(ROOT),
        "--cache-state",
        "warm",
        "--cargo-profile",
        "release-perf",
    ]
    run = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, timeout=15, check=False
    )
    if run.returncode != 0:
        raise RuntimeError(
            f"environment fingerprint failed ({run.returncode}): {run.stderr.strip()}"
        )
    document = json.loads(run.stdout)
    required = {
        "schema_version", "run_id", "captured_at_utc", "cache_state",
        "repository", "cpu", "power", "kernel", "toolchain",
        "filesystem", "isolation",
    }
    missing = sorted(required - document.keys())
    if missing:
        raise RuntimeError(f"environment fingerprint missing keys: {missing}")
    if document["schema_version"] != "fszero.perf-fingerprint.v1":
        raise RuntimeError("environment fingerprint schema version mismatch")
    if document["cache_state"] != "warm":
        raise RuntimeError("durable store-open fingerprint must declare cache_state=warm")
    if document["toolchain"].get("cargo_profile") != "release-perf":
        raise RuntimeError("durable store-open fingerprint must bind cargo_profile=release-perf")
    if document["repository"].get("status") != "available":
        raise RuntimeError("durable store-open fingerprint requires tracked git provenance")
    if document["isolation"].get("status") != "provided":
        raise RuntimeError(
            "set FSZERO_PERF_ISOLATION_NOTE before publishing durable store-open evidence"
        )
    return document


def provenance(fingerprint: dict) -> dict:
    kernel = fingerprint["kernel"]
    cpu = fingerprint["cpu"]
    power = fingerprint["power"]
    filesystem = fingerprint["filesystem"]
    toolchain = fingerprint["toolchain"]
    rustc = toolchain.get("rustc")
    runner_class = os.environ.get("FSZERO_PERF_RUNNER_CLASS", "unspecified")
    if runner_class == "unspecified":
        raise RuntimeError(
            "set FSZERO_PERF_RUNNER_CLASS before publishing durable store-open evidence"
        )
    expected_system = {"linux-rch-spark": "Linux", "darwin-local": "Darwin"}.get(
        runner_class
    )
    if expected_system and kernel.get("system") != expected_system:
        raise RuntimeError(
            f"runner class {runner_class} conflicts with kernel {kernel.get('system')}"
        )
    host_class = {
        "runner": runner_class,
        "cpu_model": cpu.get("model"),
        "logical_cores": cpu.get("logical_cores"),
        "kernel_system": kernel.get("system"),
        "kernel_machine": kernel.get("machine"),
        "filesystem_type": filesystem.get("type"),
        "power_kind": power.get("kind"),
        "power_status": power.get("status"),
        "power_value": power.get("value"),
        "rustc": rustc.splitlines()[0] if rustc else None,
        "cargo_profile": toolchain.get("cargo_profile"),
    }
    repository = fingerprint["repository"]
    captured = fingerprint["captured_at_utc"]
    return {
        "git_commit": repository["git_sha"],
        "git_dirty": repository["git_dirty"],
        "hardware": host_class,
        "host": host_class["runner"],
        "host_class": host_class,
        "date": captured,
        "generated_at": captured,
        "scenario_fingerprint": fingerprint,
    }


def parse_time(path: Path) -> dict[str, float]:
    fields: dict[str, float] = {}
    for line in path.read_text().splitlines():
        key, sep, value = line.strip().partition(": ")
        if sep:
            fields[key] = value
    return {
        "cpu_user_s": float(fields["User time (seconds)"]),
        "cpu_system_s": float(fields["System time (seconds)"]),
        "peak_rss_bytes": int(fields["Maximum resident set size (kbytes)"]) * 1024,
    }


def helper_json(command: list[str]) -> dict:
    run = subprocess.run(command, check=True, capture_output=True, text=True)
    return json.loads(run.stdout.strip().splitlines()[-1])


def heap_hwm_fields() -> dict[str, object]:
    """Allocator heap high-water for the measured child (fszero-5444).

    store_open drives an opaque release-perf helper via GNU time. Without an
    env-gated dhat/heaptrack/allocator-export on that child, heap HWM is
    **unsupported** -- still emit the keys so JSON consumers have a dual-metric
    contract. Do not substitute peak RSS or idle RSS for heap HWM.
    """
    mode = os.environ.get("FSZERO_HEAP_HWM", "").strip().lower()
    if mode in ("", "0", "off", "unsupported"):
        return {
            "heap_high_water_bytes": None,
            "heap_high_water_status": "unsupported",
            "heap_high_water_reason": (
                "no allocator HWM export on store_open child; enable dhat/"
                "heaptrack per docs/profiling.md (fszero-lghz) or set "
                "FSZERO_HEAP_HWM only when a real method is wired"
            ),
            "heap_high_water_method": None,
        }
    # Reserved for a future helper that writes HWM into stdout JSON or a side file.
    return {
        "heap_high_water_bytes": None,
        "heap_high_water_status": "unsupported",
        "heap_high_water_reason": (
            f"FSZERO_HEAP_HWM={mode!r} has no wired method in store_open yet"
        ),
        "heap_high_water_method": None,
    }


def measured_open(helper: Path, db: Path, rows: int, timing: Path) -> dict:
    command = ["/usr/bin/time", "-v", "-o", str(timing), str(helper),
               "store_open_measure", str(db), str(rows)]
    started = time.perf_counter_ns()
    sample = helper_json(command)
    sample["wall_ns"] = time.perf_counter_ns() - started
    sample.update(parse_time(timing))
    sample["cpu_s"] = sample["cpu_user_s"] + sample["cpu_system_s"]
    sample["incremental_peak_rss_bytes"] = max(
        0, sample["peak_rss_bytes"] - sample["baseline_rss_bytes"])
    # Dual metric contract (fszero-5444): peak RSS always from GNU time; heap HWM
    # separate (unsupported here unless allocator profiler is wired).
    sample["peak_rss_method"] = "gnu_time_Maximum_resident_set_size"
    sample.update(heap_hwm_fields())
    return sample


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", type=Path, required=True,
                        help="release perf_harness executable")
    parser.add_argument("--row-counts", default="100000,1000000")
    parser.add_argument("--runs", type=int, default=MIN_MEASURED_RUNS)
    parser.add_argument("--constant-factor", type=float, default=3.0)
    parser.add_argument("--max-incremental-rss-mib", type=int, default=64)
    parser.add_argument("--output", type=Path,
                        default=ROOT / "benchmarks/durable-store-open.json")
    args = parser.parse_args()
    counts = [int(value) for value in args.row_counts.split(",")]
    if len(counts) < 2 or max(counts) < 1_000_000:
        parser.error("need at least two row counts including 1,000,000")
    if args.runs < MIN_MEASURED_RUNS:
        parser.error(f"--runs must be at least {MIN_MEASURED_RUNS}")
    fingerprint = scenario_fingerprint()

    result = {**provenance(fingerprint),
              "schema_version": 1, "status": "fail", "row_counts": counts,
              "runs_per_count": args.runs,
              "statistical_profile": {
                  "minimum_measured_runs": MIN_MEASURED_RUNS,
                  "warmup_runs": 0,
                  "percentile_method": "linear interpolation over ordered measured samples",
                  "outlier_policy": "none; retain every ordered raw open",
              },
              "constant_factor_limit": args.constant_factor,
              "incremental_rss_limit_bytes": args.max_incremental_rss_mib * 1024 * 1024,
              "measurement": "GNU time -v child peak RSS minus helper pre-open /proc VmRSS",
              "memory_dual_metric": {
                  "bead": "fszero-5444",
                  "peak_rss": {
                      "field": "peak_rss_bytes",
                      "method": "gnu_time_Maximum_resident_set_size",
                      "meaning": (
                          "process peak resident set during the measured open child; "
                          "includes file-backed pages and allocator arenas"
                      ),
                  },
                  "heap_high_water": {
                      "field": "heap_high_water_bytes",
                      "method": "env-gated allocator profiler (dhat/heaptrack) when wired",
                      "default_status": "unsupported",
                      "meaning": (
                          "allocator live high-water; NOT peak RSS; empty MALLOC_LARGE "
                          "must not be labeled live heap (kflx)"
                      ),
                  },
                  "idle_rss": {
                      "field": "baseline_rss_bytes / post-open idle if present",
                      "meaning": (
                          "RSS after steady state with little live work; may retain "
                          "empty arenas -- distinct from heap HWM"
                      ),
                  },
                  "docs": "docs/profiling.md#dual-metric-peak-rss-vs-heap-high-water",
              },
              "samples": {}}
    failures: list[str] = []
    with tempfile.TemporaryDirectory(prefix="fszero-durable-open-") as tmp:
        work = Path(tmp)
        for rows in counts:
            db = work / str(rows) / "store.sqlite3"
            seed = helper_json([str(args.helper), "store_open_seed", str(db), str(rows)])
            samples = [measured_open(args.helper, db, rows, work / f"time-{rows}-{run}.txt")
                       for run in range(args.runs)]
            open_wall = [s["open_wall_ns_internal"] for s in samples]
            process_wall = [s["wall_ns"] for s in samples]
            cpu = [s["cpu_s"] for s in samples]
            result["samples"][str(rows)] = {"seed": seed, "opens": samples,
                "p50_open_wall_ns": statistics.median(open_wall),
                "p95_open_wall_ns": percentiles(open_wall)["p95"],
                "p99_open_wall_ns": percentiles(open_wall)["p99"],
                "p50_process_wall_ns": statistics.median(process_wall),
                "p95_process_wall_ns": percentiles(process_wall)["p95"],
                "p99_process_wall_ns": percentiles(process_wall)["p99"],
                "p50_cpu_s": statistics.median(cpu),
                "p95_cpu_s": percentiles(cpu)["p95"],
                "p99_cpu_s": percentiles(cpu)["p99"],
                "max_incremental_peak_rss_bytes": max(s["incremental_peak_rss_bytes"] for s in samples)}
            if any(s["payload_rows_scanned"] != 0 for s in samples):
                failures.append(f"{rows}: reopen scanned payload rows")
            if result["samples"][str(rows)]["max_incremental_peak_rss_bytes"] > result["incremental_rss_limit_bytes"]:
                failures.append(f"{rows}: incremental RSS exceeded limit")

    small, large = min(counts), max(counts)
    for metric in ("p50_open_wall_ns", "p50_cpu_s"):
        values = [result["samples"][str(n)][metric] for n in (small, large)]
        ratio = max(values) / max(min(values), 1e-12)
        result[f"historical_{metric}_ratio"] = ratio
        if ratio > args.constant_factor:
            failures.append(f"{metric} ratio {ratio:.3f} exceeded {args.constant_factor}")
    result["failures"] = failures
    result["status"] = "pass" if not failures else "fail"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "output": str(args.output), "failures": failures}))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
