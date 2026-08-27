#!/usr/bin/env bash
# FSZero package installer (fszero-ncib.3) — macOS / Linux.
#
# Installs exactly one surface artifact: fszero-mcp OR fszero-codemode.
# Replaces any prior registration. Never registers both catalogs.
#
# CRITICAL: install state + client-config are written by THIS SCRIPT only
# (installer-native atomic path). Never invoke the surface binary for
# `install` — surface bins may start a stdio server and hang on EOF.
#
# Usage:
#   ./packaging/install.sh [--surface codemode|mcp] [--prefix DIR] [--bin-dir DIR] [--json]
#   ./packaging/install.sh --uninstall [--prefix DIR] [--json]
#   ./packaging/install.sh --sbom --surface codemode|mcp
#   ./packaging/install.sh --surface codemode --skip-build   # use prebuilt target/debug|release
#   ./packaging/install.sh --surface mcp --skip-build        # legacy MCP-only prebuilt
#
# Agent contract (R-IDEA-003):
#   --json  one JSON object on stdout (human chatter on stderr only)
#   always reports PATH next-steps + Cargo.toml package_version sync
#
# Selection matrix (CodeMode-first; single install truth):
#   fresh installs / default -> fszero-codemode (canonical)
#   legacy MCP-only client   -> fszero-mcp (explicit --surface mcp only)
#   never both catalogs on one primary fszero symlink
#
# Platform simulation for e2e: FSZERO_INSTALL_PLATFORM=macos|linux

set -euo pipefail

SURFACE=""
PREFIX="${FSZERO_INSTALL_PREFIX:-${HOME}/.fszero}"
BIN_DIR="${FSZERO_BIN_DIR:-${HOME}/.local/bin}"
ACTION="install"
SKIP_BUILD=0
JSON=0
# auto: verified system SQLite on macOS/pkg-config hosts, otherwise bundled.
# Override for reproducible cross builds: FSZERO_SQLITE_LINK=system|bundled.
SQLITE_LINK="${FSZERO_SQLITE_LINK:-auto}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ "$TARGET_DIR" != /* ]]; then
  TARGET_DIR="$ROOT/$TARGET_DIR"
fi

