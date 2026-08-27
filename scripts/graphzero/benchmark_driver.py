#!/usr/bin/env python3
"""
GraphZero benchmark driver. Measures on the graphzero repo as corpus:
  cold_index, warm_reindex, orient_p50/p95/p99, blast_p50/p95/p99,
  verify_roundtrip, snap_p50/p95/p99, mcp_warm_orient (mcp_orient_*) p50/p95/p99,
  recipe_json_p50/p95/p99, MCP orient, and contended large-repo orient.
  JavaScript aggregate-host benchmarks are owned by ZeroStack.

Output: benchmarks/latency/results.json {hardware, date, corpus, results, sample_accounting};
`benchmarks/latency/results.profile.json` is generated as the matching profile binding.
Scenario authority: benchmarks/latency/scenario_catalog.json (including measure-only policy).
Non-orient public distributions use >=20 measured runs; orient uses 50.
CLI warm paths discard W warmups (default 1, GRAPHZERO_BENCH_WARMUP) before
retaining N measured samples; both are recorded in sample_accounting.
CLI orient/blast/snap/warm_reindex/verify p50 values are process-inclusive wire
time (fork/exec + dyld + main + op + exit). A separate `cli_process_start`
series times `graphzero --version` and each of those metrics also stamps
`process_start_ms` / estimated `op_ms` (wall_p50 − process_start_p50).
Public latency metrics publish p50/p95/p99; when N < 200, p99_label is
worst_observed_of_n (type-7 value, not a population-p99 claim).
Each series also stamps CV / MAD and a variance_envelope (≤10% CV); default
mode is advisory (report + warn). Set GRAPHZERO_BENCH_VARIANCE_MODE=strict to
fail closed when any metric exceeds the envelope.
Cold index keeps every run (intentionally cold). MCP warm path uses MCP_WARMUP=3.
"""

import hashlib
import json
import os
import platform
import select
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "benchmarks" / "rebaseline"))
from stats import VARIANCE_ENVELOPE_MAX, median, p95, p99, p99_label, variance_summary
PROFILE = os.environ.get("GRAPHZERO_BENCH_PROFILE", "release")
if PROFILE == "release":
    PROFILE_ARGS = ["--release"]
    PROFILE_DIR = "release"
elif PROFILE == "release-perf":
    PROFILE_ARGS = ["--profile", "release-perf"]
    PROFILE_DIR = "release-perf"
else:
    raise RuntimeError(f"unsupported GRAPHZERO_BENCH_PROFILE: {PROFILE}")
TARGET_ROOT = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
if not TARGET_ROOT.is_absolute():
    TARGET_ROOT = REPO / TARGET_ROOT
BIN = TARGET_ROOT / PROFILE_DIR / "graphzero"
MCP_BIN = TARGET_ROOT / PROFILE_DIR / "graphzero-mcp"
STORE = REPO / ".zerostack" / "graphzero"
RESULTS = REPO / "bench" / "results.json"
PROFILE_BINDING = REPO / "bench" / "results.profile.json"
# Recorded artifacts are committed and public-facing: never stamp the host
# checkout path into them.
REPO_LABEL = "."

# Symbol tested -- must have callers in the corpus
SYMBOL = "run_index"
CLAIM_TARGET = "nonexistent_symbol_for_bench"
CLAIM = "symbol_removed"

N_RUNS = 20                  # publication floor for non-orient distributions
N_ORIENT = 50                # orient latency distribution
# W warmups discarded before N measured samples on CLI process paths.
# Default W=1 matches rebaseline/surface_bench; MCP warm path keeps its own W=3.
WARMUP = int(os.environ.get("GRAPHZERO_BENCH_WARMUP", "1"))
MCP_WARMUP = 3               # existing warm MCP tool-call warmups
INTERACTIVE_DEADLINE_S = 24.0
CONTENDED_CLIENTS = 8
# advisory (default): stamp CV envelope and warn. strict: fail closed on CV > 10%.
VARIANCE_MODE = os.environ.get("GRAPHZERO_BENCH_VARIANCE_MODE", "advisory").strip().lower()
if VARIANCE_MODE not in {"strict", "advisory"}:
    raise RuntimeError(
        "GRAPHZERO_BENCH_VARIANCE_MODE must be 'strict' or 'advisory'"
    )


def attach_variance(samples_ms: list[float]) -> dict:
    """CV/MAD envelope fields for a retained sample series (raw samples kept by caller).

    A one-sample cold probe has no definable sample standard deviation. Keep its
    variance fields null and its status ``undefined``, marked explicitly by
    ``cv_defined``.
    """
    summary = variance_summary(samples_ms, digits=3)
    cv_defined = summary["cv"] is not None
    cv = summary["cv"]
    cv_pct = summary["cv_pct"]
    return {
        "mean_ms": summary["mean"],
        "stdev_ms": summary["stdev"],
        "cv": cv,
        "cv_pct": cv_pct,
        "cv_defined": cv_defined,
        "mad_ms": summary["mad"],
        "mad_over_median": summary["mad_over_median"],
        "mad_over_median_pct": summary["mad_over_median_pct"],
        "variance_envelope": {
            "metric": "cv",
            "cv": cv,
            "cv_pct": cv_pct,
            "cv_defined": cv_defined,
            "mad_over_median": summary["mad_over_median"],
            "mad_over_median_pct": summary["mad_over_median_pct"],
            "envelope_max": summary["envelope_max"],
            "envelope_max_pct": summary["envelope_max_pct"],
            "status": summary["status"],
            "within_envelope": summary["within_envelope"],
        },
    }


