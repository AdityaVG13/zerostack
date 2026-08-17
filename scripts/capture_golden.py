#!/usr/bin/env python3
"""Capture three-tier golden artifacts from live hub sources.

This is an operator tool. CI must not invoke --write.
Prefer scripts/bless-golden.sh --i-am-the-operator.

Tiers:
  Tier1Raw        exact source bytes (SHA-256)
  Tier2Canonical  sorted JSON keys, collapsed whitespace, volatile fields stripped
  Tier3Logical    counts + required keys + status histogram + SPEC-tag verifier counts

A Tier-2 match is never labeled Tier-1.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import tomllib
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
GOLDEN_ROOT = REPO_ROOT / "conformance" / "golden"
MANIFEST_NAME = "manifest.v1.json"
CHECKSUMS_NAME = "checksums.sha256"
MANIFEST_SCHEMA = "1.0.0"

VOLATILE_KEYS = frozenset(
    {
        "run_id",
        "timestamp",
        "captured_at",
        "generated_at",
        "created_at",
        "created_at_utc",
        "target_head",
        "observed_version",
        "created_by_agent",
        "extracted_at_utc",
        "phase3_updated_at_utc",
    }
)

TIER1_SOURCES: list[tuple[str, str, str]] = [
    (
        "feature-universe",
        "conformance/contracts/supported_surface_matrix.toml",
        "tier1/feature_universe.toml",
    ),
    ("contract-md", "conformance/CONTRACT.md", "tier1/CONTRACT.md"),
    (
        "spec-version-contract",
        "conformance/contracts/spec_version_contract.toml",
        "tier1/spec_version_contract.toml",
    ),
    (
        "schema-zero-result-v1",
        "conformance/schemas/zero-result-v1.schema.json",
        "tier1/zero-result-v1.schema.json",
    ),
    (
        "fixture-raw-worker-v2",
        "conformance/fixtures/raw_worker_v2_frames.json",
        "tier1/raw_worker_v2_frames.json",
    ),
]

VERIFIER_TAG_RE = re.compile(r'^\s+tag:\s*"(SPEC-[A-Z0-9-]+)"\s*,\s*$', re.MULTILINE)
SPEC_TAG_CELL_RE = re.compile(r"`\[(SPEC-[A-Z0-9-]+)\]`")
HEADING_RE = re.compile(r"^## .+$", re.MULTILINE)


def fail(message: str) -> None:
    print(f"capture-golden failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def collapse_text(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    lines = [line.rstrip() for line in text.split("\n")]
    out: list[str] = []
    blank = False
    for line in lines:
        if line == "":
            if not blank:
                out.append("")
            blank = True
        else:
            out.append(line)
            blank = False
    return "\n".join(out).strip() + "\n"


def strip_volatile(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            key: strip_volatile(item)
            for key, item in value.items()
            if key not in VOLATILE_KEYS
        }
    if isinstance(value, list):
        return [strip_volatile(item) for item in value]
    return value


def toml_to_plain(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(key): toml_to_plain(item) for key, item in value.items()}
    if isinstance(value, list):
        return [toml_to_plain(item) for item in value]
    return value


def canonical_json_bytes(value: Any) -> bytes:
    cleaned = strip_volatile(value)
    return (
        json.dumps(
            cleaned,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
        + b"\n"
    )


def load_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        fail(f"cannot parse {path}: {exc}")
    if not isinstance(data, dict):
        fail(f"{path} is not a TOML table")
    return data


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot parse {path}: {exc}")


def wired_verifier_tags(root: Path) -> list[str]:
    source = root / "crates/zerostack-harness/src/spec_oracle.rs"
    text = source.read_text(encoding="utf-8")
    tags = VERIFIER_TAG_RE.findall(text)
    if not tags:
        fail("no SPEC verifier tags found in spec_oracle.rs")
    return tags


def catalog_spec_tags(root: Path) -> list[str]:
    catalog = root / "docs/spec/SPEC-TAGS.md"
    text = catalog.read_text(encoding="utf-8")
    # Table cells only; notes may repeat tags. Preserve first-seen order.
    tags: list[str] = []
    seen: set[str] = set()
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        for tag in SPEC_TAG_CELL_RE.findall(line):
            if tag not in seen:
                seen.add(tag)
                tags.append(tag)
    if not tags:
        fail("no SPEC tags found in docs/spec/SPEC-TAGS.md")
    return tags


def feature_universe_logical(root: Path) -> dict[str, Any]:
    matrix = load_toml(root / "conformance/contracts/supported_surface_matrix.toml")
    features = matrix.get("feature")
    if not isinstance(features, list):
        fail("feature tables missing")
    declared = matrix.get("declared_feature_ids")
    if not isinstance(declared, list):
        fail("declared_feature_ids missing")
    statuses: Counter[str] = Counter()
    family_status: dict[str, Counter[str]] = defaultdict(Counter)
    weights: list[float] = []
    for row in features:
        if not isinstance(row, dict):
            fail("feature row is not a table")
        status = str(row.get("status", ""))
        family = str(row.get("family", ""))
        statuses[status] += 1
        family_status[family][status] += 1
        weight = row.get("weight")
        if isinstance(weight, bool) or not isinstance(weight, (int, float)):
            fail(f"{row.get('id')}: weight is not numeric")
        weights.append(float(weight))
    families = {
        family: {
            "present": counts.get("present", 0),
            "partial": counts.get("partial", 0),
            "missing": counts.get("missing", 0),
            "excluded": counts.get("excluded", 0),
        }
        for family, counts in sorted(family_status.items())
    }
    return {
        "feature_count": len(features),
        "declared_id_count": len(declared),
        "weight_sum": math.fsum(weights),
        "weight_policy": matrix.get("weight_policy"),
        "status_histogram": {
            "present": statuses.get("present", 0),
            "partial": statuses.get("partial", 0),
            "missing": statuses.get("missing", 0),
            "excluded": statuses.get("excluded", 0),
        },
        "family_status": families,
        "required_row_keys": ["id", "family", "name", "status", "weight"],
    }


def contract_logical(root: Path) -> dict[str, Any]:
    text = (root / "conformance/CONTRACT.md").read_text(encoding="utf-8")
    headings = HEADING_RE.findall(text)
    return {
        "section_count": len(headings),
        "required_headings": headings,
    }


def schema_logical(root: Path) -> dict[str, Any]:
    schema = load_json(root / "conformance/schemas/zero-result-v1.schema.json")
    if not isinstance(schema, dict):
        fail("zero-result-v1 schema is not an object")
    required = schema.get("required")
    return {
        "type": schema.get("type"),
        "required": required if isinstance(required, list) else [],
        "top_level_keys": sorted(schema.keys()),
    }


def fixture_logical(root: Path) -> dict[str, Any]:
    fixture = load_json(root / "conformance/fixtures/raw_worker_v2_frames.json")
    if not isinstance(fixture, list):
        fail("raw_worker_v2_frames.json is not an array")
    kinds: list[str] = []
    for entry in fixture:
        if isinstance(entry, dict) and isinstance(entry.get("kind"), str):
            kinds.append(entry["kind"])
    return {
        "entry_count": len(fixture),
        "kinds": kinds,
        "required_entry_keys": ["kind", "request"],
    }


def spec_verifier_logical(root: Path) -> dict[str, Any]:
    wired = wired_verifier_tags(root)
    catalog = catalog_spec_tags(root)
    return {
        "wired_count": len(wired),
        "catalog_tag_count": len(catalog),
        "unverified_count": len(catalog) - len(wired),
        "wired_tags": wired,
        "equivalence_predicate": "tag_set_and_counts",
    }


def build_tier3(root: Path) -> dict[str, Any]:
    return {
        "schema_version": "zerostack.golden.tier3.logical.v1",
        "equivalence_tier": "Tier3Logical",
        "equivalence_predicate": "counts_and_required_keys",
        "feature_universe": feature_universe_logical(root),
        "spec_verifiers": spec_verifier_logical(root),
        "contract_md": contract_logical(root),
        "schema_zero_result_v1": schema_logical(root),
        "fixture_raw_worker_v2": fixture_logical(root),
    }


def write_bytes(path: Path, data: bytes, write: bool) -> None:
    if not write:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def artifact_entry(
    fixture_id: str,
    tier: str,
    rel_path: str,
    digest: str,
    source_artifact_path: str,
    *,
    canonicalization_fn: str | None,
    equivalence_predicate: str,
) -> dict[str, Any]:
    if tier == "Tier1Raw" and canonicalization_fn is not None:
        fail(f"{fixture_id}: Tier1Raw cannot carry a canonicalization_fn")
    return {
        "fixture_id": fixture_id,
        "tier": tier,
        "source": "curated-corpus",
        "path": rel_path,
        "sha256": digest,
        "source_artifact_path": source_artifact_path,
        "canonicalization_fn": canonicalization_fn,
        "equivalence_predicate": equivalence_predicate,
        "reference_version": "spec-v1",
        "replay_command": "python3 scripts/check_golden_integrity.py",
    }


def capture(root: Path, write: bool) -> dict[str, Any]:
    artifacts: list[dict[str, Any]] = []

    for fixture_id, source_rel, dest_rel in TIER1_SOURCES:
        source = root / source_rel
        if not source.is_file():
            fail(f"missing live source {source_rel}")
        data = source.read_bytes()
        dest = GOLDEN_ROOT / dest_rel
        write_bytes(dest, data, write)
        artifacts.append(
            artifact_entry(
                f"{fixture_id}.tier1",
                "Tier1Raw",
                dest_rel,
                sha256_hex(data),
                source_rel,
                canonicalization_fn=None,
                equivalence_predicate="sha256_bytes",
            )
        )

    text_tier2 = [
        (
            "feature-universe",
            "conformance/contracts/supported_surface_matrix.toml",
            "tier2/feature_universe.canonical.json",
            "toml_to_sorted_json+strip_volatile",
        ),
        (
            "spec-version-contract",
            "conformance/contracts/spec_version_contract.toml",
            "tier2/spec_version_contract.canonical.json",
            "toml_to_sorted_json+strip_volatile",
        ),
    ]
    for fixture_id, source_rel, dest_rel, canon_fn in text_tier2:
        parsed = toml_to_plain(load_toml(root / source_rel))
        data = canonical_json_bytes(parsed)
        write_bytes(GOLDEN_ROOT / dest_rel, data, write)
        artifacts.append(
            artifact_entry(
                f"{fixture_id}.tier2",
                "Tier2Canonical",
                dest_rel,
                sha256_hex(data),
                source_rel,
                canonicalization_fn=canon_fn,
                equivalence_predicate="canonical_json_sha256",
            )
        )

    md_source = "conformance/CONTRACT.md"
    md_dest = "tier2/CONTRACT.canonical.txt"
    md_data = collapse_text((root / md_source).read_text(encoding="utf-8")).encode(
        "utf-8"
    )
    write_bytes(GOLDEN_ROOT / md_dest, md_data, write)
    artifacts.append(
        artifact_entry(
            "contract-md.tier2",
            "Tier2Canonical",
            md_dest,
            sha256_hex(md_data),
            md_source,
            canonicalization_fn="collapse_insignificant_whitespace",
            equivalence_predicate="canonical_text_sha256",
        )
    )

    json_tier2 = [
        (
            "schema-zero-result-v1",
            "conformance/schemas/zero-result-v1.schema.json",
            "tier2/zero-result-v1.schema.canonical.json",
        ),
        (
            "fixture-raw-worker-v2",
            "conformance/fixtures/raw_worker_v2_frames.json",
            "tier2/raw_worker_v2_frames.canonical.json",
        ),
    ]
    for fixture_id, source_rel, dest_rel in json_tier2:
        data = canonical_json_bytes(load_json(root / source_rel))
        write_bytes(GOLDEN_ROOT / dest_rel, data, write)
        artifacts.append(
            artifact_entry(
                f"{fixture_id}.tier2",
                "Tier2Canonical",
                dest_rel,
                sha256_hex(data),
                source_rel,
                canonicalization_fn="json_sort_keys+strip_volatile+compact",
                equivalence_predicate="canonical_json_sha256",
            )
        )

    tier3 = build_tier3(root)
    tier3_rel = "tier3/logical_dump.json"
    tier3_data = canonical_json_bytes(tier3)
    write_bytes(GOLDEN_ROOT / tier3_rel, tier3_data, write)
    artifacts.append(
        artifact_entry(
            "logical-dump.tier3",
            "Tier3Logical",
            tier3_rel,
            sha256_hex(tier3_data),
            "conformance/contracts/supported_surface_matrix.toml",
            canonicalization_fn="logical_counts_and_required_keys",
            equivalence_predicate="counts_and_required_keys",
        )
    )

    artifacts.sort(key=lambda item: str(item["fixture_id"]))
    manifest = {
        "schema_version": MANIFEST_SCHEMA,
        "discipline": (
            "Encode the distinction; never paper over it. "
            "A Tier2 match is not Tier1."
        ),
        "project": "ZeroStack",
        "reference_identity": "spec-v1",
        "artifacts": artifacts,
    }
    manifest_bytes = (
        json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False).encode(
            "utf-8"
        )
        + b"\n"
    )
    write_bytes(GOLDEN_ROOT / MANIFEST_NAME, manifest_bytes, write)

    checksum_rows = [
        (sha256_hex(manifest_bytes), MANIFEST_NAME),
        *[(str(item["sha256"]), str(item["path"])) for item in artifacts],
    ]
    checksum_rows.sort(key=lambda item: item[1])
    checksums = "".join(f"{digest}  {rel}\n" for digest, rel in checksum_rows)
    write_bytes(GOLDEN_ROOT / CHECKSUMS_NAME, checksums.encode("utf-8"), write)

    return {
        "schema_version": MANIFEST_SCHEMA,
        "write": write,
        "artifact_count": len(artifacts),
        "manifest": manifest,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write",
        action="store_true",
        help="write golden files (operator only; use bless-golden.sh)",
    )
    args = parser.parse_args()
    result = capture(REPO_ROOT, write=args.write)
    print(
        f"capture-golden ok: write={int(args.write)} "
        f"artifacts={result['artifact_count']} schema={MANIFEST_SCHEMA}"
    )


if __name__ == "__main__":
    main()
