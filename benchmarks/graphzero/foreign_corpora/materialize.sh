#!/usr/bin/env bash
# Materialize git-pinned foreign corpora from pins.json (shallow clone at frozen rev).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PINS="$ROOT/benchmarks/foreign_corpora/pins.json"
python3 - <<'PY' "$ROOT" "$PINS"
import json, subprocess, sys
from pathlib import Path
root, pins_path = Path(sys.argv[1]), Path(sys.argv[2])
pins = json.loads(pins_path.read_text())
for c in pins["corpora"]:
    if c.get("kind") != "git_pin":
        continue
    dest = root / c["materialize_dir"]
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists():
        head = subprocess.run(
            ["git", "-C", str(dest), "rev-parse", "HEAD"],
            capture_output=True, text=True, check=False,
        )
        if head.returncode == 0 and head.stdout.strip() == c["rev"]:
            print(f"ok pin={c['id']} already at {c['rev'][:12]}")
            continue
        print(f"refresh pin={c['id']} -> {c['rev'][:12]}")
        subprocess.run(["rm", "-rf", str(dest)], check=True)
    print(f"clone pin={c['id']} {c['url']}@{c['rev'][:12]}")
    subprocess.run(
        ["git", "clone", "--filter=blob:none", "--no-checkout", c["url"], str(dest)],
        check=True,
    )
    subprocess.run(["git", "-C", str(dest), "fetch", "--depth", "1", "origin", c["rev"]], check=True)
    subprocess.run(["git", "-C", str(dest), "checkout", "--force", c["rev"]], check=True)
    print(f"ok pin={c['id']} materialized")
PY