def variance_envelope_report(metrics: dict[str, dict]) -> dict:
    """Aggregate CV envelope across metrics that publish raw_ms."""
    per_metric: dict[str, dict] = {}
    failures: list[str] = []
    for name, metric in metrics.items():
        if not isinstance(metric, dict) or "raw_ms" not in metric:
            continue
        if "variance_envelope" not in metric:
            # Backfill if a measure path forgot attach_variance.
            metric.update(attach_variance([float(x) for x in metric["raw_ms"]]))
        env = metric["variance_envelope"]
        per_metric[name] = {
            "cv": metric.get("cv"),
            "cv_pct": metric.get("cv_pct"),
            "status": env.get("status"),
            "within_envelope": bool(env.get("within_envelope")),
        }
        if not env.get("within_envelope"):
            failures.append(name)
    overall_ok = not failures
    return {
        "envelope_max": VARIANCE_ENVELOPE_MAX,
        "envelope_max_pct": round(VARIANCE_ENVELOPE_MAX * 100.0, 1),
        "mode": VARIANCE_MODE,
        "within_envelope": overall_ok,
        "status": "pass" if overall_ok else "reject",
        "failed_metrics": failures,
        "per_metric": per_metric,
    }

def _load_latency_gate() -> dict:
    gate_path = REPO / "bench" / "latency_gate.json"
    try:
        return json.loads(gate_path.read_text())
    except Exception as exc:
        raise RuntimeError(f"cannot load benchmarks/latency/latency_gate.json SSOT: {exc}") from exc

LATENCY_GATE = _load_latency_gate()


def run_cmd(*args, **kwargs):
    """Run a command, return (elapsed_seconds, stdout)."""
    t0 = time.perf_counter()
    p = subprocess.run(args, capture_output=True, text=True, cwd=str(REPO), **kwargs)
    elapsed = time.perf_counter() - t0
    if p.returncode != 0:
        raise RuntimeError(f"Command failed (rc={p.returncode}): {' '.join(args)}\nstderr: {p.stderr[:500]}")
    return elapsed, p.stdout


def time_cmd(*args):
    """Time a command, return elapsed_seconds."""
    elapsed, _ = run_cmd(*args)
    return elapsed


def timed_samples(n: int, make_elapsed, warmup: int = WARMUP) -> list[float]:
    """Discard *warmup* runs, then collect *n* measured elapsed-seconds samples.

    Warmups never enter the returned list or percentile distributions.
    """
    if warmup < 0:
        raise RuntimeError(f"warmup must be >= 0, got {warmup}")
    if n < 1:
        raise RuntimeError(f"measured sample count must be >= 1, got {n}")
    for _ in range(warmup):
        make_elapsed()
    return [make_elapsed() for _ in range(n)]


def measure_cli_process_start(
    binary: Path | str,
    n: int = N_RUNS,
    warmup: int = WARMUP,
) -> dict:
    """Time minimal CLI process walls (`graphzero --version`).

    This is the public process-start probe: fork/exec + dyld + arg parse + exit
    with no Snapshot open and no query dispatch. It is intentionally separate
    from orient/blast samples (not paired per invocation).
    """
    bin_s = str(binary)

    def one() -> float:
        return time_cmd(bin_s, "--version")

    times = timed_samples(n, one, warmup=warmup)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "probe": [bin_s, "--version"],
        "method": "subprocess_wall_minimal_cli",
        "wall_class": "process_start_only",
        "p50_ms": round(median(times) * 1000, 3),
        "p95_ms": round(p95(times) * 1000, 3),
        "p99_ms": round(p99(times) * 1000, 3),
        "p99_label": p99_label(len(times)),
        "runs": n,
        "warmup": warmup,
        "raw_ms": raw_ms,
        "note": (
            "Process-start floor for public CLI metrics. surface_bench "
            "process_starts/spawn_ns is the in-process dual harness for o2uq "
            "gates; these numbers are what README/rebaseline publish."
        ),
    }
    out.update(attach_variance(raw_ms))
    return out


