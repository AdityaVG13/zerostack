#!/usr/bin/env bash
# graphzero-o2uq.10 — fast focused PR gates (not full workspace suite).
# Invoked by .github/workflows/rust-ci.yml job `o2uq-pr-gates`.
# Usage: bash scripts/o2uq_focused_gates.sh
set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "== o2uq PR focused gates =="
cargo test -p graphzero-query --jobs 2 --lib release_gates -- --test-threads=2
cargo test -p graphzero-query --jobs 2 --lib conformance -- --test-threads=2
cargo test -p graphzero-query --jobs 2 --test conformance_corpus -- --test-threads=2
cargo test -p graphzero-query --jobs 2 --lib surface_bench -- --test-threads=2
cargo test -p graphzero-cli --jobs 2 --lib mcp_catalog -- --test-threads=2
cargo test -p graphzero-cli --jobs 2 --lib mcp::tests -- --test-threads=2
cargo test -p graphzero-cli --jobs 2 --test o2uq_real_surfaces -- --test-threads=2
echo "OK: focused o2uq gates green"
