#!/usr/bin/env bash
# graphzero-o2uq.10 — release-tier gates: focused PR gates + packaging lifecycle smoke.
# Invoked by .github/workflows/rust-ci.yml job `o2uq-release-gates`.
#
# Does NOT remeasure host-timed orient/blast/warm_query/rebaseline benches.
# Live latency jobs live under CI `perf-gates` (and map in docs/benchmarks.md).
# Artifact/SSOT contracts for published numbers also run under `perf-gates`.
set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "== o2uq release-tier gates (focused + packaging; no bench remeasure) =="
# PR gates first
bash scripts/graphzero/o2uq_focused_gates.sh
# Release packaging lifecycle (single-surface install semantics)
cargo test -p graphzero-cli --jobs 2 --test packaging_lifecycle -- --test-threads=2
echo "OK: release o2uq gates green"
