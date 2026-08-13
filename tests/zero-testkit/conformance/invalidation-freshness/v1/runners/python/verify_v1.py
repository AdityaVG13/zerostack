#!/usr/bin/env python3
"""Independent v1 replay for the immutable invalidation/freshness KAT."""
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
vectors = json.loads((root / "vectors.json").read_text())
index = json.loads((root / "index.json").read_text())

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

def digest_json(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()

def digest_file(name):
    return hashlib.sha256((root / name).read_bytes()).hexdigest()

for name, expected in index["files"].items():
    if name != "index.json":
        assert digest_file(name) == expected, name

certificate = vectors["canonical_fresh_certificate"]
assert canonical(certificate) == vectors["canonical_fresh_bytes"]
assert hashlib.sha256(canonical(certificate).encode()).hexdigest() == vectors["canonical_fresh_bytes_sha256"]

influence = certificate["influence"]
for essential in influence["essential_dependencies"]:
    payload = {key: essential[key] for key in ("schema_version", "dependency", "witness")}
    assert digest_json(payload) == essential["certificate_digest"]
influence_payload = {key: influence[key] for key in (
    "schema_version", "model_version", "assembly_manifest_digest", "source_repository_heads",
    "producer_domains", "influence_scope", "edges", "essential_dependencies")}
assert digest_json(influence_payload) == influence["certificate_digest"]
replay_payload = {
    "domain": "zerostack.freshness.replay.v1", "index_id": certificate["index_id"],
    "index_generation": certificate["index_generation"], "influence_digest": influence["certificate_digest"]}
assert digest_json(replay_payload) == certificate["replay_identity"]
certificate_payload = {key: certificate[key] for key in (
    "schema_version", "model_version", "index_id", "index_generation", "influence", "replay_identity")}
assert digest_json(certificate_payload) == certificate["certificate_digest"]

required = vectors["required_closure"]
for case in vectors["cases"]:
    status, failure = "fresh", None
    if case["mutate_replay"]:
        status, failure = "unknown", "REPLAY_IDENTITY_MISMATCH"
    elif case["indexed_head"] != required["source_repository_heads"][0]["head"]:
        status, failure = "index_behind", "SOURCE_HEAD_MISMATCH"
    elif case["index_generation"] < case["minimum_generation"]:
        status, failure = "index_behind", "GENERATION_ROLLBACK"
    elif case.get("indexed_omit_last_edge"):
        status, failure = "index_behind", "MISSING_EDGE"
    elif case["indexed_extra_scope"]:
        status, failure = "unknown", "SCOPE_INFLATION"
    assert status == case["expected_status"], case["case_id"]
    assert failure == case["expected_failure_code"], case["case_id"]

assert vectors["clock_skew_vector"]["expected_effect"] == "none"
print("invalidation_freshness_kat:python:v1:passed")
