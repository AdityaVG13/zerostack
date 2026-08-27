import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("run.py")
spec = importlib.util.spec_from_file_location("rebaseline_run", MODULE_PATH)
run = importlib.util.module_from_spec(spec)
spec.loader.exec_module(run)


class PercentileTests(unittest.TestCase):
    def test_p95_uses_type_7_interpolation_not_max_for_five_samples(self):
        self.assertEqual(run.p95([1, 2, 3, 4, 100]), 80.8)

    def test_percentile_rejects_empty_sample_set(self):
        with self.assertRaisesRegex(ValueError, "at least one"):
            run.p95([])


class VarianceEnvelopeTests(unittest.TestCase):
    def test_cv_and_mad_helpers(self):
        import importlib
        import sys

        sys.path.insert(0, str(MODULE_PATH.parent))
        stats = importlib.import_module("stats")
        tight = [100.0] * 20
        summary = stats.variance_summary(tight)
        self.assertEqual(summary["cv"], 0.0)
        self.assertEqual(summary["cv_pct"], 0.0)
        self.assertEqual(summary["mad"], 0.0)
        self.assertTrue(summary["within_envelope"])
        self.assertEqual(summary["status"], "stable")

        # Wide series: mean 100, high stdev -> CV > 10%.
        wide = [10.0, 20.0, 30.0, 200.0, 250.0] * 4
        wide_summary = stats.variance_summary(wide)
        self.assertIsNotNone(wide_summary["cv"])
        self.assertGreater(wide_summary["cv"], 0.10)
        self.assertFalse(wide_summary["within_envelope"])
        self.assertIn(wide_summary["status"], ("investigate", "escalate"))

    def test_measure_stamps_cv_and_keeps_samples(self):
        times_ms = [1000.0] + [10.0] * 20  # 1 warmup + 20 identical measured
        call_count = {"n": 0}

        def fake_run(command):
            i = call_count["n"]
            call_count["n"] += 1
            return times_ms[i], "", ""

        with patch.object(run, "run", side_effect=fake_run):
            result = run.measure("t", ["noop"], runs=20, warmup=1)

        self.assertEqual(result["samples_ms"], [10.0] * 20)
        self.assertEqual(result["cv"], 0.0)
        self.assertEqual(result["cv_pct"], 0.0)
        self.assertTrue(result["variance_envelope"]["within_envelope"])
        self.assertEqual(result["variance_envelope"]["status"], "stable")
        self.assertEqual(result["variance_envelope"]["envelope_max"], 0.10)

    def test_measure_rejects_high_cv_in_aggregate(self):
        # Measured samples alternate 10 and 50 -> CV well above 10%.
        measured = [10.0, 50.0] * 10
        times_ms = [999.0] + measured
        call_count = {"n": 0}

        def fake_run(command):
            i = call_count["n"]
            call_count["n"] += 1
            return times_ms[i], "", ""

        with patch.object(run, "run", side_effect=fake_run):
            result = run.measure("noisy", ["noop"], runs=20, warmup=1)

        self.assertEqual(result["samples_ms"], measured)
        self.assertFalse(result["variance_envelope"]["within_envelope"])
        metrics = {"noisy": result}
        agg = run.variance_envelope_from_metrics(metrics)
        self.assertFalse(agg["within_envelope"])
        self.assertEqual(agg["status"], "reject")
        self.assertEqual(agg["failed_metrics"], ["noisy"])

    def test_same_host_drift_within_and_outside_envelope(self):
        current = {
            "warm_reindex": {
                "samples_ms": [10.0] * 20,
                "p50_ms": 10.0,
                "variance_envelope": {"within_envelope": True, "status": "stable"},
                "cv": 0.0,
            }
        }
        prior = {
            "date": "2026-01-01T00:00:00+00:00",
            "metrics": {"warm_reindex": {"p50_ms": 10.0}},
        }
        ok = run.same_host_drift_report(current, prior)
        self.assertTrue(ok["available"])
        self.assertTrue(ok["within_envelope"])
        self.assertEqual(ok["status"], "pass")

        prior_far = {
            "date": "2026-01-01T00:00:00+00:00",
            "metrics": {"warm_reindex": {"p50_ms": 5.0}},  # 100% drift
        }
        bad = run.same_host_drift_report(current, prior_far)
        self.assertFalse(bad["within_envelope"])
        self.assertEqual(bad["status"], "reject")
        self.assertEqual(bad["failed_metrics"], ["warm_reindex"])

    def test_no_prior_same_host_is_not_a_reject(self):
        current = {
            "warm_reindex": {
                "samples_ms": [10.0] * 5,
                "p50_ms": 10.0,
            }
        }
        report = run.same_host_drift_report(current, None)
        self.assertFalse(report["available"])
        self.assertTrue(report["within_envelope"])
        self.assertEqual(report["status"], "no_prior")


