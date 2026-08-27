#!/usr/bin/env bash
# FSZero NCIB release gates (fszero-ncib.10).
# Wired into .github/workflows/ncib-gates.yml. Never full-workspace cargo.
set -euo pipefail

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
JOBS="${JOBS:-1}"
THREADS="${THREADS:-1}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
WAIVERS="$ROOT/docs/fszero/ncib-release-waivers.md"

mode="${1:-pr}" # pr | release
echo "ncib_release_gates: mode=$mode CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS jobs=$JOBS threads=$THREADS"
echo "ncib_release_gates: cwd=$ROOT date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"

check_waivers() {
  if [[ ! -f "$WAIVERS" ]]; then
    echo "FAIL: missing waiver policy file $WAIVERS" >&2
    exit 1
  fi
  # Use the shipped Rust parser via a tiny test binary path: cargo test named filter.
  # Also enforce with the library tests (ncib_release_gates). Shell does ISO expiry scan.
  local today
  today="$(date -u +%Y-%m-%d)"
  # Each ### waiver block must include owner/expiry/scope/rationale/evidence rows.
  local section=""
  while IFS= read -r line; do
    if [[ "$line" =~ ^###[[:space:]] ]]; then
      section="$line"
    fi
    if [[ "$line" =~ \*\*expiry\*\*.*([0-9]{4}-[0-9]{2}-[0-9]{2}) ]]; then
      local exp="${BASH_REMATCH[1]}"
      if [[ "$exp" < "$today" ]]; then
        echo "FAIL: expired waiver $section date $exp (today=$today)" >&2
        exit 1
      fi
      echo "ncib_release_gates: waiver expiry ok ($exp >= $today) in $section"
    fi
  done < "$WAIVERS"
  for field in '**owner**' '**expiry**' '**scope**' '**rationale**' '**evidence**'; do
    if ! grep -qF "$field" "$WAIVERS"; then
      echo "FAIL: waiver doc missing $field" >&2
      exit 1
    fi
  done
  echo "ncib_release_gates: waiver policy OK ($WAIVERS)"
}

run_test() {
  local target="$1"
  local filter="${2:-}"
  local cmd=(cargo test -p fs-zero --test "$target" --jobs "$JOBS")
  if [[ -n "$filter" ]]; then
    cmd+=(-- "$filter" --test-threads="$THREADS")
  else
    cmd+=(-- --test-threads="$THREADS")
  fi
  echo "==> CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS ${cmd[*]}"
  CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" "${cmd[@]}"
}

run_wire_ratchet() {
  # Canonical release-perf persistent-wire ratchet (fszero-tep8.2).
  # Replaces the old debug in-process surface_bench keep-gate (H-PERF-016):
  # debug auto-passes ~40x relative and absolute gates, so only the
  # release-perf persistent-wire evidence produced by surface_wire_ratchet.sh
  # (with apply_bench_ratchet.py) may gate CI.
  echo "==> scripts/surface_wire_ratchet.sh (release-perf persistent-wire ratchet; jobs=1 threads=1)"
  CARGO_BUILD_JOBS=1 bash "$ROOT/scripts/fszero/surface_wire_ratchet.sh"
}

check_waivers

# Committed evidence must be reproducible on any machine (zerostack-9g4).
python3 "$ROOT/scripts/fszero/check_no_host_paths.py"

# --- PR fast gates ---
run_test operation_abi
run_test dispatcher
run_test mcp_adapter
run_test surface_handshake
run_test codemode_bindings
run_test ncib_conformance
run_test packaging_lifecycle
run_test packaging_unit
run_test repo_isolation
run_test ncib_release_gates "waiver_parser"

# Release-perf persistent-wire ratchet in PR and release gating.
run_wire_ratchet

if [[ "$mode" == "release" ]]; then
  run_test codemode_fusion
  run_test packaging_e2e
  # Dual-surface temp-prefix install + exec (fails on any error).
  run_test ncib_release_gates "dual_surface_temp_prefix_install_exec_smoke"
fi

echo "ncib_release_gates: OK ($mode)"