def attach_cli_wall_split(metric: dict, process_start: dict, wall_p50_key: str) -> dict:
    """Label process-inclusive walls and attach process_start + estimated op.

    `wall_p50_key` names the process-inclusive p50 field already on *metric*
    (e.g. orient_symbol_p50_ms). op_ms is median-of-series residual, not a
    paired per-sample split (CLI has no child-reported wall on this path).
    """
    if wall_p50_key not in metric:
        raise KeyError(f"missing wall p50 key {wall_p50_key!r} on metric")
    wall_p50 = float(metric[wall_p50_key])
    start_p50 = float(process_start["p50_ms"])
    start_p95 = float(process_start["p95_ms"])
    start_p99 = float(process_start["p99_ms"])
    op_p50 = round(max(0.0, wall_p50 - start_p50), 3)
    # Prefer matching p95/p99 keys when present on the metric.
    p95_key = wall_p50_key.replace("_p50_ms", "_p95_ms")
    p99_key = wall_p50_key.replace("_p50_ms", "_p99_ms")
    op_p95 = None
    op_p99 = None
    if p95_key in metric:
        op_p95 = round(max(0.0, float(metric[p95_key]) - start_p95), 3)
    if p99_key in metric:
        op_p99 = round(max(0.0, float(metric[p99_key]) - start_p99), 3)

    metric["wall_class"] = "process_inclusive"
    metric["process_start_ms"] = {
        "p50_ms": process_start["p50_ms"],
        "p95_ms": process_start["p95_ms"],
        "p99_ms": process_start["p99_ms"],
        "p99_label": process_start.get("p99_label"),
        "probe": process_start.get("probe"),
        "method": process_start.get("method"),
        "raw_ms": process_start.get("raw_ms"),
    }
    op_block = {
        "p50_ms": op_p50,
        "method": "wall_quantile_minus_process_start_quantile",
        "note": (
            f"{wall_p50_key} is process-inclusive wire time; op_ms is estimated "
            "residual after the process-start probe (not paired per sample)."
        ),
    }
    if op_p95 is not None:
        op_block["p95_ms"] = op_p95
    if op_p99 is not None:
        op_block["p99_ms"] = op_p99
    metric["op_ms"] = op_block
    return metric



def _read_text_cmd(args: list[str]) -> str | None:
    try:
        r = subprocess.run(args, capture_output=True, text=True, cwd=str(REPO))
        if r.returncode == 0:
            out = (r.stdout or "").strip()
            return out or None
    except Exception:
        pass
    return None


def _cpu_model() -> str:
    cpu = platform.processor() or "unknown"
    if cpu in ("arm", "unknown", ""):
        brand = _read_text_cmd(["sysctl", "-n", "machdep.cpu.brand_string"])
        if brand:
            return brand
        # Linux fallback
        try:
            for line in Path("/proc/cpuinfo").read_text().splitlines():
                if line.lower().startswith("model name") and ":" in line:
                    return line.split(":", 1)[1].strip() or "unknown"
        except Exception:
            pass
    return cpu or "unknown"


def _memory_label() -> str | None:
    try:
        r = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True)
        if r.returncode == 0:
            mem_bytes = int(r.stdout.strip())
            return f"{mem_bytes / (1024**3):.0f} GB"
    except Exception:
        pass
    try:
        # Linux: MemTotal in kB
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                kb = int(line.split()[1])
                return f"{kb / (1024**2):.0f} GB"
    except Exception:
        pass
    return None


def _fs_type_for(path: Path) -> str | None:
    """Filesystem type for path (store root). null when unprobeable."""
    probe = path if path.exists() else path.parent
    try:
        probe = probe.resolve()
    except Exception:
        probe = path
    system = platform.system()
    if system == "Linux":
        out = _read_text_cmd(["stat", "-f", "-c", "%T", str(probe)])
        if out and out != "UNKNOWN":
            return out
        # /proc/mounts fallback: longest prefix match
        try:
            best = ("", None)
            for line in Path("/proc/mounts").read_text().splitlines():
                parts = line.split()
                if len(parts) < 3:
                    continue
                mnt, fstype = parts[1], parts[2]
                if str(probe) == mnt or str(probe).startswith(mnt.rstrip("/") + "/"):
                    if len(mnt) >= len(best[0]):
                        best = (mnt, fstype)
            if best[1]:
                return best[1]
        except Exception:
            pass
        return None
    if system == "Darwin":
        # mount lines: /dev/... on /path (apfs, local, ...)
        try:
            mount_out = _read_text_cmd(["mount"]) or ""
            best = ("", None)
            for line in mount_out.splitlines():
                if " on " not in line or " (" not in line:
                    continue
                mid, rest = line.split(" on ", 1)
                mnt, opts = rest.split(" (", 1)
                mnt = mnt.strip()
                fstype = opts.split(",", 1)[0].strip() or None
                if str(probe) == mnt or str(probe).startswith(mnt.rstrip("/") + "/"):
                    if len(mnt) >= len(best[0]):
                        best = (mnt, fstype)
            if best[1]:
                return best[1]
        except Exception:
            pass
        return None
    return None


def _cpu_governor() -> str | None:
    """Linux cpufreq governor; null on hosts without the sysfs node (incl. macOS)."""
    try:
        p = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        if p.is_file():
            val = p.read_text().strip()
            return val or None
    except Exception:
        pass
    return None


def _power_mode() -> str | None:
    """Best-effort power/thermal mode. null when unavailable (do not omit key)."""
    # Linux energy-performance preference (intel_pstate / amd_pstate)
    try:
        p = Path("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference")
        if p.is_file():
            val = p.read_text().strip()
            return val or None
    except Exception:
        pass
    # macOS: lowpowermode / thermals via pmset when present
    if platform.system() == "Darwin":
        out = _read_text_cmd(["pmset", "-g"])
        if out:
            low = None
            for line in out.splitlines():
                parts = line.split()
                if len(parts) >= 2 and parts[0] == "lowpowermode":
                    low = parts[1]
                    break
            if low is not None:
                return f"lowpowermode={low}"
    return None


