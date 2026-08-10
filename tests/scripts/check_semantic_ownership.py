#!/usr/bin/env python3
"""Validate the reviewed cross-repository semantic ownership inventory."""
from __future__ import annotations
import argparse, importlib.util, json, sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any
REPOSITORIES = ("zerostack", "fszero", "graphzero", "tokenzero")
ACTIONS = {"keep-domain", "thin-adapter", "extract-hub", "delete-duplicate", "deprecated-reexport", "centralize-test-tooling"}
REQUIRED_FIELDS = {"repo", "path", "blob_digest", "source_head", "role", "current_owner", "required_owner", "action", "hub_target", "consumers", "compatibility", "evidence", "owning_bead", "temporary_duplicate", "split_boundaries"}
LEDGER_FIELDS = {"repo", "path", "blob_digest", "source_head", "hub_target", "owning_bead", "reason", "deletion_order", "temporary_duplicate", "expiry_condition"}

def _load_loc_module() -> Any:
    script = Path(__file__).with_name("check_loc_majority.py")
    spec = importlib.util.spec_from_file_location("semantic_inventory_loc", script)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load LOC inventory validator: {script}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module

def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: top-level JSON value must be an object")
    return value

def default_repos(loc_module: Any) -> list[Any]:
    hub = Path(__file__).resolve().parents[2]
    roots = {"zerostack": hub, "fszero": hub.parent / "FSZero", "graphzero": hub.parent / "GraphZero", "tokenzero": hub.parent / "TokenZero"}
    return [loc_module.Repo(repo, roots[repo]) for repo in REPOSITORIES]

def _key(value: Mapping[str, Any]) -> tuple[str, str] | None:
    repo, path = value.get("repo"), value.get("path")
    return (repo, path) if isinstance(repo, str) and isinstance(path, str) else None

