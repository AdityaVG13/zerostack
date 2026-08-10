#!/usr/bin/env python3
"""Paired provider-free Senpi/ZeroStack runtime diagnostic.

This runner never emits a product-speed claim. It records missing workload and
stage evidence explicitly so a partial run cannot become a publishable result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import random
import shutil
import subprocess
import sys
import tempfile
import time
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from queue import Empty, Queue
from threading import Thread
from typing import Any

SCHEMA = "zerostack.senpi_runtime_receipt.v1"
PROTOCOL = "zerostack.senpi_driver.v1"


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")


def digest_value(value: Any) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_text(argv: list[str], cwd: Path | None = None) -> str:
    completed = subprocess.run(argv, cwd=cwd, text=True, capture_output=True, timeout=30, check=False)
    if completed.returncode != 0:
        raise RuntimeError(f"command failed {argv!r}: {completed.stderr.strip()}")
    return completed.stdout.strip()


def git_fact(root: Path) -> dict[str, Any]:
    return {
        "head": run_text(["git", "rev-parse", "HEAD"], root),
        "tracked_dirty": bool(run_text(["git", "status", "--porcelain", "--untracked-files=no"], root)),
    }


def host_facts() -> dict[str, Any]:
    cpu = platform.processor()
    if platform.system() == "Darwin":
        completed = subprocess.run(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True, capture_output=True, timeout=5, check=False
        )
        if completed.returncode == 0 and completed.stdout.strip():
            cpu = completed.stdout.strip()
    return {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "cpu": cpu or "unknown",
        "python": platform.python_version(),
        "node": run_text(["node", "--version"]),
        "network": "unused_after_install",
    }


def parse_ps() -> dict[int, dict[str, Any]]:
    completed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,rss=,%cpu=,state=,command="],
        text=True,
        capture_output=True,
        timeout=10,
        check=False,
    )
    if completed.returncode != 0:
        return {}
    rows: dict[int, dict[str, Any]] = {}
    for raw in completed.stdout.splitlines():
        fields = raw.strip().split(None, 5)
        if len(fields) < 5:
            continue
        try:
            pid = int(fields[0])
            rows[pid] = {
                "pid": pid,
                "ppid": int(fields[1]),
                "rss_bytes": int(fields[2]) * 1024,
                "cpu_percent_text": fields[3],
                "state": fields[4],
                "command": fields[5] if len(fields) > 5 else "",
            }
        except ValueError:
            continue
    return rows


def process_tree(root_pid: int, rows: dict[int, dict[str, Any]]) -> list[int]:
    selected = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, row in rows.items():
            if pid not in selected and row["ppid"] in selected:
                selected.add(pid)
                changed = True
    return sorted(pid for pid in selected if pid in rows)


def fd_count(pid: int) -> int | None:
    proc_fd = Path(f"/proc/{pid}/fd")
    if proc_fd.is_dir():
        try:
            return sum(1 for _ in proc_fd.iterdir())
        except OSError:
            return None
    if shutil.which("lsof") is None:
        return None
    completed = subprocess.run(
        ["lsof", "-nP", "-a", "-p", str(pid), "-Ff"],
        text=True,
        capture_output=True,
        timeout=10,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return sum(1 for line in completed.stdout.splitlines() if line.startswith("f") and line[1:].isdigit())


def thread_count(pid: int) -> int | None:
    proc_task = Path(f"/proc/{pid}/task")
    if proc_task.is_dir():
        try:
            return sum(1 for _ in proc_task.iterdir())
        except OSError:
            return None
    if platform.system() != "Darwin":
        return None
    completed = subprocess.run(
        ["ps", "-M", "-p", str(pid), "-o", "pid="],
        text=True,
        capture_output=True,
        timeout=10,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return sum(1 for line in completed.stdout.splitlines() if line.strip())


def resource_snapshot(arms: dict[str, "JsonLineArm"], phase: str, calls: int | None = None) -> dict[str, Any]:
    rows = parse_ps()
    arm_values: dict[str, Any] = {}
    for name, arm in arms.items():
        pids = process_tree(arm.process.pid, rows)
        processes = []
        for pid in pids:
            row = dict(rows[pid])
            row["fd_count"] = fd_count(pid)
            row["thread_count"] = thread_count(pid)
            processes.append(row)
        arm_values[name] = {
            "root_pid": arm.process.pid,
            "pids": pids,
            "rss_bytes": sum(item["rss_bytes"] for item in processes),
            "fd_count": None if any(item["fd_count"] is None for item in processes) else sum(item["fd_count"] for item in processes),
            "thread_count": None
            if any(item["thread_count"] is None for item in processes)
            else sum(item["thread_count"] for item in processes),
            "processes": processes,
        }
    return {
        "phase": phase,
        "calls": calls,
        "monotonic_ns": time.monotonic_ns(),
        "arms": arm_values,
    }


@dataclass
class JsonLineArm:
    name: str
    process: subprocess.Popen[str]
    stdout_queue: Queue[str | None] = field(init=False, repr=False)
    stderr_lines: deque[str] = field(init=False, repr=False)
    stdout_thread: Thread = field(init=False, repr=False)
    stderr_thread: Thread = field(init=False, repr=False)

    def __post_init__(self) -> None:
        stdout = self.process.stdout
        stderr = self.process.stderr
        if stdout is None or stderr is None:
            raise RuntimeError(f"{self.name} pipes unavailable")
        self.stdout_queue = Queue()
        self.stderr_lines = deque(maxlen=100)

        def drain_stdout() -> None:
            try:
                for line in stdout:
                    self.stdout_queue.put(line)
            finally:
                self.stdout_queue.put(None)

        def drain_stderr() -> None:
            for line in stderr:
                self.stderr_lines.append(line)

        self.stdout_thread = Thread(target=drain_stdout, name=f"{self.name}-stdout", daemon=True)
        self.stderr_thread = Thread(target=drain_stderr, name=f"{self.name}-stderr", daemon=True)
        self.stdout_thread.start()
        self.stderr_thread.start()

    def stderr_tail(self) -> str:
        return "".join(self.stderr_lines)[-4000:]

    def write(self, value: dict[str, Any]) -> None:
        if self.process.stdin is None:
            raise RuntimeError(f"{self.name} stdin unavailable")
        self.process.stdin.write(canonical_bytes(value).decode("utf-8") + "\n")
        self.process.stdin.flush()

    def read(self, timeout_seconds: float = 5.0) -> dict[str, Any]:
        try:
            line = self.stdout_queue.get(timeout=timeout_seconds)
        except Empty as error:
            raise RuntimeError(
                f"{self.name} response timed out after {timeout_seconds}s "
                f"rc={self.process.poll()} stderr={self.stderr_tail()}"
            ) from error
        if line is None:
            raise RuntimeError(
                f"{self.name} exited early rc={self.process.poll()} stderr={self.stderr_tail()}"
            )
        value = json.loads(line)
        if not isinstance(value, dict):
            raise RuntimeError(f"{self.name} emitted non-object frame")
        return value

    def shutdown(self, request_id: int) -> dict[str, Any]:
        started = time.monotonic_ns()
        self.write({"type": "shutdown", "id": request_id})
        response = self.read()
        outer_ns = time.monotonic_ns() - started
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired as error:
            self.process.kill()
            raise RuntimeError(f"{self.name} did not terminate") from error
        self.stdout_thread.join(timeout=1)
        self.stderr_thread.join(timeout=1)
        stderr = self.stderr_tail()
        if self.process.returncode != 0 or stderr.strip():
            raise RuntimeError(f"{self.name} shutdown rc={self.process.returncode} stderr={stderr}")
        return {"outer_ns": outer_ns, "response": response, "returncode": self.process.returncode}


class SenpiArm(JsonLineArm):
    def call(self, request_id: int, workload: dict[str, Any], delegate_value: str) -> tuple[int, dict[str, Any]]:
        frame = {
            "type": "execute",
            "id": request_id,
            "cellId": f"senpi-{request_id}",
            "source": workload["senpi_source"],
            "timeoutMs": 2_000,
            "delegateValue": delegate_value,
            "expectedDelegateCalls": workload["delegate_calls"],
        }
        started = time.monotonic_ns()
        self.write(frame)
        response = self.read()
        outer_ns = time.monotonic_ns() - started
        if response.get("type") != "response" or response.get("id") != request_id or response.get("ok") is not True:
            raise RuntimeError(f"Senpi call failed: {response}")
        return outer_ns, response


class ZeroStackArm(JsonLineArm):
    def call(self, request_id: int, workload: dict[str, Any], delegate_value: str) -> tuple[int, dict[str, Any]]:
        started = time.monotonic_ns()
        self.write(
            {
                "type": "execute",
                "id": request_id,
                "cell_id": f"zerostack-{request_id}",
                "source": workload["zerostack_source"],
                "yield_ms": 0,
                "timeout_ms": 2_000,
            }
        )
        delegate_calls = 0
        while True:
            response = self.read()
            if response.get("type") == "delegate_request":
                delegate_calls += 1
                self.write(
                    {
                        "type": "delegate_response",
                        "delegate_id": response.get("delegate_id"),
                        "ok": True,
                        "result": delegate_value,
                    }
                )
                continue
            outer_ns = time.monotonic_ns() - started
            if response.get("type") != "response" or response.get("id") != request_id:
                raise RuntimeError(f"ZeroStack emitted uncorrelated response: {response}")
            if response.get("ok") is not True or response.get("errorText") is not None:
                raise RuntimeError(f"ZeroStack call failed: {response}")
            if delegate_calls != workload["delegate_calls"]:
                raise RuntimeError(f"ZeroStack delegate count {delegate_calls} != {workload['delegate_calls']}")
            response["delegateCalls"] = delegate_calls
            return outer_ns, response


def normalized_result(arm: str, response: dict[str, Any]) -> Any:
    if arm == "senpi":
        result = response.get("result")
        if not isinstance(result, dict) or result.get("ok") is not True:
            raise RuntimeError(f"invalid Senpi result: {result}")
        text = result.get("valueRepr")
        if text is None:
            return None
    else:
        items = response.get("contentItems")
        if not isinstance(items, list) or len(items) != 1 or not isinstance(items[0], dict):
            raise RuntimeError(f"invalid ZeroStack result items: {items}")
        text = items[0].get("text")
    if not isinstance(text, str):
        raise RuntimeError("result text is not a string")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def expected_result(workload: dict[str, Any], delegate_value: str) -> Any:
    match workload["expected"]:
        case "one":
            return 1
        case "payload":
            return delegate_value
        case "payload_array":
            return [delegate_value] * workload["delegate_calls"]
        case other:
            raise RuntimeError(f"unknown expected result {other}")


def percentile(values: list[int], numerator: int, denominator: int) -> int:
    if not values:
        raise ValueError("percentile requires samples")
    ordered = sorted(values)
    index = max(0, (len(ordered) * numerator + denominator - 1) // denominator - 1)
    return ordered[index]


def summaries(samples: list[dict[str, Any]]) -> dict[str, Any]:
    groups: dict[tuple[str, str], list[dict[str, Any]]] = {}
    for sample in samples:
        groups.setdefault((sample["arm"], sample["workload"]), []).append(sample)
    result: dict[str, Any] = {}
    for (arm, workload), rows in sorted(groups.items()):
        outer = [row["outer_ns"] for row in rows]
        inner = [row["inner_ns"] for row in rows if row["inner_ns"] is not None]
        result[f"{arm}:{workload}"] = {
            "count": len(rows),
            "outer_ns": {
                "min": min(outer),
                "p50": percentile(outer, 50, 100),
                "p95": percentile(outer, 95, 100),
                "p99": percentile(outer, 99, 100),
                "max": max(outer),
            },
            "inner_ns": None
            if len(inner) != len(rows)
            else {
                "min": min(inner),
                "p50": percentile(inner, 50, 100),
                "p95": percentile(inner, 95, 100),
                "p99": percentile(inner, 99, 100),
                "max": max(inner),
            },
            "independent_stage_closure": False,
        }
    return result


def start_arms(args: argparse.Namespace, scratch: Path) -> dict[str, JsonLineArm]:
    isolated_home = scratch / "home"
    fixture = scratch / "fixture"
    store = scratch / "store"
    cache = scratch / "cache"
    for path in [isolated_home, fixture, store, cache, scratch / "tmp"]:
        path.mkdir(parents=True, exist_ok=True)
    inherited_environment = ("PATH", "LANG", "LC_ALL", "TZ", "SYSTEMROOT", "WINDIR")
    environment = {key: os.environ[key] for key in inherited_environment if key in os.environ}
    environment.update(
        {
            "HOME": str(isolated_home),
            "XDG_CONFIG_HOME": str(isolated_home / "config"),
            "XDG_DATA_HOME": str(isolated_home / "data"),
            "XDG_CACHE_HOME": str(cache),
            "TMPDIR": str(scratch / "tmp"),
            "ZEROSTACK_STORE_ROOT": str(store),
        }
    )
    tsx = args.senpi_root / "node_modules/.bin/tsx"
    context_manager = args.senpi_root / "packages/senpi-codemode/src/kernels/js/context-manager.ts"
    if not tsx.is_file() or not context_manager.is_file():
        raise RuntimeError("Senpi must be built after `npm ci --ignore-scripts && npm run build`")
    senpi_process = subprocess.Popen(
        [str(tsx), str(args.driver), str(context_manager), str(fixture)],
        cwd=args.senpi_root,
        env=environment,
        text=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=1,
    )
    zero_process = subprocess.Popen(
        [str(args.zerostack_host)],
        cwd=fixture,
        env=environment,
        text=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=1,
    )
    arms: dict[str, JsonLineArm] = {
        "senpi": SenpiArm("senpi", senpi_process),
        "zerostack": ZeroStackArm("zerostack", zero_process),
    }
    senpi_ready = arms["senpi"].read()
    zero_ready = arms["zerostack"].read()
    if senpi_ready.get("protocol") != PROTOCOL:
        raise RuntimeError(f"unexpected Senpi ready frame: {senpi_ready}")
    if zero_ready.get("protocol") != "zerostack-codemode-host/v1":
        raise RuntimeError(f"unexpected ZeroStack ready frame: {zero_ready}")
    return arms


def comparison_identity(config: dict[str, Any], facts: dict[str, Any]) -> tuple[dict[str, Any], dict[str, str]]:
    sources = dict(config["comparison"])
    sources["hardware_network_class"] = facts["host"]
    sources["assembly_manifest"] = {
        "senpi": facts["senpi"],
        "zerostack": facts["zerostack"],
        "runner_sha256": facts["runner_sha256"],
        "driver_sha256": facts["driver_sha256"],
        "config_sha256": facts["config_sha256"],
    }
    digests = {f"{name}_digest": digest_value({"domain": f"ymp3:{name}:v1", "value": value}) for name, value in sources.items()}
    return sources, digests


def repository_root(start: Path) -> Path:
    for candidate in [start, *start.parents]:
        if (candidate / ".git").exists():
            return candidate
    raise RuntimeError("could not locate ZeroStack repository root")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--senpi-root", type=Path, required=True)
    parser.add_argument("--zerostack-host", type=Path, required=True)
    parser.add_argument("--zerostack-root", type=Path, default=repository_root(Path(__file__).resolve()))
    parser.add_argument(
        "--zerostack-revision",
        help="expected immutable ZeroStack revision; defaults to identity.json",
    )
    parser.add_argument("--identity", type=Path, default=Path(__file__).with_name("identity.json"))
    parser.add_argument("--driver", type=Path, default=Path(__file__).with_name("senpi-driver.ts"))
    parser.add_argument("--profile", choices=["quick", "full"], default="quick")
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    run_started_unix_ns = time.time_ns()
    args = parse_args()
    args.senpi_root = args.senpi_root.resolve()
    args.zerostack_host = args.zerostack_host.resolve()
    args.zerostack_root = args.zerostack_root.resolve()
    args.identity = args.identity.resolve()
    args.driver = args.driver.resolve()
    for path in [args.senpi_root, args.zerostack_root]:
        if not path.is_dir():
            raise RuntimeError(f"missing directory: {path}")
    for path in [args.zerostack_host, args.identity, args.driver]:
        if not path.is_file():
            raise RuntimeError(f"missing file: {path}")
    config = json.loads(args.identity.read_text())
    profile = config["profiles"][args.profile]
    senpi_fact = git_fact(args.senpi_root)
    if senpi_fact["head"] != config["comparison"]["assembly_manifest"]["senpi_revision"]:
        raise RuntimeError("Senpi revision does not match the frozen identity")
    expected_zero_revision = (
        args.zerostack_revision
        or config["comparison"]["assembly_manifest"]["zerostack_revision"]
    )
    if zero_fact["head"] != expected_zero_revision:
        raise RuntimeError("ZeroStack revision does not match the expected identity")
    facts = {
        "host": host_facts(),
        "senpi": senpi_fact,
        "zerostack": {
            **zero_fact,
            "binary_sha256": digest_file(args.zerostack_host),
            "binary_version": run_text([str(args.zerostack_host), "--version"]),
        },
        "runner_sha256": digest_file(Path(__file__)),
        "driver_sha256": digest_file(args.driver),
        "config_sha256": digest_file(args.identity),
    }
    identity_sources, identity_digests = comparison_identity(config, facts)
    delegate_value = "x" * config["comparison"]["tool_authority_sandbox"]["delegate_payload_bytes"]
    workloads = {row["id"]: row for row in config["workloads"]}
    scratch_owner = tempfile.TemporaryDirectory(prefix="ymp3-")
    scratch = Path(scratch_owner.name)
    arms = start_arms(args, scratch)
    samples: list[dict[str, Any]] = []
    resources: list[dict[str, Any]] = []
    output_digests: dict[str, dict[str, str]] = {name: {} for name in arms}
    request_id = 1
    try:
        resources.append(resource_snapshot(arms, "cold_ready"))
        for workload in workloads.values():
            expected_digest = digest_value(expected_result(workload, delegate_value))
            for arm_name, arm in arms.items():
                for _ in range(profile["warmups"]):
                    _, response = arm.call(request_id, workload, delegate_value)  # type: ignore[attr-defined]
                    request_id += 1
                    actual_digest = digest_value(normalized_result(arm_name, response))
                    if actual_digest != expected_digest:
                        raise RuntimeError(f"warm result mismatch {arm_name}:{workload['id']}")
                output_digests[arm_name][workload["id"]] = expected_digest
        resources.append(resource_snapshot(arms, "warm"))
        slots = [
            (arm_name, workload_id, trial)
            for workload_id in workloads
            for trial in range(profile["samples_per_arm_workload"])
            for arm_name in arms
        ]
        random.Random(config["comparison"]["decoder_seed_policy"]["seed"]).shuffle(slots)
        for arm_name, workload_id, trial in slots:
            arm = arms[arm_name]
            workload = workloads[workload_id]
            outer_ns, response = arm.call(request_id, workload, delegate_value)  # type: ignore[attr-defined]
            request_id += 1
            actual = normalized_result(arm_name, response)
            expected = expected_result(workload, delegate_value)
            if digest_value(actual) != digest_value(expected):
                raise RuntimeError(f"result mismatch {arm_name}:{workload_id}:{trial}")
            inner_ns = response.get("runNs") if arm_name == "senpi" else None
            samples.append(
                {
                    "arm": arm_name,
                    "workload": workload_id,
                    "trial": trial,
                    "outer_ns": outer_ns,
                    "inner_ns": inner_ns if isinstance(inner_ns, int) else None,
                    "delegate_calls": response.get("delegateCalls"),
                }
            )
        resources.append(resource_snapshot(arms, "measured"))
        stress_workload = workloads["exact_read_1k"]
        stress_calls = profile["stress_calls_per_arm"]
        checkpoints = {0, max(1, stress_calls // 10), stress_calls}
        for arm_name, arm in arms.items():
            resources.append(resource_snapshot({arm_name: arm}, "stress", 0))
            for count in range(1, stress_calls + 1):
                _, response = arm.call(request_id, stress_workload, delegate_value)  # type: ignore[attr-defined]
                request_id += 1
                if digest_value(normalized_result(arm_name, response)) != output_digests[arm_name]["exact_read_1k"]:
                    raise RuntimeError(f"stress result mismatch {arm_name}:{count}")
                if count in checkpoints:
                    resources.append(resource_snapshot({arm_name: arm}, "stress", count))
        idle_seconds = profile["idle_seconds"]
        resources.append(resource_snapshot(arms, "idle", 0))
        if idle_seconds > 0:
            interval = profile["resource_interval_seconds"]
            deadline = time.monotonic() + idle_seconds
            elapsed = 0
            while time.monotonic() < deadline:
                time.sleep(min(interval, max(0.0, deadline - time.monotonic())))
                elapsed = min(idle_seconds, elapsed + interval)
                resources.append(resource_snapshot(arms, "idle", elapsed))
        teardown = {}
        for arm_name in ["senpi", "zerostack"]:
            teardown[arm_name] = arms[arm_name].shutdown(request_id)
            request_id += 1
    finally:
        for arm in arms.values():
            if arm.process.poll() is None:
                arm.process.kill()
                arm.process.wait(timeout=5)
        scratch_owner.cleanup()
    warnings = [
        "diagnostic only: journaled edit, cancellation, and 1MiB finalization workloads are not implemented",
        "diagnostic only: independent nanosecond stage hooks are missing, so <=0.25ms closure is unverified",
        "diagnostic only: subjective UI, tool-card paint, and model-visible settlement are not measured",
        "diagnostic only: ps snapshots do not establish calibrated wakeups, idle CPU average, or idle CPU p99",
        "diagnostic only: the ZeroStack binary hash has no signed source-revision binding",
    ]
    if facts["zerostack"]["tracked_dirty"]:
        warnings.append("ZeroStack source tree has tracked dirt")
    if facts["senpi"]["tracked_dirty"]:
        warnings.append("Senpi source tree has tracked dirt")
    receipt: dict[str, Any] = {
        "schema": SCHEMA,
        "status": "diagnostic_incomplete",
        "publishable": False,
        "profile": args.profile,
        "run_started_unix_ns": run_started_unix_ns,
        "run_finished_unix_ns": time.time_ns(),
        "command": [sys.executable, *sys.argv],
        "facts": facts,
        "comparison_identity_sources": identity_sources,
        "comparison_identity_digests": identity_digests,
        "workloads": config["workloads"],
        "missing_workloads": config["required_but_unimplemented_workloads"],
        "samples": samples,
        "summaries": summaries(samples),
        "resources": resources,
        "teardown": teardown,
        "normalized_output_digests": output_digests,
        "warnings": warnings,
    }
    receipt["receipt_digest"] = digest_value(receipt)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(canonical_bytes(receipt) + b"\n")
    print(json.dumps({"status": receipt["status"], "output": str(args.output), "receipt_digest": receipt["receipt_digest"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
