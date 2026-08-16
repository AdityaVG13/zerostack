#!/usr/bin/env python3
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path("/Users/aditya/AI/ZeroStack/tests/unit")
HEADER = re.compile(r"^#\[cfg\(test\)\]\s*\nmod [A-Za-z0-9_]+ \{\n", re.M)


def unwrap(text: str) -> str:
    match = HEADER.match(text)
    if not match:
        return text
    body = text[match.end() :]
    if body.rstrip().endswith("}"):
        body = body.rstrip()[:-1] + "\n"
    return body.lstrip("\n")


def main() -> None:
    for path in sorted(ROOT.rglob("*_*.rs")):
        if path.name.startswith("."):
            continue
        original = path.read_text()
        updated = unwrap(original)
        if updated != original:
            path.write_text(updated)
            print(f"unwrapped {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
