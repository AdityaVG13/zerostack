#!/bin/sh
# FSZero benchmark: regenerates benchmarks/demo-bench_results.json.
# Usage: ./scripts/benchmark.sh
# Requirements: cargo, python3
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Build the profilable release-perf binary if missing.
if [ ! -x target/release-perf/fszero ]; then
    echo "Building FSZero release-perf binary..." >&2
    ./scripts/fszero/profile_build.sh -p fszero-cli --bin fszero
fi

exec python3 scripts/fszero/benchmark.py "$@"