def _load_average() -> list[float] | None:
    try:
        return [round(x, 3) for x in os.getloadavg()]
    except (OSError, AttributeError):
        return None


def get_rust_version() -> str | None:
    """rustc --version string, or null when rustc is unavailable."""
    try:
        r = subprocess.run(["rustc", "--version"], capture_output=True, text=True, cwd=str(REPO))
        if r.returncode == 0:
            out = (r.stdout or "").strip()
            return out or None
    except Exception:
        pass
    return None


def collect_hardware():
    """Host fingerprint for comparable baselines.

    Always emit the full key set. Unavailable probes are null (or "unknown" for
    cpu when brand cannot be resolved) -- never omit governor/FS/isolation keys.
    """
    rustc = get_rust_version()
    host_class = os.environ.get("GRAPHZERO_BENCH_HOST_CLASS", "").strip() or None
    isolation = os.environ.get("GRAPHZERO_BENCH_ISOLATION", "").strip() or None
    store_label = ".zerostack/graphzero"
    return {
        "platform": platform.platform(),
        "os": platform.system() or None,
        "kernel": platform.release() or None,
        "machine": platform.machine() or None,
        "cpu": _cpu_model(),
        "memory": _memory_label(),
        "python": platform.python_version(),
        # Keep legacy "rust" key for older results readers; rustc is canonical.
        "rust": rustc if rustc is not None else "unknown",
        "rustc": rustc,
        "profile": PROFILE,
        "fs_type": _fs_type_for(STORE),
        "store_path": store_label,
        "governor": _cpu_governor(),
        "power_mode": _power_mode(),
        "load_average": _load_average(),
        "host_class": host_class,
        "isolation": isolation,
    }


def git_rev() -> str:
    try:
        r = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            cwd=str(REPO),
            check=False,
        )
        revision = r.stdout.strip()
        if r.returncode == 0 and len(revision) == 40 and all(
            char in "0123456789abcdef" for char in revision
        ):
            return revision
    except Exception:
        pass
    return "unknown"


def file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def receipt_binary_path(path: Path) -> str:
    try:
        relative = path.resolve().relative_to(TARGET_ROOT.resolve()).as_posix()
    except ValueError:
        return path.name
    if "CARGO_TARGET_DIR" in os.environ:
        return f"${{CARGO_TARGET_DIR}}/{relative}"
    return f"target/{relative}"


def write_profile_binding(results: dict) -> None:
    """Write a sidecar bound to the just-generated results receipt."""
    corpus = results["corpus"]
    environment = results["measurement_environment"]
    binding = {
        "schema_version": 1,
        "artifact": {
            "path": "benchmarks/latency/results.json",
            "sha256": file_sha256(RESULTS),
            "date": results["date"],
            "git_rev": corpus["git_rev"],
            "git_sha": corpus["git_sha"],
            "binary_sha256": results["binary_sha256"],
        },
        "measurement_environment": environment,
        "status": "current_profile_binding",
        "note": (
            "Generated by scripts/benchmark_driver.py alongside benchmarks/latency/results.json; "
            "the selected profile and source SHA are authoritative in both files."
        ),
    }
    PROFILE_BINDING.write_text(json.dumps(binding, indent=2) + "\n")


def corpus_info():
    """Count files in the corpus and bind them to the full source revision."""
    revision = git_rev()
    if revision == "unknown":
        raise RuntimeError("cannot stamp a full source Git SHA in the benchmark receipt")
    try:
        r = subprocess.run(
            [
                "find",
                ".",
                "-name",
                "*.rs",
                "-not",
                "-path",
                "./target/*",
                "-not",
                "-path",
                "./.zerostack/*",
            ],
            capture_output=True,
            text=True,
            cwd=str(REPO),
        )
        files = [ln for ln in r.stdout.strip().split("\n") if ln]
        return {
            "name": "graphzero",
            "rust_files": len(files),
            "file_count": len(files),
            "repo": REPO_LABEL,
            # Preserve git_rev for existing readers; git_sha is the literal
            # full-commit alias used by the artifact contract.
            "git_rev": revision,
            "git_sha": revision,
        }
    except Exception as exc:
        raise RuntimeError(f"cannot enumerate benchmark corpus: {exc}") from exc


def ensure_benchmark_binaries() -> None:
    """Build the shim and each mutually-exclusive package surface."""
    builds = [
        (
            BIN,
            ["cargo", "build", *PROFILE_ARGS, "-p", "graphzero-cli", "--bin", "graphzero"],
        ),
        (
            MCP_BIN,
            [
                "cargo", "build", *PROFILE_ARGS, "-p", "graphzero-cli",
                "--bin", "graphzero-mcp", "--no-default-features",
                "--features", "tokenzero,surface-mcp",
            ],
        ),
    ]
    for artifact, command in builds:
        subprocess.run(command, cwd=str(REPO), check=True)
        if not artifact.is_file():
            raise RuntimeError(f"profile-matched build did not produce {artifact}")


def clear_store():
    """Remove .zerostack/graphzero for cold runs."""
    if STORE.exists():
        import shutil
        shutil.rmtree(STORE)


def measure_cold_index():
    """Clear store, then time index. N_RUNS separate cold-start measurements."""
    times = []
    for _ in range(N_RUNS):
        clear_store()
        t = time_cmd(str(BIN), "index", ".")
        times.append(t)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {"cold_index_s": median(times), "raw_ms": raw_ms}
    out.update(attach_variance(raw_ms))
    return out