# Prefer live Cargo.toml package version; fall back only if unreadable.
read_cargo_package_version() {
  local v=""
  if [[ -f "$ROOT/Cargo.toml" ]]; then
    v="$(awk '
      /^\[package\]/ { in_pkg=1; next }
      in_pkg && /^\[/ { exit }
      in_pkg && /^version[[:space:]]*=/ {
        if (match($0, /"[^"]+"/)) {
          print substr($0, RSTART+1, RLENGTH-2)
          exit
        }
      }
    ' "$ROOT/Cargo.toml")"
  fi
  if [[ -n "$v" ]]; then
    printf '%s' "$v"
  else
    printf '%s' "0.1.0"
  fi
}
VERSION="$(read_cargo_package_version)"
CARGO_TOML_VERSION="$VERSION"

usage() {
  cat <<EOF
usage: $0 [--surface codemode|mcp] [--prefix DIR] [--bin-dir DIR] [--skip-build] [--json]
       $0 --uninstall [--prefix DIR] [--json]
       $0 --sbom --surface codemode|mcp

Default surface: codemode (canonical). Use --surface mcp for legacy MCP-only.
Artifacts: fszero-codemode (default) | fszero-mcp (compat) | fszero (shim symlink)
Never install both surfaces. Dual client registration is unsupported.
There is no 'fszero mcp-server' verb -- launch via serve/--mode=mcp or fszero-mcp.
Installer writes state/client-config itself (never hangs on server stdio).

Agent flags:
  --json   emit one machine-readable result object on stdout (R-IDEA-003)
           includes path.next_steps and version_sync (Cargo.toml package version)
EOF
}

# True if BIN_DIR already appears as a PATH entry.
bin_dir_on_path() {
  case ":${PATH}:" in
    *":${BIN_DIR}:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# Human PATH guidance (stdout in human mode; folded into JSON for agents).
print_path_next_steps() {
  if bin_dir_on_path; then
    echo "path: ok bin_dir_on_path=true bin_dir=$BIN_DIR"
    echo "next: fszero is ready on PATH (bin_dir=$BIN_DIR)"
  else
    echo "path: warn bin_dir_on_path=false bin_dir=$BIN_DIR"
    echo "next: export PATH=\"$BIN_DIR:\$PATH\""
    echo "next: hash -r  # refresh shell command cache if needed"
    echo "next: verify with: command -v fszero && fszero --version"
  fi
}

# Compare installer VERSION (from Cargo.toml) to installed state / binary --version.
# Prints human lines; sets globals for JSON emission.
VERSION_SYNC_STATUS="unknown"
BINARY_VERSION=""
check_version_sync() {
  local binary="${1:-}"
  local state_version=""
  BINARY_VERSION=""
  VERSION_SYNC_STATUS="unknown"
  if [[ -f "$PREFIX/install-state.json" ]]; then
    state_version="$(sed -n 's/.*"package_version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PREFIX/install-state.json" | head -1)"
  fi
  if [[ -n "$binary" && -x "$binary" ]] && command -v python3 >/dev/null 2>&1; then
    BINARY_VERSION="$(python3 - "$binary" <<'PY' 2>/dev/null || true
import subprocess, sys, re
bin_path = sys.argv[1]
try:
    p = subprocess.run([bin_path, "--version"], capture_output=True, text=True, timeout=10, check=False)
except Exception:
    raise SystemExit(0)
text = ((p.stdout or "") + " " + (p.stderr or "")).strip()
m = re.findall(r"\d+\.\d+(?:\.\d+)?(?:[-+][A-Za-z0-9.]+)?", text)
print(m[-1] if m else "")
PY
)"
  fi
  if [[ -z "$CARGO_TOML_VERSION" ]]; then
    VERSION_SYNC_STATUS="unknown"
  elif [[ -n "$state_version" && "$state_version" != "$CARGO_TOML_VERSION" ]]; then
    VERSION_SYNC_STATUS="mismatch"
  elif [[ -n "$BINARY_VERSION" && "$BINARY_VERSION" != "$CARGO_TOML_VERSION" ]]; then
    VERSION_SYNC_STATUS="mismatch"
  else
    VERSION_SYNC_STATUS="ok"
  fi
  echo "version_sync: status=$VERSION_SYNC_STATUS package_version=$VERSION cargo_toml=$CARGO_TOML_VERSION binary_version=${BINARY_VERSION:-unknown} state_version=${state_version:-unknown}"
  if [[ "$VERSION_SYNC_STATUS" == "mismatch" ]]; then
    echo "version_sync: warn installer/state/binary package_version drift -- rebuild from this tree or re-run without --skip-build" >&2
  fi
}

# Emit a single JSON object on stdout via python3 (no jq required).
emit_json_result() {
  if ! command -v python3 >/dev/null 2>&1; then
    echo "install: --json requires python3" >&2
    exit 2
  fi
  PATH_ON=0
  bin_dir_on_path && PATH_ON=1
  FSZERO_JSON_PATH_ON="$PATH_ON" \
  FSZERO_JSON_BIN_DIR="$BIN_DIR" \
  FSZERO_JSON_PREFIX="$PREFIX" \
  FSZERO_JSON_VERSION="$VERSION" \
  FSZERO_JSON_CARGO_VER="$CARGO_TOML_VERSION" \
  FSZERO_JSON_BIN_VER="${BINARY_VERSION:-}" \
  FSZERO_JSON_SYNC="$VERSION_SYNC_STATUS" \
  python3 - "$@" <<'PY'
import json, os, sys

def split_kv(s):
    k, _, v = s.partition("=")
    return k, v

fields = dict(split_kv(a) for a in sys.argv[1:])
for k, v in list(fields.items()):
    if v == "true":
        fields[k] = True
    elif v == "false":
        fields[k] = False
    elif v.isdigit():
        fields[k] = int(v)

bin_dir = os.environ.get("FSZERO_JSON_BIN_DIR", "")
path_on = os.environ.get("FSZERO_JSON_PATH_ON", "0") == "1"
next_steps = []
if path_on:
    next_steps.append(f"fszero ready on PATH (bin_dir={bin_dir})")
else:
    next_steps.append(f'export PATH="{bin_dir}:$PATH"')
    next_steps.append("hash -r")
    next_steps.append("command -v fszero && fszero --version")

doc = {
    **fields,
    "package_version": os.environ.get("FSZERO_JSON_VERSION", ""),
    "path": {
        "bin_dir": bin_dir,
        "bin_dir_on_path": path_on,
        "next_steps": next_steps,
    },
    "version_sync": {
        "status": os.environ.get("FSZERO_JSON_SYNC", "unknown"),
        "installer_version": os.environ.get("FSZERO_JSON_VERSION", ""),
        "cargo_toml_version": os.environ.get("FSZERO_JSON_CARGO_VER", ""),
        "binary_version": os.environ.get("FSZERO_JSON_BIN_VER") or None,
    },
}
print(json.dumps(doc, separators=(",", ":"), sort_keys=True))
PY
}

human() {
  # Human-facing lines go to stdout unless --json (then stderr so stdout stays pure JSON).
  if [[ "$JSON" -eq 1 ]]; then
    echo "$*" >&2
  else
    echo "$*"
  fi
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
    --json) JSON=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

os_name() {
  if [[ -n "${FSZERO_INSTALL_PLATFORM:-}" ]]; then
    case "${FSZERO_INSTALL_PLATFORM}" in
      macos|linux|windows|other) echo "${FSZERO_INSTALL_PLATFORM}" ;;
      *) echo "invalid FSZERO_INSTALL_PLATFORM=${FSZERO_INSTALL_PLATFORM}" >&2; exit 2 ;;
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
    mcp) echo fszero-mcp ;;
    codemode) echo fszero-codemode ;;
    *) echo "bad surface: $1" >&2; exit 2 ;;
  esac
}

