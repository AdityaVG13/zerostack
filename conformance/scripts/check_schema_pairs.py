#!/usr/bin/env python3
"""Check generated embedded schemas against their documented schema sources."""

from __future__ import annotations

import argparse
import copy
import difflib
import json
import sys
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

JsonValue = dict[str, Any] | list[Any] | str | int | float | bool | None

SCHEMA_DIR = Path(__file__).resolve().parents[1] / "schemas"
PAIRS = {
    "capability-manifest.schema.json": "capability_manifest.json",
    "telemetry.schema.json": "telemetry.json",
    "error.schema.json": "error.json",
    "execution-record.schema.json": "execution_record.json",
    "limits.schema.json": "limits_echo.json",
}


class ResolutionError(ValueError):
    """A schema reference cannot be resolved safely."""


def load_json(path: Path) -> JsonValue:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ResolutionError(f"{path}: {error}") from error


def confined_path(current: Path, reference: str) -> Path:
    parsed = urlsplit(reference)
    if parsed.scheme or parsed.netloc:
        raise ResolutionError(f"remote $ref is forbidden: {reference!r}")
    raw_path = unquote(parsed.path)
    if Path(raw_path).is_absolute():
        raise ResolutionError(f"absolute $ref is forbidden: {reference!r}")
    target = (current.parent / raw_path).resolve() if raw_path else current.resolve()
    root = SCHEMA_DIR.resolve()
    if target != root and root not in target.parents:
        raise ResolutionError(f"$ref escapes schema directory: {reference!r}")
    return target


def pointer_value(document: JsonValue, fragment: str, reference: str) -> JsonValue:
    if not fragment:
        return document
    decoded = unquote(fragment)
    if not decoded.startswith("/"):
        raise ResolutionError(f"unsupported non-pointer $ref fragment: {reference!r}")
    value: JsonValue = document
    for raw_token in decoded[1:].split("/"):
        token = raw_token.replace("~1", "/").replace("~0", "~")
        if isinstance(value, dict) and token in value:
            value = value[token]
        elif isinstance(value, list):
            try:
                value = value[int(token)]
            except (ValueError, IndexError) as error:
                raise ResolutionError(f"invalid JSON pointer in $ref: {reference!r}") from error
        else:
            raise ResolutionError(f"invalid JSON pointer in $ref: {reference!r}")
    return value


def resolve_children(node: dict[str, Any], current: Path, stack: tuple[tuple[Path, str], ...]) -> dict[str, Any]:
    return {key: resolve(value, current, stack) for key, value in node.items()}


def resolve_reference(reference: object, current: Path, stack: tuple[tuple[Path, str], ...]) -> tuple[JsonValue, str]:
    if not isinstance(reference, str):
        raise ResolutionError(f"{current}: $ref must be a string")
    target = confined_path(current, reference)
    fragment = urlsplit(reference).fragment
    marker = (target, fragment)
    if marker in stack:
        chain = " -> ".join(f"{path.name}#{part}" for path, part in (*stack, marker))
        raise ResolutionError(f"cyclic $ref: {chain}")
    selected = pointer_value(load_json(target), fragment, reference)
    return resolve(copy.deepcopy(selected), target, (*stack, marker)), reference


def merge_siblings(resolved: JsonValue, siblings: dict[str, Any], reference: str, current: Path, stack: tuple[tuple[Path, str], ...]) -> JsonValue:
    if not siblings:
        return resolved
    if not isinstance(resolved, dict):
        raise ResolutionError(f"cannot apply $ref siblings to non-object: {reference!r}")
    overlap = resolved.keys() & siblings.keys()
    if overlap:
        raise ResolutionError(f"$ref sibling collision for {sorted(overlap)!r}: {reference!r}")
    return {**resolved, **resolve_children(siblings, current, stack)}


def resolve(node: JsonValue, current: Path, stack: tuple[tuple[Path, str], ...] = ()) -> JsonValue:
    if isinstance(node, list):
        return [resolve(item, current, stack) for item in node]
    if not isinstance(node, dict):
        return node
    if "$ref" not in node:
        return resolve_children(node, current, stack)
    resolved, reference = resolve_reference(node["$ref"], current, stack)
    siblings = {key: value for key, value in node.items() if key != "$ref"}
    return merge_siblings(resolved, siblings, reference, current, stack)


def rendered(value: JsonValue) -> str:
    return f"{json.dumps(value, indent=2, ensure_ascii=False)}\n"


def check_pair(source_name: str, snapshot_name: str, write: bool) -> bool:
    source = SCHEMA_DIR / source_name
    snapshot = SCHEMA_DIR / snapshot_name
    expected = resolve(load_json(source), source)
    if write:
        snapshot.write_text(rendered(expected), encoding="utf-8")
        print(f"wrote {snapshot.relative_to(SCHEMA_DIR.parent.parent)}")
        return True
    actual = load_json(snapshot)
    if actual == expected:
        print(f"ok: {snapshot_name} == resolved {source_name}")
        return True
    diff = difflib.unified_diff(
        rendered(actual).splitlines(), rendered(expected).splitlines(),
        fromfile=snapshot_name, tofile=f"resolved {source_name}", lineterm="",
    )
    print(f"drift: {snapshot_name} is not generated from {source_name}", file=sys.stderr)
    print("\n".join(diff), file=sys.stderr)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate embedded snapshots")
    args = parser.parse_args()
    try:
        results = [check_pair(source, snapshot, args.write) for source, snapshot in PAIRS.items()]
        passed = all(results)
    except ResolutionError as error:
        print(f"schema pair error: {error}", file=sys.stderr)
        return 1
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
