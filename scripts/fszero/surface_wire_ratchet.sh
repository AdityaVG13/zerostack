#!/usr/bin/env bash
# Release-perf persistent-wire ratchet for fszero-2uln.
# The ignored test builds both shipped surfaces in the inherited target pool.
set -euo pipefail

export CARGO_BUILD_JOBS=1
export FSZERO_BENCH_PROFILE=release-perf

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR="$TARGET_DIR"
OUT_DIR="${FSZERO_WIRE_RATCHET_OUT_DIR:-$TARGET_DIR/fszero-evidence/surface-wire-ratchet}"
mkdir -p "$OUT_DIR"
EVIDENCE="$OUT_DIR/evidence.json"
TEST_LOG="$OUT_DIR/test.log"

# Preserve evidence and the original test status, including ratchet failures.
set +e
cargo test --profile release-perf -p fs-zero --no-default-features --features dev-harness,sqlite-system --test surface_bench n_ge_3_codemode_json_vs_fastmcp_ratchet_passes -- --ignored --nocapture --test-threads=1 >"$TEST_LOG" 2>&1
test_status=$?
set -e

# Extract one complete JSON object. raw_decode permits trailing cargo/test output.
set +e
python3 - "$TEST_LOG" "$EVIDENCE" <<'PY'
import json
import pathlib
import sys

log = pathlib.Path(sys.argv[1]).read_text()
out = pathlib.Path(sys.argv[2])
starts = [i for i, char in enumerate(log) if char == "{"]
for start in reversed(starts):
    try:
        document, _ = json.JSONDecoder().raw_decode(log[start:])
    except json.JSONDecodeError:
        continue
    if not isinstance(document, dict) or document.get("schema") != "fszero.surface_wire_ratchet.v1":
        continue
    provenance = document.get("provenance", {})
    if provenance.get("profile") != "release-perf" or provenance.get("cargo_profile") != "release-perf":
        raise SystemExit("surface_wire_ratchet: evidence is not release-perf")
    if document.get("scope") != "persistent_stdio_json_rpc":
        raise SystemExit("surface_wire_ratchet: evidence scope is not persistent stdio")
    if document.get("ratchet", {}).get("threshold_multiplier") != 2:
        raise SystemExit("surface_wire_ratchet: evidence multiplier is not exactly 2")
    out.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    raise SystemExit(0)
raise SystemExit("surface_wire_ratchet: no release-perf wire evidence JSON found")
PY
extract_status=$?
set -e
if [[ "$extract_status" -ne 0 ]]; then
  echo "surface_wire_ratchet: no valid evidence; preserved $TEST_LOG" >&2
  exit 1
fi
cat "$EVIDENCE"

# Apply the committed validator (fszero-tep8.2): exact release-perf /
# persistent-wire / 2x p50+p95 gate, ordered equal samples >= 12, real
# binary SHA-256 fields, response validation, and reported cv_pct. Invalid
# evidence fails the ratchet loudly, independent of the test status.
set +e
python3 "$ROOT/scripts/fszero/apply_bench_ratchet.py" "$EVIDENCE" >"$OUT_DIR/ratchet.out" 2>&1
validator_status=$?
set -e
cat "$OUT_DIR/ratchet.out"
if [[ "$validator_status" -ne 0 ]]; then
  echo "surface_wire_ratchet: evidence failed validation; preserved $EVIDENCE" >&2
  exit 1
fi
# Preserve the original test status, including ratchet failures.
exit "$test_status"
