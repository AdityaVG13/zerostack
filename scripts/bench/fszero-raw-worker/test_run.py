#!/usr/bin/env python3
"""Targeted unit tests for the raw-worker v2 acceptance gate."""

from __future__ import annotations

import importlib.util
import json
import os
import stat
import tempfile
import textwrap
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("fszero_raw_worker_run", HERE / "run.py")
assert SPEC is not None and SPEC.loader is not None
run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(run)

ABI = "b" * 64
REF = f"fz://blob/{run.sha256_bytes(run.FIXTURE_BYTES)}"
FAKE_WORKER = textwrap.dedent(
    f"""\
    #!/usr/bin/env python3
    import hashlib
    import json
    import os
    import sys

    ABI = {ABI!r}
    REF = {REF!r}
    PROTOCOL = {run.PROTOCOL!r}
    DIGEST = {run.DEFAULT_PROTOCOL_DIGEST!r}
    PAYLOAD = {run.FIXTURE_BYTES.decode()!r}
    root = None
    for index, argument in enumerate(sys.argv):
        if argument == "--root" and index + 1 < len(sys.argv):
            root = sys.argv[index + 1]

    def emit(value):
        sys.stdout.write(json.dumps(value, separators=(",", ":"), sort_keys=True) + "\\n")
        sys.stdout.flush()

    for line in sys.stdin:
        frame = json.loads(line)
        kind = frame.get("kind")
        if kind == "handshake":
            request = frame["request"]
            root = request["root"]
            session = request["session_id"]
            emit({{
                "kind": "handshake_ack",
                "ack": {{
                    "protocol_version": PROTOCOL,
                    "binding": {{
                        "engine": "fszero", "root": root, "session_id": session,
                        "worker_revision": "fake-revision",
                        "semantic_contract_version": "1.1.0",
                        "semantic_contract_digest": ABI,
                        "operation_registry_digest": ABI,
                        "ref_scheme": "fz://"
                    }},
                    "capabilities": {{"cancellation": False, "deadlines": True, "approvals": False, "revert": True, "snapshots": True}},
                    "limits": {{"max_frame_bytes": 1048576, "max_output_bytes": 65536, "max_in_flight": 1, "default_deadline_ms": 30000}},
                    "protocol_digest": DIGEST
                }}
            }})
        elif kind == "call":
            request = frame["request"]
            request_id = request["request_id"]
            if request["args"].get("path") == "missing-fixture.bin":
                emit({{
                    "kind": "error", "request_id": request_id,
                    "error": {{"kind": "not_found", "message": "not found", "retryable": False}}
                }})
                continue
            value = {{
                "operation": "fs.read", "ok": True, "mutated": False,
                "value": {{"ref": REF, "payload_utf8": PAYLOAD}}, "refs": [REF]
            }}
            response = {{
                "kind": "result", "request_id": request_id,
                "result": {{"value": value, "metadata": {{}}}}
            }}
            if request.get("telemetry_request", {{}}).get("engine_stage_timeline"):
                response["engine_timeline"] = {{"total_ns": 1000, "spans": [{{"stage": "fake.engine", "start_ns": 0, "duration_ns": 1000}}]}}
            emit(response)
        elif kind == "shutdown":
            emit({{"kind": "shutdown_ack"}})
            break
        else:
            raise SystemExit("unexpected frame")
    """
)