package_for_surface() {
  case "$1" in
    mcp) echo fszero-mcp ;;
    codemode) echo fszero-worker ;;
    *) echo "bad surface: $1" >&2; exit 2 ;;
  esac
}

sqlite_feature() {
  case "$SQLITE_LINK" in
    system) echo sqlite-system ;;
    bundled) echo sqlite-bundled ;;
    auto)
      if [[ "$(uname -s)" == "Darwin" ]]; then
        # SQLite is part of the macOS SDK and libsqlite3 is linkable on every
        # supported deployment target.
        echo sqlite-system
      elif command -v pkg-config >/dev/null 2>&1 && pkg-config --exists sqlite3; then
        echo sqlite-system
      else
        echo sqlite-bundled
      fi
      ;;
    *) echo "FSZERO_SQLITE_LINK must be auto, system, or bundled (got $SQLITE_LINK)" >&2; exit 2 ;;
  esac
}

atomic_write() {
  # $1=path $2=contents via stdin
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
  # Dedicated surface artifacts reject --mode= (forbidden_mode_flag / unsupported_arg).
  # Only the compatibility shim needs --mode; install.sh registers the artifact itself.
  atomic_write "$PREFIX/client-config.json" <<EOF
{
  "name": "FSZero (${surface})",
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
  "abi_digest": "",
  "schemas_digest": "",
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
  # Non-interactive: sbom exits immediately with JSON (must not open stdio server).
  if [[ ! -x "$bin" ]]; then
    echo "unknown"
    return
  fi
  # Timeout guard: never hang if a bad binary waits on stdin.
  local out
  if command -v python3 >/dev/null 2>&1; then
    out="$(python3 - "$bin" <<'PY' 2>/dev/null || true
import json, subprocess, sys
bin_path = sys.argv[1]
try:
    p = subprocess.run([bin_path, "sbom"], capture_output=True, text=True, timeout=15, check=False)
except Exception:
    print("unknown")
    raise SystemExit(0)
text = (p.stdout or "") + "\n" + (p.stderr or "")
for line in text.splitlines():
    line=line.strip()
    if line.startswith("{"):
        try:
            doc=json.loads(line)
            print(doc.get("semantic_contract_digest") or "unknown")
            raise SystemExit(0)
        except json.JSONDecodeError:
            pass
# try whole stdout
try:
    doc=json.loads(p.stdout)
    print(doc.get("semantic_contract_digest") or "unknown")
except Exception:
    print("unknown")
PY
)"
  else
    out="$("$bin" sbom 2>/dev/null | head -c 20000 || true)"
    out="$(printf '%s' "$out" | sed -n 's/.*"semantic_contract_digest"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  fi
  if [[ -z "$out" ]]; then
    echo "unknown"
  else
    echo "$out"
  fi
}

