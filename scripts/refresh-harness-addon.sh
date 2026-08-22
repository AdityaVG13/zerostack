#!/usr/bin/env bash
# Refresh the ZeroKernel addon embedded into harness binaries (zmp/omp).
#
# Run this AFTER any change to zero-kernel / fszero / graphzero / tokenzero /
# zero-codemode so harnesses pick up fixes without ZERO_KERNEL_NATIVE_ADDON.
#
# Flow: build release prebuilds -> copy into zero-my-pi native dir ->
# regenerate embedded-addon.ts -> rebuild zmp binary -> print verification.
#
# Optional env:
#   SKIP_LINUX=1        skip linux-arm64 prebuild copy
#   SKIP_ZMP_BUILD=1    only refresh + re-embed, skip zmp compile

set -euo pipefail

ZERO_STACK="${ZERO_STACK:-$HOME/AI/ZeroStack}"
HARNESS="${HARNESS:-$HOME/Developer/zero-my-pi}"
ARCH="$(uname -m)"
PLATFORM="$(uname -s | tr '[:upper:]' '[:lower:]')"
TAG="$PLATFORM-$ARCH"

echo "==> building release prebuild ($TAG)"
(cd "$ZERO_STACK" && RUSTC_WRAPPER= cargo build --release --profile release-node -p zero-kernel-node)

SRC_LIB=$(ls "$ZERO_STACK"/target/release-node/libzero_kernel_node.* 2>/dev/null | head -1)
[ -n "$SRC_LIB" ] || { echo "ERROR: no built addon found under target/release-node"; exit 1; }

NATIVE_DIR="$HARNESS/packages/zero-kernel/native"
DST="$NATIVE_DIR/zero_kernel_product.$TAG.node"
echo "==> installing $DST"
mkdir -p "$NATIVE_DIR"
cp "$SRC_LIB" "$DST"

if [ "${SKIP_LINUX:-0}" != "1" ]; then
  echo "==> note: refresh linux artifact by building on the linux host or via rch,"
  echo "    then: TARGET_PLATFORM=linux TARGET_ARCH=arm64 bun scripts/embed-native.ts"
fi

echo "==> regenerating embedded addon"
cd "$HARNESS/packages/zero-kernel"
bun scripts/embed-native.ts

if [ "${SKIP_ZMP_BUILD:-0}" != "1" ]; then
  echo "==> rebuilding zmp"
  cd "$HARNESS/packages/coding-agent"
  bun scripts/build-binary.ts
fi

PINNED_SHA=$(grep -o '"sha256: \\"[0-9a-f]\{16\}' "$HARNESS/packages/zero-kernel/src/embedded-addon.ts" 2>/dev/null | grep -o '[0-9a-f]\{16\}$' || true)
ACTUAL_SHA=$(shasum -a 256 "$DST" | cut -c1-16)
echo "==> fresh addon sha16: $ACTUAL_SHA ${PINNED_SHA:+(embedded pin starts: $PINNED_SHA)}"
echo "Done. Restart zmp WITHOUT ZERO_KERNEL_NATIVE_ADDON to run the new kernel."
