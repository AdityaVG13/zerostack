#!/usr/bin/env python3
"""Move #[cfg(test)] mod blocks from crates/src into <repo>/tests/unit/."""

from __future__ import annotations

import re
import sys
from pathlib import Path

MOD_RE = re.compile(r"\n#\[cfg\(test\)\]\nmod ([A-Za-z0-9_]+) \{")
HEADER = re.compile(r"^#\[cfg\(test\)\]\s*\nmod [A-Za-z0-9_]+ \{\n")


def extract_modules(text: str) -> tuple[str, list[tuple[str, str]]]:
    found: list[tuple[str, str]] = []
    pieces: list[str] = []
    pos = 0
    while True:
        match = MOD_RE.search(text, pos)
        if not match:
            pieces.append(text[pos:])
            break
        pieces.append(text[pos : match.start()])
        name = match.group(1)
        brace = match.end() - 1
        depth = 0
        i = brace
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    i += 1
                    break
            i += 1
        body = text[match.start() + 1 : i]
        found.append((name, body.rstrip() + "\n"))
        pos = i
        if pos < len(text) and text[pos] == "\n":
            pos += 1
    return "".join(pieces).rstrip() + "\n", found


def unwrap(text: str) -> str:
    match = HEADER.match(text)
    if not match:
        return text
    body = text[match.end() :]
    if body.rstrip().endswith("}"):
        body = body.rstrip()[:-1] + "\n"
    return body.lstrip("\n")


def os_rel(start: Path, dest: Path) -> str:
    start_parts = start.resolve().parts
    dest_parts = dest.resolve().parts
    i = 0
    while i < min(len(start_parts), len(dest_parts)) and start_parts[i] == dest_parts[i]:
        i += 1
    up = [".."] * (len(start_parts) - i)
    return str(Path(*up, *dest_parts[i:])) if up or dest_parts[i:] else "."


def crate_of(crates: Path, src: Path) -> str:
    return src.resolve().relative_to(crates).parts[0]


def process_repo(repo: Path) -> int:
    crates = repo / "crates"
    out = repo / "tests" / "unit"
    if not crates.is_dir():
        print(f"skip {repo}: no crates/")
        return 0
    moved = 0
    for src in sorted(crates.rglob("*.rs")):
        if "target" in src.parts:
            continue
        text = src.read_text()
        if "#[cfg(test)]\nmod " not in text:
            continue
        stripped, mods = extract_modules(text)
        if not mods:
            continue
        dest_dir = out / crate_of(crates, src)
        dest_dir.mkdir(parents=True, exist_ok=True)
        hooks: list[str] = []
        for name, body in mods:
            dest = dest_dir / f"{src.stem}_{name}.rs"
            dest.write_text(unwrap(body))
            rel = os_rel(src.parent, dest)
            hooks.append(f'#[cfg(test)]\n#[path = "{rel}"]\nmod {name};\n')
            print(f"{repo.name}: {src.relative_to(repo)} -> {dest.relative_to(repo)}")
            moved += 1
        src.write_text(stripped.rstrip() + "\n\n" + "".join(hooks))
    return moved


def main() -> None:
    repos = [Path(p) for p in sys.argv[1:]]
    total = 0
    for repo in repos:
        total += process_repo(repo)
    print(f"moved {total} modules")


if __name__ == "__main__":
    main()
