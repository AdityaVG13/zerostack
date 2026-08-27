#!/usr/bin/env bash
# GraphZero package installer (graphzero-o2uq.3) — macOS / Linux.
#
# Installs exactly one surface artifact: graphzero-mcp OR graphzero-codemode.
# Replaces any prior registration. Never registers both catalogs.
#
# CRITICAL: install state + client-config are written by THIS SCRIPT only
# (installer-native atomic path). Never invoke the surface binary for
# `install` — surface bins may start a stdio server and hang on EOF.
#
# Usage:
#   ./packaging/install.sh --surface mcp|codemode [--prefix DIR] [--bin-dir DIR]
#   ./packaging/install.sh --uninstall [--prefix DIR]
#   ./packaging/install.sh --sbom --surface mcp|codemode
#   ./packaging/install.sh --surface mcp --skip-build
#
# Selection matrix:
#   fresh/default           -> install graphzero-codemode
#   explicit MCP compat     -> install graphzero-mcp
#
# Platform simulation for e2e: GRAPHZERO_INSTALL_PLATFORM=macos|linux

set -euo pipefail

SURFACE=""
PREFIX="${GRAPHZERO_INSTALL_PREFIX:-${HOME}/.graphzero-install}"
BIN_DIR="${GRAPHZERO_BIN_DIR:-${HOME}/.local/bin}"
ACTION="install"
SKIP_BUILD=0
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="0.1.0"

usage() {
  cat <<EOF
usage: $0 [--surface mcp|codemode]  # default: codemode [--prefix DIR] [--bin-dir DIR] [--skip-build]
       $0 --uninstall [--prefix DIR]
       $0 --sbom --surface mcp|codemode

Artifacts: graphzero-mcp | graphzero-codemode | graphzero (compat shim symlink)
Never install both surfaces. Dual client registration is unsupported.
Installer writes state/client-config itself (never hangs on server stdio).
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
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

os_name() {
  if [[ -n "${GRAPHZERO_INSTALL_PLATFORM:-}" ]]; then
    case "${GRAPHZERO_INSTALL_PLATFORM}" in
      macos|linux|windows|other) echo "${GRAPHZERO_INSTALL_PLATFORM}" ;;
      *) echo "invalid GRAPHZERO_INSTALL_PLATFORM=${GRAPHZERO_INSTALL_PLATFORM}" >&2; exit 2 ;;
    esac
    return
  fi
  case "$(uname -s)" in
    Darwin) echo macos ;;
    Linux) echo linux ;;
    *) echo other ;;
  esac
}

artifact_for_surface() {
  case "$1" in
    mcp) echo graphzero-mcp ;;
    codemode) echo graphzero-codemode ;;
    *) echo "bad surface: $1" >&2; exit 2 ;;
  esac
}

package_for_surface() {
  case "$1" in
    mcp) echo graphzero-cli ;;
    codemode) echo graphzero-worker ;;
    *) echo "bad surface: $1" >&2; exit 2 ;;
  esac
}

atomic_write() {
  local path="$1"
  local dir tmp
  dir="$(dirname "$path")"
  mkdir -p "$dir"
  tmp="${path}.tmp.$$"
  cat >"$tmp"
  mv -f "$tmp" "$path"
}

write_install_state() {
  local surface="$1" artifact="$2" binary="$3" digest="$4" platform="$5"
  local now
  now="$(date +%s)"
  atomic_write "$PREFIX/client-config.json" <<EOF
{
  "name": "GraphZero (${surface})",
  "surface": "${surface}",
  "command": "${binary}",
  "args": [],
  "semantic_contract_digest": "${digest}",
  "package_version": "${VERSION}"
}
EOF
  atomic_write "$PREFIX/install-state.json" <<EOF
{
  "surface": "${surface}",
  "artifact": "${artifact}",
  "binary_path": "${binary}",
  "prefix": "${PREFIX}",
  "semantic_contract_digest": "${digest}",
  "package_version": "${VERSION}",
  "installed_at_unix": ${now},
  "platform": "${platform}",
  "client_config": "${PREFIX}/client-config.json"
}
EOF
  atomic_write "$PREFIX/shim-target" <<EOF
${surface}
EOF
}

