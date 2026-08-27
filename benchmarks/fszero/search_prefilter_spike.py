#!/usr/bin/env python3
"""Bakeoff bigram_memmem for default-on (fszero-kbo).

Re-runs the gold spike corpus (>=1000 files / >=100MB) measuring baseline /
memmem / bigram+memmem (rare/common/absent/ascii/unicode, cold+warm), cold
ingest upsert, memory, and watch create/modify/delete upsert parity+cost.

Gates (must all hold on release-ish build; same as fszero-9ot/up8):
  - rare/absent p50 improve >= 25% vs baseline
  - common p50 regress <= 10%
  - cold ingest regress <= 20% (from_bytes during read+AST extract)
  - memory <= 1.25x
  - watch upsert hit-set parity (create/modify) + deleted absent
  - ACCEPT default-on only if bigram clears all; else REJECT (keep opt-in)

Usage:
  python3 benchmarks/search_prefilter_spike.py [--files 1200] [--iters 20]
"""
from __future__ import annotations

import argparse
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_JSON = ROOT / "benchmarks" / "search-prefilter-spike.json"
OUT_MD = ROOT / "benchmarks" / "search-prefilter-spike.md"
DECISION = ROOT / "docs" / "design" / "search-prefilter-eval.md"
MIN_MEASURED_RUNS = 20

WORDS = (
    "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima "
    "mike november oscar papa quebec romeo sierra tango uniform victor whiskey "
    "xray yankee zulu parse index store walk merge commit journal blob ref"
).split()

RARE = "rare_zz9q_unique_needle"
ASCII_MARK = "alpha_bravo_marker"
UNICODE_MARK = "café_ユニーク_٩"
COMMON = "wrapping_add"
ABSENT = "ABSENT_needle_xyz_never_9yq"


def git_provenance() -> dict[str, object]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(
        [
            "git",
            "status",
            "--porcelain",
            "-uno",
            "--",
            ".",
            ":(exclude)benchmarks/search-prefilter-spike.json",
            ":(exclude)benchmarks/search-prefilter-spike.md",
            ":(exclude)docs/design/search-prefilter-eval.md",
        ],
        cwd=ROOT,
        text=True,
    )
    return {"git_commit": commit, "git_dirty": bool(status.strip())}


def hardware() -> str:
    try:
        cpu = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
        ).strip()
        ram = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True).strip())
        return f"{cpu} / {ram // (1024 ** 3)} GB"
    except Exception:
        return "unknown"


