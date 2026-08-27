#!/usr/bin/env bash
# P0 graphzero-222h: nondeterminism injection guard for CI.
#
# Runs the deterministic_graph_renders test suite N times. Each process
# gets fresh HashMap random seeds (Rust's RandomState). If any output
# depends on HashMap iteration order, a run will fail.
#
# Usage: scripts/determinism_guard.sh [N]
#   N — number of repeated process runs (default: 10)

set -euo pipefail

N="${1:-10}"
CRATE="graphzero-query"
TEST_FILTER="deterministic_graph_renders"

echo "[determinism_guard] running ${TEST_FILTER} ${N}x across fresh processes"

for i in $(seq 1 "$N"); do
  echo "  run ${i}/${N}..."
  cargo test -p "$CRATE" --test "$TEST_FILTER" -- --nocapture 2>&1 | tail -5
done

echo "[determinism_guard] all ${N} runs passed — output is class-1 deterministic"
