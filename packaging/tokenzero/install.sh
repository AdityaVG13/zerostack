#!/usr/bin/env bash
# Canonical TokenZero worker selector/probe. Direct installation remains blocked
# until ZeroStack central worker discovery adoption (zerostack-uf1u).

set -euo pipefail

SURFACE=""
PREFIX="${TOKENZERO_INSTALL_PREFIX:-${HOME}/.tokenzero-install}"
BIN_DIR="${TOKENZERO_BIN_DIR:-${HOME}/.local/bin}"
ACTION="install"
DRY_RUN=0
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

usage() {
  cat <<EOF
usage: $0 --surface raw-worker --dry-run
       $0 --sbom --surface raw-worker [--bin-dir DIR]
       $0 --uninstall [--prefix DIR] [--bin-dir DIR]

Canonical backend: planner-free tokenzero-codemode from package tokenzero-worker.
Classic MCP remains a separate compatibility package. Direct worker install waits for ZeroStack central discovery.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --surface) SURFACE="${2:-}"; shift 2 ;;
    --surface=*) SURFACE="${1#*=}"; shift ;;
    --prefix) PREFIX="${2:-}"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    --bin-dir) BIN_DIR="${2:-}"; shift 2 ;;
    --bin-dir=*) BIN_DIR="${1#*=}"; shift ;;
    --uninstall) ACTION="uninstall"; shift ;;
    --sbom) ACTION="sbom"; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

os_name() {
  if [[ -n "${TOKENZERO_INSTALL_PLATFORM:-}" ]]; then
    case "${TOKENZERO_INSTALL_PLATFORM}" in
      macos|linux|windows) echo "${TOKENZERO_INSTALL_PLATFORM}" ;;
      *) echo "invalid TOKENZERO_INSTALL_PLATFORM=${TOKENZERO_INSTALL_PLATFORM}" >&2; exit 2 ;;
    esac
    return
  fi
  case "$(uname -s)" in
    Darwin) echo macos ;;
    Linux) echo linux ;;
    *) echo other ;;
  esac
}

json_field() {
  local file="$1" field="$2"
  if [[ ! -f "$file" ]] || ! command -v python3 >/dev/null 2>&1; then
    echo ""
    return
  fi
  python3 - "$file" "$field" <<'PY' 2>/dev/null || true
import json, sys
path, field = sys.argv[1], sys.argv[2]
try:
    with open(path) as handle:
        value = json.load(handle).get(field, "")
    print(value if isinstance(value, str) else "")
except Exception:
    pass
PY
}

if [[ "$ACTION" == "uninstall" ]]; then
  state="$PREFIX/install-state.json"
  if [[ ! -f "$state" ]]; then
    echo "uninstall: ok uninstalled=false reason=no_install_state prefix=$PREFIX platform=$(os_name)"
    exit 0
  fi

  prev_surface="$(json_field "$state" surface)"
  prev_artifact="$(json_field "$state" artifact)"
  prev_binary="$(json_field "$state" binary_path)"
  prev_digest="$(json_field "$state" semantic_contract_digest)"
  case "$prev_artifact" in
    tokenzero-mcp|tokenzero-codemode) ;;
    *)
      echo "uninstall: refused unrecognized install-state artifact; state preserved" >&2
      exit 2
      ;;
  esac
  expected_binary="$BIN_DIR/$prev_artifact"
  if [[ "$prev_binary" != "$expected_binary" ]]; then
    echo "uninstall: refused install-state binary_path mismatch; state preserved" >&2
    exit 2
  fi

  rm -f "$expected_binary"
  shim="$BIN_DIR/tokenzero"
  if [[ -L "$shim" && "$(readlink "$shim")" == "$expected_binary" ]]; then
    rm -f "$shim"
  fi
  rm -f "$state" "$PREFIX/client-config.json" "$PREFIX/shim-target"
  echo "uninstall: ok uninstalled=true artifact=$prev_artifact surface=${prev_surface:-?} semantic_contract_digest=${prev_digest:-?} prefix=$PREFIX platform=$(os_name)"
  exit 0
fi

if [[ -z "$SURFACE" ]]; then
  echo "require --surface raw-worker" >&2
  usage >&2
  exit 2
fi
case "$SURFACE" in
  raw-worker|raw_worker|worker|codemode) SURFACE="raw-worker" ;;
  mcp)
    echo "tokenzero: this script selects the raw worker only; classic MCP compatibility is built separately with surface-mcp" >&2
    exit 2
    ;;
  both|all|mcp+codemode|codemode+mcp)
    echo "tokenzero: dual package surface rejected (fail closed)" >&2
    exit 2
    ;;
  *) echo "surface must be raw-worker" >&2; exit 2 ;;
esac
if [[ -n "${TOKENZERO_ENABLE_MCP:-}" && -n "${TOKENZERO_ENABLE_CODEMODE:-}" ]]; then
  echo "tokenzero: dual package surface rejected (fail closed): both surface envs are set" >&2
  exit 2
fi

ARTIFACT="tokenzero-codemode"
PACKAGE="tokenzero-worker"
if [[ "$DRY_RUN" -eq 1 ]]; then
  if [[ "$ACTION" != "install" ]]; then
    echo "--dry-run cannot be combined with --sbom or --uninstall" >&2
    exit 2
  fi
  echo "cargo build --release -p $PACKAGE --bin $ARTIFACT --no-default-features"
  exit 0
fi

if [[ "$ACTION" == "sbom" ]]; then
  for candidate in "$BIN_DIR/$ARTIFACT" "$TARGET_DIR/release/$ARTIFACT" "$TARGET_DIR/debug/$ARTIFACT"; do
    if [[ -x "$candidate" ]]; then
      "$candidate" sbom
      exit 0
    fi
  done
  echo "sbom: canonical worker $ARTIFACT not found; run --dry-run for its build selector" >&2
  exit 1
fi

echo "tokenzero: direct worker installation is blocked until ZeroStack central discovery adoption (zerostack-uf1u)" >&2
echo "tokenzero: use --dry-run to print the canonical build selector" >&2
exit 2