def gen_file(rng: random.Random, module: str, idx: int, pad_bytes: int, plant: str | None) -> str:
    lines: list[str] = [f"//! Synthetic module {module}, file {idx}.", ""]
    for _ in range(rng.randint(2, 5)):
        lines.append(f"use crate::{rng.choice(WORDS)}::{rng.choice(WORDS)};")
    lines.append("")
    n_fns = rng.randint(4, 10)
    fn_names = [
        f"{rng.choice(WORDS)}_{rng.choice(WORDS)}_{rng.randint(0, 999)}" for _ in range(n_fns)
    ]
    for name in fn_names:
        lines.append(f"pub fn {name}(input: &str) -> usize {{")
        for _ in range(rng.randint(3, 10)):
            a, b = rng.choice(WORDS), rng.randint(1, 9999)
            lines.append(f"    let {a} = input.len().wrapping_add({b});")
        callee = rng.choice(fn_names)
        lines.append(f"    // calls {callee}(...) downstream")
        lines.append(f"    {callee}_helper(input.len())")
        lines.append("}")
        lines.append(f"fn {name}_helper(n: usize) -> usize {{ n.wrapping_mul(31) }}")
        lines.append("")
    if plant:
        lines.append(f"// planted: {plant}")
        lines.append(f'const PLANTED: &str = "{plant}";')
        lines.append("")
    body = "\n".join(lines) + "\n"
    if pad_bytes > len(body.encode()):
        # Deterministic padding comment block to hit corpus byte target.
        need = pad_bytes - len(body.encode())
        chunk = ("// " + ("pad " * 16) + "\n")
        reps = max(1, need // len(chunk.encode()))
        body = body + (chunk * reps)
    return body


def generate_corpus(out: Path, files: int, target_bytes: int, seed: int) -> dict[str, object]:
    rng = random.Random(seed)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)
    pad_each = max(1024, target_bytes // files)
    rare_idx = files // 7
    ascii_idx = files // 3
    unicode_idx = (2 * files) // 3
    written = 0
    leaf = 0
    per_leaf = 40
    while written < files:
        top = f"mod_{leaf // 32:03d}"
        sub = f"sub_{leaf % 32:03d}"
        d = out / top / sub
        d.mkdir(parents=True, exist_ok=True)
        for i in range(min(per_leaf, files - written)):
            idx = written
            plant = None
            if idx == rare_idx:
                plant = RARE
            elif idx == ascii_idx:
                plant = ASCII_MARK
            elif idx == unicode_idx:
                plant = UNICODE_MARK
            (d / f"f_{i:03d}.rs").write_text(
                gen_file(rng, f"{top}::{sub}", i, pad_each, plant),
                encoding="utf-8",
            )
            written += 1
        leaf += 1
    total = sum(p.stat().st_size for p in out.rglob("*.rs"))
    # Guarantee >= target_bytes with a final deterministic pad file if short.
    if total < target_bytes:
        need = target_bytes - total
        pad_path = out / "pad" / "tail_pad.rs"
        pad_path.parent.mkdir(parents=True, exist_ok=True)
        chunk = b"// " + (b"pad " * 16) + b"\n"
        reps = max(1, (need + len(chunk) - 1) // len(chunk))
        pad_path.write_bytes(b"//! tail pad\n" + chunk * reps)
        written += 1
        total = sum(p.stat().st_size for p in out.rglob("*.rs"))
    return {
        "files": written,
        "bytes": total,
        "rare_file_index": rare_idx,
        "ascii_file_index": ascii_idx,
        "unicode_file_index": unicode_idx,
        "pad_each_target": pad_each,
        "seed": seed,
    }


def build_spike() -> None:
    env = os.environ.copy()
    env.setdefault("CARGO_BUILD_JOBS", "2")
    cargo = [
        str(ROOT / "scripts" / "profile_build.sh"),
        "--cargo-command",
        "bench",
        "--bench",
        "search_prefilter_spike",
        "--no-run",
    ]
    if shutil.which("rch"):
        cargo = ["rch", "exec", "--"] + cargo
    lock = "/tmp/zerostack-swarm-locks/fszero.lock"
    os.makedirs(os.path.dirname(lock), exist_ok=True)
    # flock serializes FSZero cargo; rch (if present) sits inside the lock.
    cmd = ["flock", lock] + cargo
    print("+", " ".join(cmd), flush=True)
    subprocess.check_call(cmd, cwd=ROOT, env=env)


def run_spike(corpus: Path, iters: int) -> list[dict]:
    bench_bin = find_bench_bin()
    cmd = [str(bench_bin), str(corpus), str(iters), "16"]
    print("+", " ".join(cmd), flush=True)
    out = subprocess.check_output(cmd, cwd=ROOT, text=True)
    events = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        events.append(json.loads(line))
    return events


def find_bench_bin() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    # Cargo places harness=false custom-profile benches under release-perf/deps.
    candidates = list(target.glob("release-perf/deps/search_prefilter_spike-*"))
    candidates += list(target.glob("release-perf/search_prefilter_spike"))
    candidates = [c for c in candidates if c.is_file() and os.access(c, os.X_OK) and not c.name.endswith(".d")]
    if not candidates:
        # Fallback: cargo bench runs and we parse — rebuild with explicit OUT_DIR probe.
        raise SystemExit(
            "spike binary not found; ran the release-perf bench build? "
            f"looked under {target}/release-perf"
        )
    candidates.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return candidates[0]


def improve_pct(base: float, cand: float) -> float:
    if base <= 0:
        return 0.0
    return (base - cand) / base * 100.0


def decide(events: list[dict]) -> dict:
    by_label = {e["label"]: e for e in events if e.get("event") == "query"}
    corpus = next(e for e in events if e.get("event") == "corpus")
    ingest = next(e for e in events if e.get("event") == "ingest_incremental")
    watch = next(e for e in events if e.get("event") == "watch_upsert")
    bulk = next(
        (e for e in events if e.get("event") in ("amortization_bulk_proxy", "amortization")),
        None,
    )

    rare = by_label["rare"]
    absent = by_label["absent"]
    common = by_label["common"]

    def p50_gain(row: dict, key: str) -> float:
        return improve_pct(row["baseline_p50_ms"], row[key])

    mem_rare = p50_gain(rare, "memmem_p50_ms")
    mem_absent = p50_gain(absent, "memmem_p50_ms")
    mem_common = p50_gain(common, "memmem_p50_ms")
    big_rare = p50_gain(rare, "bigram_warm_p50_ms")
    big_absent = p50_gain(absent, "bigram_warm_p50_ms")
    big_common = p50_gain(common, "bigram_warm_p50_ms")

    rss_b = corpus.get("rss_before_bytes") or 0
    rss_a = corpus.get("rss_after_bytes") or 0
    mem_ratio = (rss_a / rss_b) if rss_b else None
    # fszero-9ot gate: marginal from_bytes during read+AST extract.
    cold_ingest_regress = float(ingest["cold_ingest_regress_pct"])
    # Retained 9yq bulk proxy numbers for cross-check only.
    bulk_cold_regress = None
    if bulk and bulk.get("read_all_ms"):
        bulk_cold_regress = (
            (bulk["build_bigrams_ms"] - bulk["read_all_ms"]) / bulk["read_all_ms"] * 100.0
        )

    watch_parity = bool(watch.get("parity_ok")) and bool(watch.get("deleted_absent_ok"))
    # Soft ceiling: 1ms p95 per create/modify/delete; parity is the hard gate.
    watch_cost_ok = (
        float(watch["create_upsert_p95_us"]) <= 1000.0
        and float(watch["modify_upsert_p95_us"]) <= 1000.0
        and float(watch["delete_remove_p95_us"]) <= 1000.0
    )

    gates = {
        "memmem": {
            "rare_absent_p50_improve_ge_25": mem_rare >= 25 and mem_absent >= 25,
            "common_regress_le_10": mem_common >= -10,
            "cold_ingest_regress_le_20": True,
            "memory_le_1_25x": True,
            "watch_upsert_parity": True,
            "watch_upsert_cost_p95_le_1ms": True,
            "rare_improve_pct": mem_rare,
            "absent_improve_pct": mem_absent,
            "common_improve_pct": mem_common,
        },
        "bigram_memmem": {
            "rare_absent_p50_improve_ge_25": big_rare >= 25 and big_absent >= 25,
            "common_regress_le_10": big_common >= -10,
            "cold_ingest_regress_le_20": cold_ingest_regress <= 20,
            "memory_le_1_25x": (mem_ratio is None) or (mem_ratio <= 1.25),
            "watch_upsert_parity": watch_parity,
            "watch_upsert_cost_p95_le_1ms": watch_cost_ok,
            "rare_improve_pct": big_rare,
            "absent_improve_pct": big_absent,
            "common_improve_pct": big_common,
            "cold_ingest_regress_pct": cold_ingest_regress,
            "baseline_ingest_ms": ingest["baseline_ingest_ms"],
            "with_bigram_ingest_ms": ingest["with_bigram_ingest_ms"],
            "from_bytes_sum_ms": ingest["from_bytes_sum_ms"],
            "from_bytes_p50_us": ingest["from_bytes_p50_us"],
            "from_bytes_p95_us": ingest["from_bytes_p95_us"],
            "rss_ratio": mem_ratio,
            "bigram_index_approx_bytes": corpus.get("bigram_index_approx_bytes"),
            "bulk_proxy_cold_regress_pct": bulk_cold_regress,
            "watch_upsert": {
                "k": watch["k"],
                "create_upsert_p50_us": watch["create_upsert_p50_us"],
                "create_upsert_p95_us": watch["create_upsert_p95_us"],
                "modify_upsert_p50_us": watch["modify_upsert_p50_us"],
                "modify_upsert_p95_us": watch["modify_upsert_p95_us"],
                "delete_remove_p50_us": watch["delete_remove_p50_us"],
                "delete_remove_p95_us": watch["delete_remove_p95_us"],
                "parity_ok": watch["parity_ok"],
                "deleted_absent_ok": watch["deleted_absent_ok"],
                "create_hit_count": watch["create_hit_count"],
                "modify_hit_count": watch["modify_hit_count"],
            },
        },
    }

    def passes(g: dict) -> bool:
        return all(
            g[k]
            for k in (
                "rare_absent_p50_improve_ge_25",
                "common_regress_le_10",
                "cold_ingest_regress_le_20",
                "memory_le_1_25x",
                "watch_upsert_parity",
                "watch_upsert_cost_p95_le_1ms",
            )
        )

    # fszero-kbo: ACCEPT default-on only when 9ot/up8 gates + watch upsert hold.
    big_ok = passes(gates["bigram_memmem"])
    if big_ok:
        winner = "bigram_memmem"
        decision = "ACCEPT"
    else:
        winner = None
        decision = "REJECT"

    return {
        "decision": decision,
        "winner": winner,
        "gates": gates,
        "bead": "fszero-kbo",
        "default_on": decision == "ACCEPT",
        "rationale": (
            "ACCEPT default-on for bigram+memmem only if rare/absent p50 improve >=25%, "
            "common regress <=10%, cold ingest (from_bytes during AST extract) "
            "regress <=20%, memory <=1.25x, and watch upsert parity+cost hold; "
            "else REJECT and keep FSZERO_SEARCH_PREFILTER opt-in."
        ),
    }


def write_artifacts(payload: dict) -> None:
    OUT_JSON.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    d = payload["decision"]
    bg = d["gates"]["bigram_memmem"]
    watch = bg.get("watch_upsert") or {}
    default_verdict = (
        "FLIP default to bigram_memmem"
        if d.get("default_on")
        else "KEEP default contains (opt-in only)"
    )
    lines = [
        "# Search prefilter spike (fszero-kbo default-on bakeoff)",
        "",
        f"- **Decision:** `{d['decision']}`",
        f"- **Default-on:** `{default_verdict}`",
        f"- **Winner:** `{d.get('winner')}`",
        f"- **Bead:** `{payload['bead']}`",
        f"- **Commit tip at run:** `{payload['provenance']['git_commit']}`",
        f"- **Dirty:** `{payload['provenance']['git_dirty']}`",
        f"- **Hardware:** {payload['hardware']}",
        f"- **Date:** {payload['date']}",
        f"- **Corpus:** {payload['corpus_stats']['files']} files / "
        f"{payload['corpus_stats']['bytes']} bytes",
        "",
        "## Query p50 (ms)",
        "",
        "| label | baseline | memmem | bigram warm | rare/absent gain mem / big |",
        "| --- | ---: | ---: | ---: | --- |",
    ]
    for row in payload["queries"]:
        br = row["baseline_p50_ms"]
        mm = row["memmem_p50_ms"]
        bg_ms = row["bigram_warm_p50_ms"]
        lines.append(
            f"| {row['label']} | {br:.3f} | {mm:.3f} | {bg_ms:.3f} | "
            f"{improve_pct(br, mm):+.1f}% / {improve_pct(br, bg_ms):+.1f}% |"
        )
    ingest = payload["ingest_incremental"]
    lines += [
        "",
        "## Cold ingest (lazy incremental — fszero-9ot gate)",
        "",
        "Per-file `BigramBitset::from_bytes` during read+AST extract "
        "(mirrors `ingest_one_file`), not bulk rebuild-vs-read-all.",
        "",
        f"- baseline ingest (read+extract): **{ingest['baseline_ingest_ms']:.3f} ms**",
        f"- with bigram upsert: **{ingest['with_bigram_ingest_ms']:.3f} ms**",
        f"- from_bytes sum (instrumented): **{ingest['from_bytes_sum_ms']:.3f} ms**",
        f"- from_bytes p50 / p95: **{ingest['from_bytes_p50_us']:.1f} / "
        f"{ingest['from_bytes_p95_us']:.1f} µs**",
        f"- **cold_ingest_regress: {ingest['cold_ingest_regress_pct']:.2f}%** "
        f"(from_bytes_sum / baseline_ingest; gate ≤20%)",
        "",
        "## Watch upsert (fszero-kbo)",
        "",
        f"- K={watch.get('k')} create / modify / delete on warm lazy index",
        f"- create upsert p50/p95: **{watch.get('create_upsert_p50_us', 0):.1f} / "
        f"{watch.get('create_upsert_p95_us', 0):.1f} µs**",
        f"- modify upsert p50/p95: **{watch.get('modify_upsert_p50_us', 0):.1f} / "
        f"{watch.get('modify_upsert_p95_us', 0):.1f} µs**",
        f"- delete remove p50/p95: **{watch.get('delete_remove_p50_us', 0):.1f} / "
        f"{watch.get('delete_remove_p95_us', 0):.1f} µs**",
        f"- parity_ok={watch.get('parity_ok')} deleted_absent_ok={watch.get('deleted_absent_ok')} "
        f"(create_hits={watch.get('create_hit_count')} "
        f"modify_hits={watch.get('modify_hit_count')})",
        "",
        "## Gates",
        "",
        "```json",
        json.dumps(d["gates"], indent=2),
        "```",
        "",
        "## Bulk proxy (fszero-9yq REJECT reference)",
        "",
        "```json",
        json.dumps(payload.get("amortization_bulk_proxy") or payload.get("amortization"), indent=2),
        "```",
        "",
        "## Verdict notes",
        "",
    ]
    if d["decision"] == "ACCEPT":
        lines.append(
            f"ACCEPT default-on: cold ingest regress {bg['cold_ingest_regress_pct']:.2f}% ≤20%; "
            f"rare/absent +{bg['rare_improve_pct']:.1f}% / +{bg['absent_improve_pct']:.1f}%; "
            f"common {bg['common_improve_pct']:+.1f}%; watch upsert parity+cost hold."
        )
    else:
        lines.append(
            f"REJECT default-on: keep `FSZERO_SEARCH_PREFILTER=bigram_memmem` opt-in. "
            f"cold={bg.get('cold_ingest_regress_pct', 'n/a')}% "
            f"rare={bg['rare_improve_pct']:.1f}% absent={bg['absent_improve_pct']:.1f}% "
            f"common={bg['common_improve_pct']:.1f}% rss_ratio={bg.get('rss_ratio')} "
            f"watch_parity={bg.get('watch_upsert_parity')} "
            f"watch_cost={bg.get('watch_upsert_cost_p95_le_1ms')}."
        )
    lines.append("")
    OUT_MD.write_text("\n".join(lines))

    default_line = (
        "`bigram_memmem` (escape: `FSZERO_SEARCH_PREFILTER=contains`)"
        if d.get("default_on")
        else "`str::contains` (opt-in: `FSZERO_SEARCH_PREFILTER=bigram_memmem`)"
    )
    dec_lines = [
        "# Search prefilter evaluation (fszero-9yq / 9ot / up8 / kbo)",
        "",
        f"**fszero-kbo decision: `{d['decision']}`** — default → {default_line}.",
        "",
        "## Production default",
        "",
        "| Env | Behavior |",
        "| --- | --- |",
    ]
    if d.get("default_on"):
        dec_lines += [
            "| unset / `bigram_memmem` | lazy incremental bigram filter + `memchr::memmem` |",
            "| `FSZERO_SEARCH_PREFILTER=contains` | `direct_literal_scan` uses `str::contains` |",
        ]
    else:
        dec_lines += [
            "| unset / other | `direct_literal_scan` uses `str::contains` (default) |",
            "| `FSZERO_SEARCH_PREFILTER=bigram_memmem` | lazy incremental bigram filter + `memchr::memmem` |",
        ]
    dec_lines += [
        "",
        "When bigram path is active:",
        "",
        "- First query `ensure_files` fills missing bigram bitsets from disk (lazy).",
        "- Incremental `ingest_file` upserts from bytes already loaded for extract.",
        "- Watch/remove and index rebuild drop stale entries.",
        "- **Never** bulk-rebuilds the whole corpus at `build_index` time (9yq REJECT).",
        "",
        "## Acceptance gates (fszero-9ot / up8; still hold for kbo)",
        "",
        "- rare/absent search p50 improve ≥ 25% (preserve 9yq warm gains)",
        "- common p50 regress ≤ 10%",
        "- cold ingest regress ≤ 20% (`from_bytes` during read+AST extract)",
        "- memory ≤ 1.25× RSS after materializing the lazy bigram index",
        "- watch upsert create/modify hit parity + delete absent; upsert p95 ≤ 1ms",
        "- exact hit-set parity with baseline (enforced in spike + unit tests)",
        "- no fuzzy ranking / mmap scope",
        "",
        "## Measured outcome",
        "",
        f"See [`benchmarks/search-prefilter-spike.md`](../../benchmarks/search-prefilter-spike.md) "
        f"(artifact tip at kbo run: `{payload['provenance']['git_commit']}`).",
        "",
        "### Prior REJECT (fszero-9yq)",
        "",
        "Bulk materialization failed amortization (`build_bigrams` ~4.16× `read_all`).",
        "Incremental ingest accounting (9ot) cleared the cold gate.",
        "",
        "### fszero-up8",
        "",
        "Wired `scan_bigram_memmem` into `direct_literal_scan` behind "
        "`FSZERO_SEARCH_PREFILTER=bigram_memmem` (opt-in first land).",
        "",
        "### fszero-kbo result",
        "",
    ]
    if d["decision"] == "ACCEPT":
        dec_lines.append(
            "Re-bench on gold spike corpus cleared 9ot/up8 gates plus watch upsert. "
            "Production default flipped to `bigram_memmem`; `contains` remains an escape hatch."
        )
    else:
        dec_lines.append(
            "Re-bench failed one or more default-on gates. Keep production default on "
            "`str::contains`; opt-in `FSZERO_SEARCH_PREFILTER=bigram_memmem` unchanged."
        )
    dec_lines += [
        "",
        "## History",
        "",
        "- `fszero-9yq`: REJECT bulk amortization",
        "- `fszero-9ot`: ACCEPT incremental ingest",
        "- `fszero-up8`: wire opt-in production path",
        "- `fszero-kbo`: default-on bakeoff (this doc)",
        "",
    ]
    DECISION.parent.mkdir(parents=True, exist_ok=True)
    DECISION.write_text("\n".join(dec_lines))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--files", type=int, default=1200)
    ap.add_argument("--target-bytes", type=int, default=100 * 1024 * 1024)
    ap.add_argument("--iters", type=int, default=MIN_MEASURED_RUNS)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--keep-corpus", type=Path, default=None)
    args = ap.parse_args()

    if args.iters < MIN_MEASURED_RUNS:
        ap.error(f"--iters must be at least {MIN_MEASURED_RUNS}")
    if args.files < 1000:
        raise SystemExit("--files must be >= 1000")
    if args.target_bytes < 100 * 1024 * 1024:
        raise SystemExit("--target-bytes must be >= 100MB")

    build_spike()
    corpus_parent = args.keep_corpus or Path(tempfile.mkdtemp(prefix="fszero-kbo-"))
    corpus = corpus_parent if args.keep_corpus else corpus_parent / "corpus"
    stats = generate_corpus(corpus, args.files, args.target_bytes, args.seed)
    if stats["files"] < 1000 or stats["bytes"] < 100 * 1024 * 1024:
        raise SystemExit(f"corpus too small: {stats}")

    t0 = time.time()
    events = run_spike(corpus, args.iters)
    elapsed = time.time() - t0
    decision = decide(events)
    bulk = next(
        (e for e in events if e.get("event") in ("amortization_bulk_proxy", "amortization")),
        None,
    )
    watch = next(e for e in events if e.get("event") == "watch_upsert")
    payload = {
        "bead": "fszero-kbo",
        "date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "hardware": hardware(),
        "provenance": git_provenance(),
        "corpus_stats": stats,
        "iters": args.iters,
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_policy": "strategy setup is explicit; no measured query trial excluded",
            "percentile_method": "nearest index after sorting a copy of ordered raw samples",
            "outlier_policy": "none; retain every ordered raw sample",
            "sample_size_exceptions": [
                {
                    "metric": "queries[].bigram_cold_fill_ms",
                    "sample_size_exception": True,
                    "sample_count": 1,
                    "reason": "one cold materialization precedes the warm trial vector",
                    "conservative_tail": True,
                    "tail_status": "unresolved; the single observation is the maximum",
                },
                {
                    "metric": "ingest_incremental paired cold-ingest walls",
                    "sample_size_exception": True,
                    "sample_count": 1,
                    "reason": "preserve the existing paired amortization decision contract",
                    "conservative_tail": True,
                    "tail_status": "unresolved; each arm's single observation is its maximum",
                },
                {
                    "metric": "amortization_bulk_proxy",
                    "sample_size_exception": True,
                    "sample_count": 1,
                    "reason": "historical rejected proxy retained for comparison only",
                    "conservative_tail": True,
                    "tail_status": "unresolved; each single observation is its maximum",
                },
            ],
        },
        "elapsed_s": elapsed,
        "events": events,
        "queries": [e for e in events if e.get("event") == "query"],
        "ingest_incremental": next(
            e for e in events if e.get("event") == "ingest_incremental"
        ),
        "watch_upsert": watch,
        "amortization_bulk_proxy": bulk,
        "decision": decision,
        "follow_ups": [],
    }
    write_artifacts(payload)
    print(
        json.dumps(
            {
                "decision": decision["decision"],
                "winner": decision["winner"],
                "default_on": decision.get("default_on"),
            },
            indent=2,
        )
    )
    print(f"wrote {OUT_JSON}")
    print(f"wrote {OUT_MD}")
    print(f"wrote {DECISION}")
    if not args.keep_corpus:
        shutil.rmtree(corpus_parent, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