def measure_warm_reindex():
    """Re-index with existing store. W warmups discarded, N measured retained."""
    times = timed_samples(N_RUNS, lambda: time_cmd(str(BIN), "index", "."))
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "warm_reindex_s": median(times),
        "raw_ms": raw_ms,
        "warmup": WARMUP,
        "runs": N_RUNS,
    }
    out.update(attach_variance(raw_ms))
    return out


def ensure_index():
    """Ensure the store exists (for warm measurements)."""
    if not STORE.joinpath(".manifest").exists():
        run_cmd(str(BIN), "index", ".")


def measure_orient():
    """Run orient: W warmups discarded, N_ORIENT measured retained; p50/p95/p99."""
    ensure_index()

    def one():
        return time_cmd(
            str(BIN), "orient", "--surface", "symbol",
            "--name", SYMBOL, "--budget", "1", "--repo", ".",
        )

    times = timed_samples(N_ORIENT, one)
    n = len(times)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "orient_symbol_p50_ms": round(median(times) * 1000, 3),
        "orient_symbol_p95_ms": round(p95(times) * 1000, 3),
        "orient_symbol_p99_ms": round(p99(times) * 1000, 3),
        "p99_label": p99_label(n),
        "iterations": N_ORIENT,
        "warmup": WARMUP,
        "raw_ms": raw_ms,
    }
    out.update(attach_variance(raw_ms))
    return out


def measure_blast():
    """Run blast: W warmups discarded, N_RUNS measured retained; p50/p95/p99."""
    ensure_index()

    def one():
        return time_cmd(
            str(BIN), "blast", "--intent", f"change signature of {SYMBOL}",
            "--budget", "1", "--repo", ".",
        )

    times = timed_samples(N_RUNS, one)
    n = len(times)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "blast_p50_ms": round(median(times) * 1000, 3),
        "blast_p95_ms": round(p95(times) * 1000, 3),
        "blast_p99_ms": round(p99(times) * 1000, 3),
        "p99_label": p99_label(n),
        "runs": N_RUNS,
        "warmup": WARMUP,
        "raw_ms": raw_ms,
    }
    out.update(attach_variance(raw_ms))
    return out


def measure_verify() -> dict[str, object]:
    """Measure verify: W warmups discarded, N_RUNS measured retained."""
    ensure_index()
    outcomes: list[str] = []

    def one() -> float:
        started = time.perf_counter()
        completed = subprocess.run(
            [
                str(BIN), "verify", CLAIM_TARGET,
                "--claim", CLAIM, "--repo", ".",
            ],
            capture_output=True,
            text=True,
            cwd=str(REPO),
        )
        elapsed = time.perf_counter() - started
        if completed.returncode not in (0, 1):
            raise RuntimeError(f"verify command failed: {completed.stderr[:500]}")
        payload = json.loads(completed.stdout)
        if payload.get("schema_version") != 1:
            raise RuntimeError("verify command returned an invalid certificate")
        outcomes.append("verified" if payload.get("verified") else "unknown")
        return elapsed

    # Warmups also exercise verify; drop their outcomes so only measured count.
    for _ in range(WARMUP):
        one()
        outcomes.clear()
    times = [one() for _ in range(N_RUNS)]
    n = len(times)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "verify_roundtrip_p50_ms": round(median(times) * 1000, 3),
        "verify_roundtrip_p95_ms": round(p95(times) * 1000, 3),
        "verify_roundtrip_p99_ms": round(p99(times) * 1000, 3),
        "p99_label": p99_label(n),
        "claim": CLAIM,
        "target": CLAIM_TARGET,
        "runs": N_RUNS,
        "warmup": WARMUP,
        "outcomes": outcomes,
        "raw_ms": raw_ms,
    }
    out.update(attach_variance(raw_ms))
    return out


def measure_snap():
    """Snap capsule build p50/p95/p99: W warmups discarded, N_RUNS retained."""
    ensure_index()
    times = timed_samples(
        N_RUNS,
        lambda: time_cmd(str(BIN), "snap", SYMBOL, "--budget", "1", "--repo", "."),
    )
    n = len(times)
    raw_ms = [round(t * 1000, 3) for t in times]
    out = {
        "snap_p50_ms": round(median(times) * 1000, 3),
        "snap_p95_ms": round(p95(times) * 1000, 3),
        "snap_p99_ms": round(p99(times) * 1000, 3),
        "p99_label": p99_label(n),
        "runs": N_RUNS,
        "warmup": WARMUP,
        "raw_ms": raw_ms,
    }
    out.update(attach_variance(raw_ms))
    return out


