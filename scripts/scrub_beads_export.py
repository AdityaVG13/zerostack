#!/usr/bin/env python3
"""Strip absolute host paths from the tracked beads export.

The four repos all use br, and br stamps every issue record with
`source_repo_path`, an absolute canonical workspace path. That field is
deliberate upstream (beads_rust#289 uses it to route beads back to the right
directory in multi-repo fleets), but .beads/issues.jsonl is tracked in a public
repo, so the field publishes the author's username and directory layout on
every single issue. br 0.2.16 has no config knob to omit or relativize it:
`br config list` exposes only issue_prefix, metrics.*, and sync.remote.

So this is a scrub, run before the export is staged. Two transformations:

1. `source_repo_path` is rewritten to the repo name. That is what
   `source_repo` already holds, so the field stays present and typed (schema
   allows string) without carrying host detail. Fleet routing that needs a real
   local path can recompute it from the checkout it is standing in.

2. Absolute host paths appearing in **any** string field of an issue record
   (recursive walk of dict/list values) are rewritten to `~/`-relative form.
   This is lossless for a human reader and preserves meaning, unlike deleting
   the path. Nested structures (labels metadata, agent_context blobs, etc.)
   are covered — not only a fixed top-level field list.

Idempotent: running it twice is a no-op, which matters because it runs on every
sync and must not fight br.

Exit codes: 0 clean or scrubbed, 1 on malformed input. Use --check to verify
without writing, which is what CI runs.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

# Any absolute home-directory path, capturing what follows the user segment so
# it can be re-expressed as ~/... rather than dropped.
HOME_PATH = re.compile(r"(?:/Users|/home)/[A-Za-z0-9_.-]+(?=/|\b)")

# A bare drive-letter Windows home, normalized the same way.
WIN_HOME_PATH = re.compile(r"[A-Za-z]:[\\/]Users[\\/][A-Za-z0-9_.-]+(?=[\\/]|\b)")


def relativize(text: str) -> str:
    """Rewrite absolute home paths to ~-relative form, leaving the rest intact."""
    text = HOME_PATH.sub("~", text)
    return WIN_HOME_PATH.sub("~", text)


def _scrub_string(value: str, *, key: str | None, source_repo: str | None) -> tuple[str, bool]:
    """source_repo_path → repo name; other strings → home paths to ~/."""
    if key == "source_repo_path" and value:
        replacement = source_repo or Path(value).name
        return (replacement, True) if value != replacement else (value, False)
    scrubbed = relativize(value)
    return scrubbed, scrubbed != value


def _scrub_list_items(value: list, *, source_repo: str | None) -> bool:
    """Mutate list items in place; return whether any child changed."""
    changed = False
    for i, item in enumerate(value):
        nv, c = scrub_value(item, key=None, source_repo=source_repo)
        if c:
            value[i] = nv
            changed = True
    return changed


def scrub_value(value: Any, *, key: str | None = None, source_repo: str | None = None) -> tuple[Any, bool]:
    """Recursively scrub strings in JSON-compatible values. Returns (value, changed)."""
    if isinstance(value, str):
        return _scrub_string(value, key=key, source_repo=source_repo)
    if isinstance(value, dict):
        nested = value.get("source_repo")
        src = nested if isinstance(nested, str) else source_repo
        changed = False
        for k, v in value.items():
            nv, c = scrub_value(v, key=k, source_repo=src)
            if c:
                value[k] = nv
                changed = True
        return value, changed
    if isinstance(value, list):
        return value, _scrub_list_items(value, source_repo=source_repo)
    return value, False


def scrub_record(record: dict) -> bool:
    """Scrub one issue record in place via recursive walk. Returns True if changed."""
    source_repo = record.get("source_repo") if isinstance(record.get("source_repo"), str) else None
    _, changed = scrub_value(record, source_repo=source_repo)
    return changed


def scrub_file(path: Path, *, check_only: bool) -> int:
    if not path.is_file():
        print(f"{path}: not found", file=sys.stderr)
        return 1

    original = path.read_text(encoding="utf-8")
    out_lines: list[str] = []
    dirty = 0

    for lineno, line in enumerate(original.splitlines(), 1):
        if not line.strip():
            out_lines.append(line)
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as exc:
            print(f"{path}:{lineno}: malformed JSON: {exc}", file=sys.stderr)
            return 1
        if isinstance(record, dict) and scrub_record(record):
            dirty += 1
        # separators match br's compact export so the diff stays minimal.
        out_lines.append(json.dumps(record, ensure_ascii=False, separators=(",", ":")))

    if dirty == 0:
        print(f"{path}: clean")
        return 0

    if check_only:
        print(
            f"{path}: {dirty} record(s) carry absolute host paths; "
            f"run scripts/scrub_beads_export.py to fix",
            file=sys.stderr,
        )
        return 1

    trailing = "\n" if original.endswith("\n") else ""
    path.write_text("\n".join(out_lines) + trailing, encoding="utf-8")
    print(f"{path}: scrubbed {dirty} record(s)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="JSONL exports to scrub (default: .beads/issues.jsonl beside this repo)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="report offenders and exit nonzero without writing (for CI)",
    )
    args = parser.parse_args()

    paths = args.paths or [Path(__file__).resolve().parent.parent / ".beads" / "issues.jsonl"]
    status = 0
    for path in paths:
        status |= scrub_file(path, check_only=args.check)
    return status


if __name__ == "__main__":
    raise SystemExit(main())
