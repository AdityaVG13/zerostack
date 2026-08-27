#!/usr/bin/env bash
# Opt-in sampler build; default product and CI builds remain unchanged.
set -euo pipefail

# Required tool: cargo. Missing cargo is a mandatory failure (exit 2), never a
# silent skip; the build command itself propagates its exact exit status.
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required for profilable builds (install via rustup: https://rustup.rs/)" >&2
  exit 2
fi

frame_flag='-C force-frame-pointers=yes'
case " ${RUSTFLAGS:-} " in
  *" $frame_flag "*) ;;
  *) RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }$frame_flag" ;;
esac
export RUSTFLAGS
exec cargo build --profile release-perf "$@"
