#!/usr/bin/env python3
"""Savings-bench headline runner: ten rch release-perf repeats.

Bead: zerostack-gauntlet-perf-0001-bench-aq5p (PERF-0001 / OPEN-0015).

Headline under test (from benchmarks/savings-bench.json seed):
    zero.token.read("compact_50k.txt", { max_visible_tokens: 200 })
    primary metric: headline_exact.billed_over_raw  (seed: 0.044, 198/4500)

Each repeat is one fresh `zsx exec` process (cold session) executed on the
rch worker with:
    CARGO_TARGET_DIR=${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack_savings_bench
    RUSTFLAGS='-C force-frame-pointers=yes'
    --profile release-perf

The fixture must be byte-identical to the seed fixture
(sha256 d7bd8d9611e6b4b02440ce8d5a2f462b70f36597c52cafab92540ea823b6db06,
36,645 bytes) or the runner refuses to measure.

Output: a v3 comprehensive-bench-report candidate JSON with numeric cv_pct
under .bench-history/, plus the raw rch log. The seed
(.bench-history/savings-bench.latest.json) is never overwritten.

Usage:
    python3 scripts/bench/savings_bench.py [--repeats 10] [--out PATH] [--keep-log]

Exit codes: 0 = candidate written; 2 = fixture mismatch; 3 = repeat failed.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
HISTORY_DIR = REPO_ROOT / ".bench-history"
FIXTURE_DIR = HISTORY_DIR / "fixtures"
PLANS_DIR = HISTORY_DIR / "plans"
DEFAULT_FIXTURE = FIXTURE_DIR / "compact_50k.txt"
PLAN_FILE = PLANS_DIR / "savings_headline.js"

SEED_FIXTURE_SHA256 = "d7bd8d9611e6b4b02440ce8d5a2f462b70f36597c52cafab92540ea823b6db06"
SEED_FIXTURE_BYTES = 36645

RCH_TARGET_DIR = "${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_zerostack_savings_bench"
RUSTFLAGS = "-C force-frame-pointers=yes"

HEADLINE_PLAN = """\
const r = await zero.token.read("compact_50k.txt", { max_visible_tokens: 200 });
return r.content.value.accounting;
"""

SCHEMA_V3 = "zerostack.comprehensive-bench-report.v3"


def fail(message: str) -> int:
    print(f"savings-bench: {message}", file=sys.stderr)
    return 2


def sha256_hex(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_repeats(repeats: int) -> tuple[list[dict], str]:
    """Build once on one rch worker, then run `repeats` fresh zsx processes."""
    shell = f"""
set -euo pipefail
export CARGO_TARGET_DIR=\"{RCH_TARGET_DIR}\"
export RUSTFLAGS={RUSTFLAGS!r}
cargo build --profile release-perf -p zsx
for repeat in $(seq 1 {repeats}); do
  echo \"=== zsx repeat $repeat ===\" >&2
  \"$CARGO_TARGET_DIR/release-perf/zsx\" exec \
    -C .bench-history/fixtures \
    --file .bench-history/plans/savings_headline.js \
    --timeout-ms 60000
