#!/usr/bin/env bash
# Narrow Miri gate for zero-ref (#![forbid(unsafe_code)]).
#
# This is the F-MIRI-NARROW floor: one crate, one thread, rch-offloaded.
# A failing UB report must fail the gate. Not a host rustup-probe.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export TMPDIR=/tmp
exec rch exec -- env RUSTUP_TOOLCHAIN=nightly \
  cargo-miri miri test -p zero-ref -- --test-threads=1
