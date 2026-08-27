#!/usr/bin/env python3
"""Seal and verify the canonical digest in rotation snapshot JSON.

Canonical bytes are UTF-8 JSON for every root field except
``snapshot_sha256``. Objects sort keys by Unicode code point. Arrays preserve
order. Strings preserve code points and use JSON escaping without ASCII
replacement. Integers are signed 64-bit. Floats are rejected. Separators are
exactly ``,`` and ``:`` and there is no trailing newline. ``created_at`` is
included, so it is part of snapshot identity.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any

DIGEST_FIELD = "snapshot_sha256"
_HEX_64 = re.compile(r"^[0-9a-f]{64}$")


class SnapshotDigestError(ValueError):
    """The snapshot cannot be canonicalized or its digest does not match."""


def _reject_duplicate_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise SnapshotDigestError(f"duplicate JSON object key: {key}")
        result[key] = value
    return result


def load_snapshot(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_reject_duplicate_pairs)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SnapshotDigestError(f"cannot read rotation snapshot {path}: {error}") from error
    if not isinstance(value, dict):
        raise SnapshotDigestError("rotation snapshot root must be a JSON object")
    return value


def _validate_domain(value: Any, location: str = "$") -> None:
    if value is None or isinstance(value, (str, bool)):
        return
    if isinstance(value, int):
        if not -(2**63) <= value < 2**63:
            raise SnapshotDigestError(f"integer outside signed 64-bit range at {location}")
        return
    if isinstance(value, float):
        raise SnapshotDigestError(f"floating-point value is not canonical at {location}")
    if isinstance(value, list):
        for index, item in enumerate(value):
            _validate_domain(item, f"{location}[{index}]")
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise SnapshotDigestError(f"non-string object key at {location}")
            _validate_domain(item, f"{location}.{key}")
        return
    raise SnapshotDigestError(f"unsupported JSON value at {location}: {type(value).__name__}")


def canonical_snapshot_bytes(snapshot: dict[str, Any]) -> bytes:
    identity = {key: value for key, value in snapshot.items() if key != DIGEST_FIELD}
    _validate_domain(identity)
    text = json.dumps(
        identity,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    return text.encode("utf-8")


def snapshot_sha256(snapshot: dict[str, Any]) -> str:
    return hashlib.sha256(canonical_snapshot_bytes(snapshot)).hexdigest()


def seal_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    sealed = dict(snapshot)
    sealed[DIGEST_FIELD] = snapshot_sha256(snapshot)
    return sealed


def verify_snapshot(snapshot: dict[str, Any]) -> str:
    stored = snapshot.get(DIGEST_FIELD)
    if not isinstance(stored, str) or _HEX_64.fullmatch(stored) is None:
        raise SnapshotDigestError(f"{DIGEST_FIELD} must be 64 lowercase hex characters")
    actual = snapshot_sha256(snapshot)
    if actual != stored:
        raise SnapshotDigestError(f"rotation snapshot digest mismatch: stored={stored} actual={actual}")
    return actual


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path, help="rotation snapshot JSON to verify")
    args = parser.parse_args()
    try:
        digest = verify_snapshot(load_snapshot(args.snapshot))
    except SnapshotDigestError as error:
        print(json.dumps({"ok": False, "error": str(error)}, separators=(",", ":")))
        return 1
    print(json.dumps({"ok": True, "snapshot_sha256": digest}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