def validate_documents(semantic: Mapping[str, Any], loc: Mapping[str, Any], ledger: Mapping[str, Any], *, repos: Sequence[Any] | None = None, loc_module: Any | None = None) -> list[str]:
    errors: list[str] = []
    if semantic.get("schema") != "zerostack.semantic-ownership.v1": errors.append("semantic inventory schema must be zerostack.semantic-ownership.v1")
    if ledger.get("schema") != "zerostack.temporary-adoption-ledger.v1": errors.append("temporary adoption ledger schema must be zerostack.temporary-adoption-ledger.v1")
    semantic_heads, loc_heads, ledger_heads = semantic.get("generated_from_heads"), loc.get("generated_from_heads"), ledger.get("generated_from_heads")
    if semantic_heads != loc_heads: errors.append("semantic and LOC inventory source heads differ")
    if ledger_heads != semantic_heads: errors.append("temporary ledger and semantic inventory source heads differ")
    if not isinstance(semantic_heads, dict) or set(semantic_heads) != set(REPOSITORIES): errors.append(f"source heads must name exactly {REPOSITORIES}")
    loc_entries, records, ledger_entries = loc.get("files"), semantic.get("records"), ledger.get("entries")
    if not isinstance(loc_entries, list): return [*errors, "LOC inventory files must be an array"]
    if not isinstance(records, list): return [*errors, "semantic inventory records must be an array"]
    if not isinstance(ledger_entries, list): return [*errors, "temporary adoption ledger entries must be an array"]
    loc_by_key: dict[tuple[str, str], Mapping[str, Any]] = {}
    for entry in loc_entries:
        if not isinstance(entry, dict) or _key(entry) is None:
            errors.append("LOC inventory contains a record without repo/path"); continue
        key = _key(entry); assert key is not None
        if key in loc_by_key: errors.append(f"duplicate LOC inventory record: {key[0]}:{key[1]}")
        loc_by_key[key] = entry
    record_by_key: dict[tuple[str, str], Mapping[str, Any]] = {}; temporary_keys: set[tuple[str, str]] = set()
    for record in records:
        if not isinstance(record, dict): errors.append("semantic inventory record must be an object"); continue
        missing, key = REQUIRED_FIELDS - set(record), _key(record)
        label = f"{key[0]}:{key[1]}" if key else "<missing repo/path>"
        if missing: errors.append(f"{label}: missing fields {sorted(missing)}")
        if key is None: continue
        if key in record_by_key: errors.append(f"duplicate semantic inventory record: {label}")
        record_by_key[key] = record
        source = loc_by_key.get(key)
        if source is None: errors.append(f"uncovered-by-LOC semantic record: {label}")
        else:
            if record.get("blob_digest") != source.get("blob_digest"): errors.append(f"{label}: semantic blob digest differs from LOC inventory")
            expected_head = semantic_heads.get(key[0]) if isinstance(semantic_heads, dict) else None
            if record.get("source_head") != expected_head: errors.append(f"{label}: source head does not match repository binding")
        action = record.get("action")
        if action not in ACTIONS: errors.append(f"{label}: invalid action {action!r}")
        if action != "keep-domain" and not record.get("hub_target"): errors.append(f"{label}: non-domain action requires a hub target")
        if record.get("required_owner") not in REPOSITORIES: errors.append(f"{label}: required_owner must name one repository")
        if not isinstance(record.get("consumers"), list): errors.append(f"{label}: consumers must be an array")
        if not isinstance(record.get("evidence"), str) or not record.get("evidence"): errors.append(f"{label}: evidence is required")
        if not isinstance(record.get("owning_bead"), str) or not record.get("owning_bead"): errors.append(f"{label}: owning_bead is required")
        boundaries = record.get("split_boundaries")
        if not isinstance(boundaries, list): errors.append(f"{label}: split_boundaries must be an array")
        else:
            for boundary in boundaries:
                if not isinstance(boundary, dict): errors.append(f"{label}: split boundary must be an object"); continue
                start, end = boundary.get("start_line"), boundary.get("end_line")
                if not isinstance(start, int) or not isinstance(end, int) or start < 1 or end < start: errors.append(f"{label}: invalid split boundary {boundary!r}")
        if record.get("temporary_duplicate") is True: temporary_keys.add(key)
        elif record.get("temporary_duplicate") is not False: errors.append(f"{label}: temporary_duplicate must be boolean")
    for repo, path in sorted(set(loc_by_key) - set(record_by_key)): errors.append(f"uncovered tracked implementation file: {repo}:{path}")
    for repo, path in sorted(set(record_by_key) - set(loc_by_key)): errors.append(f"semantic record absent from LOC inventory: {repo}:{path}")
    ledger_by_key: dict[tuple[str, str], Mapping[str, Any]] = {}
    for entry in ledger_entries:
        if not isinstance(entry, dict): errors.append("temporary adoption ledger entry must be an object"); continue
        missing, key = LEDGER_FIELDS - set(entry), _key(entry)
        label = f"{key[0]}:{key[1]}" if key else "<missing repo/path>"
        if missing: errors.append(f"ledger {label}: missing fields {sorted(missing)}")
        if key is None: continue
        if key in ledger_by_key: errors.append(f"duplicate temporary adoption ledger entry: {label}")
        ledger_by_key[key] = entry; record = record_by_key.get(key)
        if record is None: errors.append(f"ledger entry has no semantic record: {label}"); continue
        for field in ("blob_digest", "source_head", "hub_target", "owning_bead"):
            if entry.get(field) != record.get(field): errors.append(f"ledger {label}: {field} differs from semantic record")
        if entry.get("temporary_duplicate") is not True: errors.append(f"ledger {label}: temporary_duplicate must be true")
        if not isinstance(entry.get("deletion_order"), list) or not entry.get("deletion_order"): errors.append(f"ledger {label}: deletion_order is required")
        if not isinstance(entry.get("expiry_condition"), str) or not entry.get("expiry_condition"): errors.append(f"ledger {label}: expiry_condition is required")
    ledger_keys = set(ledger_by_key)
    for repo, path in sorted(temporary_keys - ledger_keys): errors.append(f"temporary duplicate missing from ledger: {repo}:{path}")
    for repo, path in sorted(ledger_keys - temporary_keys): errors.append(f"ledger entry is not marked temporary in semantic inventory: {repo}:{path}")
    if repos is not None:
        if loc_module is None: raise ValueError("loc_module is required when repos are supplied")
        errors.extend(loc_module.validate_inventory(loc, repos))
    return errors

def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(); data = Path(__file__).parents[1] / "data"
    parser.add_argument("--inventory", type=Path, default=data / "semantic_ownership_inventory_v1.json")
    parser.add_argument("--loc-inventory", type=Path, default=data / "loc_ownership_v1.json")
    parser.add_argument("--ledger", type=Path, default=data / "temporary_adoption_ledger_v1.json")
    parser.add_argument("roots", nargs="*", type=Path); args = parser.parse_args(argv)
    loc_module = _load_loc_module()
    if args.roots and len(args.roots) != len(REPOSITORIES): parser.error(f"expected four roots in order {REPOSITORIES}")
    repos = [loc_module.Repo(repo, path.resolve()) for repo, path in zip(REPOSITORIES, args.roots, strict=True)] if args.roots else default_repos(loc_module)
    errors = validate_documents(load_json(args.inventory), load_json(args.loc_inventory), load_json(args.ledger), repos=repos, loc_module=loc_module)
    if errors:
        print("semantic ownership inventory: FAIL", file=sys.stderr)
        for error in errors: print(f"  - {error}", file=sys.stderr)
        return 1
    print(f"semantic ownership inventory: ok ({len(REPOSITORIES)} repositories)"); return 0
if __name__ == "__main__": raise SystemExit(main())
