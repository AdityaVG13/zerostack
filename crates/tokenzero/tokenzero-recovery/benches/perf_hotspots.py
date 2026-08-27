#!/usr/bin/env python3
"""Reproduce TokenZero process CPU/RSS spikes with deterministic workloads.

Usage:
  python3 crates/tokenzero-recovery/benches/perf_hotspots.py --label baseline
  python3 crates/tokenzero-recovery/benches/perf_hotspots.py --label candidate
  python3 crates/tokenzero-recovery/benches/perf_hotspots.py --check-budget

The debug binary must already exist. Results are written beside this script in
perf_hotspots/<label>.json. The harness holds the machine-wide heavy-process
guard for the complete measurement run.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
BIN = REPO / "target/debug/tokenzero"
EVIDENCE = Path(__file__).with_suffix("").with_name("perf_hotspots")
GUARD = Path("/tmp/zerostack-heavy-process.guard")
TIME_RE = re.compile(r"^\s*([0-9.]+) real\s+([0-9.]+) user\s+([0-9.]+) sys\s*$", re.MULTILINE)
RSS_RE = re.compile(r"^\s*(\d+)\s+maximum resident set size\s*$", re.MULTILINE)
SIZES = {
    "large_read_bytes": 2 * 1024 * 1024,
    "shell_bytes": 5 * 1024 * 1024,
    # `expand --raw` is intentionally capped at 256 KiB. Keep this CAS
    # round-trip workload at that public boundary instead of relying on the
    # retired uncapped CLI behavior.
    "payload_bytes": 256 * 1024,
}
COUNTS = {"warm_reads": 20, "recovery_persists": 50}
BUDGETS = {"large_shell_capture": {"wall_seconds": 3.0, "cpu_seconds": 3.0, "max_rss_bytes": 96 * 1024 * 1024}}


def acquire_guard(command: str) -> None:
    try:
        GUARD.mkdir()
    except FileExistsError:
        pid_path = GUARD / "pid"
        try:
            pid = int(pid_path.read_text().strip())
            os.kill(pid, 0)
        except (FileNotFoundError, ValueError, ProcessLookupError):
            for child in GUARD.iterdir():
                if child.is_file():
                    child.unlink()
            GUARD.rmdir()
            GUARD.mkdir()
        except PermissionError as exc:
            raise SystemExit(f"heavy-process guard owner cannot be inspected: {exc}")
        else:
            raise SystemExit(f"heavy-process guard held by live pid {pid}")
    (GUARD / "pid").write_text(f"{os.getpid()}\n")
    (GUARD / "repository").write_text(f"{REPO}\n")
    (GUARD / "command").write_text(f"{command}\n")
    (GUARD / "started_at").write_text(time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()) + "\n")


def release_guard() -> None:
    if not GUARD.exists():
        return
    owner = (GUARD / "pid").read_text().strip() if (GUARD / "pid").exists() else ""
    if owner != str(os.getpid()):
        return
    for child in GUARD.iterdir():
        if child.is_file():
            child.unlink()
    GUARD.rmdir()


def timed(command: list[str], *, capture_stdout: bool = False) -> tuple[dict[str, float | int], str]:
    env = {**os.environ, "LC_ALL": "C", "LANG": "C"}
    started = time.perf_counter()
    proc = subprocess.run(
        ["/usr/bin/time", "-l", *command],
        cwd=REPO,
        env=env,
        stdout=subprocess.PIPE if capture_stdout else subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        timeout=60,
        check=False,
    )
    observed_wall = time.perf_counter() - started
    if proc.returncode != 0:
        raise RuntimeError(f"command failed ({proc.returncode}): {command!r}\n{proc.stderr[-2000:]}")
    tm = TIME_RE.search(proc.stderr)
    rss = RSS_RE.search(proc.stderr)
    if not tm or not rss:
        raise RuntimeError(f"could not parse /usr/bin/time -l output: {proc.stderr[-2000:]}")
    real, user, system = map(float, tm.groups())
    return {
        "wall_seconds": real,
        "observed_wall_seconds": round(observed_wall, 6),
        "user_seconds": user,
        "system_seconds": system,
        "cpu_seconds": user + system,
        "max_rss_bytes": int(rss.group(1)),
    }, proc.stdout if capture_stdout else ""


def aggregate(samples: list[dict[str, float | int]]) -> dict[str, object]:
    return {
        "samples": len(samples),
        "wall_seconds": round(sum(float(x["wall_seconds"]) for x in samples), 6),
        "cpu_seconds": round(sum(float(x["cpu_seconds"]) for x in samples), 6),
        "max_rss_bytes": max(int(x["max_rss_bytes"]) for x in samples),
        "per_sample": samples,
    }


def aggregate_replicates(label: str, runs: list[dict[str, object]]) -> dict[str, object]:
    result = {key: value for key, value in runs[0].items() if key != "workloads"}
    result.update(
        schema="tokenzero.perf-hotspots.aggregate.v2",
        label=label,
        replicate_count=len(runs),
    )
    workloads = {}
    first_workloads = runs[0]["workloads"]
    assert isinstance(first_workloads, dict)
    for name, first in first_workloads.items():
        assert isinstance(first, dict)
        workload = {
            key: value
            for key, value in first.items()
            if key not in {"wall_seconds", "cpu_seconds", "max_rss_bytes", "per_sample"}
        }
        for metric in ("wall_seconds", "cpu_seconds", "max_rss_bytes"):
            values = [run["workloads"][name][metric] for run in runs]
            workload[metric] = statistics.median(values)
            workload[f"{metric}_replicates"] = values
        workload["raw_runs"] = [run["workloads"][name] for run in runs]
        workloads[name] = workload
    result["workloads"] = workloads
    return result


def compare(
    baseline_size: int,
    candidate_size: int,
    baseline_revision: str,
    candidate_revision: str,
    baseline_zero_abi_source: str,
    candidate_zero_abi_source: str,
) -> Path:
    baseline = json.loads((EVIDENCE / "baseline.json").read_text())
    candidate = json.loads((EVIDENCE / "candidate.json").read_text())
    replicate_count = baseline.get("replicate_count")
    if replicate_count != candidate.get("replicate_count"):
        raise SystemExit("baseline and candidate replicate counts differ")
    rows = {}
    for name, baseline_workload in baseline["workloads"].items():
        rows[name] = {}
        for metric in ("wall_seconds", "cpu_seconds", "max_rss_bytes"):
            before = baseline_workload[metric]
            after = candidate["workloads"][name][metric]
            rows[name][metric] = {
                "baseline": before,
                "candidate": after,
                "absolute_delta": after - before,
                "percent_delta": None if before == 0 else round((after - before) / before * 100, 2),
            }
    size_delta = round((candidate_size - baseline_size) / baseline_size * 100, 2)
    max_wall = max(row["wall_seconds"]["percent_delta"] for row in rows.values())
    max_rss = max(row["max_rss_bytes"]["percent_delta"] for row in rows.values())
    result = {
        "schema": "tokenzero.perf-comparison.v2",
        "baseline": "baseline.json",
        "candidate": "candidate.json",
        "methodology_note": f"{replicate_count} complete baseline and {replicate_count} complete candidate runs; medians are compared. No run was discarded.",
        "identical_conditions": {
            "os": baseline["environment"]["os"],
            "cpu": baseline["environment"]["cpu"],
            "commands": baseline["commands"],
            "sizes": baseline["environment"]["sizes"],
            "counts": baseline["environment"]["counts"],
            "replicate_count": replicate_count,
            "baseline_hub_rev": baseline_revision,
            "candidate_hub_rev": candidate_revision,
            "baseline_zero_abi_source": baseline_zero_abi_source,
            "candidate_zero_abi_source": candidate_zero_abi_source,
        },
        "budget": {
            "max_regression_percent": 5.0,
            "metrics": ["median_wall_seconds", "median_max_rss_bytes", "binary_size_bytes"],
        },
        "rows": rows,
        "binary_size_bytes": {
            "baseline": baseline_size,
            "candidate": candidate_size,
            "absolute_delta": candidate_size - baseline_size,
            "percent_delta": size_delta,
        },
        "summary": {
            "max_wall_regression_percent": max_wall,
            "max_rss_regression_percent": max_rss,
            "binary_size_regression_percent": size_delta,
        },
        "verdict": "PASS" if max(max_wall, max_rss, size_delta) <= 5.0 else "FAIL",
    }
    destination = EVIDENCE / "comparison.json"
    destination.write_text(json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n")
    return destination


def deterministic_text(size: int, seed: str) -> str:
    line = f"{seed}:0123456789abcdef: TokenZero deterministic performance corpus\n"
    return (line * (size // len(line) + 1))[:size]


def environment() -> dict[str, object]:
    def output(command: list[str]) -> str:
        return subprocess.run(command, cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
    return {
        "os": platform.platform(),
        "machine": platform.machine(),
        "cpu": output(["sysctl", "-n", "machdep.cpu.brand_string"]),
        "logical_cpus": os.cpu_count(),
        "commit": output(["git", "rev-parse", "HEAD"]),
        "binary": str(BIN.relative_to(REPO)),
        "binary_mtime_ns": BIN.stat().st_mtime_ns,
        "python": platform.python_version(),
        "sizes": SIZES,
        "counts": COUNTS,
        "time_command": "/usr/bin/time -l",
    }


def run(label: str) -> Path:
    if not BIN.is_file():
        raise SystemExit("target/debug/tokenzero is missing; build it once before running the harness")
    acquire_guard(f"perf_hotspots.py --label {label}")
    try:
        with tempfile.TemporaryDirectory(prefix="tokenzero-perf-") as raw_tmp:
            tmp = Path(raw_tmp)
            large = tmp / "large.txt"
            large.write_text(deterministic_text(SIZES["large_read_bytes"], "read"))
            allowed = str(tmp)

            cold_cache = tmp / "cold.json"
            cold, _ = timed([str(BIN), "read", str(large), "--allowed-root", allowed, "--cache-path", str(cold_cache), "--json"])
            warm_samples = [timed([str(BIN), "read", str(large), "--allowed-root", allowed, "--cache-path", str(cold_cache), "--json"])[0] for _ in range(COUNTS["warm_reads"])]

            shell_cache = tmp / "shell.json"
            producer = f"import sys;sys.stdout.write('S'*{SIZES['shell_bytes']})"
            shell, _ = timed([str(BIN), "run", "--allowed-root", allowed, "--cache-path", str(shell_cache), "--json", "--", sys.executable, "-c", producer])

            payload = tmp / "payload.txt"
            payload.write_text(deterministic_text(SIZES["payload_bytes"], "payload"))
            payload_cache = tmp / "payload.json"
            ingest, stdout = timed([str(BIN), "ingest", str(payload), "--allowed-root", allowed, "--cache-path", str(payload_cache), "--json"], capture_stdout=True)
            ingest_json = json.loads(stdout)
            refs = ingest_json["refs"]
            blob_ref = next(
                item if isinstance(item, str) else item["ref"]
                for item in refs
                if (item if isinstance(item, str) else item.get("ref", "")).startswith("tz://blob/")
            )
            expand, _ = timed([str(BIN), "expand", blob_ref, "--cache-path", str(payload_cache), "--raw"])

            persist_cache = tmp / "persist.json"
            persist_samples = []
            for idx in range(COUNTS["recovery_persists"]):
                small = tmp / f"small-{idx:02}.txt"
                small.write_text(deterministic_text(512, f"persist-{idx:02}"))
                metric, _ = timed([str(BIN), "ingest", str(small), "--allowed-root", allowed, "--cache-path", str(persist_cache), "--json"])
                persist_samples.append(metric)

            result = {
                "schema": "tokenzero.perf-hotspots.v1",
                "label": label,
                "environment": environment(),
                "commands": {
                    "cold_warm_read": "tokenzero read <2MiB file>; 1 cold + 20 warm processes",
                    "large_shell_capture": "tokenzero run -- python3 -c <5MiB deterministic stdout>",
                    "ingest_expand": "tokenzero ingest <256KiB file>; tokenzero expand <blob-ref> --raw",
                    "repeated_recovery_persist": "50 tokenzero ingest calls with distinct 512-byte files and one cache",
                },
                "workloads": {
                    "cold_read": {**cold, "samples": 1},
                    "warm_reads": aggregate(warm_samples),
                    "large_shell_capture": {**shell, "samples": 1},
                    "large_payload_ingest": {**ingest, "samples": 1},
                    "large_payload_expand": {**expand, "samples": 1},
                    "repeated_recovery_persist": aggregate(persist_samples),
                },
            }
            EVIDENCE.mkdir(parents=True, exist_ok=True)
            destination = EVIDENCE / f"{label}.json"
            destination.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
            return destination
    finally:
        release_guard()


def sample_shell_hotspot() -> None:
    if not BIN.is_file():
        raise SystemExit("target/debug/tokenzero is missing")
    acquire_guard("perf_hotspots.py --sample-shell")
    try:
        with tempfile.TemporaryDirectory(prefix="tokenzero-sample-") as raw_tmp:
            tmp = Path(raw_tmp)
            producer = "import sys,time;chunk=b'S'*(5*1024*1024//100);[(sys.stdout.buffer.write(chunk),sys.stdout.buffer.flush(),time.sleep(.02)) for _ in range(100)]"
            child = subprocess.Popen(
                [str(BIN), "run", "--allowed-root", str(tmp), "--cache-path", str(tmp / "shell.json"), "--json", "--", sys.executable, "-c", producer],
                cwd=REPO,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            time.sleep(0.25)
            EVIDENCE.mkdir(parents=True, exist_ok=True)
            sample_path = EVIDENCE / "baseline-shell.sample.txt"
            sampled = subprocess.run(
                ["/usr/bin/sample", str(child.pid), "1", "1", "-file", str(sample_path)],
                capture_output=True,
                text=True,
                timeout=10,
            )
            rc = child.wait(timeout=15)
            if sampled.returncode != 0 or rc != 0:
                raise RuntimeError(f"sample rc={sampled.returncode}, workload rc={rc}: {sampled.stderr}")
            print(sample_path.relative_to(REPO))
    finally:
        release_guard()


def check_budget() -> None:
    budgets = BUDGETS
    check_path = run("budget-check")
    measured = json.loads(check_path.read_text())["workloads"]
    failures = []
    for name, limits in budgets.items():
        for metric, limit in limits.items():
            actual = measured[name][metric]
            if actual > limit:
                failures.append(f"{name}.{metric}: {actual} > {limit}")
    check_path.unlink()
    if failures:
        raise SystemExit("performance budget exceeded:\n" + "\n".join(failures))
    print("performance budget passed")

def main() -> int:
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--label", choices=("baseline", "candidate"))
    group.add_argument("--compare", action="store_true")
    group.add_argument("--check-budget", action="store_true")
    group.add_argument("--sample-shell", action="store_true")
    parser.add_argument("--replicates", type=int, default=1)
    parser.add_argument("--baseline-size", type=int)
    parser.add_argument("--candidate-size", type=int)
    parser.add_argument("--baseline-revision")
    parser.add_argument("--candidate-revision")
    parser.add_argument("--baseline-zero-abi-source")
    parser.add_argument("--candidate-zero-abi-source")
    args = parser.parse_args()
    if args.check_budget:
        check_budget()
    elif args.sample_shell:
        sample_shell_hotspot()
    elif args.compare:
        required = {
            "baseline-size": args.baseline_size,
            "candidate-size": args.candidate_size,
            "baseline-revision": args.baseline_revision,
            "candidate-revision": args.candidate_revision,
            "baseline-zero-abi-source": args.baseline_zero_abi_source,
            "candidate-zero-abi-source": args.candidate_zero_abi_source,
        }
        missing = [name for name, value in required.items() if value is None]
        if missing:
            parser.error(f"--compare requires: {', '.join(missing)}")
        print(
            compare(
                args.baseline_size,
                args.candidate_size,
                args.baseline_revision,
                args.candidate_revision,
                args.baseline_zero_abi_source,
                args.candidate_zero_abi_source,
            ).relative_to(REPO)
        )
    else:
        if args.replicates < 1:
            parser.error("--replicates must be >= 1")
        runs = []
        destination = None
        for _ in range(args.replicates):
            destination = run(args.label)
            runs.append(json.loads(destination.read_text()))
        if args.replicates > 1:
            destination.write_text(
                json.dumps(
                    aggregate_replicates(args.label, runs),
                    sort_keys=True,
                    separators=(",", ":"),
                )
                + "\n"
            )
        print(destination.relative_to(REPO))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
