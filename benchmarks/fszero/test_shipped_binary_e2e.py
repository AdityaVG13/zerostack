#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "shipped_binary_e2e.py"
SPEC = importlib.util.spec_from_file_location("shipped_binary_e2e", SCRIPT)
assert SPEC and SPEC.loader
HARNESS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = HARNESS
SPEC.loader.exec_module(HARNESS)


class MatrixFixtureTests(unittest.TestCase):
    def test_matrix_is_complete_and_revision_independent(self) -> None:
        matrix = HARNESS.build_matrix(trials=3, batch_width=4)
        self.assertEqual(len(matrix["cells"]), 11 * 3 * 2 * 2)
        self.assertEqual(set(matrix["operations"]), set(HARNESS.REQUIRED_OPERATIONS))
        self.assertEqual(set(matrix["surfaces"]), {"cli", "mcp", "codemode"})
        self.assertEqual(set(matrix["temperatures"]), {"cold", "warm"})
        self.assertEqual(set(matrix["invocations"]), {"single", "batch"})
        self.assertTrue(all(cell["trials"] >= 3 for cell in matrix["cells"]))
        self.assertTrue(all(cell["required_if_surface_available"] for cell in matrix["cells"]))
        for cell in matrix["cells"]:
            expected = HARNESS.sha256_bytes(HARNESS.canonical_json(cell["payload"]).encode())
            self.assertEqual(cell["payload_sha256"], expected)
            self.assertNotIn("revision", cell["payload"])

    def test_each_required_operation_has_all_twelve_surface_cells(self) -> None:
        matrix = HARNESS.build_matrix(trials=3, batch_width=4)
        for operation in HARNESS.REQUIRED_OPERATIONS:
            cells = [cell for cell in matrix["cells"] if cell["operation"] == operation]
            self.assertEqual(len(cells), 12, operation)
            coordinates = {
                (cell["surface"], cell["temperature"], cell["invocation"])
                for cell in cells
            }
            self.assertEqual(len(coordinates), 12, operation)

    def test_dry_run_executes_no_benchmark_and_reports_all_trials(self) -> None:
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), "--dry-run", "--trials", "3", "--batch-width", "4"],
            cwd=HARNESS.ROOT, text=True, capture_output=True, check=True,
        )
        result = json.loads(proc.stdout)
        self.assertEqual(result["mode"], "dry-run")
        self.assertFalse(result["full_benchmark_executed"])
        self.assertEqual(result["cell_count"], 132)
        self.assertEqual(result["expected_trial_records"], (132 + 2) * 3 * 2)
        self.assertEqual(result["baseline_revision"], "36a23a8")
        self.assertEqual(result["candidate_revision"], "15436a6")

    def test_cli_dag_plan_preserves_structured_payloads_without_interpreter(self) -> None:
        calls = [
            HARNESS.operation_payload("edit", 0),
            HARNESS.operation_payload("undo", 0),
        ]
        plan = json.loads(HARNESS.dag_plan(calls))
        self.assertEqual([step["call"] for step in plan["steps"]], ["fs.edit", "fs.undo"])
        self.assertEqual(plan["steps"][0]["args"], calls[0]["args"])
        self.assertEqual(plan["steps"][1]["args"], calls[1]["args"])
        self.assertEqual(plan["steps"][0]["needs"], [])
        self.assertEqual(plan["steps"][1]["needs"], ["s0"])

    def test_rejects_fewer_than_three_trials(self) -> None:
        with self.assertRaisesRegex(ValueError, "at least 3"):
            HARNESS.build_matrix(trials=2, batch_width=4)