class WarmupMeasureTests(unittest.TestCase):
    def test_measure_discards_warmup_retains_n(self):
        # First three calls are warmups (discarded); next five are measured.
        times_ms = [1000.0, 900.0, 800.0, 10.0, 11.0, 12.0, 13.0, 14.0]
        call_count = {"n": 0}

        def fake_run(command):
            i = call_count["n"]
            call_count["n"] += 1
            return times_ms[i], "", ""

        with patch.object(run, "run", side_effect=fake_run):
            result = run.measure("t", ["noop"], runs=5, warmup=3)

        self.assertEqual(call_count["n"], 8)
        self.assertEqual(result["runs"], 5)
        self.assertEqual(result["warmup"], 3)
        self.assertEqual(result["samples_ms"], [10.0, 11.0, 12.0, 13.0, 14.0])
        # Warm samples must not affect percentiles.
        self.assertEqual(result["p50_ms"], 12.0)
        self.assertIn("p99_ms", result)
        self.assertEqual(result["p99_label"], "worst_observed_of_n")

    def test_sample_accounting_records_w_and_n(self):
        metrics = {
            "warm_reindex": {
                "label": "warm_reindex",
                "runs": 20,
                "warmup": 1,
                "samples_ms": [1.0] * 20,
            },
            "orient_symbol": {
                "label": "orient_symbol",
                "runs": 20,
                "warmup": 1,
                "samples_ms": [2.0] * 20,
            },
            "blast": {
                "label": "blast",
                "runs": 20,
                "warmup": 1,
                "samples_ms": [3.0] * 20,
            },
            "incremental_update": {
                "status": "not_available",
                "reason": "n/a",
            },
        }
        acc = run.sample_accounting_from_metrics(metrics)
        self.assertEqual(acc["priming_index_runs"], 1)
        self.assertEqual(acc["warmup_discarded"], 1)
        self.assertEqual(acc["measured_retained"], 20)
        self.assertEqual(acc["total_samples"], 60)
        self.assertEqual(acc["measured_total"], 60)
        self.assertEqual(acc["warmup_total"], 3)
        self.assertEqual(acc["dropped_count"], 0)
        self.assertEqual(acc["losses"], [])
        self.assertEqual(acc["per_metric"]["blast"]["measured_retained"], 20)
        self.assertEqual(acc["per_metric"]["blast"]["warmup_discarded"], 1)



class PercentileP99Tests(unittest.TestCase):
    def test_p99_type7_not_max_for_small_n(self):
        # N=20: rank = 1 + 19*0.99 = 19.81 -> interpolate between idx 18 and 19
        # Must not collapse to max (index int(N*0.99)=19).
        import importlib
        import sys
        sys.path.insert(0, str(MODULE_PATH.parent))
        stats = importlib.import_module("stats")
        samples = list(range(1, 21))  # 1..20
        p99 = stats.p99(samples)
        self.assertLess(p99, 20.0)
        self.assertGreater(p99, 19.0)
        self.assertEqual(stats.p99_label(20), "worst_observed_of_n")
        self.assertEqual(stats.p99_label(200), "hyndman_fan_type7")

    def test_measure_publishes_p99_with_small_n_label(self):
        times_ms = [5.0] + [float(i) for i in range(1, 21)]  # 1 warmup + 20 measured
        call_count = {"n": 0}

        def fake_run(command):
            i = call_count["n"]
            call_count["n"] += 1
            return times_ms[i], "", ""

        with patch.object(run, "run", side_effect=fake_run):
            result = run.measure("t", ["noop"], runs=20, warmup=1)

        self.assertEqual(result["runs"], 20)
        self.assertEqual(result["p50_ms"], run.p50(list(range(1, 21)), digits=3))
        self.assertEqual(result["p95_ms"], run.p95(list(range(1, 21)), digits=3))
        self.assertEqual(result["p99_ms"], run.p99(list(range(1, 21)), digits=3))
        self.assertEqual(result["p99_label"], "worst_observed_of_n")

    def test_accounting_rejects_runs_samples_mismatch(self):
        with self.assertRaisesRegex(RuntimeError, "runs="):
            run.sample_accounting_from_metrics(
                {
                    "warm_reindex": {
                        "runs": 20,
                        "warmup": 1,
                        "samples_ms": [1.0] * 5,
                    }
                }
            )




