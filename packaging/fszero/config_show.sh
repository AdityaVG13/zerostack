#!/usr/bin/env bash
# R-IDEA-009 / fszero-l0hb.3 -- aggregate install-state + store config for agents.
# Preferred long-term surface: `fszero config show --json` (shim). This script is
# the hang-safe installer-native path that never starts a stdio server.
set -euo pipefail

JSON=1
PREFIX="${FSZERO_INSTALL_PREFIX:-${HOME}/.fszero}"
ROOT="${FSZERO_ROOT:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --json) JSON=1; shift ;;
    --human) JSON=0; shift ;;
    --prefix) PREFIX="${2:-}"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    --root) ROOT="${2:-}"; shift 2 ;;
    --root=*) ROOT="${1#*=}"; shift ;;
    -h|--help)
      cat <<H
usage: $0 [--json|--human] [--prefix DIR] [--root WORKSPACE]
Aggregate install-state.json + client-config.json + active store config.json.
Never launches fszero server binaries.
H
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$ROOT" ]]; then
  # Prefer cwd when it looks like a workspace; else leave empty.
  if [[ -d .git || -f Cargo.toml || -d .zerostack || -d .fszero ]]; then
    ROOT="$(pwd)"
  fi
fi

python3 - "$PREFIX" "$ROOT" "$JSON" <<'PY'
import json, os, sys
from pathlib import Path

prefix = Path(sys.argv[1]).expanduser()
root = Path(sys.argv[2]).expanduser() if sys.argv[2] else None
want_json = sys.argv[3] == "1"

def read_json(path: Path):
    if not path.is_file():
        return None, f"missing:{path}"
    try:
        return json.loads(path.read_text()), None
    except Exception as e:
        return None, f"invalid_json:{path}:{e}"

install_state, install_err = read_json(prefix / "install-state.json")
client_config, client_err = read_json(prefix / "client-config.json")

store_root = None
store_config = None
store_err = None
candidates = []
if root is not None:
    candidates.extend([
        root / ".zerostack" / "config.json",
        root / ".zerostack" / "fszero" / "config.json",
        root / ".fszero" / "config.json",
    ])
# env pins
for k in ("ZEROSTACK_STORE_ROOT", "ZERO_STACK_STORE_ROOT", "FSZERO_SHARED_STORE"):
    v = os.environ.get(k)
    if v:
        candidates.insert(0, Path(v).expanduser() / "config.json")
        candidates.insert(1, Path(v).expanduser() / "fszero" / "config.json")

for c in candidates:
    if c.is_file():
        store_root = str(c.parent)
        store_config, store_err = read_json(c)
        break
if store_config is None and store_err is None:
    store_err = "missing:store_config.json"

doc = {
    "schema": "fszero.config_show/v1",
    "ok": install_state is not None or client_config is not None or store_config is not None,
    "prefix": str(prefix),
    "workspace_root": str(root) if root else None,
    "install_state": install_state,
    "client_config": client_config,
    "store": {
        "root": store_root,
        "config": store_config,
    },
    "errors": [e for e in (install_err, client_err, store_err) if e],
    "note": "installer-native aggregate; does not start surface servers. Shim verb: fszero config show --json (when wired).",
}

if want_json:
    print(json.dumps(doc, indent=2, sort_keys=True))
else:
    print(f"config_show: ok={doc['ok']} prefix={prefix}")
    print(f"  install_state: {'present' if install_state else install_err}")
    print(f"  client_config: {'present' if client_config else client_err}")
    print(f"  store_config:  {'present' if store_config else store_err} root={store_root}")
sys.exit(0 if doc["ok"] or True else 1)  # always 0 when script runs; empty is valid discovery
PY
