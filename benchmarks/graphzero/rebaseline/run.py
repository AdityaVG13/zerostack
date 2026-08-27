#!/usr/bin/env python3
"""Re-baseline GraphZero Northstar performance numbers with retained history."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from stats import (
    VARIANCE_ENVELOPE_MAX,
    drift_summary,
    p50,
    p95,
    p99,
    p99_label,
    variance_summary,
)

ROOT = Path(__file__).resolve().parents[2]
# Optional non-self corpus root (graphzero-e1k1). Empty = self-repo Rust corpus.
# Accept absolute path or path relative to GraphZero ROOT.
_CORPUS_ENV = os.environ.get("GRAPHZERO_BASELINE_CORPUS", "").strip()
CORPUS_ROOT = (
    Path(_CORPUS_ENV)
    if _CORPUS_ENV and Path(_CORPUS_ENV).is_absolute()
    else (ROOT / _CORPUS_ENV if _CORPUS_ENV else ROOT)
)
CORPUS_ID = os.environ.get("GRAPHZERO_BASELINE_CORPUS_ID", "").strip() or (
    "self" if CORPUS_ROOT.resolve() == ROOT.resolve() else CORPUS_ROOT.name
)
OUT_DIR = ROOT / "benchmarks" / "rebaseline"
HISTORY = OUT_DIR / "history.jsonl"
LATEST_JSON = OUT_DIR / "latest.json"
LATEST_MD = OUT_DIR / "latest.md"
SYMBOL = os.environ.get("GRAPHZERO_BASELINE_SYMBOL", "run_index")
RUNS = int(os.environ.get("GRAPHZERO_BASELINE_RUNS", "20"))
# W warmups discarded before N measured samples (CLI process baselines).
# Default W=1 matches surface_bench; override with GRAPHZERO_BASELINE_WARMUP.
WARMUP = int(os.environ.get("GRAPHZERO_BASELINE_WARMUP", "1"))
PROFILE = os.environ.get("GRAPHZERO_BENCH_PROFILE", "release")
HOST_CLASS = os.environ.get("GRAPHZERO_BENCH_HOST_CLASS", "")
ISOLATION = os.environ.get("GRAPHZERO_BENCH_ISOLATION", "")
# strict (default): exit non-zero when CV or same-host drift exceeds 10%.
# advisory: stamp envelope fields and print warnings but exit 0.
VARIANCE_MODE = os.environ.get("GRAPHZERO_BASELINE_VARIANCE_MODE", "strict").strip().lower()

# Append-only history schema. Every retained row is schema_version 1; old
# minimal rows (no binary_sha256/freshness/hardware) and current extended rows
# both parse under this version. Only the schema_version key is mandatory.
HISTORY_SCHEMA_VERSION = 1


def target_root() -> Path:
    configured = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    return configured if configured.is_absolute() else ROOT / configured


def receipt_binary_path(path: Path) -> str:
    """Return a portable path without leaking the remote checkout root."""
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        try:
            relative = resolved.relative_to(target_root().resolve()).as_posix()
        except ValueError:
            return path.name
        return f"${{CARGO_TARGET_DIR}}/{relative}"


def rust_corpus_files(corpus_root: Path | None = None) -> list[Path]:
    """List source files for corpus digest/measurement.

    Self-repo uses git ls-files *.rs. Foreign corpora walk the tree for *.rs
    (and *.ts/*.tsx when present) so non-git materializations still work.
    """
    root = corpus_root or CORPUS_ROOT
    if root.resolve() == ROOT.resolve():
        proc = subprocess.run(
            ["git", "ls-files", "-z", "--", "*.rs"],
            cwd=ROOT,
            capture_output=True,
            check=True,
        )
        files = []
        for raw_path in proc.stdout.split(b"\0"):
            if not raw_path:
                continue
            relative = Path(os.fsdecode(raw_path))
            if relative.parts[:2] == ("benchmarks", "gold"):
                continue
            # Foreign fixtures live under benchmarks/foreign_corpora -- exclude
            # them from the self-repo digest so self and foreign stay distinct.
            if relative.parts[:2] == ("benchmarks", "foreign_corpora"):
                continue
            files.append(ROOT / relative)
        return files
    # Foreign corpus: walk tree (git optional).
    patterns = ("*.rs", "*.ts", "*.tsx", "*.js", "*.jsx")
    found: list[Path] = []
    for pattern in patterns:
        found.extend(p for p in root.rglob(pattern) if p.is_file())
    # Prefer deterministic order; skip node_modules/target/.git
    skip_parts = {".git", "node_modules", "target", "dist", "build"}
    files = [
        p
        for p in found
        if not any(part in skip_parts for part in p.parts)
    ]
    return sorted(set(files), key=lambda p: str(p))


def run(command: list[str]) -> tuple[float, str, str]:
    start = time.perf_counter_ns()
    proc = subprocess.run(
        command, cwd=ROOT, text=True, capture_output=True, check=False
    )
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed rc={proc.returncode}: {' '.join(command)}\n{proc.stderr[:1000]}"
        )
    return elapsed_ms, proc.stdout, proc.stderr


def bin_path() -> Path:
    env = os.environ.get("GRAPHZERO_BIN")
    if env:
        return Path(env)
    if PROFILE == "release":
        profile_args = ["--release"]
        profile_dir = "release"
    elif PROFILE == "release-perf":
        profile_args = ["--profile", "release-perf"]
        profile_dir = "release-perf"
    else:
        raise RuntimeError(f"unsupported GRAPHZERO_BENCH_PROFILE: {PROFILE}")
    candidate = target_root() / profile_dir / "graphzero"
    if not candidate.exists():
        run(["cargo", "build", *profile_args, "-p", "graphzero", "--bin", "graphzero"])
    return candidate


def validate_measurement_config() -> None:
    if not CORPUS_ROOT.is_dir():
        raise RuntimeError(
            f"GRAPHZERO_BASELINE_CORPUS does not exist or is not a directory: {CORPUS_ROOT}"
        )
    if PROFILE not in {"release", "release-perf"}:
        raise RuntimeError(f"unsupported GRAPHZERO_BENCH_PROFILE: {PROFILE}")
    if RUNS < 20:
        raise RuntimeError("GRAPHZERO_BASELINE_RUNS must be at least 20")
    if WARMUP < 0:
        raise RuntimeError("GRAPHZERO_BASELINE_WARMUP must be >= 0")
    if not HOST_CLASS.strip() or not ISOLATION.strip():
        raise RuntimeError(
            "GRAPHZERO_BENCH_HOST_CLASS and GRAPHZERO_BENCH_ISOLATION are required"
        )
    if VARIANCE_MODE not in {"strict", "advisory"}:
        raise RuntimeError(
            "GRAPHZERO_BASELINE_VARIANCE_MODE must be 'strict' or 'advisory'"
        )


def measurement_environment(binary: Path) -> dict:
    return {
        "profile": PROFILE,
        "binary_path": receipt_binary_path(binary),
        "binary_sha256": file_sha256(binary),
        "host_class": HOST_CLASS,
        "isolation": ISOLATION,
    }


def freshness_manifest() -> dict:
    rust_files = rust_corpus_files()
    is_self = CORPUS_ROOT.resolve() == ROOT.resolve()

    def _rel(path: Path) -> str:
        try:
            return path.relative_to(CORPUS_ROOT if not is_self else ROOT).as_posix()
        except ValueError:
            return path.as_posix()

    rows = [
        f"{_rel(path)}\0{file_sha256(path)}"
        for path in sorted(rust_files, key=_rel)
    ]
    corpus_payload = "\n".join(rows).encode()
    return {
        "report_kind": "live_measurement",
        "generated_by": "benchmarks/rebaseline/run.py",
        "generator_sha256": file_sha256(ROOT / "benchmarks/rebaseline/run.py"),
        "methodology": {
            "path": "benchmarks/rebaseline/METHODOLOGY.md",
            "sha256": file_sha256(ROOT / "benchmarks/rebaseline/METHODOLOGY.md"),
        },
        "inputs": [
            {
                "path": "benchmarks/rebaseline/stats.py",
                "sha256": file_sha256(ROOT / "benchmarks/rebaseline/stats.py"),
            },
            {
                "path": "benchmarks/foreign_corpora/pins.json",
                "sha256": file_sha256(ROOT / "benchmarks/foreign_corpora/pins.json"),
            },
        ],
        "corpus": {
            "kind": "self_repo_rust_files" if is_self else "foreign_corpus",
            "corpus_id": CORPUS_ID,
            "is_self_repo": is_self,
            "file_count": len(rows),
            "sha256": hashlib.sha256(corpus_payload).hexdigest(),
        },
    }



def measure_cli_process_start(
    binary: Path,
    runs: int = RUNS,
    warmup: int = WARMUP,
) -> dict:
    """Time minimal CLI process walls (`graphzero --version`).

    Public process-start probe used to split process-inclusive orient/blast
    walls into process_start_ms vs estimated op_ms. Not paired per sample.
    """
    command = [str(binary), "--version"]
    for _ in range(warmup):
        run(command)
    samples = []
    for _ in range(runs):
        elapsed, _, _ = run(command)
        samples.append(round(elapsed, 3))
    n = len(samples)
    variance = variance_summary(samples, digits=3)
    return {
        "label": "cli_process_start",
        "wall_class": "process_start_only",
        "probe": command,
        "method": "subprocess_wall_minimal_cli",
        "runs": runs,
        "warmup": warmup,
        "p50_ms": p50(samples, digits=3),
        "p95_ms": p95(samples, digits=3),
        "p99_ms": p99(samples, digits=3),
        "p99_label": p99_label(n),
        "mean_ms": variance["mean"],
        "stdev_ms": variance["stdev"],
        "cv": variance["cv"],
        "cv_pct": variance["cv_pct"],
        "mad_ms": variance["mad"],
        "mad_over_median": variance["mad_over_median"],
        "mad_over_median_pct": variance["mad_over_median_pct"],
        "variance_envelope": {
            "metric": "cv",
            "cv": variance["cv"],
            "cv_pct": variance["cv_pct"],
            "mad_over_median": variance["mad_over_median"],
            "mad_over_median_pct": variance["mad_over_median_pct"],
            "envelope_max": variance["envelope_max"],
            "envelope_max_pct": variance["envelope_max_pct"],
            "status": variance["status"],
            "within_envelope": variance["within_envelope"],
        },
        "samples_ms": samples,
        "note": (
            "Process-start floor for published CLI metrics. Dual harness: "
            "crates/graphzero-query surface_bench records process_starts + "
            "spawn_ns for o2uq gates; this probe is what rebaseline publishes."
        ),
    }


def attach_cli_wall_split(metric: dict, process_start: dict) -> dict:
    """Attach process_start_ms and estimated op_ms to a process-inclusive metric."""
    wall_p50 = float(metric["p50_ms"])
    wall_p95 = float(metric["p95_ms"])
    wall_p99 = float(metric["p99_ms"])
    start_p50 = float(process_start["p50_ms"])
    start_p95 = float(process_start["p95_ms"])
    start_p99 = float(process_start["p99_ms"])
    metric["wall_class"] = "process_inclusive"
    metric["process_start_ms"] = {
        "p50_ms": process_start["p50_ms"],
        "p95_ms": process_start["p95_ms"],
        "p99_ms": process_start["p99_ms"],
        "p99_label": process_start.get("p99_label"),
        "probe": process_start.get("probe"),
        "method": process_start.get("method"),
        "samples_ms": process_start.get("samples_ms"),
    }
    metric["op_ms"] = {
        "p50_ms": round(max(0.0, wall_p50 - start_p50), 3),
        "p95_ms": round(max(0.0, wall_p95 - start_p95), 3),
        "p99_ms": round(max(0.0, wall_p99 - start_p99), 3),
        "method": "wall_quantile_minus_process_start_quantile",
        "note": (
            "p50_ms on this metric is process-inclusive wire time; op_ms is "
            "estimated residual after the process-start probe (not paired)."
        ),
    }
    return metric


def measure(
    label: str, command: list[str], runs: int, warmup: int = WARMUP
) -> dict:
    """Run *warmup* discarded invocations, then retain *runs* measured samples.

    Warmups never enter p50/p95/p99 or samples_ms. They are protocol discards
    (not sample losses): sample_accounting.dropped_count stays 0 when no
    measured sample is lost. p99 uses type-7 interpolation; when N < 200 it is
    labeled worst_observed_of_n (not a population-p99 claim).

    Variance envelope: CV (and MAD/median) from retained samples; status is
    outside the envelope when CV > 10%. Raw samples_ms stay for audit.
    """
    if warmup < 0:
        raise RuntimeError(f"warmup must be >= 0, got {warmup}")
    if runs < 1:
        raise RuntimeError(f"runs must be >= 1, got {runs}")
    for _ in range(warmup):
        run(command)
    samples = []
    for _ in range(runs):
        elapsed, _, _ = run(command)
        samples.append(round(elapsed, 3))
    n = len(samples)
    variance = variance_summary(samples, digits=3)
    return {
        "label": label,
        "wall_class": "process_inclusive",
        "runs": runs,
        "warmup": warmup,
        "p50_ms": p50(samples, digits=3),
        "p95_ms": p95(samples, digits=3),
        "p99_ms": p99(samples, digits=3),
        "p99_label": p99_label(n),
        "mean_ms": variance["mean"],
        "stdev_ms": variance["stdev"],
        "cv": variance["cv"],
        "cv_pct": variance["cv_pct"],
        "mad_ms": variance["mad"],
        "mad_over_median": variance["mad_over_median"],
        "mad_over_median_pct": variance["mad_over_median_pct"],
        "variance_envelope": {
            "metric": "cv",
            "cv": variance["cv"],
            "cv_pct": variance["cv_pct"],
            "mad_over_median": variance["mad_over_median"],
            "mad_over_median_pct": variance["mad_over_median_pct"],
            "envelope_max": variance["envelope_max"],
            "envelope_max_pct": variance["envelope_max_pct"],
            "status": variance["status"],
            "within_envelope": variance["within_envelope"],
        },
        "samples_ms": samples,
    }


def _latency_metrics(metrics: dict) -> dict[str, dict]:
    return {
        name: metric
        for name, metric in metrics.items()
        if isinstance(metric, dict) and "samples_ms" in metric
    }


def variance_envelope_from_metrics(metrics: dict) -> dict:
    """Aggregate per-metric CV envelope; reject if any metric exceeds 10% CV."""
    per_metric: dict[str, dict] = {}
    failures: list[str] = []
    for name, metric in _latency_metrics(metrics).items():
        env = metric.get("variance_envelope") or {}
        per_metric[name] = {
            "cv": metric.get("cv"),
            "cv_pct": metric.get("cv_pct"),
            "mad_over_median": metric.get("mad_over_median"),
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


class HistoryRowError(ValueError):
    """Raised when a history.jsonl line is malformed or not schema_version 1.

    The message always names the 1-based row number so the corrupt line can be
    located in the append-only file.
    """


def parse_history_row(line: str, row_number: int) -> dict:
    """Parse one append-only history line into a validated row dict.

    Raises HistoryRowError (with the 1-based ``row_number``) when the line is
    empty, is not valid JSON, is not a JSON object, or is not schema_version 1.
    Old minimal rows and current extended rows both parse; only the
    schema_version key is mandatory.
    """
    stripped = line.strip()
    if not stripped:
        raise HistoryRowError(f"history row {row_number}: empty line")
    try:
        row = json.loads(stripped)
    except json.JSONDecodeError as exc:
        raise HistoryRowError(
            f"history row {row_number}: malformed JSON: {exc}"
        ) from exc
    if not isinstance(row, dict):
        raise HistoryRowError(
            f"history row {row_number}: expected a JSON object, "
            f"got {type(row).__name__}"
        )
    if row.get("schema_version") != HISTORY_SCHEMA_VERSION:
        raise HistoryRowError(
            f"history row {row_number}: unsupported schema_version "
            f"{row.get('schema_version')!r} (expected {HISTORY_SCHEMA_VERSION})"
        )
    return row


def iter_history_rows(path: Path = HISTORY):
    """Yield ``(row_number, row)`` for every valid append-only history row.

    Row numbers are 1-based. Malformed rows raise HistoryRowError instead of
    being silently skipped so append-only history corruption is never hidden
    from the same-host drift gate.
    """
    if not path.is_file():
        return
    for row_number, line in enumerate(path.read_text().splitlines(), start=1):
        yield row_number, parse_history_row(line, row_number)


def load_prior_same_host_report(
    *,
    host_class: str,
    isolation: str,
    profile: str,
    history_path: Path = HISTORY,
) -> dict | None:
    """Most recent history row matching host_class, isolation, and profile.

    Old and current schema_version 1 rows both parse. A malformed line raises
    HistoryRowError naming its 1-based row number; corruption is never silently
    skipped because that would weaken the drift gate without notice.
    """
    if not history_path.is_file():
        return None
    host_class = host_class.strip()
    isolation = isolation.strip()
    if not host_class or not isolation:
        return None
    prior: dict | None = None
    for _row_number, row in iter_history_rows(history_path):
        env = row.get("measurement_environment") or {}
        if (
            env.get("host_class") == host_class
            and env.get("isolation") == isolation
            and env.get("profile") == profile
        ):
            prior = row
    return prior


def same_host_drift_report(
    metrics: dict,
    prior: dict | None,
    *,
    compare_field: str = "p50_ms",
) -> dict:
    """p50 (default) drift vs prior same-host baseline; ≤10% is within envelope."""
    if prior is None:
        return {
            "available": False,
            "reason": "no_prior_same_host_baseline",
            "envelope_max": VARIANCE_ENVELOPE_MAX,
            "envelope_max_pct": round(VARIANCE_ENVELOPE_MAX * 100.0, 1),
            "within_envelope": True,
            "status": "no_prior",
            "compare_field": compare_field,
            "per_metric": {},
            "failed_metrics": [],
        }
    prior_metrics = prior.get("metrics") or {}
    per_metric: dict[str, dict] = {}
    failures: list[str] = []
    for name, metric in _latency_metrics(metrics).items():
        prior_metric = prior_metrics.get(name) or {}
        if compare_field not in metric or compare_field not in prior_metric:
            per_metric[name] = {
                "available": False,
                "reason": f"missing_{compare_field}",
                "within_envelope": True,
                "status": "skipped",
            }
            continue
        summary = drift_summary(
            float(metric[compare_field]),
            float(prior_metric[compare_field]),
        )
        summary["available"] = True
        per_metric[name] = summary
        if not summary["within_envelope"]:
            failures.append(name)
    overall_ok = not failures
    return {
        "available": True,
        "prior_date": prior.get("date"),
        "compare_field": compare_field,
        "envelope_max": VARIANCE_ENVELOPE_MAX,
        "envelope_max_pct": round(VARIANCE_ENVELOPE_MAX * 100.0, 1),
        "within_envelope": overall_ok,
        "status": "pass" if overall_ok else "reject",
        "failed_metrics": failures,
        "per_metric": per_metric,
    }


def sample_accounting_from_metrics(
    metrics: dict, *, priming_index_runs: int = 1
) -> dict:
    """Derive accounting from retained sample vectors; record priming/W/N.

    total_samples is sum(len(samples_ms)) over latency metrics -- never a
    hardcoded RUNS*k product. Warmups and store priming are protocol counts
    recorded explicitly; they are not measured-sample losses.
    """
    measured_per_metric: dict[str, int] = {}
    warmup_per_metric: dict[str, int] = {}
    for name, metric in metrics.items():
        if not isinstance(metric, dict) or "samples_ms" not in metric:
            continue
        measured_per_metric[name] = len(metric["samples_ms"])
        warmup_per_metric[name] = int(metric.get("warmup", 0))
        declared_runs = metric.get("runs")
        if declared_runs is not None and int(declared_runs) != measured_per_metric[name]:
            raise RuntimeError(
                f"{name}: runs={declared_runs} != len(samples_ms)="
                f"{measured_per_metric[name]}"
            )
    measured_total = sum(measured_per_metric.values())
    warmup_total = sum(warmup_per_metric.values())
    # W and N are uniform across latency metrics by construction.
    measured_retained = next(iter(measured_per_metric.values()), 0)
    warmup_discarded = next(iter(warmup_per_metric.values()), 0)
    if priming_index_runs < 0:
        raise RuntimeError("priming_index_runs must be >= 0")
    return {
        "total_samples": measured_total,
        "dropped_count": 0,
        "losses": [],
        "priming_index_runs": priming_index_runs,
        "warmup_discarded": warmup_discarded,
        "measured_retained": measured_retained,
        "warmup_total": warmup_total,
        "measured_total": measured_total,
        "per_metric": {
            name: {
                "warmup_discarded": warmup_per_metric[name],
                "measured_retained": measured_per_metric[name],
            }
            for name in measured_per_metric
        },
    }


def _read_text_cmd(args: list[str]) -> str | None:
    try:
        proc = subprocess.run(args, capture_output=True, text=True, cwd=ROOT)
        if proc.returncode == 0:
            out = (proc.stdout or "").strip()
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
        try:
            for line in Path("/proc/cpuinfo").read_text().splitlines():
                if line.lower().startswith("model name") and ":" in line:
                    return line.split(":", 1)[1].strip() or "unknown"
        except Exception:
            pass
    return cpu or "unknown"


def _memory_label() -> str | None:
    try:
        proc = subprocess.run(
            ["sysctl", "-n", "hw.memsize"], capture_output=True, text=True
        )
        if proc.returncode == 0:
            mem_bytes = int(proc.stdout.strip())
            return f"{mem_bytes / (1024**3):.0f} GB"
    except Exception:
        pass
    try:
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
        try:
            best: tuple[str, str | None] = ("", None)
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
        try:
            mount_out = _read_text_cmd(["mount"]) or ""
            best = ("", None)
            for line in mount_out.splitlines():
                if " on " not in line or " (" not in line:
                    continue
                _dev, rest = line.split(" on ", 1)
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
    """Linux cpufreq governor; null when sysfs node is absent (incl. macOS)."""
    try:
        p = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        if p.is_file():
            val = p.read_text().strip()
            return val or None
    except Exception:
        pass
    return None


def _power_mode() -> str | None:
    """Best-effort power mode. null when unavailable (key still present)."""
    try:
        p = Path("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference")
        if p.is_file():
            val = p.read_text().strip()
            return val or None
    except Exception:
        pass
    if platform.system() == "Darwin":
        out = _read_text_cmd(["pmset", "-g"])
        if out:
            for line in out.splitlines():
                parts = line.split()
                if len(parts) >= 2 and parts[0] == "lowpowermode":
                    return f"lowpowermode={parts[1]}"
    return None


def _load_average() -> list[float] | None:
    try:
        return [round(x, 3) for x in os.getloadavg()]
    except (OSError, AttributeError):
        return None


def _rustc_version() -> str | None:
    try:
        proc = subprocess.run(
            ["rustc", "--version"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if proc.returncode == 0:
            out = (proc.stdout or "").strip()
            return out or None
    except Exception:
        pass
    return None


def hardware() -> dict:
    """Host fingerprint for comparable baselines.

    Always emit the full key set. Unavailable probes are null (cpu falls back to
    "unknown") -- never omit governor/FS/isolation fields.
    """
    store = ROOT / ".zerostack" / f"graphzero-baseline-{CORPUS_ID}"
    host_class = HOST_CLASS.strip() or None
    isolation = ISOLATION.strip() or None
    return {
        "platform": platform.platform(),
        "os": platform.system() or None,
        "kernel": platform.release() or None,
        "machine": platform.machine() or None,
        "cpu": _cpu_model(),
        "memory": _memory_label(),
        "python": platform.python_version(),
        "rustc": _rustc_version(),
        "profile": PROFILE,
        "fs_type": _fs_type_for(store),
        "store_path": ".zerostack/graphzero",
        "governor": _cpu_governor(),
        "power_mode": _power_mode(),
        "load_average": _load_average(),
        "host_class": host_class,
        "isolation": isolation,
    }


def git_rev() -> str:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if proc.returncode == 0:
            return proc.stdout.strip()
    except Exception:
        pass
    return "unknown"


def file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def corpus() -> dict:
    files = rust_corpus_files()
    is_self = CORPUS_ROOT.resolve() == ROOT.resolve()
    try:
        rev = git_rev_at(CORPUS_ROOT) if not is_self else git_rev()
    except Exception:
        rev = None
    return {
        "name": CORPUS_ID if not is_self else "graphzero",
        "corpus_id": CORPUS_ID,
        "is_self_repo": is_self,
        "rust_files": sum(1 for f in files if f.suffix == ".rs"),
        "file_count": len(files),
        "repo": str(CORPUS_ROOT if not is_self else Path(".")),
        "repo_path": str(CORPUS_ROOT.resolve()),
        "git_rev": rev,
        "foreign": not is_self,
    }


def git_rev_at(repo: Path) -> str | None:
    proc = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        return None
    return proc.stdout.strip() or None


def _metric_cv_label(metric: dict) -> str:
    cv_pct = metric.get("cv_pct")
    status = (metric.get("variance_envelope") or {}).get("status", "undefined")
    if cv_pct is None:
        return f"cv=n/a ({status})"
    return f"cv={cv_pct}% ({status})"


def to_markdown(report: dict) -> str:
    warm = report["metrics"]["warm_reindex"]
    orient = report["metrics"]["orient_symbol"]
    blast = report["metrics"]["blast"]
    inc = report["metrics"]["incremental_update"]
    var_env = report.get("variance_envelope") or {}
    drift = report.get("same_host_drift") or {}
    return "\n".join(
        [
            "# GraphZero Northstar re-baseline",
            "",
            f"- Date: {report['date']}",
            f"- Corpus: {report['corpus']['name']} ({report['corpus']['rust_files']} Rust files)",
            f"- Build profile: {report['measurement_environment']['profile']}",
            f"- Binary: {report['measurement_environment']['binary_path']} ({report['binary_sha256']})",
            f"- Host class: {report['measurement_environment']['host_class']}",
            f"- Isolation: {report['measurement_environment']['isolation']}",
            f"- Sample protocol: W={report['sample_accounting']['warmup_discarded']} warmups discarded, "
            f"N={report['sample_accounting']['measured_retained']} measured retained per latency metric",
            (
                f"- Warm reindex p50/p95/p99: {warm['p50_ms']} / {warm['p95_ms']} / "
                f"{warm['p99_ms']} ms ({warm['runs']} runs, p99={warm['p99_label']}, "
                f"{_metric_cv_label(warm)})"
            ),
            (
                f"- Orient symbol p50/p95/p99: {orient['p50_ms']} / {orient['p95_ms']} / "
                f"{orient['p99_ms']} ms ({orient['runs']} runs, symbol {SYMBOL}, "
                f"p99={orient['p99_label']}, {_metric_cv_label(orient)})"
            ),
            (
                f"- Blast p50/p95/p99: {blast['p50_ms']} / {blast['p95_ms']} / "
                f"{blast['p99_ms']} ms ({blast['runs']} runs, p99={blast['p99_label']}, "
                f"{_metric_cv_label(blast)})"
            ),
            f"- Per-save incremental update: {inc['status']} ({inc['reason']})",
            (
                f"- Variance envelope (CV ≤ {var_env.get('envelope_max_pct', 10)}%): "
                f"{var_env.get('status', 'unknown')} "
                f"(mode={var_env.get('mode', VARIANCE_MODE)})"
            ),
            (
                f"- Same-host p50 drift ≤ {drift.get('envelope_max_pct', 10)}%: "
                f"{drift.get('status', 'unknown')}"
                + (
                    f" vs prior {drift.get('prior_date')}"
                    if drift.get("prior_date")
                    else ""
                )
            ),
            "- Report class: live measurement; freshness binds run.py, stats.py, methodology, and the Rust corpus digest.",
            "",
        ]
    )


def main() -> int:
    validate_measurement_config()
    binary = bin_path()
    environment = measurement_environment(binary)
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    # Store priming (not a latency-metric warmup): open/populate store once.
    run([str(binary), "index", str(CORPUS_ROOT)])
    cli_process_start = measure_cli_process_start(binary, runs=RUNS, warmup=WARMUP)
    metrics = {
        "cli_process_start": cli_process_start,
        "warm_reindex": attach_cli_wall_split(
            measure("warm_reindex", [str(binary), "index", str(CORPUS_ROOT)], RUNS),
            cli_process_start,
        ),
        "orient_symbol": attach_cli_wall_split(
            measure(
                "orient_symbol",
                [
                    str(binary),
                    "orient",
                    "--surface",
                    "symbol",
                    "--name",
                    SYMBOL,
                    "--budget",
                    "1",
                    "--repo",
                    str(CORPUS_ROOT),
                ],
                RUNS,
            ),
            cli_process_start,
        ),
        "blast": attach_cli_wall_split(
            measure(
                "blast",
                [
                    str(binary),
                    "blast",
                    "--intent",
                    f"change signature of {SYMBOL}",
                    "--budget",
                    "1",
                    "--repo",
                    str(CORPUS_ROOT),
                ],
                RUNS,
            ),
            cli_process_start,
        ),
        "incremental_update": {
            "status": "not_available",
            "reason": "per-save incremental update API has not landed in GraphZero yet",
        },
    }
    accounting = sample_accounting_from_metrics(metrics, priming_index_runs=1)
    var_env = variance_envelope_from_metrics(metrics)
    prior = load_prior_same_host_report(
        host_class=HOST_CLASS,
        isolation=ISOLATION,
        profile=PROFILE,
    )
    drift = same_host_drift_report(metrics, prior)
    report = {
        "schema_version": 1,
        "generated_by": "benchmarks/rebaseline/run.py",
        "date": datetime.now(timezone.utc).isoformat(),
        "binary": environment["binary_path"],
        "binary_sha256": environment["binary_sha256"],
        "measurement_environment": environment,
        "hardware": hardware(),
        "corpus": corpus(),
        "freshness": freshness_manifest(),
        "sample_accounting": accounting,
        "variance_envelope": var_env,
        "same_host_drift": drift,
        "metrics": metrics,
        "integrity": {
            "runs_per_latency_metric": RUNS,
            "warmup_per_latency_metric": WARMUP,
            "percentile_estimator": "Hyndman-Fan type 7 interpolation",
            # N defaults to 20; type-7 p99 is published but labeled worst_observed_of_n.
            "p99_label_when_n_lt_200": "worst_observed_of_n",
            "history_retained": True,
            # Measured samples are never silently discarded; warmups are protocol.
            "no_samples_dropped": accounting["dropped_count"] == 0
            and not accounting["losses"],
            "variance_envelope_max_pct": round(VARIANCE_ENVELOPE_MAX * 100.0, 1),
            "variance_mode": VARIANCE_MODE,
        },
    }
    LATEST_JSON.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    LATEST_MD.write_text(to_markdown(report))
    with HISTORY.open("a") as fh:
        fh.write(json.dumps(report, sort_keys=True) + "\n")
    print(to_markdown(report))
    envelope_ok = var_env["within_envelope"] and drift["within_envelope"]
    if not envelope_ok:
        detail = []
        if not var_env["within_envelope"]:
            detail.append(
                f"CV outside ≤{var_env['envelope_max_pct']}% on {var_env['failed_metrics']}"
            )
        if not drift["within_envelope"]:
            detail.append(
                f"same-host drift outside ≤{drift['envelope_max_pct']}% on "
                f"{drift['failed_metrics']}"
            )
        msg = "variance envelope reject: " + "; ".join(detail)
        if VARIANCE_MODE == "advisory":
            print(f"ADVISORY: {msg}", file=sys.stderr)
            return 0
        print(f"ERROR: {msg}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
