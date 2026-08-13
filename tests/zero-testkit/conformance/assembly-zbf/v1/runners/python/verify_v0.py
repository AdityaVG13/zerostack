#!/usr/bin/env python3
"""Archived verifier N-1: immutable digest-only replay from before structural checks."""
import hashlib
import json
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(64)
    root = Path(sys.argv[1])
    index = json.loads((root / "index.json").read_bytes())
    for name in ("assembly_manifest", "zbf_leaf", "zbf_container"):
        vector = index[name]
        data = (root / vector["file"]).read_bytes()
        if len(data) != vector["byte_len"] or hashlib.sha256(data).hexdigest() != vector["sha256"]:
            print("fixture_digest_mismatch", file=sys.stderr)
            raise SystemExit(2)
    print("assembly_zbf_kat:python:v0:passed")


if __name__ == "__main__":
    main()
