#!/usr/bin/env bash
# R-IDEA-003 / fszero-ip47.2 -- static + optional skip-build smoke (never full cargo suite).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
python3 - <<'PY'
from pathlib import Path
import json, os, re, subprocess, tempfile

root = Path(".").resolve()
script = root / "packaging/fszero/install.sh"
text = script.read_text()
assert "--json" in text
assert "version_sync" in text
assert "read_cargo_package_version" in text
assert "emit_json_result" in text
help_out = subprocess.run(["bash", str(script), "--help"], capture_output=True, text=True, check=True)
assert "--json" in help_out.stdout

candidates = [
    root / "target/release/fszero-codemode",
    root / "target/debug/fszero-codemode",
]
src = next((p for p in candidates if p.is_file()), None)
if src is None:
    print("check_install_agent_contract: ok (static only; no prebuilt binary)")
    raise SystemExit(0)

with tempfile.TemporaryDirectory() as td:
    td = Path(td)
    prefix, bin_dir = td / "prefix", td / "bin"
    bin_dir.mkdir()
    env = os.environ.copy()
    env["FSZERO_INSTALL_PLATFORM"] = "macos"
    h = subprocess.run(
        ["bash", str(script), "--surface", "codemode", "--prefix", str(prefix),
         "--bin-dir", str(bin_dir), "--skip-build"],
        cwd=root, env=env, capture_output=True, text=True,
    )
    assert h.returncode == 0, h.stdout + h.stderr
    assert "install: ok" in h.stdout
    assert "version_sync:" in h.stdout
    assert "path:" in h.stdout and "next:" in h.stdout
    j = subprocess.run(
        ["bash", str(script), "--surface", "codemode", "--prefix", str(prefix),
         "--bin-dir", str(bin_dir), "--skip-build", "--json"],
        cwd=root, env=env, capture_output=True, text=True,
    )
    assert j.returncode == 0, j.stdout + j.stderr
    doc = json.loads(j.stdout.strip())
    assert doc.get("ok") is True
    assert doc.get("action") == "install"
    assert isinstance(doc["path"].get("next_steps"), list)
    assert doc["version_sync"].get("cargo_toml_version")
    cargo = (root / "Cargo.toml").read_text()
    m = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M)
    assert m and doc["package_version"] == m.group(1)
print("check_install_agent_contract: ok")
PY
