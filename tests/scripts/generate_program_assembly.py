#!/usr/bin/env python3
"""Generate the assembly manifest bound to the tracked Program evidence."""

from __future__ import annotations

import hashlib
import json
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
EVIDENCE_PATH = ROOT / "conformance/models/program-aggregate-2026-08-11.json"
MANIFEST_PATH = ROOT / "conformance/models/program-assembly-2026-08-11.json"
REPORT_ROOT = ROOT / "tests/data/program-aggregate-reports"
ASSEMBLY_DOMAIN = b"zerostack.assembly_manifest.v1\0"
ASSEMBLY_ABI_CONTRACT_DIGEST = (
    "f9320787ce17676c1eff1b2e38f1897ca40f9a72a02d5d72ffba37d70aa70d70"
)
RAW_WORKER_PROTOCOL_DIGEST = (
    "e2daca4d95cbd2780f2e10b30b823e9398747bfe15e38ca0810f634a387aeace"
)
ENGINE_DATA = {
    "fszero": ("fszero", "fs_zero", "FSZero"),
    "graphzero": ("graphzero", "graph_zero", "GraphZero"),
    "tokenzero": ("tokenzero", "token_zero", "TokenZero"),
}


def canonical_bytes(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def update_report_paths(evidence: dict[str, object]) -> None:
    engines = evidence["engines"]
    assert isinstance(engines, dict)
    for engine in ENGINE_DATA:
        record = engines[engine]
        assert isinstance(record, dict)
        for surface in ("worker", "mcp"):
            report = record[surface]
            assert isinstance(report, dict)
            report["report"] = str(
                Path("tests/data/program-aggregate-reports") / Path(str(report["report"])).name
            )


def build_manifest(evidence: dict[str, object]) -> dict[str, object]:
    engines = evidence["engines"]
    execution = evidence["execution"]
    plan = evidence["plan"]
    assert isinstance(engines, dict)
    assert isinstance(execution, dict)
    assert isinstance(plan, dict)

    platform_profile = {
        "architecture": "aarch64",
        "operating_system": "linux",
        "profile_id": "native-linux-aarch64-v1",
    }
    platform_digest = digest_bytes(canonical_bytes(platform_profile))
    runtime_profile = {
        "runtime": "zero-codemode-v1",
        "verifier": "zerostack-shared-conformance",
    }
    runtime_digest = digest_bytes(canonical_bytes(runtime_profile))

    artifacts: list[dict[str, object]] = []
    workers: list[dict[str, object]] = []
    for key, (engine_wire, owner, repository) in ENGINE_DATA.items():
        record = engines[key]
        assert isinstance(record, dict)
        binary = record["binary"]
        attribution = record["executionAttribution"]
        worker_report = record["worker"]
        assert isinstance(binary, dict)
        assert isinstance(attribution, dict)
        assert isinstance(worker_report, dict)
        trace = attribution["trace"]
        assert isinstance(trace, dict)
        report = json.loads((ROOT / str(worker_report["report"])).read_text(encoding="utf-8"))
        contract_digest = str(trace["contract"])
        artifact_digest = str(binary["sha256"])
        capability_digest = str(report["provenance"]["checks_digest"])
        artifacts.append(
            {
                "artifact_digest": artifact_digest,
                "artifact_id": f"{key}.worker",
                "artifact_version": "raw-worker-v2",
                "contract_digest": contract_digest,
                "owner": owner,
                "source_repository": f"https://github.com/AdityaVG13/{repository}",
                "source_revision": str(record["sourceHead"]),
            }
        )
        workers.append(
            {
                "artifact_digest": artifact_digest,
                "capability_catalog_digest": capability_digest,
                "engine": engine_wire,
                "operation_registry_digest": contract_digest,
                "semantic_contract_digest": contract_digest,
                "worker_protocol_digest": RAW_WORKER_PROTOCOL_DIGEST,
            }
        )

    captured_at = datetime.fromisoformat(str(evidence["capturedAt"]).replace("Z", "+00:00"))
    return {
        "abi_contract_digest": ASSEMBLY_ABI_CONTRACT_DIGEST,
        "aggregate_capability_catalog_digest": str(plan["sha256"]),
        "assembly_epoch": int(captured_at.timestamp()),
        "linked_artifacts": artifacts,
        "linked_profiles": [
            {
                "profile_digest": platform_digest,
                "profile_id": "native-linux-aarch64-v1",
                "profile_kind": "platform",
                "profile_version": "1",
            },
            {
                "profile_digest": runtime_digest,
                "profile_id": "zero-codemode-v1",
                "profile_kind": "runtime",
                "profile_version": "1",
            },
        ],
        "platform": {
            "profile_digest": platform_digest,
            "profile_id": "native-linux-aarch64-v1",
            "profile_version": "1",
        },
        "receipt_schema": {
            "schema_digest": digest_bytes(
                b"zerostack.program.aggregate_execution_evidence.v1"
            ),
            "schema_id": "zerostack.program.aggregate_execution_evidence",
            "schema_version": "1",
        },
        "required_abi_contract_version": 1,
        "runtime_generation": int(execution["generation"]),
        "schema_version": 1,
        "target": {
            "abi": "gnu",
            "architecture": "aarch64",
            "operating_system": "linux",
            "target_triple": "aarch64-unknown-linux-gnu",
        },
        "verifiers": [
            {
                "verifier_digest": str(
                    json.loads(
                        (ROOT / str(engines["fszero"]["worker"]["report"])).read_text(
                            encoding="utf-8"
                        )
                    )["provenance"]["checks_digest"]
                ),
                "verifier_id": "zerostack-shared-conformance",
                "verifier_version": "raw-worker-v2",
            }
        ],
        "workers": workers,
    }


def main() -> None:
    evidence = json.loads(EVIDENCE_PATH.read_text(encoding="utf-8"))
    update_report_paths(evidence)
    manifest = build_manifest(evidence)
    manifest_digest = digest_bytes(ASSEMBLY_DOMAIN + canonical_bytes(manifest))
    evidence["assembly"] = {
        "abiContractDigest": ASSEMBLY_ABI_CONTRACT_DIGEST,
        "dispatchBoundary": "before_dispatch",
        "generatedBy": "tests/scripts/generate_program_assembly.py",
        "manifest": str(MANIFEST_PATH.relative_to(ROOT)),
        "manifestDigest": manifest_digest,
        "mismatchFailureCode": "manifest_digest_mismatch",
        "validation": "zero_abi::validate_assembly_pre_dispatch_v1",
    }
    MANIFEST_PATH.write_text(f"{json.dumps(manifest, indent=2, sort_keys=True)}\n", encoding="utf-8")
    EVIDENCE_PATH.write_text(f"{json.dumps(evidence, indent=2, sort_keys=True)}\n", encoding="utf-8")


if __name__ == "__main__":
    main()