class HostFingerprintTests(unittest.TestCase):
    REQUIRED_KEYS = (
        "platform",
        "os",
        "kernel",
        "machine",
        "cpu",
        "memory",
        "python",
        "rustc",
        "profile",
        "fs_type",
        "store_path",
        "governor",
        "power_mode",
        "load_average",
        "host_class",
        "isolation",
    )

    def test_hardware_emits_full_key_set(self):
        hw = run.hardware()
        for key in self.REQUIRED_KEYS:
            self.assertIn(key, hw, f"fingerprint must include {key} (null ok)")
        self.assertIsInstance(hw["cpu"], str)
        self.assertTrue(hw["cpu"])
        self.assertEqual(hw["store_path"], ".zerostack/graphzero")
        self.assertIn(hw["profile"], ("release", "release-perf"))

    def test_hardware_records_env_isolation_and_nulls_missing_probes(self):
        with patch.object(run, "HOST_CLASS", "test-host-class"):
            with patch.object(run, "ISOLATION", "solo; no concurrent heavy jobs"):
                with patch.object(run, "_cpu_governor", return_value=None):
                    with patch.object(run, "_power_mode", return_value=None):
                        with patch.object(run, "_fs_type_for", return_value=None):
                            hw = run.hardware()
        self.assertEqual(hw["host_class"], "test-host-class")
        self.assertEqual(hw["isolation"], "solo; no concurrent heavy jobs")
        # Missing probes stay explicit null, not omitted.
        self.assertIsNone(hw["governor"])
        self.assertIsNone(hw["power_mode"])
        self.assertIsNone(hw["fs_type"])
        self.assertIn("governor", hw)
        self.assertIn("fs_type", hw)


if __name__ == "__main__":
    unittest.main()


class AttachCliWallSplitTests(unittest.TestCase):
    def test_process_inclusive_split_fields(self):
        metric = {
            "label": "orient_symbol",
            "wall_class": "process_inclusive",
            "p50_ms": 100.0,
            "p95_ms": 120.0,
            "p99_ms": 150.0,
            "samples_ms": [100.0] * 20,
        }
        process_start = {
            "p50_ms": 40.0,
            "p95_ms": 45.0,
            "p99_ms": 50.0,
            "p99_label": "worst_observed_of_n",
            "probe": ["graphzero", "--version"],
            "method": "subprocess_wall_minimal_cli",
            "samples_ms": [40.0] * 20,
        }
        out = run.attach_cli_wall_split(metric, process_start)
        self.assertEqual(out["wall_class"], "process_inclusive")
        self.assertEqual(out["process_start_ms"]["p50_ms"], 40.0)
        self.assertEqual(out["op_ms"]["p50_ms"], 60.0)
        self.assertEqual(out["op_ms"]["p95_ms"], 75.0)
        self.assertEqual(out["op_ms"]["p99_ms"], 100.0)
        self.assertEqual(
            out["op_ms"]["method"], "wall_quantile_minus_process_start_quantile"
        )

    def test_op_ms_clamps_at_zero(self):
        metric = {"p50_ms": 10.0, "p95_ms": 12.0, "p99_ms": 15.0}
        process_start = {
            "p50_ms": 40.0,
            "p95_ms": 45.0,
            "p99_ms": 50.0,
            "probe": ["x"],
            "method": "m",
            "samples_ms": [],
        }
        out = run.attach_cli_wall_split(metric, process_start)
        self.assertEqual(out["op_ms"]["p50_ms"], 0.0)


