#!/usr/bin/env python3
"""Render ranked hotspot-table.md + hypothesis-ledger.md from perf campaign JSON.

Consumes:
  - hyperfine native export: { "results": [ { command, mean, stddev, min, max, ... } ] }
  - optional hyperfine_summary.json: { scenario_id: { mean, stddev, min, max, times } }
  - optional span/samply notes via --notes JSON: { "path": "note about stack" }

Outputs (into --out-dir, default = input dir):
  - hotspot-table.md
  - hypothesis-ledger.md

Refresh policy
--------------
Regenerate when starting or closing a profiling campaign, after a host-timed
rebaseline, or when a skill hand-off requires ranked hotspots. Historical
packets under tests/artifacts/perf/ are frozen evidence -- re-render into a
new dated directory rather than rewriting past campaign roots.

Example:
  python3 scripts/render_hotspot_table.py \\
    --input-dir tests/artifacts/perf/20260702-130427-graphzero-jeffrey-perf \\
    --check

graphzero-xdtcp
"""

from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class Sample:
    scenario: str
    source: str
    mean_s: float
    stddev_s: float | None = None
    min_s: float | None = None
    max_s: float | None = None
    n: int = 0
    command: str | None = None
    notes: list[str] = field(default_factory=list)

    @property
    def mean_ms(self) -> float:
        return self.mean_s * 1000.0


def _load_json(path: Path) -> Any:
    text = path.read_text(encoding="utf-8").strip()
    if not text:
        raise ValueError(f"empty JSON file: {path}")
    return json.loads(text)


def _scenario_from_name(path: Path) -> str:
    name = path.stem
    name = re.sub(r"^hyperfine_", "", name)
    return name


def load_hyperfine_file(path: Path) -> list[Sample]:
    data = _load_json(path)
    samples: list[Sample] = []
    scenario = _scenario_from_name(path)

    if isinstance(data, dict) and "results" in data:
        for i, row in enumerate(data["results"]):
            times = row.get("times") or []
            sid = scenario if len(data["results"]) == 1 else f"{scenario}#{i}"
            samples.append(
                Sample(
                    scenario=sid,
                    source=str(path.name),
                    mean_s=float(row["mean"]),
                    stddev_s=float(row["stddev"]) if row.get("stddev") is not None else None,
                    min_s=float(row["min"]) if row.get("min") is not None else None,
                    max_s=float(row["max"]) if row.get("max") is not None else None,
                    n=len(times) if times else int(row.get("times_count") or 0),
                    command=row.get("command"),
                )
            )
        return samples

    # summary map form: { scenario: {mean, ...}, ... }
    if isinstance(data, dict) and all(isinstance(v, dict) and "mean" in v for v in data.values()):
        for sid, row in data.items():
            times = row.get("times") or []
            samples.append(
                Sample(
                    scenario=str(sid),
                    source=str(path.name),
                    mean_s=float(row["mean"]),
                    stddev_s=float(row["stddev"]) if row.get("stddev") is not None else None,
                    min_s=float(row["min"]) if row.get("min") is not None else None,
                    max_s=float(row["max"]) if row.get("max") is not None else None,
                    n=len(times),
                    command=row.get("command"),
                )
            )
        return samples

    raise ValueError(f"unrecognized hyperfine/span JSON shape: {path}")


def collect_samples(input_dir: Path, notes: dict[str, str]) -> list[Sample]:
    samples: list[Sample] = []
    paths = sorted(input_dir.glob("hyperfine_*.json"))
    # Prefer per-scenario native files; include summary only if no per-scenario.
    native = [p for p in paths if p.name != "hyperfine_summary.json"]
    if native:
        for path in native:
            try:
                samples.extend(load_hyperfine_file(path))
            except (ValueError, json.JSONDecodeError, KeyError, TypeError) as err:
                print(f"warning: skip {path.name}: {err}", file=sys.stderr)
    else:
        summary = input_dir / "hyperfine_summary.json"
        if summary.is_file():
            samples.extend(load_hyperfine_file(summary))

    # Optional bare span JSON (array of {name, mean_s|mean_ms|duration_ms})
    for path in sorted(input_dir.glob("span_*.json")):
        data = _load_json(path)
        rows = data if isinstance(data, list) else data.get("spans") or data.get("results") or []
        for row in rows:
            if "mean_s" in row:
                mean_s = float(row["mean_s"])
            elif "mean_ms" in row:
                mean_s = float(row["mean_ms"]) / 1000.0
            elif "duration_ms" in row:
                mean_s = float(row["duration_ms"]) / 1000.0
            elif "mean" in row:
                mean_s = float(row["mean"])
            else:
                continue
            samples.append(
                Sample(
                    scenario=str(row.get("name") or row.get("scenario") or path.stem),
                    source=path.name,
                    mean_s=mean_s,
                    n=int(row.get("n") or 1),
                )
            )

    for s in samples:
        key = s.scenario
        if key in notes:
            s.notes.append(notes[key])
        if s.source in notes:
            s.notes.append(notes[s.source])
    return samples


