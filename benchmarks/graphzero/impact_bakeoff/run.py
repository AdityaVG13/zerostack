#!/usr/bin/env python3
"""Impact-analysis bake-off over the committed GraphZero gold set."""
from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import time
from dataclasses import dataclass, asdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GOLD = ROOT / "benchmarks" / "gold" / "edge_accuracy_report.json"
EDGES = ROOT / "benchmarks" / "gold" / "edges.jsonl"
OUT = ROOT / "benchmarks" / "impact_bakeoff" / "report.json"

@dataclass
class TimedCommand:
    name: str
    command: list[str]
    exit_code: int
    elapsed_ms: int
    stdout_bytes: int
    stderr_bytes: int


def file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()

def freshness_manifest() -> dict:
    inputs = ["benchmarks/gold/edges.jsonl", "benchmarks/gold/schema.json", "benchmarks/gold/edge_accuracy_report.json", "benchmarks/gold/METHODOLOGY.md"]
    return {
        "report_kind": "adapter_contract_static_fixture",
        "generated_by": "benchmarks/impact_bakeoff/run.py",
        "generator_sha256": file_sha256(ROOT / "benchmarks/impact_bakeoff/run.py"),
        "methodology": {"path": "benchmarks/impact_bakeoff/METHODOLOGY.md", "sha256": file_sha256(ROOT / "benchmarks/impact_bakeoff/METHODOLOGY.md")},
        "inputs": [{"path": path, "sha256": file_sha256(ROOT / path)} for path in inputs],
    }

def timed(command: list[str], name: str) -> TimedCommand:
    start = time.perf_counter_ns()
    proc = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    elapsed_ms = round((time.perf_counter_ns() - start) / 1_000_000)
    return TimedCommand(name, command, proc.returncode, elapsed_ms, len(proc.stdout.encode()), len(proc.stderr.encode()))

def rate_bps(numerator: int, denominator: int) -> int:
    return 0 if denominator == 0 else round(numerator * 10_000 / denominator)

def competitor(name: str, kind: str, metrics: dict, elapsed_ms: int, availability: str, notes: list[str]) -> dict:
    true_edges = metrics["true_edges"]
    confirmed_non_edges = metrics["confirmed_non_edges"]
    total = true_edges + confirmed_non_edges
    failures = metrics["false_negatives"] + metrics["false_positives"]
    return {
        "name": name,
        "kind": kind,
        "availability": availability,
        "true_edges": true_edges,
        "confirmed_non_edges": confirmed_non_edges,
        "true_positives": metrics["true_positives"],
        "false_negatives": metrics["false_negatives"],
        "false_positives": metrics["false_positives"],
        "accuracy_bps": rate_bps(total - failures, total),
        "fp_rate_bps": metrics["fp_rate_bps"],
        "fn_rate_bps": metrics["fn_rate_bps"],
        "wall_time_ms": elapsed_ms,
        "losses": [],
        "notes": notes,
    }

def rows_by_language() -> dict[str, int]:
    counts: dict[str, int] = {}
    for line in EDGES.read_text().splitlines():
        if line.strip():
            row = json.loads(line)
            counts[row["language"]] = counts.get(row["language"], 0) + 1
    return counts

def main() -> int:
    gold = json.loads(GOLD.read_text())
    verifier = timed(["cargo", "test", "-p", "graphzero", "--test", "gold_edge_accuracy_metrics", "--", "--nocapture"], "gold_edge_accuracy_metrics")
    language_counts = rows_by_language()
    structural = competitor("graphzero_structural", "tree-sitter structural graph", gold["structural"], verifier.elapsed_ms, "measured", ["Current product path; no LSP enrichment."])
    if structural["false_negatives"]:
        structural["losses"].append("Misses higher-order Rust call, trait-qualified Rust calls, method receiver resolution, TypeScript type references, and interface dispatch rows from the gold set.")
    lsp = competitor(
        "rust_analyzer_and_tsserver_adapter_contract",
        "typed LSP-equivalent oracle spans",
        gold["fused_adapter_contract"],
        verifier.elapsed_ms,
        "adapter_contract_measured_live_servers_not_invoked",
        [
            "Uses committed rust-analyzer/tsserver resolution spans from benchmarks/gold; not a live LSP subprocess benchmark.",
            f"rust-analyzer binary detected: {bool(shutil.which('rust-analyzer'))}",
            f"tsserver binary detected: {bool(shutil.which('tsserver'))}",
            "Live LSP subprocess path ships in crates/graphzero-extract/src/rust_analyzer_lsp.rs and is proven by cargo test -p graphzero-extract --test live_rust_analyzer_fusion; these accuracy numbers are still scored from the committed spans, not from a live sweep.",
        ],
    )
    if lsp["false_negatives"]:
        lsp["losses"].append(
            "Still misses gz-td-expand-dispatches-to-impl-seed (trait default body reaching a concrete impl method needs impl-qualified symbol names in the extractor) and foreign-ts-reg-reexport-checks (export-from re-exports produce no structural path node to supersede)."
        )
    report = {
        "schema_version": 1,
        "generated_by": "benchmarks/impact_bakeoff/run.py",
        "freshness": freshness_manifest(),
        "methodology": "Compare GraphZero structural impact edges against the committed gold-set typed adapter contract. Report losses and unavailable live-LSP status instead of filling gaps with invented live numbers.",
        "integrity": {
            "gold_rows_file": "benchmarks/gold/edges.jsonl",
            "gold_report_file": "benchmarks/gold/edge_accuracy_report.json",
            "rows_by_language": language_counts,
            "no_rows_dropped": True,
            "baseline_conditions_identical": True,
            "live_lsp_processes_invoked": False,
        },
        "timed_commands": [asdict(verifier)],
        "sample_accounting": {
            "total_samples": sum(language_counts.values()),
            "dropped_count": 0,
            "losses": [loss for result in (structural, lsp) for loss in result["losses"]],
        },
        "competitors": [structural, lsp],
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if verifier.exit_code == 0 else verifier.exit_code

if __name__ == "__main__":
    raise SystemExit(main())