if [[ "$ACTION" == "uninstall" ]]; then
  # Installer-native uninstall — never invoke surface server binaries.
  prev_surface=""
  prev_artifact=""
  prev_digest=""
  if [[ -f "$PREFIX/install-state.json" ]]; then
    prev_surface="$(sed -n 's/.*"surface"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PREFIX/install-state.json" | head -1)"
    prev_artifact="$(sed -n 's/.*"artifact"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PREFIX/install-state.json" | head -1)"
    prev_digest="$(sed -n 's/.*"semantic_contract_digest"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$PREFIX/install-state.json" | head -1)"
  fi
  rm -f "$PREFIX/install-state.json" "$PREFIX/client-config.json" "$PREFIX/shim-target"
  rm -f "$BIN_DIR/fszero-mcp" "$BIN_DIR/fszero-codemode" "$BIN_DIR/fszero"
  PLATFORM="$(os_name)"
  if [[ -n "$prev_artifact" ]]; then
    human "uninstall: ok uninstalled=true artifact=${prev_artifact} surface=${prev_surface:-?} semantic_contract_digest=${prev_digest:-?} prefix=$PREFIX platform=$PLATFORM"
    if [[ "$JSON" -eq 1 ]]; then
      VERSION_SYNC_STATUS="ok"
      emit_json_result \
        "ok=true" "action=uninstall" "uninstalled=true" \
        "artifact=${prev_artifact}" "surface=${prev_surface:-}" \
        "semantic_contract_digest=${prev_digest:-}" \
        "prefix=$PREFIX" "bin_dir=$BIN_DIR" "platform=$PLATFORM"
    fi
  else
    human "uninstall: ok uninstalled=false reason=no_install_state prefix=$PREFIX platform=$PLATFORM"
    if [[ "$JSON" -eq 1 ]]; then
      VERSION_SYNC_STATUS="unknown"
      emit_json_result \
        "ok=true" "action=uninstall" "uninstalled=false" \
        "reason=no_install_state" "prefix=$PREFIX" "bin_dir=$BIN_DIR" "platform=$PLATFORM"
    fi
  fi
  exit 0
fi

if [[ -z "$SURFACE" ]]; then
  SURFACE="codemode"
  echo "install: defaulting to --surface codemode (canonical FSZero surface)" >&2
fi

case "$SURFACE" in
  mcp|codemode) ;;
  both|all|mcp+codemode|codemode+mcp)
    echo "fszero: dual package surface rejected (fail closed): install requests both surfaces" >&2
    exit 2
    ;;
  *) echo "surface must be mcp or codemode (not both)" >&2; exit 2 ;;
esac

# Dual env fail-closed
if [[ -n "${FSZERO_ENABLE_MCP:-}" && -n "${FSZERO_ENABLE_CODEMODE:-}" ]]; then
  echo "fszero: dual package surface rejected (fail closed): both FSZERO_ENABLE_MCP and FSZERO_ENABLE_CODEMODE are set" >&2
  exit 2
fi

if [[ "$SURFACE" == "mcp" ]]; then
  echo "WARNING: fszero-mcp is a compatibility-only package (security/correctness fixes, no new features)." >&2
  echo "         For canonical FSZero, use --surface codemode (default)." >&2
fi

ARTIFACT="$(artifact_for_surface "$SURFACE")"
PACKAGE="$(package_for_surface "$SURFACE")"
SQLITE_FEATURE="$(sqlite_feature)"
PACKAGE_FEATURES="$SQLITE_FEATURE"
if [[ "$SURFACE" == "codemode" ]]; then
  # Compatibility installer preserves the retired standalone CodeMode surface.
  # The canonical raw worker uses the manifest's --no-default-features build.
  PACKAGE_FEATURES="$PACKAGE_FEATURES,surface-codemode"
