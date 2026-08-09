#!/usr/bin/env python3
"""Acceptance gate for the persistent FSZero raw-worker v2 child path.

The harness measures only the private NDJSON worker boundary.  It does not
measure CodeMode, a model, or an engine catalog.  A receipt is written for
both passing and failing runs so a failed or mutant run cannot be promoted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import selectors
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

SCHEMA = "zerostack.fszero_raw_worker_latency.v1"
PROTOCOL = "zerostack.raw_worker.v2"
ENGINE = "fszero"
DEFAULT_PROTOCOL_DIGEST = "e2daca4d95cbd2780f2e10b30b823e9398747bfe15e38ca0810f634a387aeace"
FIXTURE_NAME = "fixture.bin"
FIXTURE_BYTES = b"abcdef"
DEFAULT_WARMUPS = 100
DEFAULT_MEASURED_REQUESTS = 1000
STAGE_CLOSURE_TOLERANCE_NS = 250_000


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def percentile_ns(values: list[int], quantile: float) -> int:
    """Linear-interpolated percentile, retaining integer nanosecond output."""
    if not values:
        raise ValueError("percentile requires at least one value")
    if not 0 <= quantile <= 1:
        raise ValueError("quantile must be between 0 and 1")
    ordered = sorted(values)
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return int(round(ordered[lower] + (ordered[upper] - ordered[lower]) * fraction))


def summary_ns(values: list[int]) -> dict[str, int | None]:
    if not values:
        return {"p50_ns": None, "p95_ns": None, "p99_ns": None, "mean_ns": None}
    return {
        "p50_ns": percentile_ns(values, 0.50),
        "p95_ns": percentile_ns(values, 0.95),
        "p99_ns": percentile_ns(values, 0.99),
        "mean_ns": int(round(sum(values) / len(values))),
    }


def _hex_digest(value: str, field: str) -> None:
    if len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
        raise ValueError(f"{field} must be 64 lowercase hexadecimal characters")


def _source_facts(source_root: Path | None, source_head: str | None = None) -> dict[str, Any]:
    if source_root is None:
        return {"root": None, "head": source_head, "head_source": "argument" if source_head else None, "dirty": None}
    root = source_root.resolve()
    dirty: bool | None = None
    discovered_head: str | None = None
    try:
        discovered_head = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        ).stdout.strip()
        dirty = bool(
            subprocess.run(
                ["git", "-C", str(root), "status", "--porcelain", "--untracked-files=no"],
                capture_output=True,
                text=True,
                timeout=5,
                check=True,
            ).stdout.strip()
        )
    except (OSError, subprocess.SubprocessError):
        pass
    return {
        "root": str(root),
        "head": source_head or discovered_head,
        "head_source": "argument" if source_head else ("git" if discovered_head else None),
        "dirty": dirty,
    }


def _guess_source_root(binary: Path) -> Path | None:
    for parent in (binary.resolve(), *binary.resolve().parents):
        if (parent / ".git").exists():
            return parent
    return None


def _extract_digest(document: Any) -> str:
    candidates = [
        document.get("package", {}).get("abi_digest") if isinstance(document, dict) else None,
        document.get("semantic_contract_digest") if isinstance(document, dict) else None,
        document.get("abi_digest") if isinstance(document, dict) else None,
    ]
    for candidate in candidates:
        if isinstance(candidate, str):
            _hex_digest(candidate, "ABI digest")
            return candidate
    raise ValueError("capabilities output did not contain package.abi_digest")


def resolve_abi_digest(binary: Path, explicit: str | None) -> tuple[str, dict[str, Any]]:
    """Resolve the expected FSZero ABI without weakening handshake validation."""
    if explicit:
        _hex_digest(explicit, "ABI digest")
        return explicit, {"kind": "argument"}
    from_environment = os.environ.get("FSZERO_ABI_DIGEST")
    if from_environment:
        _hex_digest(from_environment, "ABI digest")
        return from_environment, {"kind": "environment", "name": "FSZERO_ABI_DIGEST"}
    argv = [str(binary), "capabilities", "--json"]
    completed = subprocess.run(argv, capture_output=True, text=True, timeout=10, check=False)
    if completed.returncode != 0:
        raise RuntimeError(f"ABI probe failed ({completed.returncode}): {completed.stderr.strip()}")
    digest = _extract_digest(json.loads(completed.stdout))
    return digest, {"kind": "capabilities_probe", "argv": argv}


def _timeline_evidence(timeline: Any) -> tuple[int | None, dict[str, Any]]:
    if not isinstance(timeline, dict):
        return None, {"present": False, "closed": False, "reason": "missing engine_timeline"}
    total = timeline.get("total_ns")
    spans = timeline.get("spans")
    if not isinstance(total, int) or total <= 0 or not isinstance(spans, list) or not spans:
        return None, {"present": True, "closed": False, "reason": "malformed engine_timeline"}
    duration_sum = 0
    final_end = 0
    prior_end = 0
    for span in spans:
        if not isinstance(span, dict):
            return None, {"present": True, "closed": False, "reason": "malformed stage span"}
        start = span.get("start_ns")
        duration = span.get("duration_ns")
        if not isinstance(start, int) or not isinstance(duration, int) or start < prior_end or duration <= 0:
            return None, {"present": True, "closed": False, "reason": "invalid stage ordering or duration"}
        duration_sum += duration
        final_end = start + duration
        prior_end = final_end
    sum_delta = abs(duration_sum - total)
    end_delta = max(0, final_end - total)
    closed = sum_delta <= STAGE_CLOSURE_TOLERANCE_NS and end_delta <= STAGE_CLOSURE_TOLERANCE_NS
    return total, {
        "present": True,
        "closed": closed,
        "total_ns": total,
        "duration_sum_ns": duration_sum,
        "final_end_ns": final_end,
        "sum_delta_ns": sum_delta,
        "end_delta_ns": end_delta,
        "tolerance_ns": STAGE_CLOSURE_TOLERANCE_NS,
        "spans": spans,
        **({"reason": "timeline does not close"} if not closed else {}),
    }


def _result_value(response: dict[str, Any]) -> dict[str, Any]:
    result = response.get("result")
    if not isinstance(result, dict) or not isinstance(result.get("value"), dict):
        raise ValueError(f"result value is not an object: {response}")
    return result["value"]


def _success_semantics(response: dict[str, Any], request_id: str, expected_ref: str) -> dict[str, Any]:
    if response.get("kind") != "result" or response.get("request_id") != request_id:
        raise ValueError(f"request {request_id} did not return a correlated result")
    value = _result_value(response)
    nested = value.get("value") if isinstance(value.get("value"), dict) else value
    payload = nested.get("payload_utf8") if isinstance(nested, dict) else None
    if payload is None and isinstance(nested, dict):
        payload = nested.get("payload")
    if payload != FIXTURE_BYTES.decode("ascii"):
        raise ValueError(f"request {request_id} payload mismatch: {payload!r}")
    refs = value.get("refs", [])
    if not isinstance(refs, list) or expected_ref not in refs:
        # Some current adapters expose the ref only in the recovered value.
        ref = nested.get("ref") if isinstance(nested, dict) else None
        if ref != expected_ref:
            raise ValueError(f"request {request_id} ref mismatch: {refs!r} / {ref!r}")
    nested_ref = nested.get("ref") if isinstance(nested, dict) else None
    if nested_ref is not None and nested_ref != expected_ref:
        raise ValueError(f"request {request_id} payload ref mismatch: {nested_ref!r}")
    return {"payload_utf8": payload, "ref": expected_ref, "refs": refs}


def _error_semantics(response: dict[str, Any], request_id: str) -> dict[str, Any]:
    if response.get("kind") != "error" or response.get("request_id") != request_id:
        raise ValueError(f"error probe did not return a correlated error: {response}")
    error = response.get("error")
    if not isinstance(error, dict) or error.get("kind") != "not_found" or error.get("retryable") is not False:
        raise ValueError(f"error probe changed semantics: {response}")
    return {"kind": error["kind"], "retryable": error["retryable"], "message": error.get("message", "")}


class RawWorkerClient:
    """One persistent raw-worker process with bounded line reads."""

    def __init__(self, argv: list[str], env: dict[str, str], timeout_seconds: float) -> None:
        self.argv = argv
        self.timeout_seconds = timeout_seconds
        self.process = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        if self.process.stdin is None or self.process.stdout is None or self.process.stderr is None:
            raise RuntimeError("worker pipes unavailable")
        self._selector = selectors.DefaultSelector()
        self._selector.register(self.process.stdout, selectors.EVENT_READ)
        self._buffer = bytearray()
        self.stderr_lines: list[bytes] = []
        self._stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self._stderr_thread.start()

    def _drain_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self.stderr_lines.append(line)

    def stderr_tail(self) -> str:
        return b"".join(self.stderr_lines)[-4000:].decode("utf-8", "replace")

    def _read_frame(self) -> bytes:
        assert self.process.stdout is not None
        deadline = time.monotonic() + self.timeout_seconds
        while True:
            newline = self._buffer.find(b"\n")
            if newline >= 0:
                line = bytes(self._buffer[:newline]).rstrip(b"\r")
                del self._buffer[: newline + 1]
                if line:
                    return line
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"worker response timeout; rc={self.process.poll()} stderr={self.stderr_tail()}")
            if not self._selector.select(remaining):
                raise TimeoutError(f"worker response timeout; rc={self.process.poll()} stderr={self.stderr_tail()}")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError(f"worker exited early rc={self.process.poll()} stderr={self.stderr_tail()}")
            self._buffer.extend(chunk)

    def send(self, frame: dict[str, Any], inject_transport_delay_us: int = 0) -> tuple[dict[str, Any], int, int, int]:
        if self.process.stdin is None:
            raise RuntimeError("worker stdin unavailable")
        encoded = canonical_json(frame)
        wire = encoded + b"\n"
        started = time.monotonic_ns()
        self.process.stdin.write(wire)
        self.process.stdin.flush()
        if inject_transport_delay_us:
            time.sleep(inject_transport_delay_us / 1_000_000)
        response_wire = self._read_frame()
        elapsed = time.monotonic_ns() - started
        response = json.loads(response_wire)
        if not isinstance(response, dict):
            raise ValueError("worker emitted a non-object response")
        return response, elapsed, len(encoded), len(response_wire)

    def shutdown(self) -> dict[str, Any]:
        response, _, _, _ = self.send({"kind": "shutdown", "request": {"reason": "acceptance complete"}})
        if response != {"kind": "shutdown_ack"}:
            raise ValueError(f"shutdown acknowledgement mismatch: {response}")
        try:
            self.process.wait(timeout=self.timeout_seconds)
        except subprocess.TimeoutExpired as error:
            self.process.kill()
            raise RuntimeError("worker did not terminate after shutdown_ack") from error
        if self.process.returncode != 0:
            raise RuntimeError(f"worker exited {self.process.returncode}: {self.stderr_tail()}")
        return {"kind": response["kind"], "returncode": self.process.returncode}

    def close(self) -> None:
        try:
            self._selector.close()
        finally:
            if self.process.poll() is None:
                self.process.kill()
            for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
                if stream is not None:
                    stream.close()


def _handshake_frame(root: Path, session_id: str, abi_digest: str, worker_revision: str | None) -> dict[str, Any]:
    request: dict[str, Any] = {
        "protocol_version": PROTOCOL,
        "root": str(root),
        "session_id": session_id,
        "expected_engine": ENGINE,
        "expected_contract_digest": abi_digest,
        "expected_registry_digest": abi_digest,
    }
    if worker_revision:
        request["expected_worker_revision"] = worker_revision
    return {"kind": "handshake", "request": request}


def _call_frame(request_id: str, root: Path, session_id: str, abi_digest: str, worker_revision: str, path: str) -> dict[str, Any]:
    return {
        "kind": "call",
        "request": {
            "request_id": request_id,
            "op": "fs.read",
            "args": {"path": path},
            "trace": {
                "runtime_id": "zerostack-kw2p",
                "cell_id": request_id,
                "request_id": request_id,
                "trace_id": f"trace-{request_id}",
                "worker_revision": worker_revision,
                "contract_digest": abi_digest,
            },
            "telemetry_request": {"engine_stage_timeline": True, "worker_token_accounting": False},
        },
    }


def _error_frame(request_id: str, abi_digest: str, worker_revision: str) -> dict[str, Any]:
    return {
        "kind": "call",
        "request": {
            "request_id": request_id,
            "op": "fs.read",
            "args": {"path": "missing-fixture.bin"},
            "trace": {
                "runtime_id": "zerostack-kw2p",
                "cell_id": request_id,
                "request_id": request_id,
                "trace_id": f"trace-{request_id}",
                "worker_revision": worker_revision,
                "contract_digest": abi_digest,
            },
        },
    }


def gate_receipt(receipt: dict[str, Any]) -> tuple[bool, list[str]]:
    failures: list[str] = list(receipt.get("failures", []))
    trials = receipt.get("trials", [])
    expected = receipt.get("configuration", {}).get("measured_requests", DEFAULT_MEASURED_REQUESTS)
    if len(trials) != expected:
        failures.append(f"measured request count {len(trials)} != {expected}")
    rtt = receipt.get("statistics", {}).get("rtt_ns", {})
    rtt_p50_failed = isinstance(rtt.get("p50_ns"), (int, float)) and rtt["p50_ns"] > 1_000_000
    rtt_p95_failed = isinstance(rtt.get("p95_ns"), (int, float)) and rtt["p95_ns"] > 2_000_000
    engine = receipt.get("statistics", {}).get("engine_ns", {})
    if rtt.get("p50_ns") is None or rtt_p50_failed:
        failures.append("RTT p50 exceeds 1 ms")
    if rtt.get("p95_ns") is None or rtt_p95_failed:
        failures.append("RTT p95 exceeds 2 ms")
    if engine.get("p95_ns") is None or engine["p95_ns"] > 1_000_000:
        failures.append("engine p95 exceeds 1 ms or telemetry is missing")
    closure = receipt.get("stage_closure", {})
    if closure.get("evidence_count") != expected:
        failures.append("missing engine stage closure evidence")
    if closure.get("closed_count") != expected or (closure.get("max_sum_delta_ns") or 0) > STAGE_CLOSURE_TOLERANCE_NS:
        failures.append("engine stage timeline does not close within 0.25 ms")
    if receipt.get("handshake", {}).get("passed") is not True:
        failures.append("handshake validation failed")
    if receipt.get("shutdown", {}).get("passed") is not True:
        failures.append("shutdown validation failed")
    if receipt.get("semantics", {}).get("passed") is not True:
        failures.append("byte/ref/error semantics failed")
    source = receipt.get("source", {})
    if source.get("dirty") is True:
        failures.append("source checkout is dirty")
    if not source.get("head"):
        failures.append("source head binding is missing")
    binary = receipt.get("binary", {})
    if not binary.get("sha256"):
        failures.append("binary SHA-256 binding is missing")
    if receipt.get("configuration", {}).get("inject_transport_delay_us", 0):
        mutant_caught = rtt_p50_failed or rtt_p95_failed
        receipt["mutant_gate_failure"] = mutant_caught
        if not mutant_caught:
            failures.append("injected transport delay did not trip latency gate")
    return not failures, sorted(set(failures))


def run_benchmark(
    binary: Path,
    *,
    abi_digest: str | None = None,
    protocol_digest: str = DEFAULT_PROTOCOL_DIGEST,
    source_root: Path | None = None,
    source_head: str | None = None,
    warmups: int = DEFAULT_WARMUPS,
    measured_requests: int = DEFAULT_MEASURED_REQUESTS,
    inject_transport_delay_us: int = 0,
    timeout_ms: int = 5000,
    session_id: str | None = None,
) -> dict[str, Any]:
    if warmups < 0 or measured_requests <= 0:
        raise ValueError("warmups must be non-negative and measured_requests must be positive")
    _hex_digest(protocol_digest, "protocol digest")
    binary = binary.resolve()
    if not binary.is_file():
        raise FileNotFoundError(binary)
    expected_abi, abi_source = resolve_abi_digest(binary, abi_digest)
    _hex_digest(expected_abi, "ABI digest")
    # A remote RCH artifact can sit beside a stale checkout. When the caller
    # pins the source head but gives no checkout, do not infer provenance from
    # the binary path; the sync/build binding remains observational only.
    effective_source_root = source_root
    if effective_source_root is None and source_head is None:
        effective_source_root = _guess_source_root(binary)
    source = _source_facts(effective_source_root, source_head)
    session_id = session_id or f"kw2p-{os.getpid()}-{time.time_ns()}"
    receipt: dict[str, Any] = {
        "schema": SCHEMA,
        "protocol": PROTOCOL,
        "engine": ENGINE,
        "run_kind": "mutant" if inject_transport_delay_us else "acceptance",
        "promotable": False,
        "configuration": {
            "warmups": warmups,
            "measured_requests": measured_requests,
            "fixture_name": FIXTURE_NAME,
            "fixture_bytes_hex": FIXTURE_BYTES.hex(),
            "inject_transport_delay_us": inject_transport_delay_us,
            "timeout_ms": timeout_ms,
            "thresholds_ns": {"rtt_p50": 1_000_000, "rtt_p95": 2_000_000, "engine_p95": 1_000_000},
        },
        "source": source,
        "binary": {"path": str(binary), "sha256": sha256_file(binary)},
        "platform": {"system": platform.system(), "release": platform.release(), "machine": platform.machine(), "python": platform.python_version()},
        "argv": [str(binary), "--raw-worker", "--root", "<isolated-workspace>"],
        "abi": {"semantic_contract_digest": expected_abi, "operation_registry_digest": expected_abi, "protocol_digest": protocol_digest, "source": abi_source},
        "fixtures": {"path": FIXTURE_NAME, "bytes_hex": FIXTURE_BYTES.hex(), "sha256": sha256_bytes(FIXTURE_BYTES)},
        "handshake": {"passed": False},
        "shutdown": {"passed": False},
        "semantics": {"passed": False},
        "statistics": {"rtt_ns": summary_ns([]), "engine_ns": summary_ns([])},
        "stage_closure": {"evidence_count": 0, "closed_count": 0, "max_sum_delta_ns": None, "max_end_delta_ns": None},
        "trials": [],
        "failures": [],
        "residual_assumptions": [
            "The binary/source revision binding is observational; no signed provenance is claimed.",
            "The worker stage timeline is engine-reported and does not include unreported kernel work.",
            "Only the private raw-worker v2 child boundary is measured; no model-visible latency claim is made.",
        ],
    }
    client: RawWorkerClient | None = None
    workspace_tmp: tempfile.TemporaryDirectory[str] | None = None
    try:
        workspace_tmp = tempfile.TemporaryDirectory(prefix="zerostack-kw2p-")
        temp_root = Path(workspace_tmp.name)
        workspace = temp_root / "workspace"
        store = temp_root / "store"
        workspace.mkdir()
        store.mkdir()
        (workspace / FIXTURE_NAME).write_bytes(FIXTURE_BYTES)
        env = os.environ.copy()
        env.update({"ZEROSTACK_STORE_ROOT": str(store), "ZEROSTACK_SHARED_STORE": "1", "FSZERO_SHARED_STORE": "1", "ZEROSTACK_SESSION_ID": session_id})
        argv = [str(binary), "--raw-worker", "--root", str(workspace)]
        receipt["argv"] = argv
        receipt["fixtures"].update({"workspace": str(workspace), "store": str(store)})
        client = RawWorkerClient(argv, env, timeout_ms / 1000)
        handshake_request = _handshake_frame(workspace, session_id, expected_abi, None)
        handshake_response, handshake_ns, request_bytes, response_bytes = client.send(handshake_request)
        ack = handshake_response.get("ack") if handshake_response.get("kind") == "handshake_ack" else None
        if not isinstance(ack, dict):
            raise ValueError(f"handshake failed: {handshake_response}")
        binding = ack.get("binding", {})
        checks = {
            "protocol_version": ack.get("protocol_version") == PROTOCOL,
            "protocol_digest": ack.get("protocol_digest") == protocol_digest,
            "engine": binding.get("engine") == ENGINE,
            "root": binding.get("root") == str(workspace),
            "session_id": binding.get("session_id") == session_id,
            "semantic_contract_digest": binding.get("semantic_contract_digest") == expected_abi,
            "operation_registry_digest": binding.get("operation_registry_digest") == expected_abi,
        }
        if not all(checks.values()):
            raise ValueError(f"handshake binding mismatch: {checks}")
        worker_revision = binding.get("worker_revision")
        if not isinstance(worker_revision, str) or not worker_revision:
            raise ValueError("handshake omitted worker revision")
        receipt["handshake"] = {"passed": True, "elapsed_ns": handshake_ns, "request_bytes": request_bytes, "response_bytes": response_bytes, "ack": ack, "checks": checks}
        expected_ref = f"fz://blob/{sha256_bytes(FIXTURE_BYTES)}"
        error_id = "error-probe"
        error_response, error_ns, error_req_bytes, error_resp_bytes = client.send(_error_frame(error_id, expected_abi, worker_revision))
        error_semantics = _error_semantics(error_response, error_id)
        receipt["semantics"]["error_probe"] = {**error_semantics, "elapsed_ns": error_ns, "request_bytes": error_req_bytes, "response_bytes": error_resp_bytes}
        for index in range(warmups):
            request_id = f"warmup-{index + 1}"
            response, _, _, _ = client.send(_call_frame(request_id, workspace, session_id, expected_abi, worker_revision, FIXTURE_NAME))
            _success_semantics(response, request_id, expected_ref)
        engine_values: list[int] = []
        rtt_values: list[int] = []
        closure_records: list[dict[str, Any]] = []
        for index in range(measured_requests):
            request_id = f"read-{index + 1}"
            response, rtt_ns, request_bytes, response_bytes = client.send(_call_frame(request_id, workspace, session_id, expected_abi, worker_revision, FIXTURE_NAME), inject_transport_delay_us)
            semantics = _success_semantics(response, request_id, expected_ref)
            engine_ns, closure = _timeline_evidence(response.get("engine_timeline"))
            if engine_ns is not None:
                engine_values.append(engine_ns)
            if closure.get("present"):
                closure_records.append(closure)
            trial = {"index": index + 1, "request_id": request_id, "rtt_ns": int(rtt_ns), "engine_ns": engine_ns, "request_bytes": int(request_bytes), "response_bytes": int(response_bytes), "semantics": semantics, "engine_timeline": closure}
            receipt["trials"].append(trial)
            rtt_values.append(int(rtt_ns))
        receipt["statistics"] = {"rtt_ns": summary_ns(rtt_values), "engine_ns": summary_ns(engine_values), "request_bytes_total": sum(t["request_bytes"] for t in receipt["trials"]), "response_bytes_total": sum(t["response_bytes"] for t in receipt["trials"])}
        receipt["stage_closure"] = {"evidence_count": len(closure_records), "closed_count": sum(1 for item in closure_records if item.get("closed")), "max_sum_delta_ns": max((item.get("sum_delta_ns", 0) for item in closure_records), default=None), "max_end_delta_ns": max((item.get("end_delta_ns", 0) for item in closure_records), default=None), "tolerance_ns": STAGE_CLOSURE_TOLERANCE_NS}
        receipt["semantics"].update({"passed": True, "success": {"payload_utf8": FIXTURE_BYTES.decode("ascii"), "ref": expected_ref}})
    except Exception as error:  # receipt must survive every failed gate
        receipt["failures"].append(f"runtime: {error}")
    finally:
        if client is not None:
            try:
                if client.process.poll() is None and receipt["handshake"].get("passed"):
                    receipt["shutdown"] = {"passed": True, **client.shutdown()}
            except Exception as error:
                receipt["failures"].append(f"shutdown: {error}")
            finally:
                client.close()
        if workspace_tmp is not None:
            workspace_tmp.cleanup()
    passed, failures = gate_receipt(receipt)
    if source_head is not None and source_root is None:
        receipt["residual_assumptions"].append(
            "RCH sync/build source binding is observational because no source checkout was provided."
        )
    receipt["failures"] = failures
    receipt["passed"] = passed
    receipt["promotable"] = receipt["passed"] and receipt["run_kind"] == "acceptance"
    if inject_transport_delay_us:
        receipt["classification"] = "mutant/non-promotable" if receipt.get("mutant_gate_failure") else "mutant-gate-regression"
    else:
        receipt["classification"] = "promotable" if receipt["promotable"] else "non-promotable"
    return receipt


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(os.environ.get("FSZERO_CODEMODE_BIN", "fszero-codemode")))
    parser.add_argument("--abi-digest", default=None, help="expected FSZero ABI digest; otherwise use FSZERO_ABI_DIGEST or capabilities --json")
    parser.add_argument("--protocol-digest", default=DEFAULT_PROTOCOL_DIGEST)
    parser.add_argument("--source-root", type=Path, default=None, help="source checkout containing the binary")
    parser.add_argument("--source-head", default=None, help="override the source git HEAD when the binary lives on a remote Spark")
    parser.add_argument("--warmups", type=int, default=DEFAULT_WARMUPS)
    parser.add_argument("--measured-requests", type=int, default=DEFAULT_MEASURED_REQUESTS)
    parser.add_argument("--inject-transport-delay-us", type=int, default=0)
    parser.add_argument("--timeout-ms", type=int, default=5000)
    parser.add_argument("--session-id", default=None)
    parser.add_argument("--output", type=Path, default=Path("fszero-raw-worker-receipt.json"))
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        receipt = run_benchmark(args.binary, abi_digest=args.abi_digest, protocol_digest=args.protocol_digest, source_root=args.source_root, source_head=args.source_head, warmups=args.warmups, measured_requests=args.measured_requests, inject_transport_delay_us=args.inject_transport_delay_us, timeout_ms=args.timeout_ms, session_id=args.session_id)
    except Exception as error:
        receipt = {"schema": SCHEMA, "passed": False, "promotable": False, "classification": "non-promotable", "failures": [f"setup: {error}"]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"passed": receipt.get("passed", False), "promotable": receipt.get("promotable", False), "output": str(args.output), "failures": receipt.get("failures", [])}, sort_keys=True))
    return 0 if receipt.get("passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