class HistorySchemaTests(unittest.TestCase):
    """Append-only history schema_version 1 back-compat and row-number errors."""

    def _env_row(self, date: str, *, host_class: str = "h", isolation: str = "i") -> dict:
        return {
            "schema_version": 1,
            "date": date,
            "generated_by": "benchmarks/rebaseline/run.py",
            "binary": "target/release/graphzero",
            "measurement_environment": {
                "host_class": host_class,
                "isolation": isolation,
                "profile": "release",
            },
        }

    def test_retained_old_minimal_row_parses(self):
        # First-generation row: schema_version 1 without the later
        # binary_sha256 / freshness / hardware fields.
        old = self._env_row("2026-07-07T11:54:22.057361+00:00")
        row = run.parse_history_row(json.dumps(old), 3)
        self.assertEqual(row["schema_version"], 1)
        self.assertEqual(row["date"], old["date"])
        self.assertNotIn("binary_sha256", row)
        self.assertNotIn("freshness", row)

    def test_current_extended_row_parses(self):
        current = {
            **self._env_row("2026-08-09T21:00:00+00:00"),
            "binary_sha256": "a" * 64,
            "freshness": {"report_kind": "live_measurement"},
            "hardware": {"platform": "macOS", "machine": "arm64"},
        }
        row = run.parse_history_row(json.dumps(current), 7)
        self.assertEqual(row["schema_version"], 1)
        self.assertEqual(row["binary_sha256"], "a" * 64)
        self.assertEqual(row["freshness"]["report_kind"], "live_measurement")

    def test_malformed_json_reports_row_number(self):
        with self.assertRaisesRegex(run.HistoryRowError, r"row 4:.*malformed JSON"):
            run.parse_history_row('{"schema_version": 1, "broken"', 4)

    def test_structurally_malformed_rows_report_row_number(self):
        # Valid JSON object but missing schema_version.
        with self.assertRaisesRegex(run.HistoryRowError, r"row 2:.*schema_version"):
            run.parse_history_row('{"date": "2026-01-01"}', 2)
        # Valid JSON object with an unsupported schema version.
        with self.assertRaisesRegex(run.HistoryRowError, r"row 9:.*schema_version"):
            run.parse_history_row('{"schema_version": 2}', 9)
        # JSON value that is not an object.
        with self.assertRaisesRegex(run.HistoryRowError, r"row 5:.*JSON object"):
            run.parse_history_row("[1, 2]", 5)

    def test_loader_returns_most_recent_same_host_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "history.jsonl"
            path.write_text(
                json.dumps(self._env_row("2026-01-01")) + "\n"
                + json.dumps(self._env_row("2026-01-02")) + "\n"
                + json.dumps(self._env_row("2026-01-03", host_class="other")) + "\n"
            )
            prior = run.load_prior_same_host_report(
                host_class="h", isolation="i", profile="release", history_path=path
            )
        self.assertIsNotNone(prior)
        self.assertEqual(prior["date"], "2026-01-02")

    def test_loader_surfaces_malformed_row_number(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "history.jsonl"
            path.write_text(
                json.dumps(self._env_row("2026-01-01")) + "\n"
                + json.dumps(self._env_row("2026-01-02")) + "\n"
                + '{"schema_version": 1, "broken"\n'
            )
            with self.assertRaisesRegex(run.HistoryRowError, r"row 3:.*malformed JSON"):
                run.load_prior_same_host_report(
                    host_class="h",
                    isolation="i",
                    profile="release",
                    history_path=path,
                )


class ForeignCorpusTests(unittest.TestCase):
    def test_foreign_corpus_root_is_not_self(self):
        mini = (
            Path(__file__).resolve().parents[1]
            / "foreign_corpora"
            / "fixtures"
            / "rust-mini"
        )
        self.assertTrue(mini.is_dir(), f"missing fixture {mini}")
        files = run.rust_corpus_files(corpus_root=mini)
        self.assertGreaterEqual(len(files), 1)
        self.assertTrue(all(mini in f.parents or f.parent == mini for f in files))
        # Self-repo listing excludes foreign_corpora fixtures
        self_files = run.rust_corpus_files(corpus_root=run.ROOT)
        foreign_under_self = [f for f in self_files if "foreign_corpora" in f.parts]
        self.assertEqual(
            foreign_under_self,
            [],
            "self-repo digest must not include foreign_corpora fixtures",
        )

    def test_pins_registry_has_large_git_pins(self):
        pins = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "foreign_corpora"
                / "pins.json"
            ).read_text()
        )
        ids = {c["id"] for c in pins["corpora"]}
        self.assertIn("rust-regex", ids)
        self.assertIn("ts-typescript", ids)
        for c in pins["corpora"]:
            if c.get("kind") == "git_pin":
                self.assertNotEqual(c.get("rev"), "PLACEHOLDER_REV")
                self.assertEqual(len(c["rev"]), 40)

    def test_foreign_corpus_digest_differs_from_self(self):
        mini = (
            Path(__file__).resolve().parents[1]
            / "foreign_corpora"
            / "fixtures"
            / "rust-mini"
        )
        self_files = run.rust_corpus_files(corpus_root=run.ROOT)
        foreign_files = run.rust_corpus_files(corpus_root=mini)
        self.assertNotEqual(
            {p.resolve() for p in self_files},
            {p.resolve() for p in foreign_files},
        )
