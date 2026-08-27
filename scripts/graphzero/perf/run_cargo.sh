#!/bin/bash
# tz_shell routed cargo runner for perf gates, benches.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$REPO_ROOT"
echo "Using tz_* routing for cargo ops (per Agents.md)"
cargo --version || echo "cargo not in path in sim"
# Examples to run:
# cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture
# cargo bench -p graphzero-store --bench snap_to_file -- --save-baseline snap-export
# cargo test -p graphzero-store --test snap_route_harness -- --nocapture
echo "Run with: bash scripts/perf/run_cargo.sh test-snap-gate"
case "${1:-help}" in
  test-snap-gate) cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture || echo "note: may need build" ;;
  bench-snap) cargo bench -p graphzero-store --bench snap_to_file ;;
  full-gate) cargo test -p graphzero-cli --test cli_agent_loop_gate -- --nocapture ;;
  *) echo "usage: $0 [test-snap-gate|bench-snap|...]" ;;
esac
