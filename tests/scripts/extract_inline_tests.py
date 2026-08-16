#!/usr/bin/env python3
"""Move #[cfg(test)] mod blocks from crate src/ into repo-root tests/rust/."""

from __future__ import annotations

import re
from pathlib import Path

REPO = Path("/Users/aditya/AI/ZeroStack")
CRATES = REPO / "crates"
OUT = REPO / "tests" / "rust"

MOD_RE = re.compile(
    r"\n#\[cfg\(test\)\]\nmod ([A-Za-z0-9_]+) \{",
)


def extract_modules(text: str) -> tuple[str, list[tuple[str, str]]]:
    """Return (src_without_mods, [(mod_name, body_with_header), ...])."""
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
            ch = text[i]
            if ch == "{":
                depth += 1
            elif ch == "}":
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


def rel_path_include(src_file: Path, dest: Path) -> str:
    return Path(os_rel(src_file.parent, dest)).as_posix()


def os_rel(start: Path, dest: Path) -> str:
    start_parts = start.resolve().parts
    dest_parts = dest.resolve().parts
    i = 0
    while i < min(len(start_parts), len(dest_parts)) and start_parts[i] == dest_parts[i]:
        i += 1
    up = [".."] * (len(start_parts) - i)
    return str(Path(*up, *dest_parts[i:])) if up or dest_parts[i:] else "."


def crate_of(src: Path) -> str:
    parts = src.resolve().relative_to(CRATES).parts
    return parts[0]


def main() -> None:
    moved = 0
    for src in sorted(CRATES.rglob("*.rs")):
        if "/target/" in str(src):
            continue
        text = src.read_text()
        if "#[cfg(test)]\nmod " not in text:
            continue
        stripped, mods = extract_modules(text)
        if not mods:
            continue
        crate = crate_of(src)
        dest_dir = OUT / crate
        dest_dir.mkdir(parents=True, exist_ok=True)
        hooks: list[str] = []
        for name, body in mods:
            dest = dest_dir / f"{src.stem}_{name}.rs"
            dest.write_text(body)
            rel = os_rel(src.parent, dest)
            hooks.append(f'#[cfg(test)]\n#[path = "{rel}"]\nmod {name};\n')
            print(f"{src.relative_to(REPO)} -> {dest.relative_to(REPO)} ({name})")
            moved += 1
        src.write_text(stripped.rstrip() + "\n\n" + "".join(hooks))
    print(f"moved {moved} modules")


if __name__ == "__main__":
    main()
