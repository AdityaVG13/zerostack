#!/usr/bin/env bash
# GraphZero benchmark suite.
# Regenerates benchmarks/latency/results.json from a clean checkout on this machine.
# Usage: ./scripts/benchmark.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_ROOT"

echo "=== GraphZero Benchmark Suite ==="
echo "Repo: $REPO_ROOT"

# The driver builds all three mutually-exclusive surfaces under one profile.
# Ordinary release is the shipping-latency default; choose release-perf for
# line-table-compatible comparison with CI host-timed gates.
GRAPHZERO_BENCH_PROFILE="${GRAPHZERO_BENCH_PROFILE:-release}"
case "$GRAPHZERO_BENCH_PROFILE" in
  release|release-perf) ;;
  *) echo "unsupported GRAPHZERO_BENCH_PROFILE: $GRAPHZERO_BENCH_PROFILE" >&2; exit 2 ;;
esac
export GRAPHZERO_BENCH_PROFILE
echo "--- Benchmark profile: $GRAPHZERO_BENCH_PROFILE ---"

# Required tool: uv resolves the project environment and runs the driver.
# Missing uv is a mandatory failure (exit 2), never a silent skip.
if ! command -v uv >/dev/null 2>&1; then
  echo "error: uv is required to run the benchmark suite (install from https://docs.astral.sh/uv/)" >&2
  exit 2
fi

# Run driver (it builds missing profile-matched artifacts). exec preserves the
# driver's exact exit status; there are no pipelines to mask it.
echo ""
exec uv run --locked --no-dev python "$SCRIPT_DIR/benchmark_driver.py"
