#!/usr/bin/env python3
"""Focused tests for the retained ZeroRef native-matrix gate."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from scripts import zeroref_conformance_gate as gate


ENGINES = ("fszero", "graphzero", "tokenzero")
OS_NAMES = ("macos", "linux", "windows")
HASH = "a" * 64
COMMIT = "b" * 40


def complete_evidence() -> dict[str, Any]:
    rows = []
    binaries = []
    for os_name in OS_NAMES:
        rows.append(
            {
                "os": os_name,
                "cells": [
                    {
                        "writer": writer,
                        "reader": reader,
                        "status": "pass",
                        "expected_hash": HASH,
                        "actual_hash": HASH,
                    }
                    for writer in ENGINES
                    for reader in ENGINES
                ],
            }
        )
        binaries.extend(
            {
                "os": os_name,
                "engine": engine,
                "commit": COMMIT,
                "sha256": HASH,
            }
            for engine in ENGINES
        )
    return {
        "schema": "zeroref-conformance-evidence/v1",
        "descriptor_tests": "matrix.rs",
        "docs_audit": "ok",
        "timestamp": "2026-08-09T00:00:00Z",
        "matrix": {
            "status": "green",
            "rows": rows,
            "sibling_shas": binaries,
        },
    }


class ZeroRefConformanceGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.old_evidence_file = gate.EVIDENCE_FILE
        gate.EVIDENCE_FILE = Path(self.tempdir.name) / "evidence.json"

    def tearDown(self) -> None:
        gate.EVIDENCE_FILE = self.old_evidence_file
        self.tempdir.cleanup()

    def write_evidence(self, data: dict[str, Any]) -> None:
        gate.EVIDENCE_FILE.write_text(json.dumps(data), encoding="utf-8")

    def test_complete_native_three_by_three_matrix_passes(self) -> None:
        self.write_evidence(complete_evidence())
        self.assertEqual(gate.audit_evidence(), [])

    def test_green_label_cannot_hide_skipped_platform_rows(self) -> None:
        evidence = complete_evidence()
        linux = next(row for row in evidence["matrix"]["rows"] if row["os"] == "linux")
        linux["cells"] = [
            {
                "writer": writer,
                "reader": reader,
                "status": "skip",
                "skip_reason": "not executed on this host",
            }
            for writer in ENGINES
            for reader in ENGINES
        ]
        self.write_evidence(evidence)
        findings = gate.audit_evidence()
        self.assertTrue(any("linux row has 9 non-pass cells" in finding for finding in findings), findings)

    def test_every_os_engine_binary_must_be_pinned(self) -> None:
        evidence = complete_evidence()
        evidence["matrix"]["sibling_shas"].pop()
        self.write_evidence(evidence)
        findings = gate.audit_evidence()
        self.assertTrue(any("misses pinned native binaries" in finding for finding in findings), findings)


if __name__ == "__main__":
    unittest.main()