class IntegrityFixtureTests(unittest.TestCase):
    def test_resource_parsers_preserve_documented_units(self) -> None:
        darwin = HARNESS.parse_resources(
            "0.12 real 0.03 user 0.02 sys\n123456 maximum resident set size\n",
            "darwin-time-l",
        )
        self.assertEqual(darwin["user_cpu_ms"], 30.0)
        self.assertEqual(darwin["system_cpu_ms"], 20.0)
        self.assertEqual(darwin["cpu_total_ms"], 50.0)
        self.assertEqual(darwin["peak_rss_bytes"], 123456)
        gnu = HARNESS.parse_resources(
            "User time (seconds): 0.04\nSystem time (seconds): 0.01\n"
            "Maximum resident set size (kbytes): 2048\n",
            "gnu-time-v",
        )
        self.assertEqual(gnu["cpu_total_ms"], 50.0)
        self.assertEqual(gnu["peak_rss_bytes"], 2 * 1024 * 1024)

    def test_frozen_binary_checksum_is_enforced(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "fszero-mcp"
            binary.write_bytes(b"frozen")
            artifact = {
                "path": str(binary), "size_bytes": binary.stat().st_size,
                "sha256": HARNESS.sha256_file(binary),
            }
            HARNESS.verify_artifact(artifact)
            binary.write_bytes(b"changed")
            with self.assertRaisesRegex(RuntimeError, "frozen binary changed"):
                HARNESS.verify_artifact(artifact)

    def test_summary_emits_percent_hit90_and_hard_limit(self) -> None:
        records = []
        for revision, wall in (("baseline", 100.0), ("candidate", 9.0)):
            for trial in range(1, 4):
                records.append({
                    "kind": "trial", "status": "ok", "revision": revision,
                    "cell_id": "read/mcp/warm/single", "trial": trial,
                    "metrics": {
                        "wall_ms": wall, "process_wall_ms": wall,
                        "user_cpu_ms": wall, "system_cpu_ms": wall,
                        "cpu_total_ms": wall * 2, "peak_rss_bytes": wall * 1000,
                    },
                })
        manifests = {
            revision: {"artifacts": {
                surface: {"available": True, "size_bytes": 100 if revision == "baseline" else 9}
                for surface in HARNESS.SURFACES
            }} for revision in ("baseline", "candidate")
        }
        summary = HARNESS.summarize(records, manifests, trials=3, max_regression_pct=5.0)
        wall = next(c for c in summary["comparisons"] if c["key"] == "read/mcp/warm/single" and c["metric"] == "wall_ms")
        self.assertAlmostEqual(wall["improvement_pct"], 91.0)
        self.assertTrue(wall["hit_90"])
        self.assertTrue(wall["hard_limit"]["pass"])
        self.assertEqual(wall["hard_limit"]["kind"], "max_regression_pct")

    def test_zero_idle_baseline_and_candidate_pass_hard_limit(self) -> None:
        comparison = HARNESS.comparison_entry(
            "warm_idle/mcp", "idle_cpu_pct", [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0], 5.0,
        )
        self.assertEqual(comparison["improvement_pct"], 0.0)
        self.assertEqual(comparison["hard_limit"]["observed_regression_pct"], 0.0)
        self.assertTrue(comparison["hard_limit"]["pass"])

    def test_any_error_suppresses_publishable_comparisons(self) -> None:
        records = [{
            "kind": "trial", "status": "error", "revision": "candidate",
            "cell_id": "read/mcp/cold/single", "metrics": None,
            "error": {"class": "RuntimeError", "stage": "trial", "message": "failed"},
        }]
        manifests = {revision: {"artifacts": {}} for revision in ("baseline", "candidate")}
        summary = HARNESS.summarize(records, manifests, trials=3, max_regression_pct=5.0)
        self.assertEqual(summary["integrity_status"], "failed")
        self.assertFalse(summary["publishable"])
        self.assertEqual(summary["comparisons"], [])
        self.assertEqual(summary["error_count"], 1)

    def test_cpu_time_parser_accepts_darwin_and_day_formats(self) -> None:
        self.assertEqual(HARNESS.parse_cpu_time("01:02.50"), 62500.0)
        self.assertEqual(HARNESS.parse_cpu_time("1-01:00:00"), 25 * 3600 * 1000)


if __name__ == "__main__":
    unittest.main()