read_digest_from_sbom() {
  local bin="$1"
  if [[ ! -x "$bin" ]]; then
    echo "unknown"
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$bin" <<'PY' 2>/dev/null || echo "unknown"
import json, subprocess, sys
bin_path = sys.argv[1]
try:
    p = subprocess.run([bin_path, "sbom"], capture_output=True, text=True, timeout=15, check=False)
except Exception:
    print("unknown")
    raise SystemExit(0)
text = (p.stdout or "") + "\n" + (p.stderr or "")
for line in text.splitlines():
    line = line.strip()
    if line.startswith("{"):
        try:
            doc = json.loads(line)
            print(doc.get("semantic_contract_digest") or "unknown")
            raise SystemExit(0)
        except json.JSONDecodeError:
            pass
try:
    doc = json.loads(p.stdout or "")
    print(doc.get("semantic_contract_digest") or "unknown")
except Exception:
    print("unknown")
PY
  else
    # Fallback without sed: python is preferred; otherwise mark unknown.
    echo "unknown"
  fi
}

json_field() {
  # Read a top-level string field from a JSON file via python (no sed).
  local file="$1" field="$2"
  if [[ ! -f "$file" ]]; then
    echo ""
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$file" "$field" <<'PY' 2>/dev/null || true
import json, sys
path, field = sys.argv[1], sys.argv[2]
try:
    with open(path) as f:
        doc = json.load(f)
    v = doc.get(field, "")
    if isinstance(v, str):
        print(v)
    else:
        print(v if v is not None else "")
except Exception:
    pass
PY
  fi
}

if [[ "$ACTION" == "uninstall" ]]; then
  # Installer-native uninstall — never invoke surface server binaries.
  prev_surface="$(json_field "$PREFIX/install-state.json" surface)"
  prev_artifact="$(json_field "$PREFIX/install-state.json" artifact)"
  prev_digest="$(json_field "$PREFIX/install-state.json" semantic_contract_digest)"
  rm -f "$PREFIX/install-state.json" "$PREFIX/client-config.json" "$PREFIX/shim-target"
  rm -f "$BIN_DIR/graphzero-mcp" "$BIN_DIR/graphzero-codemode" "$BIN_DIR/graphzero"
  if [[ -n "${prev_artifact:-}" ]]; then
    echo "uninstall: ok uninstalled=true artifact=${prev_artifact} surface=${prev_surface:-?} semantic_contract_digest=${prev_digest:-?} prefix=$PREFIX platform=$(os_name)"
  else
    echo "uninstall: ok uninstalled=false reason=no_install_state prefix=$PREFIX platform=$(os_name)"
  fi
  exit 0
fi

if [[ -z "$SURFACE" ]]; then
  # Fresh / unattended default: CodeMode (graphzero-zerostack-parity-b5ci.4.3).
  SURFACE=codemode
fi

case "$SURFACE" in
  mcp|codemode) ;;
  both|all|mcp+codemode|codemode+mcp)
    echo "graphzero: dual package surface rejected (fail closed): install requests both surfaces" >&2
    exit 2
    ;;
  *) echo "surface must be mcp or codemode (not both)" >&2; exit 2 ;;
esac

if [[ -n "${GRAPHZERO_ENABLE_MCP:-}" && -n "${GRAPHZERO_ENABLE_CODEMODE:-}" ]]; then
  echo "graphzero: dual package surface rejected (fail closed): both GRAPHZERO_ENABLE_MCP and GRAPHZERO_ENABLE_CODEMODE are set" >&2
  exit 2
fi

