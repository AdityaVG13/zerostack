#!/bin/bash
# Hyperfine experiment script for snap-to-file export cold/warm measurements.
# Place in scripts/perf/ . Run after cargo build.
# Uses real fixtures if .graphzero present in repo or passed via GRAPHZERO_REAL_FIXTURE.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$REPO_ROOT"

# Shared hyperfine floors with cargo_bench_harness.sh (claim-eligible: measured N ≥ 20).
HYPERFINE_WARMUP="${HYPERFINE_WARMUP:-5}"
HYPERFINE_RUNS="${HYPERFINE_RUNS:-30}"

REAL_FIXTURE="${GRAPHZERO_REAL_FIXTURE:-}"
if [ -d ".graphzero" ]; then
  REAL_FIXTURE="${REAL_FIXTURE:-$REPO_ROOT}"
fi

echo "=== Snap-to-file perf experiments (sub-1ms target, <512B for b=1) ==="
echo "Using fixture: ${REAL_FIXTURE:-synthetic (GRAPHZERO_BENCH_FILES=50)}"

# Build release for fair timing
cargo build -p graphzero-cli --release 2>/dev/null || cargo build -p graphzero-cli

BIN="./target/release/graphzero"

# 1. hyperfine for CLI snap export minimal warm (export to /tmp , use --to-file)
# Use -N (no-shell) for accurate low latency measurement; post-patch re-profile
# Concrete for NEXT_STEPS: hyperfine -N --warmup $HYPERFINE_WARMUP --runs $HYPERFINE_RUNS 'target/release/graphzero snap ...'
echo "Hyperfine: snap export minimal (warm path, export) -N for p99 proxy"
hyperfine --warmup "$HYPERFINE_WARMUP" --runs "$HYPERFINE_RUNS" -N \
  --export-json /tmp/snap_export_minimal.json \
  --export-markdown /tmp/snap_export_minimal.md \
  --command-name "snap-export-min-b1-p99" \
  "$BIN snap 'sym_25' --budget 1 --export /tmp/snap_min_b1.json --format minimal --repo . 2>/dev/null" \
  || echo "hyperfine not installed or no real cmd; use python fallback"

# p99 proxy via stat or rerun with --shell none
# Also time with /usr/bin/time -l for instr/RSS (mac)
echo " /usr/bin/time -l for snap export"
for i in 1 2 3; do
  /usr/bin/time -l "$BIN snap 'sym_25' --budget 1 --export /tmp/tmexp$i.json --format minimal --repo . " 2>&1 | grep -E 'real|user|maximum resident|instructions retired' || true
  rm -f /tmp/tmexp$i.json
done

# Criterion direct (lib p99 proxy, no CLI)
echo "Criterion snap_to_file for lib export (run separately: cargo bench ...)"
# cargo bench -p graphzero-store --bench snap_to_file -- "export_capsule_latency/q=func_42_3_bgt=1" 2>&1 | cat || true

# Instruments if avail (mac dev)
if command -v xcrun >/dev/null && xcrun instruments --help >/dev/null 2>&1; then
  echo "Instruments available for post-patch: xcrun instruments -t 'Time Profiler' -D /tmp/snap.trace -l 5000 target/release/graphzero snap sym_25 --budget 1 --export /tmp/ins.json --format minimal --repo . || true"
fi

# 2. GraphZero output-size measurement.
# No competitor comparison is emitted: no deterministic, locally runnable competitor
# adapter exists under the same corpus and conditions. Synthetic data is not evidence.
echo "GraphZero output-size measurement (competitor comparison unavailable)"
for fmt in minimal capsule zst; do
  OUT="/tmp/snap_ab_${fmt}.out"
  $BIN snap "sym_25" --budget 1 --export "$OUT" --format $fmt --repo . >/dev/null 2>&1 || true
  if [ -f "$OUT" ]; then
    SZ=$(stat -f%z "$OUT" 2>/dev/null || stat -c%s "$OUT")
    echo "GZ $fmt b1 size: $SZ bytes (target <512)"
    rm -f "$OUT"
  fi
done

# A full-loop timing is intentionally unavailable until snap, blast, and handoff
# have one deterministic command sequence over the same committed fixture. Do not
# publish a timing for an empty or simulated loop.
echo "Full-loop timing unavailable: no deterministic real workflow adapter"

echo "=== Done. See /tmp/*.json for hyperfine data. Validate gz-snap/v1 in exported files. ==="
echo "Target: sub-1ms lib, <512B b=1 , warm vs cold deltas."

echo "Post-patch re-profile concrete cmds for NEXT_STEPS_EXPERIMENTS.md:"
echo "hyperfine -N --warmup ${HYPERFINE_WARMUP} --runs ${HYPERFINE_RUNS} -- \"target/release/graphzero snap 'sym_25' --budget 1 --export /tmp/snap-p99.json --format minimal --repo .\""
echo 'cargo bench -p graphzero-store --bench snap_to_file'
echo '/usr/bin/time -l target/release/graphzero snap "sym_25" --budget 1 --export /tmp/out.json --format minimal --repo .'
echo 'xcrun instruments -t "Time Profiler" ... (if avail)'
echo 'For gates + agent loop: GRAPHZERO_REAL_REPO=$REPO_ROOT cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture'
echo 'cargo test -p graphzero --test agent_loop_gate -- --nocapture'
