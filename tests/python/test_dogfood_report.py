import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "conformance" / "models" / "dogfood-2026-08-10.json"


class DogfoodReportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.report = json.loads(REPORT.read_text())

    def test_all_measured_workflows_are_retained_and_green(self) -> None:
        evidence = self.report["measured_adapter_workflows"]
        for harness in ("pi", "omp"):
            rows = evidence[harness]["rows"]
            self.assertEqual(len(rows), 15)
            for engine in ("fs", "graph", "token"):
                engine_rows = [row for row in rows if row["engine"] == engine]
                self.assertEqual(len(engine_rows), 5)
                self.assertTrue(all(row["error"] is None for row in engine_rows))
        self.assertEqual(self.report["comparison_policy"]["rows_filtered"], 0)

    def test_required_campaign_evidence_and_followups_are_bound(self) -> None:
        for harness in ("pi", "omp"):
            campaign = self.report["campaigns"][harness]
            self.assertTrue(campaign["spill_expansion"]["succeeded"])
            self.assertEqual(campaign["spill_expansion"]["content_kind"], "capsule")
            self.assertFalse(campaign["active_cancellation"]["tool_execution_end_observed"])
            self.assertEqual(campaign["active_cancellation"]["next_call_result"], 42)
            self.assertEqual(campaign["shutdown_reap"]["pgrep_matches"], 0)
            self.assertTrue(campaign["yield_wait"]["live"])
        repos = {row["repo"] for row in self.report["subsequent_beads"]}
        self.assertEqual(repos, {"ZeroStack", "FSZero", "GraphZero", "TokenZero", "pi-stack"})
        self.assertTrue(all(row["result"] == "closed" for row in self.report["subsequent_beads"]))

    def test_baselines_and_unmeasured_fields_are_truthful(self) -> None:
        for baseline in self.report["native_baselines"]:
            self.assertGreaterEqual(baseline["checks"], 5)
            self.assertEqual(baseline["passed"], baseline["checks"])
        policy = self.report["comparison_policy"]
        self.assertEqual(policy["native_baseline_latency"], "UNMEASURED")
        self.assertEqual(policy["native_baseline_visible_bytes"], "UNMEASURED")
        self.assertEqual(policy["performance_claim"], "none")


if __name__ == "__main__":
    unittest.main()