ARTIFACT="$(artifact_for_surface "$SURFACE")"
PACKAGE="$(package_for_surface "$SURFACE")"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ "$TARGET_DIR" != /* ]]; then
  TARGET_DIR="$ROOT/$TARGET_DIR"
fi
PLATFORM="$(os_name)"

if [[ "$ACTION" == "sbom" ]]; then
  CANDIDATES=("$BIN_DIR/$ARTIFACT" "$TARGET_DIR/release/$ARTIFACT" "$TARGET_DIR/debug/$ARTIFACT")
  for c in "${CANDIDATES[@]}"; do
    if [[ -x "$c" ]]; then
      "$c" sbom
      exit 0
    fi
  done
  echo "sbom: binary $ARTIFACT not found; build first" >&2
  exit 1
fi

echo "install: surface=$SURFACE artifact=$ARTIFACT platform=$PLATFORM prefix=$PREFIX"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "install: building $ARTIFACT (package $PACKAGE)"
  (
    cd "$ROOT"
    if [[ "$SURFACE" == "mcp" ]]; then
      cargo build --release \
        --package "$PACKAGE" \
        --bin "$ARTIFACT" \
        --no-default-features \
        --features "tokenzero,surface-mcp"
    else
      cargo build --release \
        --package "$PACKAGE" \
        --bin "$ARTIFACT" \
        --no-default-features
    fi
  )
  SRC="$TARGET_DIR/release/$ARTIFACT"
else
  if [[ -x "$TARGET_DIR/release/$ARTIFACT" ]]; then
    SRC="$TARGET_DIR/release/$ARTIFACT"
  elif [[ -x "$TARGET_DIR/debug/$ARTIFACT" ]]; then
    SRC="$TARGET_DIR/debug/$ARTIFACT"
  else
    echo "install: --skip-build but no prebuilt $ARTIFACT under $TARGET_DIR" >&2
    exit 1
  fi
  echo "install: using prebuilt $SRC"
fi

if [[ ! -x "$SRC" ]]; then
  echo "install: FAIL source binary not executable: $SRC" >&2
  exit 1
fi

mkdir -p "$PREFIX" "$BIN_DIR"
# Snapshot prior state for rollback if post-copy verification fails.
ROLLBACK_STATE=""
ROLLBACK_CFG=""
ROLLBACK_SHIM=""
if [[ -f "$PREFIX/install-state.json" ]]; then
  ROLLBACK_STATE="$(cat "$PREFIX/install-state.json")"
fi
if [[ -f "$PREFIX/client-config.json" ]]; then
  ROLLBACK_CFG="$(cat "$PREFIX/client-config.json")"
fi
if [[ -f "$PREFIX/shim-target" ]]; then
  ROLLBACK_SHIM="$(cat "$PREFIX/shim-target")"
fi

install -m 755 "$SRC" "$BIN_DIR/$ARTIFACT"

# Peer removal before shim so only one surface remains.
PEER="$([[ "$SURFACE" == mcp ]] && echo graphzero-codemode || echo graphzero-mcp)"
if [[ -e "$BIN_DIR/$PEER" ]]; then
  echo "install: replacing peer artifact $PEER (mutual exclusion)"
  rm -f "$BIN_DIR/$PEER"
fi
ln -sfn "$BIN_DIR/$ARTIFACT" "$BIN_DIR/graphzero"

DIGEST="$(read_digest_from_sbom "$BIN_DIR/$ARTIFACT")"
write_install_state "$SURFACE" "$ARTIFACT" "$BIN_DIR/$ARTIFACT" "$DIGEST" "$PLATFORM"

if [[ ! -f "$PREFIX/install-state.json" || ! -f "$PREFIX/client-config.json" ]]; then
  echo "install: FAIL state/config not written; restoring prior if any" >&2
  if [[ -n "$ROLLBACK_STATE" ]]; then
    atomic_write "$PREFIX/install-state.json" <<<"$ROLLBACK_STATE"
  fi
  if [[ -n "$ROLLBACK_CFG" ]]; then
    atomic_write "$PREFIX/client-config.json" <<<"$ROLLBACK_CFG"
  fi
  if [[ -n "$ROLLBACK_SHIM" ]]; then
    atomic_write "$PREFIX/shim-target" <<<"$ROLLBACK_SHIM"
  fi
  exit 1
fi

# Single-surface client config check (structured via python when available).
if command -v python3 >/dev/null 2>&1; then
  if ! python3 - "$PREFIX/client-config.json" "$SURFACE" <<'PY'
import json, sys
path, want = sys.argv[1], sys.argv[2]
with open(path) as f:
    doc = json.load(f)
assert doc.get("surface") == want, doc
args = doc.get("args") or []
assert args == [], args
print("ok")
PY
  then
    echo "install: FAIL client-config dual/malformed surface" >&2
    exit 1
  fi
fi

echo "install: ok surface=$SURFACE artifact=$ARTIFACT prefix=$PREFIX bin=$BIN_DIR/$ARTIFACT shim=$BIN_DIR/graphzero platform=$PLATFORM semantic_contract_digest=$DIGEST"
echo "client_config: $PREFIX/client-config.json"
echo "selection: canonical raw worker -> graphzero-codemode; legacy MCP -> graphzero-mcp"