done
""".strip()
    cmd = ["rch", "exec", "--", "bash", "-lc", shell]
    started = time.time()
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=3600,
        check=False,
    )
    elapsed = time.time() - started
    log = (
        f"=== rch batch ({repeats} repeats, exit {proc.returncode}, {elapsed:.1f}s) ===\n"
        f"$ rch exec -- bash -lc {shell!r}\n"
        f"--- stdout ---\n{proc.stdout}\n"
        f"--- stderr ---\n{proc.stderr}\n"
    )
    if proc.returncode != 0:
        raise RuntimeError(f"rch batch exited {proc.returncode}\n{log}")

    accountings: list[dict] = []
    for stream in (proc.stdout, proc.stderr):
        for line in stream.splitlines():
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                envelope = json.loads(line)
            except json.JSONDecodeError:
                continue
            if envelope.get("protocol") != "zerostack.zsx" or envelope.get("ok") is not True:
                continue
            result = envelope.get("result")
            if isinstance(result, dict) and "billed_tokens" in result:
                accountings.append(result)
    if len(accountings) != repeats:
        raise RuntimeError(
            f"rch batch returned {len(accountings)} accounting envelopes, expected {repeats}\n{log}"
        )
    return accountings, log


def build_candidate(
    accountings: list[dict],
    ratios: list[float],
    env: dict,
    git_sha: str,
    log_text: str,
) -> dict:
    mean = statistics.fmean(ratios)
    stdev = statistics.stdev(ratios) if len(ratios) > 1 else 0.0
    cv_pct = (stdev / mean * 100.0) if mean > 0 else None
    profile_self_time_pct = env.get("profile_first_self_time_pct")
    profile_first = bool(
        env.get("cargo_profile") == "release-perf"
        and isinstance(profile_self_time_pct, (int, float))
        and profile_self_time_pct >= 0.1
    )
    keep_eligible = bool(cv_pct is not None and cv_pct <= 5.0 and profile_first)
    ratchet_reasons = [
        f"cv_pct={cv_pct} (noise threshold 5.0)",
        "Seed .bench-history/savings-bench.latest.json left untouched.",
    ]
    if profile_first:
        ratchet_reasons.insert(
            0,
            f"Profiler attribution is {profile_self_time_pct}% self time under release-perf.",
        )
    else:
        ratchet_reasons.insert(
            0,
            "Record-only: no profiler attribution >=0.1% self time was captured in this run.",
        )
    rounds = [
        {
            "repeat": i + 1,
            "billed_tokens": a["billed_tokens"],
            "raw_tokens": a["raw_tokens"],
            "visible_tokens": a["visible_tokens"],
            "recovery_tokens": a["recovery_tokens"],
            "exact_ref_tokens": a.get("exact_ref_tokens"),
            "cached_tokens": a["cached_tokens"],
            "billed_over_raw": round(ratios[i], 6),
        }
        for i, a in enumerate(accountings)
    ]
    return {
        "schema_version": SCHEMA_V3,
        "source_bench": "benchmarks/savings-bench.json",
        "seed_kind": "rch-release-perf-repeats",
        "cv_pct": round(cv_pct, 4) if cv_pct is not None else None,
        "cv_pct_note": (
            "Coefficient of variation (percent) of the deterministic Exact "
            f"billed/raw ratio across {len(ratios)} fresh processes. cv_pct <= 5 "
            "is a noise check, not runtime-stability or profiler evidence."
        ),
        "keep_eligible": keep_eligible,
        "keep_ineligible_reason": (
            None
            if keep_eligible
            else (
                f"record-only: cv_pct={cv_pct}; release-perf={env.get('cargo_profile') == 'release-perf'}; "
                f"profile_first_self_time_pct={profile_self_time_pct!r} (requires >=0.1)"
            )
        ),
        "detected_environment": env,
        "summary": {
            "total_scenarios": 1,
            "primary_score": round(mean, 6),
            "primary_score_name": "headline_exact.billed_over_raw",
            "primary_score_direction": "lower_is_better",
            "primary_score_note": (
                "Exact billed/raw on token.read compact_50k.txt with "
                "max_visible_tokens=200, mean over rch repeats. A token-accounting "
                "ratio, not a wall-clock speedup versus a reference implementation."
            ),
            "geomean_ratio": None,
            "p90_ratio": None,
            "throughput": None,
            "average_ratio": None,
            "median_ratio": None,
            "per_category_weighted": {
                "score": round(mean, 6),
                "weights": {"exact_tokens": 1.0},
            },
        },
        "ci_regression_gate": {
            "schema_version": "zerostack.comprehensive-bench-ci-regression-gate.v2",
            "primary_score_max_regression_pct": 0.03,
            "geomean_max_regression_pct": 0.05,
            "category_geomean_max_regression_pct": 0.1,
            "p90_max_regression_pct": 0.15,
            "throughput_max_regression_pct": 0.05,
        },
        "categories": {
            "exact_tokens": {
                "score": round(mean, 6),
                "metric": "billed_over_raw",
                "direction": "lower_is_better",
            }
        },
        "previous_ratchet": "seed-baseline",
        "ratchet_decision": {
            "verdict": "allow" if keep_eligible else "record_only",
            "reasons": ratchet_reasons,
        },
        "sections": [
            {
                "section_id": "headline_exact",
                "title": "Exact token.read headline (rch release-perf repeats)",
                "rows": [
                    {
                        "scenario_id": "token_read_full_budgeted",
                        "scenario": "token.read compact_50k.txt max_visible_tokens=200",
                        "category": "exact_tokens",
                        "subject": {
                            "billed_tokens": round(
                                statistics.fmean(
                                    [a["billed_tokens"] for a in accountings]
                                ),
                                2,
                            ),
                            "raw_tokens": round(
                                statistics.fmean([a["raw_tokens"] for a in accountings]),
                                2,
                            ),
                            "billed_over_raw": round(mean, 6),
                            "cv_pct": round(cv_pct, 4) if cv_pct is not None else None,
                            "iterations": len(ratios),
                            "repeats": rounds,
                        },
                        "ratio": round(mean, 6),
                        "winner": "rch-release-perf-repeats",
                    }
                ],
            }
        ],
        "rch_log": log_text,
        "git_sha": git_sha,
    }


def detect_environment(git_sha: str) -> dict:
    worker_info = {}
    try:
        proc = subprocess.run(
            ["rch", "workers", "list", "-j"],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        if proc.returncode == 0:
            worker_info["rch_workers"] = proc.stdout[:2000]
        else:
            worker_info["rch_workers_error"] = proc.stderr.strip()[:500]
    except (OSError, subprocess.SubprocessError) as exc:
        worker_info["rch_workers_error"] = str(exc)[:500]
    rustc = {}
    try:
        proc = subprocess.run(
            ["rch", "exec", "--", "rustc", "-vV"],
            capture_output=True,
            text=True,
            timeout=300,
            check=False,
        )
        if proc.returncode == 0:
            rustc["rustc_remote"] = proc.stdout.strip()[:2000]
        else:
            rustc["rustc_remote_error"] = proc.stderr.strip()[:500]
    except (OSError, subprocess.SubprocessError) as exc:
        rustc["rustc_remote_error"] = str(exc)[:500]

    # Only accept host/rustc lines that came from the remote worker (Linux
    # spark). If rch ran rustc locally (darwin), keep fields null rather than
    # recording the Mac as the measurement host.
    remote_text = rustc.get("rustc_remote", "")
    host_triple = None
    rustc_version = None
    for line in remote_text.splitlines():
        if line.startswith("host:"):
            host_triple = line.split(":", 1)[1].strip()
        if line.startswith("rustc "):
            rustc_version = line.strip()[:120]
    is_remote_host = host_triple is not None and "darwin" not in host_triple
    os_name = None
    arch = None
    if is_remote_host and host_triple:
        parts = host_triple.split("-")
        arch = parts[0] if parts else None
        os_name = "linux" if "linux" in host_triple else host_triple
    return {
        "measured_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "os": os_name,
        "arch": arch,
        "cpu_count": None,
        "cpu_model": None,
        "kernel": None,
        "rustc_version": rustc_version if is_remote_host else None,
        "cargo_version": None,
        "git_sha": git_sha,
        "cargo_profile": "release-perf",
        "profile_first_self_time_pct": None,
        "feature_flags": "zsx (fszero, graphzero, tokenzero)",
        "note": (
            f"Remote rch worker (spark); CARGO_TARGET_DIR={RCH_TARGET_DIR}; "
            "RUSTFLAGS='-C force-frame-pointers=yes'. os/arch/rustc parsed from "
            "the worker's rustc -vV host triple; fields that could not be "
            "captured remotely stay null rather than invented."
        ),
        "rch_worker_info": worker_info.get("rch_workers"),
        "rch_worker_error": worker_info.get("rch_workers_error"),
        "rustc_remote_raw": remote_text[:2000] if remote_text else None,
        "rustc_remote_error": rustc.get("rustc_remote_error"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repeats", type=int, default=10)
    parser.add_argument("--out", type=Path, default=None)
    parser.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    parser.add_argument("--keep-log", action="store_true")
    args = parser.parse_args()

    if args.repeats < 1:
        return fail("--repeats must be >= 1")

    fixture = Path(args.fixture)
    if not fixture.is_file():
        return fail(f"fixture missing: {fixture}")
    actual_sha = sha256_hex(fixture)
    actual_bytes = fixture.stat().st_size
    if actual_sha != SEED_FIXTURE_SHA256 or actual_bytes != SEED_FIXTURE_BYTES:
        return fail(
            f"fixture mismatch: sha256={actual_sha} bytes={actual_bytes}; "
            f"expected {SEED_FIXTURE_SHA256} / {SEED_FIXTURE_BYTES}. "
            "Refusing to measure a different fixture."
        )

    PLAN_FILE.parent.mkdir(parents=True, exist_ok=True)
    PLAN_FILE.write_text(HEADLINE_PLAN, encoding="utf-8")

    git_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        timeout=30,
        check=True,
    ).stdout.strip()

    env = detect_environment(git_sha)

    try:
        accountings, log_text = run_repeats(args.repeats)
    except RuntimeError as exc:
        if args.keep_log:
            (HISTORY_DIR / "savings-bench.rch-failed-batch.log").write_text(
                str(exc), encoding="utf-8"
            )
        print("savings-bench: rch batch FAILED", file=sys.stderr)
        return 3

    ratios = [
        accounting["billed_tokens"] / accounting["raw_tokens"]
        for accounting in accountings
    ]
    for index, (accounting, ratio) in enumerate(zip(accountings, ratios), start=1):
        print(
            f"repeat {index}: billed={accounting['billed_tokens']} "
            f"raw={accounting['raw_tokens']} billed_over_raw={ratio:.6f}"
        )
    if args.keep_log:
        (HISTORY_DIR / f"savings-bench.rch-{datetime.now(timezone.utc):%Y%m%dT%H%M%S}.log").write_text(
            log_text, encoding="utf-8"
        )

    candidate = build_candidate(accountings, ratios, env, git_sha, log_text)
    out = args.out or (
        HISTORY_DIR
        / f"savings-bench.candidate-{datetime.now(timezone.utc):%Y%m%dT%H%M%S}.json"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(candidate, indent=2), encoding="utf-8")

    mean = statistics.fmean(ratios)
    stdev = statistics.stdev(ratios) if len(ratios) > 1 else 0.0
    cv = stdev / mean * 100.0 if mean > 0 else float("nan")
    print(
        f"savings-bench: n={len(ratios)} mean_billed_over_raw={mean:.6f} "
        f"stdev={stdev:.6f} cv_pct={cv:.4f} keep_eligible={candidate['keep_eligible']}"
    )
    print(f"savings-bench: candidate JSON: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
