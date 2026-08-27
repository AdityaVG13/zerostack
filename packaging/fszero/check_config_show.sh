#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$ROOT/packaging/fszero/config_show.sh" --help | grep -q -- '--json'
TMP=$(mktemp -d)
mkdir -p "$TMP/p" "$TMP/w/.zerostack"
printf '%s\n' '{"surface":"codemode"}' >"$TMP/p/install-state.json"
printf '%s\n' '{"surface":"codemode"}' >"$TMP/p/client-config.json"
printf '%s\n' '{"x":1}' >"$TMP/w/.zerostack/config.json"
python3 - <<PY
import json, subprocess, os
out = subprocess.check_output(
    ["bash", "$ROOT/packaging/fszero/config_show.sh", "--json", "--prefix", "$TMP/p", "--root", "$TMP/w"],
    text=True,
)
d = json.loads(out)
assert d["schema"] == "fszero.config_show/v1"
assert d["install_state"]["surface"] == "codemode"
assert d["store"]["config"]["x"] == 1
print("check_config_show: ok")
PY
