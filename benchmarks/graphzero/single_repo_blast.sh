#!/usr/bin/env bash
# Single-repository blast timing only. This script makes no org-wide or
# multi-repository scaling claim.
set -euo pipefail
repo="${1:-.}"
bin="${GRAPHZERO_BIN:-target/release/graphzero}"
intent="${GRAPHZERO_BLAST_INTENT:-change signature of record_verification_graph}"
budget="${GRAPHZERO_BLAST_BUDGET:-20}"
depth="${GRAPHZERO_BLAST_DEPTH:-4}"
runs="${GRAPHZERO_BLAST_RUNS:-5}"
"$bin" index --repo "$repo" >/dev/null
for i in $(seq 1 "$runs"); do
  start_ns=$(python3 -c "import time; print(time.perf_counter_ns())")
  out=$("$bin" blast --intent "$intent" --budget "$budget" --depth "$depth" --repo "$repo")
  end_ns=$(python3 -c "import time; print(time.perf_counter_ns())")
  bytes=$(printf "%s" "$out" | wc -c | tr -d " ")
  ms=$(python3 -c "print(round(($end_ns - $start_ns) / 1000000, 3))")
  printf '{"run":%s,"elapsed_ms":%s,"output_bytes":%s}
' "$i" "$ms" "$bytes"
done
