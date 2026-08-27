#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/profile_build.sh [--print-command] [--cargo-command COMMAND] [cargo arguments...]

Run a Cargo build or bench command with the release-perf profile and forced
frame pointers. Existing RUSTFLAGS are preserved.
EOF
}

print_command=0
cargo_command=build
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --print-command)
      print_command=1
      shift
      ;;
    --cargo-command)
      if [[ $# -lt 2 ]]; then
        echo "profile_build.sh: --cargo-command requires a value" >&2
        exit 2
      fi
      cargo_command="$2"
      shift 2
      ;;
    *)
      break
      ;;
  esac
done

export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }-C force-frame-pointers=yes"
cargo_bin="${CARGO:-cargo}"
command=("$cargo_bin" "$cargo_command" --profile release-perf "$@")

if [[ "$print_command" == 1 ]]; then
  printf 'RUSTFLAGS=%q' "$RUSTFLAGS"
  printf ' %q' "${command[@]}"
  printf '\n'
  exit 0
fi

exec "${command[@]}"