def rank_samples(samples: list[Sample]) -> list[Sample]:
    # De-dupe by scenario keeping highest mean (native usually unique).
    best: dict[str, Sample] = {}
    for s in samples:
        prev = best.get(s.scenario)
        if prev is None or s.mean_s > prev.mean_s:
            best[s.scenario] = s
    return sorted(best.values(), key=lambda s: s.mean_s, reverse=True)


def category_for(sample: Sample) -> str:
    name = sample.scenario.lower()
    if "index" in name:
        return "CPU/IO"
    if "expand" in name:
        return "IO"
    if "orient" in name or "outline" in name or "delta" in name:
        return "CPU"
    if "mem" in name or "recall" in name:
        return "CPU"
    if "serve" in name or "mcp" in name:
        return "CPU"
    return "mixed"


def location_guess(sample: Sample) -> str:
    name = sample.scenario.lower()
    if "s1a" in name or "index_fresh" in name:
        return "indexer::collect + extract (cold)"
    if "s1b" in name or "index_warm" in name:
        return "index_repo warm path"
    if "delta" in name:
        return "compute_repo_delta / worktree map"
    if "serve" in name:
        return "MCP serve init+orient"
    if "expand" in name and "window" in name:
        return "expand window fragment"
    if "expand" in name:
        return "expand whole blob"
    if "mem" in name and "without" not in name:
        return "MemoryIndex hints attach"
    if "recall" in name:
        return "recall round-trip"
    if "outline" in name:
        return "outline surface"
    if sample.command:
        cmd = sample.command
        if len(cmd) > 80:
            cmd = cmd[:77] + "..."
        return f"cmd: {cmd}"
    return sample.scenario


def render_hotspot_table(ranked: list[Sample], campaign: str) -> str:
    lines = [
        f"# Hotspot table — {campaign}",
        "",
        "Generated by `scripts/render_hotspot_table.py` (graphzero-xdtcp).",
        "Ranked by mean wall time. Categories are heuristic until a human edits the row.",
        "",
        "| Rank | Location | Metric | Value | Category | Evidence |",
        "|------|----------|--------|-------|----------|----------|",
    ]
    for i, s in enumerate(ranked, start=1):
        value = f"{s.mean_ms:.2f}ms mean"
        if s.stddev_s is not None:
            value += f" ±{s.stddev_s * 1000.0:.2f}ms"
        if s.n:
            value += f" (n={s.n})"
        notes = "; ".join(s.notes) if s.notes else ""
        evidence = s.source
        if notes:
            evidence = f"{evidence}; {notes}"
        lines.append(
            f"| {i} | {location_guess(s)} | mean wall | {value} | {category_for(s)} | {evidence} |"
        )
    if not ranked:
        lines.append("| — | (no hyperfine/span samples found) | — | — | — | — |")
    lines.append("")
    lines.append("## Refresh policy")
    lines.append("")
    lines.append(
        "Re-run this generator for a **new** dated campaign directory after host-timed "
        "profiling (hyperfine/samply/span). Do not rewrite historical artifact roots."
    )
    lines.append("")
    return "\n".join(lines)


