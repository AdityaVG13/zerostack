"""Focused tests for scripts/bench_ratchet.py.

Run: uv run python scripts/test_bench_ratchet.py
"""

import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("bench_ratchet.py")
spec = importlib.util.spec_from_file_location("graphzero_bench_ratchet", MODULE_PATH)
ratchet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ratchet)

EXIT_PASS = ratchet.EXIT_PASS
EXIT_FAIL = ratchet.EXIT_FAIL
EXIT_USAGE = ratchet.EXIT_USAGE


def make_receipt(
    blast_p50: float = 10.0,
    blast_p95: float = 14.0,
    orient_p50: float = 8.0,
    orient_p95: float = 10.0,
    runs: int = 20,
    *,
    profile: str = "release-perf",
    host_class: str = "macos-rch-test-runner",
    corpus: str = "graphzero",
    isolation: str = "dedicated isolated run",
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "corpus": {"name": corpus},
        "measurement_environment": {
            "profile": profile,
            "host_class": host_class,
            "isolation": isolation,
        },
        "metrics": {
            "blast": {
                "label": "blast",
                "p50_ms": blast_p50,
                "p95_ms": blast_p95,
                "runs": runs,
            },
            "incremental_update": {"status": "not_available"},
            "orient_symbol": {
                "label": "orient_symbol",
                "p50_ms": orient_p50,
                "p95_ms": orient_p95,
                "runs": runs,
            },
        },
    }


def make_gate() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "thresholds": {
            "orient_symbol": {
                "p50_max_ms": 120.0,
                "p95_max_ms": 150.0,
                "relative_max_multiple": 2.0,
            },
            "blast": {
                "p50_max_ms": 15.0,
                "p95_max_ms": 25.0,
                "relative_max_multiple": 2.0,
            },
        },
    }


def make_baseline() -> dict[str, Any]:
    receipt = make_receipt()
    receipt["policy"] = {"min_samples": 10, "relative_max_multiple_default": 1.5}
    return receipt


class RatchetTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def write(self, name: str, data: dict[str, Any]) -> Path:
        path = self.dir / name
        path.write_text(json.dumps(data), encoding="utf-8")
        return path

    def run_ratchet(
        self,
        baseline: dict[str, Any],
        current: dict[str, Any],
        gate: dict[str, Any] | None = None,
        extra_args: list[str] | None = None,
    ) -> tuple[int, str, str]:
        b = self.write("baseline.json", baseline)
        c = self.write("current.json", current)
        g = self.write("gate.json", gate if gate is not None else make_gate())
        args = ["--baseline", str(b), "--current", str(c), "--latency-gate", str(g)]
        args.extend(extra_args or [])
        out_buf = io.StringIO()
        err_buf = io.StringIO()
        with redirect_stdout(out_buf), redirect_stderr(err_buf):
            code = ratchet.main(args)
        return code, out_buf.getvalue(), err_buf.getvalue()


class RatchetPassTests(RatchetTestBase):
    def test_pass_under_absolute_and_relative_policy(self) -> None:
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt())
        self.assertEqual(code, EXIT_PASS)
        self.assertIn('"status": "pass"', out)


class RatchetAbsoluteFailureTests(RatchetTestBase):
    def test_absolute_p50_failure(self) -> None:
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(blast_p50=16.0))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("absolute p50 regression", out)

    def test_absolute_p95_failure(self) -> None:
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(blast_p95=26.0))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("absolute p95 regression", out)


class RatchetRelativeFailureTests(RatchetTestBase):
    def test_relative_p50_failure(self) -> None:
        # orient baseline p50 = 8.0, gate multiple 2.0 -> relative fail at 17.0
        # (still under the 120 ms absolute budget).
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(orient_p50=17.0))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("relative p50 regression", out)

    def test_relative_p95_failure(self) -> None:
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(orient_p95=21.0))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("relative p95 regression", out)

    def test_slowdown_within_multiple_passes(self) -> None:
        # orient baseline p50 = 8.0, multiple 2.0: 15.9 <= 16.0 must pass
        # (relative policy allows up to baseline * multiple).
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(orient_p50=15.9))
        self.assertEqual(code, EXIT_PASS)
        self.assertIn('"status": "pass"', out)


class RatchetIdentityTests(RatchetTestBase):
    def test_profile_mismatch_rejected(self) -> None:
        code, _, err = self.run_ratchet(make_baseline(), make_receipt(profile="release"))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("cargo profile mismatch", err)

    def test_host_class_mismatch_rejected(self) -> None:
        code, _, err = self.run_ratchet(
            make_baseline(), make_receipt(host_class="linux-ci-runner")
        )
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("host class mismatch", err)

    def test_corpus_mismatch_rejected(self) -> None:
        code, _, err = self.run_ratchet(make_baseline(), make_receipt(corpus="tokenzero"))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("corpus mismatch", err)

    def test_isolation_mismatch_rejected(self) -> None:
        code, _, err = self.run_ratchet(
            make_baseline(), make_receipt(isolation="different machine shared run")
        )
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("isolation mismatch", err)

    def test_scenario_metric_set_mismatch_rejected(self) -> None:
        current = make_receipt()
        del current["metrics"]["orient_symbol"]
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("scenario/metric set mismatch", err)

    def test_numeric_shape_change_rejected(self) -> None:
        # Same metric keys, but blast switched from numeric to status-only in
        # the current receipt; the shape change must not skip relative checks.
        current = make_receipt()
        current["metrics"]["blast"] = {"status": "not_available"}
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("numeric metric set mismatch", err)
        self.assertIn("blast", err)