fi
PLATFORM="$(os_name)"

if [[ "$ACTION" == "sbom" ]]; then
  CANDIDATES=("$BIN_DIR/$ARTIFACT" "$TARGET_DIR/release/$ARTIFACT" "$TARGET_DIR/debug/$ARTIFACT")
  for c in "${CANDIDATES[@]}"; do
    if [[ -x "$c" ]]; then
      # Non-interactive sbom only — never start a server.
      "$c" sbom
      exit 0
    fi
  done
  echo "sbom: binary $ARTIFACT not found; build first" >&2
  exit 1
fi

human "install: surface=$SURFACE artifact=$ARTIFACT platform=$PLATFORM prefix=$PREFIX package_version=$VERSION"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  human "install: building $ARTIFACT (package $PACKAGE, features $PACKAGE_FEATURES)"
  # Each package fixes exactly one surface and excludes dev-harness, mcp-http,
  # and the peer surface dependency tree. SQLite falls back explicitly above.
  (
    cd "$ROOT"
    cargo build --release \
      --package "$PACKAGE" \
      --no-default-features \
      --features "$PACKAGE_FEATURES"
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
  human "install: using prebuilt $SRC"
fi

mkdir -p "$PREFIX" "$BIN_DIR"
install -m 755 "$SRC" "$BIN_DIR/$ARTIFACT"

# Compatibility shim AFTER peer cleanup: fszero -> selected artifact only.
PEER="$([[ "$SURFACE" == mcp ]] && echo fszero-codemode || echo fszero-mcp)"
if [[ -e "$BIN_DIR/$PEER" ]]; then
  human "install: replacing peer artifact $PEER (mutual exclusion)"
  rm -f "$BIN_DIR/$PEER"
fi
ln -sfn "$BIN_DIR/$ARTIFACT" "$BIN_DIR/fszero"

# Installer-native state write (deterministic; never calls surface binary for install).
DIGEST="$(read_digest_from_sbom "$BIN_DIR/$ARTIFACT")"
write_install_state "$SURFACE" "$ARTIFACT" "$BIN_DIR/$ARTIFACT" "$DIGEST" "$PLATFORM"

# Verify state exists and is single-surface.
if [[ ! -f "$PREFIX/install-state.json" || ! -f "$PREFIX/client-config.json" ]]; then
  echo "install: FAIL state/config not written" >&2
  exit 1
fi
if grep -q '"mcp"' "$PREFIX/client-config.json" && grep -q '"codemode"' "$PREFIX/client-config.json"; then
  echo "install: FAIL dual surface in client-config" >&2
  exit 1
fi

human "install: ok surface=$SURFACE artifact=$ARTIFACT prefix=$PREFIX bin=$BIN_DIR/$ARTIFACT shim=$BIN_DIR/fszero platform=$PLATFORM semantic_contract_digest=$DIGEST package_version=$VERSION"
human "client_config: $PREFIX/client-config.json"
human "selection: CodeMode-first -> fszero-codemode (default); legacy MCP -> fszero-mcp (--surface mcp)"
# Agent contract: PATH next-steps + version sync (always; JSON folds them in).
if [[ "$JSON" -eq 1 ]]; then
  check_version_sync "$BIN_DIR/$ARTIFACT" >&2
  print_path_next_steps >&2
  emit_json_result \
    "ok=true" "action=install" "surface=$SURFACE" "artifact=$ARTIFACT" \
    "prefix=$PREFIX" "bin_dir=$BIN_DIR" "binary=$BIN_DIR/$ARTIFACT" \
    "shim=$BIN_DIR/fszero" "platform=$PLATFORM" \
    "semantic_contract_digest=$DIGEST" "client_config=$PREFIX/client-config.json" \
    "skip_build=$SKIP_BUILD"
else
  check_version_sync "$BIN_DIR/$ARTIFACT"
  print_path_next_steps
fi
