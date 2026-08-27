#!/usr/bin/env bash
# tz_* + cargo experiment runner for snap export benches/gates.
# Run: bash scripts/perf/cargo_bench_harness.sh
# Shared hyperfine floors with hyperfine_snap_export.sh (claim-eligible: measured N ≥ 20).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$REPO_ROOT"
HYPERFINE_WARMUP="${HYPERFINE_WARMUP:-5}"
HYPERFINE_RUNS="${HYPERFINE_RUNS:-30}"
echo "[tz cargo] cargo bench snap export + gate runs"
echo "Warm/cold + export_capsule + size A/B + full loop"
cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture || true
cargo bench -p graphzero-store --bench snap_to_file -- --save-baseline snap-export-current || true
# hyperfine if avail (for CLI full path)
if command -v hyperfine >/dev/null; then
  cargo build -p graphzero-cli --release
  hyperfine --warmup "$HYPERFINE_WARMUP" --runs "$HYPERFINE_RUNS" --export-json /tmp/hyperfine_snap_export.json \
    'target/release/graphzero snap sym_10 --budget 1 --export /tmp/hf_min.json --format minimal --repo . ' || true
fi
echo "Artifacts in target/gate-artifacts/snap_export_perf/ and /tmp/*.json"
echo "Validate: cat target/gate-artifacts/snap_export_perf/latest.json | grep -E 'gz-snap|size|latency' "
