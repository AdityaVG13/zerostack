#!/usr/bin/env bash
# tokenzero-irx9.10 — focused parity + packaging + dispatcher + real-transport gates.
# Never runs workspace-wide cargo. Fail-closed; prints operation/surface context.
# Mandatory dependency of `make release-check` and CI rust-release-gates.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
JOBS="${CARGO_JOBS:-2}"
THREADS="${TEST_THREADS:-2}"
fail() {
  echo "irx9_release_gate: FAIL: $*" >&2
  exit 1
}

run() {
  echo "+ $*"
  "$@"
}

echo "irx9_release_gate: start (CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS jobs=$JOBS threads=$THREADS)"

# 1) Packaging mutual exclusion (lifecycle + install-prefix runtime)
run cargo test -p tokenzero-install --jobs "$JOBS" --lib packaging -- --test-threads="$THREADS" \
  || fail "packaging unit tests (surface=install)"
run cargo test -p tokenzero-cli --jobs "$JOBS" --test packaging_lifecycle -- --test-threads="$THREADS" \
  || fail "packaging lifecycle (surface=package)"
run cargo build -p tokenzero-worker --bin tokenzero-codemode --no-default-features --jobs "$JOBS" \
  || fail "canonical worker build (surface=package)"
run cargo test -p tokenzero-cli --jobs "$JOBS" --test packaging_e2e -- --test-threads="$THREADS" \
  || fail "canonical packaging selector/block/uninstall smoke (surface=package)"

# 2) Operation ABI + digest ratchet
run cargo test -p tokenzero-core --jobs "$JOBS" --lib operation_abi -- --test-threads="$THREADS" \
  || fail "operation ABI contract (surface=core)"

# 3) Dispatcher identity + classic FastMCP / aggregate-binding evidence
run cargo test -p tokenzero-engine --jobs "$JOBS" --test dispatcher -- --test-threads="$THREADS" \
  || fail "dispatcher identity (surface=engine)"
run cargo test -p tokenzero-engine --jobs "$JOBS" --test fastmcp_adapter_from_registry -- --test-threads="$THREADS" \
  || fail "FastMCP adapter derivation (surface=mcp)"
run cargo test -p tokenzero-engine --jobs "$JOBS" --test codemode_bindings_dispatcher -- --test-threads="$THREADS" \
  || fail "aggregate bindings (surface=aggregate)"

# 4) Private worker + handshake
run cargo test -p tokenzero-engine --jobs "$JOBS" --lib raw_worker:: -- --test-threads="$THREADS" \
  || fail "raw worker protocol (surface=raw_worker)"
run cargo test -p tokenzero-engine --jobs "$JOBS" --lib surface_handshake:: -- --test-threads="$THREADS" \
  || fail "surface handshake (surface=handshake)"

# 5) In-process corpus + REAL transport matrix
run cargo test -p tokenzero-engine --jobs "$JOBS" --test irx9_conformance_corpus -- --test-threads="$THREADS" \
  || fail "conformance corpus (surface=parity)"
run cargo test -p tokenzero-cli --jobs "$JOBS" --test irx9_transport_matrix -- --test-threads=1 \
  || fail "real transport matrix CLI/classic-MCP/raw-worker (surface=transport)"

# 6) Surface latency: in-process + real process starts kill-test
run cargo test -p tokenzero-engine --jobs "$JOBS" --test irx9_surface_bench -- --test-threads="$THREADS" \
  || fail "in-process surface bench (surface=bench)"
run cargo test -p tokenzero-cli --jobs "$JOBS" --test irx9_surface_bench_process -- --test-threads=1 \
  || fail "real process surface bench + kill-test (surface=bench_process)"

# 7) Gate-C dependency and source boundary guards
WORKER_TREE="$(cargo tree -p tokenzero-worker --no-default-features -e normal --depth 1)"
printf '%s\n' "$WORKER_TREE"
if printf '%s\n' "$WORKER_TREE" | grep -Eiq '(^|── )(rquickjs|fastmcp|zerostack-machine-permit|zero-machine-permit|zero-codemode|tokenzero-mcp)( |$)'; then
  fail "planner-free worker graph contains a forbidden host dependency"
fi
ENGINE_TREE="$(cargo tree -p tokenzero-engine -e normal --depth 2)"
if printf '%s\n' "$ENGINE_TREE" | grep -Eiq 'zero-gate|machine-permit|rquickjs|fastmcp'; then
  fail "TokenZero engine graph contains a hub-owned gate/host dependency"
fi
if grep -Rq 'surface-codemode' crates/tokenzero/*/Cargo.toml; then
  fail "retired surface-codemode feature remains in a crate manifest"
fi
if find crates/tokenzero/tokenzero-codemode/src -type f ! -name main.rs | grep -q .; then
  fail "retired engine-local CodeMode source remains beside the raw-worker entrypoint"
fi
run cargo test -p tokenzero-cli --jobs "$JOBS" --test gate_c_retirement_contract -- --test-threads=1 \
  || fail "Gate-C semantic retirement contract"

# 8) Makefile wiring: release-check must depend on irx9-gate
if ! grep -E '^release-check:.*irx9-gate' project-meta/tokenzero/Makefile >/dev/null; then
  fail "Makefile release-check must depend on irx9-gate"
fi
echo "irx9_release_gate: Makefile release-check depends on irx9-gate"

echo "irx9_release_gate: PASS"