class RawWorkerGateTests(unittest.TestCase):
    def fake_binary(self, directory: Path) -> Path:
        path = directory / "fake-fszero-codemode"
        path.write_text(FAKE_WORKER, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def test_fake_worker_acceptance_records_exact_trials_and_semantics(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            binary = self.fake_binary(Path(temporary))
            receipt = run.run_benchmark(
                binary,
                abi_digest=ABI,
                source_head="a" * 40,
                warmups=2,
                measured_requests=4,
            )

        self.assertTrue(receipt["passed"], receipt["failures"])
        self.assertTrue(receipt["promotable"])
        self.assertEqual(receipt["source"]["root"], None)
        self.assertIsNone(receipt["source"]["dirty"])
        self.assertEqual(receipt["source"]["head_source"], "argument")
        self.assertTrue(any("RCH sync/build" in item for item in receipt["residual_assumptions"]))
        self.assertEqual(receipt["classification"], "promotable")
        self.assertEqual(len(receipt["trials"]), 4)
        self.assertEqual(receipt["configuration"]["fixture_bytes_hex"], "616263646566")
        self.assertTrue(receipt["handshake"]["passed"])
        self.assertTrue(receipt["shutdown"]["passed"])
        self.assertTrue(receipt["semantics"]["passed"])
        self.assertEqual(receipt["semantics"]["success"]["ref"], REF)
        self.assertEqual(receipt["semantics"]["error_probe"]["kind"], "not_found")
        self.assertEqual(receipt["stage_closure"]["evidence_count"], 4)
        self.assertEqual(receipt["stage_closure"]["closed_count"], 4)
        self.assertIn("--raw-worker", receipt["argv"])

    def test_injected_transport_delay_is_a_failed_non_promotable_mutant(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            binary = self.fake_binary(Path(temporary))
            receipt = run.run_benchmark(
                binary,
                abi_digest=ABI,
                source_head="a" * 40,
                warmups=0,
                measured_requests=3,
                inject_transport_delay_us=1_000,
            )

        self.assertFalse(receipt["passed"])
        self.assertFalse(receipt["promotable"])
        self.assertEqual(receipt["classification"], "mutant/non-promotable")
        self.assertTrue(receipt["mutant_gate_failure"])
        self.assertTrue(any("RTT" in failure for failure in receipt["failures"]))

    def test_injected_delay_that_does_not_cross_latency_gate_is_regression(self) -> None:
        receipt = {
            "configuration": {"measured_requests": 1, "inject_transport_delay_us": 1000},
            "trials": [{}],
            "statistics": {
                "rtt_ns": {"p50_ns": 1, "p95_ns": 1},
                "engine_ns": {"p95_ns": 1},
            },
            "stage_closure": {"evidence_count": 1, "closed_count": 1, "max_sum_delta_ns": 0},
            "handshake": {"passed": True},
            "shutdown": {"passed": True},
            "semantics": {"passed": True},
            "source": {"head": "a" * 40, "dirty": False},
            "binary": {"sha256": "b" * 64},
        }
        passed, failures = run.gate_receipt(receipt)
        self.assertFalse(passed)
        self.assertFalse(receipt["mutant_gate_failure"])
        self.assertIn("injected transport delay did not trip latency gate", failures)

    def test_gate_rejects_missing_stage_evidence(self) -> None:
        receipt = {
            "configuration": {"measured_requests": 1, "inject_transport_delay_us": 0},
            "trials": [{}],
            "statistics": {
                "rtt_ns": {"p50_ns": 1, "p95_ns": 1},
                "engine_ns": {"p95_ns": 1},
            },
            "stage_closure": {"evidence_count": 0, "closed_count": 0},
            "handshake": {"passed": True},
            "shutdown": {"passed": True},
            "semantics": {"passed": True},
            "source": {"head": "a" * 40},
            "binary": {"sha256": "b" * 64},
        }
        passed, failures = run.gate_receipt(receipt)
        self.assertFalse(passed)
        self.assertIn("missing engine stage closure evidence", failures)

    def test_dirty_source_checkout_is_not_promotable(self) -> None:
        receipt = {
            "configuration": {"measured_requests": 1, "inject_transport_delay_us": 0},
            "trials": [{}],
            "statistics": {
                "rtt_ns": {"p50_ns": 1, "p95_ns": 1},
                "engine_ns": {"p95_ns": 1},
            },
            "stage_closure": {"evidence_count": 1, "closed_count": 1, "max_sum_delta_ns": 0},
            "handshake": {"passed": True},
            "shutdown": {"passed": True},
            "semantics": {"passed": True},
            "source": {"root": "/tmp/source", "head": "a" * 40, "dirty": True},
            "binary": {"sha256": "b" * 64},
        }
        passed, failures = run.gate_receipt(receipt)
        self.assertFalse(passed)
        self.assertIn("source checkout is dirty", failures)

    def test_percentiles_are_integer_nanoseconds(self) -> None:
        self.assertEqual(run.percentile_ns([1, 2, 3, 4], 0.50), 2)
        self.assertEqual(run.percentile_ns([1, 2, 3, 4], 0.95), 4)
        self.assertIsInstance(run.summary_ns([1, 2, 3])["mean_ns"], int)


if __name__ == "__main__":
    unittest.main()