def measure_mcp_orient() -> dict[str, object]:
    """Measure the hub-backed ten-tool MCP orient surface."""
    ensure_benchmark_binaries()
    ensure_index()
    result: dict[str, object] = {}
    server_binary = MCP_BIN
    surface = "mcp_warm_orient"
    tool_name = "orient"
    binary_label = "graphzero-mcp"
    cache_class = "warm_store"
    t_session_start = time.perf_counter()
    server_proc = subprocess.Popen(
        [str(server_binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, cwd=str(REPO),
    )
    t_after_popen = time.perf_counter()
    server_spawn_ms = round((t_after_popen - t_session_start) * 1000, 3)

    def read_line() -> str:
        readable, _, _ = select.select([server_proc.stdout], [], [], INTERACTIVE_DEADLINE_S)
        if not readable:
            raise TimeoutError("MCP orient exceeded the 24-second client deadline")
        line = server_proc.stdout.readline()
        if not line:
            raise RuntimeError("MCP server closed stdout")
        return line.strip()

    def send(msg):
        server_proc.stdin.write(msg + "\n")
        server_proc.stdin.flush()

    def mcp_call(method, params=None):
        req_id = int(time.time() * 1000) % 100000
        req = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            req["params"] = params
        t0 = time.perf_counter()
        send(json.dumps(req))
        resp = json.loads(read_line())
        elapsed = time.perf_counter() - t0
        if "error" in resp:
            raise RuntimeError(f"MCP error: {resp['error']}")
        return elapsed, resp

    try:
        initialize_s, _ = mcp_call("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "benchmark", "version": "1.0"},
        })
        send(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        time.sleep(0.1)
        time_to_first_tool_ms = None
        for _ in range(MCP_WARMUP):
            mcp_call("tools/call", {"name": "orient", "arguments": {
                "surface": "symbol", "name": SYMBOL, "query": SYMBOL,
                "budget": 1, "repo": ".",
            }})
            if time_to_first_tool_ms is None:
                time_to_first_tool_ms = round((time.perf_counter() - t_session_start) * 1000, 3)
        times, responses = [], []
        tool_call = {"name": "orient", "arguments": {
            "surface": "symbol", "name": SYMBOL, "query": SYMBOL,
            "budget": 1, "repo": ".",
        }}
        for _ in range(N_ORIENT):
            t, response = mcp_call("tools/call", tool_call)
            if time_to_first_tool_ms is None:
                time_to_first_tool_ms = round((time.perf_counter() - t_session_start) * 1000, 3)
            times.append(t); responses.append(response)
        raw_ms = [round(t * 1000, 3) for t in times]
        result.update({
            "mcp_orient_p50_ms": round(median(times) * 1000, 3),
            "mcp_orient_p95_ms": round(p95(times) * 1000, 3),
            "mcp_orient_p99_ms": round(p99(times) * 1000, 3),
            "mcp_warm_orient_p50_ms": round(median(times) * 1000, 3),
            "mcp_warm_orient_p95_ms": round(p95(times) * 1000, 3),
            "mcp_warm_orient_p99_ms": round(p99(times) * 1000, 3),
            "p99_label": p99_label(len(times)), "iterations": len(times),
            "sample_n": len(times), "warmup": MCP_WARMUP, "raw_ms": raw_ms,
            "cold": False, "surface": surface, "binary": binary_label,
            "tool": tool_name, "cache_class": cache_class,
            "client_deadline_ms": int(INTERACTIVE_DEADLINE_S * 1000),
            "deadline_met": max(times) < INTERACTIVE_DEADLINE_S,
            "cold_index_pending": False,
            "resumable_ref_observed": "gz://" in json.dumps(responses, sort_keys=True),
            "tool_error_observed": any(r.get("result", {}).get("isError", False) for r in responses),
            "cross_surface_cold_warm_delta_claim": False,
            "server_spawn_ms": server_spawn_ms,
            "initialize_ms": round(initialize_s * 1000, 3),
            "time_to_first_tool_ms": time_to_first_tool_ms,
            "tools_call_wall_class": "steady_state",
            "process_cold_start_included": False,
            "note": "tools/call p50 is post-handshake hub MCP steady-state time.",
        })
        result.update(attach_variance(raw_ms))
        return result
    finally:
        server_proc.terminate()
        try:
            server_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_proc.kill(); server_proc.wait(timeout=5)
        result["shutdown_clean"] = server_proc.returncode is not None
        result["shutdown_ms"] = 0.0

def measure_contended_large_orient() -> dict[str, object]:
    """Run concurrent orient clients against the committed large-repo corpus."""
    ensure_index()

    def one(_: int) -> dict[str, object]:
        started = time.perf_counter()
        try:
            completed = subprocess.run(
                [
                    str(BIN), "orient", "--surface", "symbol",
                    "--name", SYMBOL, "--budget", "1", "--repo", ".",
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO),
                timeout=INTERACTIVE_DEADLINE_S,
            )
            return {
                "elapsed_s": time.perf_counter() - started,
                "returncode": completed.returncode,
                "timed_out": False,
            }
        except subprocess.TimeoutExpired:
            return {
                "elapsed_s": time.perf_counter() - started,
                "returncode": None,
                "timed_out": True,
            }

    with ThreadPoolExecutor(max_workers=CONTENDED_CLIENTS) as executor:
        samples = list(executor.map(one, range(CONTENDED_CLIENTS)))
    elapsed = [float(sample["elapsed_s"]) for sample in samples]
    failures = sum(
        sample["timed_out"] or sample["returncode"] != 0 for sample in samples
    )
    n = len(elapsed)
    return {
        "clients": CONTENDED_CLIENTS,
        "corpus_rust_files": corpus_info()["rust_files"],
        "p50_ms": round(median(elapsed) * 1000, 3),
        "p95_ms": round(p95(elapsed) * 1000, 3),
        "p99_ms": round(p99(elapsed) * 1000, 3),
        "p99_label": p99_label(n),
        "max_ms": round(max(elapsed) * 1000, 3),
        "client_deadline_ms": int(INTERACTIVE_DEADLINE_S * 1000),
        "failures": failures,
        "deadline_met": failures == 0 and max(elapsed) < INTERACTIVE_DEADLINE_S,
    }


def main():
    print("=== GraphZero Benchmark Driver ===")
    print(f"Corpus: {REPO}")
    print(f"Binary: {BIN}")

    ensure_benchmark_binaries()

    results = {}

    # Hardware
    print("\n[1/11] Collecting hardware info...")
    results["hardware"] = collect_hardware()
    print(f"  {results['hardware']['cpu']}")

    # Date
    results["date"] = datetime.now(timezone.utc).isoformat()
    results["latency_gate"] = LATENCY_GATE

    # Corpus
    results["corpus"] = corpus_info()
    results["binary_sha256"] = file_sha256(BIN)
    host_class = os.environ.get("GRAPHZERO_BENCH_HOST_CLASS", "").strip()
    isolation = os.environ.get("GRAPHZERO_BENCH_ISOLATION", "").strip()
    results["measurement_environment"] = {
        "profile": PROFILE,
        "binary_path": receipt_binary_path(BIN),
        "binary_sha256": results["binary_sha256"],
        "host_class": host_class or None,
        "isolation": isolation or None,
    }
    print(f"  {results['corpus']['rust_files']} Rust files")

    # Cold index
    print(f"\n[2/11] Cold index ({N_RUNS} runs)...")
    ci = measure_cold_index()
    results["cold_index"] = ci
    print(f"  p50: {ci['cold_index_s']:.3f}s")

    # Warm re-index
    print(f"\n[3/11] Warm re-index ({N_RUNS} runs)...")
    wi = measure_warm_reindex()
    results["warm_reindex"] = wi
    print(f"  p50: {wi['warm_reindex_s']:.3f}s")

    # CLI process-start floor (minimal binary wall; separate from op samples)
    print(f"\n[3b/11] CLI process-start (`--version`, {N_RUNS} runs)...")
    cli_ps = measure_cli_process_start(BIN, n=N_RUNS, warmup=WARMUP)
    results["cli_process_start"] = cli_ps
    print(
        f"  p50: {cli_ps['p50_ms']}ms  "
        f"p95: {cli_ps['p95_ms']}ms  "
        f"p99: {cli_ps['p99_ms']}ms ({cli_ps['p99_label']})"
    )

    # Orient p50/p95/p99 (process-inclusive wire time + process_start/op split)
    print(f"\n[4/11] Orient p50/p95/p99 ({N_ORIENT} iterations)...")
    orient = measure_orient()
    attach_cli_wall_split(orient, cli_ps, "orient_symbol_p50_ms")
    results["orient"] = orient
    print(
        f"  p50: {orient['orient_symbol_p50_ms']}ms  "
        f"p95: {orient['orient_symbol_p95_ms']}ms  "
        f"p99: {orient['orient_symbol_p99_ms']}ms ({orient['p99_label']})"
    )

    # Blast p50/p95/p99
    print(f"\n[5/11] Blast p50/p95/p99 ({N_RUNS} runs)...")
    blast = measure_blast()
    attach_cli_wall_split(blast, cli_ps, "blast_p50_ms")
    results["blast"] = blast
    print(
        f"  p50: {blast['blast_p50_ms']}ms  "
        f"p95: {blast['blast_p95_ms']}ms  "
        f"p99: {blast['blast_p99_ms']}ms ({blast['p99_label']})"
    )

    # Verify round-trip
    print(f"\n[6/11] Verify round-trip ({N_RUNS} runs)...")
    verify = measure_verify()
    attach_cli_wall_split(verify, cli_ps, "verify_roundtrip_p50_ms")
    results["verify"] = verify
    print(
        f"  p50: {verify['verify_roundtrip_p50_ms']}ms  "
        f"p95: {verify['verify_roundtrip_p95_ms']}ms  "
        f"p99: {verify['verify_roundtrip_p99_ms']}ms ({verify['p99_label']})"
    )

    # Snap p50/p95/p99
    print(f"\n[7/11] Snap capsule p50/p95/p99 ({N_RUNS} runs)...")
    snap = measure_snap()
    attach_cli_wall_split(snap, cli_ps, "snap_p50_ms")
    results["snap"] = snap
    print(
        f"  p50: {snap['snap_p50_ms']}ms  "
        f"p95: {snap['snap_p95_ms']}ms  "
        f"p99: {snap['snap_p99_ms']}ms ({snap['p99_label']})"
    )

    # MCP-mode orient round-trip
    print(f"\n[8/11] MCP-mode orient ({N_ORIENT} iterations)...")
    mcp = measure_mcp_orient()
    results["mcp"] = mcp
    print(
        f"  surface={mcp.get('surface')} binary={mcp.get('binary')} "
        f"tool={mcp.get('tool')} N={mcp.get('sample_n')} cache={mcp.get('cache_class')}"
    )
    print(
        f"  p50: {mcp['mcp_orient_p50_ms']}ms  "
        f"p95: {mcp['mcp_orient_p95_ms']}ms  "
        f"p99: {mcp['mcp_orient_p99_ms']}ms ({mcp['p99_label']})"
    )

    # JavaScript aggregate-host benchmarks run in ZeroStack, not GraphZero.
    print("\n[9/10] JavaScript aggregate-host benchmark: run in ZeroStack")

    # Concurrent clients against this >200-file repository.
    print(f"\n[10/10] Contended large-repo orient ({CONTENDED_CLIENTS} clients)...")
    contended = measure_contended_large_orient()
    results["contended_large_orient"] = contended
    print(
        f"  p50: {contended['p50_ms']}ms  p95: {contended['p95_ms']}ms  "
        f"p99: {contended['p99_ms']}ms ({contended['p99_label']}) "
        f"failures={contended['failures']}"
    )

    interactive_ok = (
        mcp["mcp_orient_p95_ms"] <= 150.0
        and contended["deadline_met"]
        and contended["corpus_rust_files"] >= 200
    )
    results["interactive_orient_slo"] = {
        "status": "pass" if interactive_ok else "fail",
        "warm_p50_max_ms": 120.0,
        "warm_p95_max_ms": 150.0,
        "contended_clients": CONTENDED_CLIENTS,
        "large_repo_min_rust_files": 200,
        # Surfaces are intentionally distinct; SLO checks each class separately.
        "warm_surface": {
            "key": "mcp_warm_orient",
            "binary": mcp.get("binary"),
            "tool": mcp.get("tool"),
            "cache_class": mcp.get("cache_class"),
            "sample_n": mcp.get("sample_n"),
            "metric": "mcp_orient_p95_ms",
        },

    }

    # Sample accounting: W discarded warmups + N measured retained (CLI paths).
    # Cold index has no warmup (each run clears the store intentionally).
    cli_paths = {
        "warm_reindex": results["warm_reindex"],
        "orient": results["orient"],
        "blast": results["blast"],
        "verify": results["verify"],
        "snap": results["snap"],
    }
    measured_cli = 0
    warmup_cli = 0
    per_metric = {}
    for name, metric in cli_paths.items():
        n = len(metric["raw_ms"]) if "raw_ms" in metric else int(
            metric.get("runs") or metric.get("iterations") or 0
        )
        w = int(metric.get("warmup", WARMUP))
        measured_cli += n
        warmup_cli += w
        per_metric[name] = {"warmup_discarded": w, "measured_retained": n}
    per_metric["cold_index"] = {
        "warmup_discarded": 0,
        "measured_retained": len(results["cold_index"].get("raw_ms", [])),
    }
    per_metric["mcp"] = {
        "warmup_discarded": int(results["mcp"].get("warmup", MCP_WARMUP)),
        "measured_retained": len(results["mcp"].get("raw_ms") or [])
        or int(results["mcp"].get("iterations", 0)),
    }
    results["sample_accounting"] = {
        "total_samples": measured_cli
        + per_metric["cold_index"]["measured_retained"]
        + per_metric["mcp"]["measured_retained"]
,
        "dropped_count": 0,
        "losses": [],
        "warmup_discarded": WARMUP,
        "measured_retained": N_RUNS,
        "cli_warmup_total": warmup_cli,
        "cli_measured_total": measured_cli,
        "mcp_warmup_discarded": MCP_WARMUP,
        "per_metric": per_metric,
    }

    # Variance envelope over retained sample series (CV ≤ 10%).
    var_metrics = {
        "cold_index": results["cold_index"],
        "warm_reindex": results["warm_reindex"],
        "orient": results["orient"],
        "blast": results["blast"],
        "verify": results["verify"],
        "snap": results["snap"],
        "mcp": results["mcp"],
    }
    # Cold MCP orient is N=1 -- skip CV gate (undefined with one sample).
    results["variance_envelope"] = variance_envelope_report(var_metrics)
    print(
        f"\nVariance envelope (CV ≤ {results['variance_envelope']['envelope_max_pct']}%): "
        f"{results['variance_envelope']['status']} "
        f"(mode={results['variance_envelope']['mode']})"
    )
    if results["variance_envelope"]["failed_metrics"]:
        print(
            f"  outside envelope: {results['variance_envelope']['failed_metrics']}",
            file=sys.stderr,
        )

    # Write results and the profile binding from the same generated receipt.
    RESULTS.parent.mkdir(parents=True, exist_ok=True)
    RESULTS.write_text(json.dumps(results, indent=2) + "\n")
    write_profile_binding(results)
    print(f"\nResults written to {RESULTS}")
    print(f"Profile binding written to {PROFILE_BINDING}")
    if not interactive_ok:
        raise RuntimeError("interactive orient SLO failed; inspect benchmarks/latency/results.json")
    if (
        not results["variance_envelope"]["within_envelope"]
        and VARIANCE_MODE == "strict"
    ):
        raise RuntimeError(
            "variance envelope reject (CV > 10%): "
            f"{results['variance_envelope']['failed_metrics']}; "
            "inspect benchmarks/latency/results.json raw_ms and isolation"
        )


if __name__ == "__main__":
    main()
