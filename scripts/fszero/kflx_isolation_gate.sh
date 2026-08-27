#!/usr/bin/env bash
# kflx isolation release gate — per-repo SQLite under global store root.
set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export JOBS="${JOBS:-2}"
export TEST_THREADS="${TEST_THREADS:-2}"
cd "$(dirname "$0")/../.."
echo "kflx isolation gate: repo_isolation + store_migration unit tests"
cargo test -p fs-zero --test repo_isolation --lib --jobs 1 \
  store_migration global_host_shards two_repos_under_global \
  -- --test-threads="${TEST_THREADS}"
echo "kflx isolation gate: OK"
