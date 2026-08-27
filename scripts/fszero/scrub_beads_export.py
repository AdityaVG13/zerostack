#!/usr/bin/env python3
"""Remove absolute home-directory prefixes from exported Beads JSONL."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
HOME_PATH = re.compile(r"(?:/Users/[^/\s]+|/home/[^/\s]+)")
WIN_HOME_PATH = re.compile(r"[A-Za-z]:[\\/]Users[\\/][^\\/\s]+")


def scrub_value(value: Any) -> Any:
    if isinstance(value, str):
        return HOME_PATH.sub("~", WIN_HOME_PATH.sub("~", value))
    if isinstance(value, list):
        return [scrub_value(item) for item in value]
    if isinstance(value, dict):
        return {key: scrub_value(item) for key, item in value.items()}
    return value


def scrub_file(path: Path, *, check_only: bool) -> int:
    if not path.exists():
        print(f"skip missing Beads export: {path}")
        return 0
    lines = path.read_text(encoding="utf-8").splitlines()
    output: list[str] = []
    changed = False
    for number, line in enumerate(lines, 1):
        try:
            record = json.loads(line)  # ubs:ignore — JSONDecodeError is converted below.
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}:{number}: invalid JSON: {error}") from error
        rendered = json.dumps(scrub_value(record), separators=(",", ":"), ensure_ascii=False)
        output.append(rendered)
        changed |= rendered != line
    if check_only:
        if changed:
            print(f"Beads export contains private host paths: {path}")
            return 1
        print(f"Beads export is portable: {path}")
        return 0
    path.write_text("\n".join(output) + ("\n" if output else ""), encoding="utf-8")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.paths and args.check:
        tracked = subprocess.run(  # ubs:ignore — return code answers whether the default export is tracked.
            ["git", "ls-files", "--error-unmatch", ".beads/issues.jsonl"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if tracked.returncode != 0:
            print("skip untracked local Beads export")
            return 0
    paths = args.paths or [ROOT / ".beads" / "issues.jsonl"]
    return max(scrub_file(path, check_only=args.check) for path in paths)


if __name__ == "__main__":
    raise SystemExit(main())