def render_hypothesis_ledger(ranked: list[Sample], campaign: str, seeds: list[dict[str, str]]) -> str:
    lines = [
        f"# Hypothesis ledger — {campaign}",
        "",
        "Generated by `scripts/render_hotspot_table.py` (graphzero-xdtcp).",
        "Seed rows are **open** until a human records verdict + evidence.",
        "",
        "| Hypothesis | Verdict | Evidence |",
        "|------------|---------|----------|",
    ]
    if seeds:
        for row in seeds:
            lines.append(
                f"| {row.get('hypothesis', '').strip()} | {row.get('verdict', 'open')} | {row.get('evidence', '')} |"
            )
    else:
        # Auto-seed from top hotspots for the campaign operator to close.
        for s in ranked[:8]:
            hyp = (
                f"Dominant cost for `{s.scenario}` is {location_guess(s)} "
                f"at ~{s.mean_ms:.1f}ms mean"
            )
            lines.append(f"| {hyp} | open | {s.source} |")
        if not ranked:
            lines.append("| (no samples; add hyperfine JSON) | open | |")
    lines.append("")
    lines.append("Verdict vocabulary: `open` | `supports` | `rejects` | `inconclusive`.")
    lines.append("")
    return "\n".join(lines)


def load_hypothesis_seeds(path: Path | None) -> list[dict[str, str]]:
    if path is None or not path.is_file():
        return []
    data = _load_json(path)
    if isinstance(data, list):
        return [dict(x) for x in data]
    if isinstance(data, dict) and "hypotheses" in data:
        return [dict(x) for x in data["hypotheses"]]
    raise ValueError(f"hypothesis seed must be a list or {{hypotheses: [...]}}: {path}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--input-dir", type=Path, required=True, help="Campaign directory with hyperfine_*.json")
    ap.add_argument("--out-dir", type=Path, default=None, help="Default: same as --input-dir")
    ap.add_argument("--notes", type=Path, default=None, help="Optional JSON map scenario→note")
    ap.add_argument(
        "--hypothesis-seed",
        type=Path,
        default=None,
        help="Optional JSON list of {hypothesis,verdict,evidence}",
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="Render to memory and require ≥1 sample; do not write files",
    )
    ap.add_argument(
        "--stale-days",
        type=int,
        default=0,
        help="If >0 with --check, warn (exit 0) when no hotspot-table.md or mtime older than N days",
    )
    args = ap.parse_args(argv)

    input_dir = args.input_dir
    if not input_dir.is_dir():
        print(f"error: input dir not found: {input_dir}", file=sys.stderr)
        return 2

    notes: dict[str, str] = {}
    if args.notes and args.notes.is_file():
        raw = _load_json(args.notes)
        if not isinstance(raw, dict):
            print("error: --notes must be a JSON object", file=sys.stderr)
            return 2
        notes = {str(k): str(v) for k, v in raw.items()}

    samples = collect_samples(input_dir, notes)
    ranked = rank_samples(samples)
    campaign = input_dir.name
    seeds = load_hypothesis_seeds(args.hypothesis_seed)

    table = render_hotspot_table(ranked, campaign)
    ledger = render_hypothesis_ledger(ranked, campaign, seeds)

    if args.check:
        if not ranked:
            print("error: no hyperfine/span samples found", file=sys.stderr)
            return 1
        print(f"ok: {len(ranked)} ranked scenarios from {input_dir}")
        print(table.splitlines()[6] if len(table.splitlines()) > 6 else table)
        if args.stale_days > 0:
            ht = input_dir / "hotspot-table.md"
            if not ht.is_file():
                print(f"advisory: missing {ht} (stale check)", file=sys.stderr)
            else:
                import time

                age_days = (time.time() - ht.stat().st_mtime) / 86400.0
                if age_days > args.stale_days:
                    print(
                        f"advisory: hotspot-table.md is {age_days:.0f}d old (>{args.stale_days}d)",
                        file=sys.stderr,
                    )
        return 0

    out_dir = args.out_dir or input_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "hotspot-table.md").write_text(table, encoding="utf-8")
    (out_dir / "hypothesis-ledger.md").write_text(ledger, encoding="utf-8")
    print(f"wrote {out_dir / 'hotspot-table.md'}")
    print(f"wrote {out_dir / 'hypothesis-ledger.md'}")
    print(f"ranked {len(ranked)} scenarios")
    return 0


if __name__ == "__main__":
    # silence unused-import lint for statistics/math kept for future CV rows
    _ = (math, statistics)
    raise SystemExit(main())
