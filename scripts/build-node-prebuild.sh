#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAX_BYTES=50000000

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) platform=darwin-arm64; library=libzero_kernel_node.dylib ;;
  Darwin:x86_64) platform=darwin-x64; library=libzero_kernel_node.dylib ;;
  Linux:aarch64) platform=linux-arm64-gnu; library=libzero_kernel_node.so ;;
  Linux:x86_64) platform=linux-x64-gnu; library=libzero_kernel_node.so ;;
  *) printf 'unsupported Node prebuild target: %s/%s\n' "$(uname -s)" "$(uname -m)" >&2; exit 1 ;;
esac

cd "$ROOT"
if [[ "${1:-}" != "--stage-only" ]]; then
  cargo build --profile release-node -p zero-kernel-node
fi

source="target/release-node/$library"
destination="bindings/node/prebuilds/$platform/zero_kernel_product.node"
install -d "$(dirname "$destination")"
install -m 0755 "$source" "$destination"

bytes="$(wc -c < "$destination" | tr -d ' ')"
if (( bytes >= MAX_BYTES )); then
  printf 'zero_kernel_product.node is %s bytes; budget is less than %s bytes\n' "$bytes" "$MAX_BYTES" >&2
  exit 1
fi
printf '%s: %s bytes\n' "$destination" "$bytes"
