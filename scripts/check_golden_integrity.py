#!/usr/bin/env python3
"""Verify three-tier golden integrity. Never bless. Never rewrite.

Checks:
  1. checksums.sha256 matches on-disk bytes (sha256sum / shasum format)
  2. manifest.v1.json schema_version == 1.0.0 and every artifact hash
  3. Tier1Raw goldens are byte-equal to their live source_artifact_path
  4. no artifact labeled Tier1Raw carries a canonicalization_fn

Exit 1 on any drift.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
GOLDEN_ROOT = REPO_ROOT / "conformance" / "golden"
MANIFEST_NAME = "manifest.v1.json"
CHECKSUMS_NAME = "checksums.sha256"
MANIFEST_SCHEMA = "1.0.0"
ALLOWED_TIERS = ("Tier1Raw", "Tier2Canonical", "Tier3Logical")
CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  (.+)$")


def fail(message: str) -> None:
    print(f"golden-integrity failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_manifest() -> dict[str, Any]:
    path = GOLDEN_ROOT / MANIFEST_NAME
    if not path.is_file():
        fail(f"missing {path.relative_to(REPO_ROOT)}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        fail(f"manifest is not JSON: {exc}")
    if not isinstance(data, dict):
        fail("manifest is not an object")
    return data


def verify_checksums() -> int:
    path = GOLDEN_ROOT / CHECKSUMS_NAME
    if not path.is_file():
        fail(f"missing {path.relative_to(REPO_ROOT)}")
    rows = 0
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not raw or raw.startswith("#"):
            continue
        match = CHECKSUM_LINE.match(raw)
        if match is None:
            fail(f"{CHECKSUMS_NAME}:{line_no} is not `<sha256>  <relpath>`")
        expected, rel = match.group(1), match.group(2)
        target = GOLDEN_ROOT / rel
        if not target.is_file():
            fail(f"{CHECKSUMS_NAME}:{line_no} missing file {rel}")
        actual = sha256_hex(target.read_bytes())
        if actual != expected:
            fail(f"{rel} checksum drifted: expected {expected} got {actual}")
        rows += 1
    if rows == 0:
        fail(f"{CHECKSUMS_NAME} has no checksum rows")
    return rows


def verify_manifest(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    schema = manifest.get("schema_version")
    if schema != MANIFEST_SCHEMA:
        fail(f"manifest schema_version is {schema!r}, expected {MANIFEST_SCHEMA!r}")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        fail("manifest.artifacts must be a non-empty array")
    ids: list[str] = []
    for index, item in enumerate(artifacts):
        if not isinstance(item, dict):
            fail(f"artifact {index} is not an object")
        fixture_id = item.get("fixture_id")
        tier = item.get("tier")
        source = item.get("source")
        rel = item.get("path")
        digest = item.get("sha256")
        if not all(isinstance(value, str) and value for value in (fixture_id, tier, source, rel, digest)):
            fail(f"artifact {index} missing fixture_id/tier/source/path/sha256")
        assert isinstance(fixture_id, str)
        assert isinstance(tier, str)
        assert isinstance(rel, str)
        assert isinstance(digest, str)
        if tier not in ALLOWED_TIERS:
            fail(f"{fixture_id}: unknown tier {tier!r}")
        if item.get("canonicalization_fn") is not None and tier == "Tier1Raw":
            fail(f"{fixture_id}: Tier1Raw must not carry canonicalization_fn")
        target = GOLDEN_ROOT / rel
        if not target.is_file():
            fail(f"{fixture_id}: missing {rel}")
        actual = sha256_hex(target.read_bytes())
        if actual != digest:
            fail(f"{fixture_id}: manifest hash drifted for {rel}")
        ids.append(fixture_id)
    if ids != sorted(ids):
        fail("manifest.artifacts is not sorted by fixture_id")
    if len(ids) != len(set(ids)):
        fail("manifest.artifacts has duplicate fixture_id values")
    return [item for item in artifacts if isinstance(item, dict)]


def verify_tier1(artifacts: list[dict[str, Any]]) -> int:
    checked = 0
    for item in artifacts:
        if item.get("tier") != "Tier1Raw":
            continue
        fixture_id = str(item["fixture_id"])
        source_rel = item.get("source_artifact_path")
        if not isinstance(source_rel, str) or not source_rel:
            fail(f"{fixture_id}: Tier1Raw requires source_artifact_path")
        live = REPO_ROOT / source_rel
        golden = GOLDEN_ROOT / str(item["path"])
        if not live.is_file():
            fail(f"{fixture_id}: live source missing: {source_rel}")
        live_bytes = live.read_bytes()
        golden_bytes = golden.read_bytes()
        if live_bytes != golden_bytes:
            fail(
                f"{fixture_id}: Tier1Raw bytes drifted vs {source_rel} "
                f"(live={sha256_hex(live_bytes)} golden={sha256_hex(golden_bytes)})"
            )
        if sha256_hex(live_bytes) != item["sha256"]:
            fail(f"{fixture_id}: live source hash does not match manifest")
        checked += 1
    if checked == 0:
        fail("no Tier1Raw artifacts in manifest")
    return checked


def main() -> None:
    checksum_rows = verify_checksums()
    manifest = load_manifest()
    artifacts = verify_manifest(manifest)
    tier1 = verify_tier1(artifacts)
    print(
        f"golden-integrity ok: checksums={checksum_rows} "
        f"artifacts={len(artifacts)} tier1={tier1} schema={MANIFEST_SCHEMA}"
    )


if __name__ == "__main__":
    main()
