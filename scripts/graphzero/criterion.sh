#!/usr/bin/env bash
# Supported Criterion entrypoint: keep line tables and the release-perf codegen contract.
set -euo pipefail

# Required tool: cargo. Missing cargo is a mandatory failure (exit 2), never a
# silent skip; the bench command itself propagates its exact exit status.
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required for criterion benches (install via rustup: https://rustup.rs/)" >&2
  exit 2
fi

exec cargo bench --profile release-perf "$@"