class RatchetSampleTests(RatchetTestBase):
    def test_insufficient_samples_rejected(self) -> None:
        code, out, _ = self.run_ratchet(make_baseline(), make_receipt(runs=5))
        self.assertEqual(code, EXIT_FAIL)
        self.assertIn("insufficient samples", out)


class RatchetMalformedDataTests(RatchetTestBase):
    def test_malformed_current_json_is_usage_error(self) -> None:
        b = self.write("baseline.json", make_baseline())
        g = self.write("gate.json", make_gate())
        c = self.dir / "current.json"
        c.write_text("{not json", encoding="utf-8")
        out_buf = io.StringIO()
        err_buf = io.StringIO()
        with redirect_stdout(out_buf), redirect_stderr(err_buf):
            code = ratchet.main(["--baseline", str(b), "--current", str(c), "--latency-gate", str(g)])
        self.assertEqual(code, EXIT_USAGE)

    def test_unsupported_schema_version_is_usage_error(self) -> None:
        current = make_receipt()
        current["schema_version"] = 2
        code, _, _ = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)

    def test_missing_required_field_is_usage_error(self) -> None:
        current = make_receipt()
        del current["measurement_environment"]["host_class"]
        code, _, _ = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)

    def test_metrics_not_object_is_usage_error(self) -> None:
        current = make_receipt()
        current["metrics"] = []
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("metrics must be an object", err)

    def test_p50_non_numeric_is_usage_error(self) -> None:
        current = make_receipt()
        current["metrics"]["blast"]["p50_ms"] = "fast"
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be a number", err)

    def test_p50_nan_is_usage_error(self) -> None:
        current = make_receipt()
        current["metrics"]["blast"]["p50_ms"] = float("nan")
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be finite", err)

    def test_p50_negative_is_usage_error(self) -> None:
        current = make_receipt()
        current["metrics"]["blast"]["p50_ms"] = -1.0
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be nonnegative", err)

    def test_runs_non_integer_is_usage_error(self) -> None:
        current = make_receipt()
        current["metrics"]["blast"]["runs"] = "twenty"
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("runs must be an integer", err)

    def test_metric_with_only_p50_is_usage_error(self) -> None:
        current = make_receipt()
        del current["metrics"]["blast"]["p95_ms"]
        code, _, err = self.run_ratchet(make_baseline(), current)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must define both p50_ms and p95_ms or neither", err)

    def test_gate_limit_non_numeric_is_usage_error(self) -> None:
        gate = make_gate()
        gate["thresholds"]["blast"]["p50_max_ms"] = "15"
        code, _, err = self.run_ratchet(make_baseline(), make_receipt(), gate)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be a number", err)

    def test_gate_relative_multiple_below_one_is_usage_error(self) -> None:
        gate = make_gate()
        gate["thresholds"]["blast"]["relative_max_multiple"] = 0.5
        code, _, err = self.run_ratchet(make_baseline(), make_receipt(), gate)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be >= 1.0", err)

    def test_gate_thresholds_not_object_is_usage_error(self) -> None:
        gate = make_gate()
        gate["thresholds"] = {"blast": []}
        code, _, err = self.run_ratchet(make_baseline(), make_receipt(), gate)
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("thresholds.blast must be an object", err)

    def test_policy_min_samples_zero_is_usage_error(self) -> None:
        baseline = make_baseline()
        baseline["policy"]["min_samples"] = 0
        code, _, err = self.run_ratchet(baseline, make_receipt())
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("min_samples must be a positive integer", err)

    def test_policy_multiple_below_one_is_usage_error(self) -> None:
        baseline = make_baseline()
        baseline["policy"]["relative_max_multiple_default"] = 0.9
        code, _, err = self.run_ratchet(baseline, make_receipt())
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be >= 1.0", err)

    def test_policy_not_object_is_usage_error(self) -> None:
        baseline = make_baseline()
        baseline["policy"] = []
        code, _, err = self.run_ratchet(baseline, make_receipt())
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("policy must be an object", err)


class RatchetCliOverrideTests(RatchetTestBase):
    def test_min_samples_zero_via_cli_is_usage_error(self) -> None:
        code, _, err = self.run_ratchet(
            make_baseline(), make_receipt(), extra_args=["--min-samples", "0"]
        )
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("--min-samples must be a positive integer", err)

    def test_relative_multiple_below_one_via_cli_is_usage_error(self) -> None:
        code, _, err = self.run_ratchet(
            make_baseline(), make_receipt(), extra_args=["--relative-multiple", "0.9"]
        )
        self.assertEqual(code, EXIT_USAGE)
        self.assertIn("must be >= 1.0", err)


class RatchetReadOnlyTests(RatchetTestBase):
    def test_ratchet_never_writes_files(self) -> None:
        baseline = self.write("baseline.json", make_baseline())
        current = self.write("current.json", make_receipt())
        gate = self.write("gate.json", make_gate())
        before = {p.name: p.read_bytes() for p in self.dir.iterdir()}
        code, _, _ = self.run_ratchet(make_baseline(), make_receipt())
        self.assertEqual(code, EXIT_PASS)
        self.assertEqual({p.name: p.read_bytes() for p in self.dir.iterdir()}, before)
        self.assertEqual(baseline.read_text(encoding="utf-8"), json.dumps(make_baseline()))
        self.assertEqual(current.read_text(encoding="utf-8"), json.dumps(make_receipt()))
        self.assertEqual(gate.read_text(encoding="utf-8"), json.dumps(make_gate()))


if __name__ == "__main__":
    unittest.main()
