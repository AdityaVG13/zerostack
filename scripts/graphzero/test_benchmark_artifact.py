"""Focused contract checks for the committed benchmark artifact.

Run with::

    uv run python -m unittest scripts.test_benchmark_artifact

The driver owns artifact generation. These checks only validate the generated
receipt and its profile-binding sidecar; they never rewrite either file.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
RESULTS_PATH = ROOT / "bench" / "results.json"
PROFILE_BINDING_PATH = ROOT / "bench" / "results.profile.json"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
NON_ORIENT_FLOOR = 20
ORIENT_FLOOR = 50
DIAGNOSTIC_ONLY = {"cold_orient"}


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise AssertionError(f"{path} must contain a JSON object")
    return value


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class BenchmarkArtifactContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.results = load_json(RESULTS_PATH)
        cls.binding = load_json(PROFILE_BINDING_PATH)

    def test_source_identity_profile_and_binary_binding(self) -> None:
        corpus = self.results["corpus"]
        self.assertRegex(corpus["git_rev"], HEX40)
        self.assertRegex(corpus["git_sha"], HEX40)
        self.assertEqual(corpus["git_sha"], corpus["git_rev"])

        environment = self.results["measurement_environment"]
        self.assertEqual(environment["profile"], "release-perf")
        self.assertTrue(environment["binary_path"])
        self.assertRegex(environment["binary_sha256"], HEX64)
        self.assertTrue(environment["host_class"])
        self.assertTrue(environment["isolation"])

        artifact = self.binding["artifact"]
        self.assertEqual(artifact["path"], "benchmarks/latency/results.json")
        self.assertEqual(artifact["sha256"], sha256(RESULTS_PATH))
        self.assertEqual(artifact["git_sha"], corpus["git_sha"])
        self.assertEqual(artifact["git_rev"], corpus["git_rev"])
        self.assertEqual(artifact["binary_sha256"], environment["binary_sha256"])
        self.assertEqual(self.binding["measurement_environment"], environment)
        self.assertEqual(self.binding["status"], "current_profile_binding")

        self.assertRegex(environment["binary_path"], r"^\$\{CARGO_TARGET_DIR\}/release-perf/graphzero$")

    def test_publishing_sample_floors_hold(self) -> None:
        raw_series = {
            name: value
            for name, value in self.results.items()
            if isinstance(value, dict) and "raw_ms" in value
        }
        self.assertTrue(raw_series)
        for name, series in raw_series.items():
            samples = series["raw_ms"]
            self.assertIsInstance(samples, list)
            if name in DIAGNOSTIC_ONLY:
                self.assertEqual(len(samples), 1, name)
                continue
            floor = ORIENT_FLOOR if name in {"orient", "mcp"} else NON_ORIENT_FLOOR
            self.assertGreaterEqual(len(samples), floor, f"{name}: {len(samples)} < {floor}")

    def test_raw_series_have_cv_and_variance_envelopes(self) -> None:
        for name, series in self.results.items():
            if not isinstance(series, dict) or "raw_ms" not in series:
                continue
            cv_defined = series.get("cv_defined")
            self.assertIsInstance(cv_defined, bool, name)
            envelope = series.get("variance_envelope")
            self.assertIsInstance(envelope, dict, name)
            self.assertIsInstance(envelope["within_envelope"], bool, name)
            if cv_defined:
                cv_pct = series.get("cv_pct")
                self.assertIsInstance(cv_pct, (int, float), name)
                self.assertTrue(math.isfinite(float(cv_pct)), name)
                self.assertEqual(envelope["status"], series["variance_envelope"]["status"])
            else:
                self.assertIsNone(series.get("cv"), name)
                self.assertIsNone(series.get("cv_pct"), name)
                self.assertIsNone(envelope.get("cv"), name)
                self.assertIsNone(envelope.get("cv_pct"), name)
                self.assertEqual(envelope.get("status"), "undefined", name)


if __name__ == "__main__":
    unittest.main()
