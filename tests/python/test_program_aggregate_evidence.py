import hashlib
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "conformance" / "models" / "program-aggregate-2026-08-11.json"


class ProgramAggregateEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.evidence = json.loads(EVIDENCE.read_text())

    def test_real_aggregate_plan_binds_all_engine_receipts(self) -> None:
        evidence = self.evidence
        self.assertEqual(evidence["schema"], "zerostack.program.aggregate_execution_evidence.v1")
        self.assertTrue(evidence["execution"]["ok"])
        self.assertEqual(evidence["plan"]["stepCount"], 3)
        self.assertEqual(set(evidence["execution"]["result"]), {"fs", "graph", "token"})
        self.assertTrue(all(evidence["claims"].values()))

    def test_worker_receipts_are_complete_current_and_content_bound(self) -> None:
        for engine, evidence in self.evidence["engines"].items():
            path = ROOT / evidence["worker"]["report"]
            data = path.read_bytes()
            report = json.loads(data)
            self.assertEqual(hashlib.sha256(data).hexdigest(), evidence["worker"]["reportSha256"])
            self.assertEqual(report["surface"], "codemode")
            self.assertEqual(report["completion_status"], "complete")
            self.assertTrue(report["passed"])
            self.assertEqual(report["provenance"]["source_head"], evidence["sourceHead"])
            self.assertEqual(report["provenance"]["hub_head"], self.evidence["hubHead"])
            self.assertEqual(report["provenance"]["fail_count"], 0)
            self.assertEqual(report["provenance"]["skip_count"], 0)
            self.assertEqual(
                evidence["executionAttribution"]["trace"]["revision"],
                report["provenance"]["artifact_sha256"],
                engine,
            )

    def test_mcp_receipts_measure_exposure_without_overclaiming_scope(self) -> None:
        for evidence in self.evidence["engines"].values():
            path = ROOT / evidence["mcp"]["report"]
            data = path.read_bytes()
            report = json.loads(data)
            self.assertEqual(hashlib.sha256(data).hexdigest(), evidence["mcp"]["reportSha256"])
            self.assertEqual(report["surface"], "mcp")
            self.assertEqual(report["completion_status"], "partial")
            self.assertFalse(report["passed"])
            self.assertEqual(report["checks"][0]["id"], "G1")
            self.assertEqual(report["checks"][0]["status"], "pass")
            self.assertEqual(report["provenance"]["source_head"], evidence["sourceHead"])
            self.assertEqual(report["provenance"]["hub_head"], self.evidence["hubHead"])
            self.assertEqual(report["provenance"]["fail_count"], 0)
            self.assertEqual(report["provenance"]["skip_count"], 9)


if __name__ == "__main__":
    unittest.main()
