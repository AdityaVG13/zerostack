#!/usr/bin/env python3
"""Frozen shipped-binary E2E comparison for FSZero revisions.

The harness builds each revision in a detached worktree, copies the shipped
process-exclusive artifacts to a checksum-locked directory, generates one
immutable corpus, and runs identical matrix payloads against baseline and
candidate. Failed operations never contribute timing summaries.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import selectors
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

ROOT = Path(__file__).resolve().parent.parent
GEN_CORPUS = ROOT / "benchmarks" / "gen_corpus.py"
DEFAULT_BASELINE = "36a23a8"
DEFAULT_CANDIDATE = "15436a6"
REQUIRED_OPERATIONS = (
    "read", "search", "list", "write", "edit", "world", "history",
    "undo", "ast", "recovery", "ref_expand",
)
SURFACES = ("cli", "mcp", "codemode")
TEMPERATURES = ("cold", "warm")
INVOCATIONS = ("single", "batch")
PERSISTENT_SURFACES = ("mcp", "codemode")
DEFAULT_BATCH_WIDTH = 4
SCHEMA = "fszero-shipped-e2e/v1"


def canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_checked(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> str:
    proc = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed ({proc.returncode}): {command!r}\n"
            f"stdout={proc.stdout[-2000:]}\nstderr={proc.stderr[-2000:]}"
        )
    return proc.stdout.strip()


def operation_payload(operation: str, index: int, *, warmup: bool = False) -> dict[str, Any]:
    lane = "warmup" if warmup else "measure"
    suffix = f"{lane}_{index}"
    if operation == "read":
        return {"method": "fs.read", "args": {"path": "mod_000/sub_000/f_000.rs"}}
    if operation == "search":
        return {"method": "fs.search", "args": {"query": "wrapping_add(3141)"}}
    if operation == "list":
        return {"method": "fs.ls", "args": {"path": "mod_000/sub_000"}}
    if operation == "write":
        return {"method": "fs.write", "args": {
            "path": f"bench/write_{suffix}.txt", "content": f"written-{suffix}\n",
        }}
    if operation == "edit":
        return {"method": "fs.edit", "args": {
            "spec": f"bench/edit_{suffix}.txt:alpha-{suffix}|beta-{suffix}",
        }}
    if operation == "world":
        return {"method": "fs.world", "args": {
            "arg": f"new:bench/world_{suffix}.txt:before-{suffix}|after-{suffix}",
        }}
    if operation == "history":
        return {"method": "fs.history", "args": {"arg": f"bench/history_{suffix}.txt|10"}}
    if operation == "undo":
        return {"method": "fs.undo", "args": {"arg": f"bench/undo_{suffix}.txt"}}
    if operation == "ast":
        return {"method": "fs.search", "args": {
            "query": "ast-sgrep:fn $NAME($$$ARGS) { $$$BODY }",
        }}
    if operation in ("recovery", "ref_expand"):
        return {"method": "fs.expand", "args": {"ref": "read"}}
    raise ValueError(f"unknown operation: {operation}")


def setup_payloads(operation: str, width: int, *, warmup: bool = False) -> list[dict[str, Any]]:
    lane = "warmup" if warmup else "measure"
    if operation in ("history", "undo"):
        result: list[dict[str, Any]] = []
        for index in range(width):
            path = f"bench/{operation}_{lane}_{index}.txt"
            result.extend([
                {"method": "fs.write", "args": {"path": path, "content": f"v0-{lane}-{index}\n"}},
                {"method": "fs.write", "args": {"path": path, "content": f"v1-{lane}-{index}\n"}},
            ])
        return result
    if operation in ("recovery", "ref_expand"):
        return [{"method": "fs.read", "args": {"path": "mod_000/sub_000/f_000.rs"}}]
    return []


def cell_payload(operation: str, invocation: str, batch_width: int) -> dict[str, Any]:
    width = 1 if invocation == "single" else batch_width
    return {
        "setup": setup_payloads(operation, width),
        "warmup_setup": setup_payloads(operation, width, warmup=True),
        "warmup": [operation_payload(operation, i, warmup=True) for i in range(width)],
        "measure": [operation_payload(operation, i) for i in range(width)],
        "batch_semantics": "one request" if width == 1 else "one pipelined transport burst/one CodeMode plan",
    }


def build_matrix(trials: int, batch_width: int) -> dict[str, Any]:
    if trials < 3:
        raise ValueError("benchmark integrity requires at least 3 trials")
    if batch_width < 2:
        raise ValueError("batch width must be at least 2")
    cells: list[dict[str, Any]] = []
    for operation in REQUIRED_OPERATIONS:
        for surface in SURFACES:
            for temperature in TEMPERATURES:
                for invocation in INVOCATIONS:
                    payload = cell_payload(operation, invocation, batch_width)
                    cell_id = "/".join((operation, surface, temperature, invocation))
                    cells.append({
                        "id": cell_id,
                        "operation": operation,
                        "surface": surface,
                        "temperature": temperature,
                        "invocation": invocation,
                        "trials": trials,
                        "required_if_surface_available": True,
                        "payload": payload,
                        "payload_sha256": sha256_bytes(canonical_json(payload).encode()),
                    })
    matrix = {
        "schema": SCHEMA,
        "operations": list(REQUIRED_OPERATIONS),
        "surfaces": list(SURFACES),
        "temperatures": list(TEMPERATURES),
        "invocations": list(INVOCATIONS),
        "trials": trials,
        "batch_width": batch_width,
        "cells": cells,
        "warm_idle": {
            "surfaces": list(PERSISTENT_SURFACES),
            "trials": trials,
            "workload": operation_payload("read", 0),
        },
    }
    validate_matrix(matrix)
    matrix["matrix_sha256"] = sha256_bytes(canonical_json(matrix).encode())
    return matrix


def validate_matrix(matrix: dict[str, Any]) -> None:
    expected = {
        "/".join((operation, surface, temperature, invocation))
        for operation in REQUIRED_OPERATIONS
        for surface in SURFACES
        for temperature in TEMPERATURES
        for invocation in INVOCATIONS
    }
    cells = matrix.get("cells", [])
    ids = [cell.get("id") for cell in cells]
    if len(ids) != len(set(ids)):
        raise ValueError("matrix contains duplicate cells")
    missing = sorted(expected - set(ids))
    extra = sorted(set(ids) - expected)
    if missing or extra:
        raise ValueError(f"matrix mismatch: missing={missing}, extra={extra}")
    if matrix.get("trials", 0) < 3 or any(cell.get("trials", 0) < 3 for cell in cells):
        raise ValueError("all cells require at least 3 trials")
    for cell in cells:
        actual = sha256_bytes(canonical_json(cell["payload"]).encode())
        if actual != cell.get("payload_sha256"):
            raise ValueError(f"payload checksum mismatch: {cell['id']}")


def exact_commit(revision: str) -> str:
    return run_checked(["git", "rev-parse", f"{revision}^{{commit}}"], cwd=ROOT)


def freeze_binary(source: Path, destination: Path) -> dict[str, Any]:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    destination.chmod(destination.stat().st_mode | 0o111)
    return {
        "path": str(destination.resolve()),
        "sha256": sha256_file(destination),
        "size_bytes": destination.stat().st_size,
    }


def prepare_revision(revision: str, artifact_dir: Path) -> dict[str, Any]:
    commit = exact_commit(revision)
    frozen_dir = artifact_dir / commit
    manifest_path = frozen_dir / "manifest.json"
    if manifest_path.is_file():
        manifest = json.loads(manifest_path.read_text())
        verify_frozen_revision(manifest, commit)
        return manifest

    artifact_dir.mkdir(parents=True, exist_ok=True)
    worktree = Path(tempfile.mkdtemp(prefix=f".fszero-e2e-{commit[:8]}-", dir=ROOT.parent))
    shutil.rmtree(worktree)
    target_dir = artifact_dir / "build" / commit
    commands: list[list[str]] = []
    try:
        run_checked(["git", "worktree", "add", "--detach", str(worktree), commit], cwd=ROOT)
        cargo_toml = (worktree / "Cargo.toml").read_text()
        split = 'name = "fszero-mcp"' in cargo_toml and 'name = "fszero-codemode"' in cargo_toml
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(target_dir)
        if split:
            commands = [
                ["cargo", "build", "--release", "--bin", "fszero"],
                ["cargo", "build", "--release", "--bin", "fszero-mcp", "--no-default-features", "--features", "fszero-ast-sgrep,surface-mcp"],
                ["cargo", "build", "--release", "--bin", "fszero-codemode", "--no-default-features", "--features", "fszero-ast-sgrep,surface-codemode"],
            ]
        else:
            commands = [["cargo", "build", "--release", "--bin", "fszero"]]
        for command in commands:
            run_checked(command, cwd=worktree, env=env)
        dirty = run_checked(["git", "status", "--porcelain", "--untracked-files=no"], cwd=worktree)
        if dirty:
            raise RuntimeError(f"build changed tracked source at {commit}: {dirty}")

        release = target_dir / "release"
        artifacts: dict[str, dict[str, Any]] = {}
        if split:
            roles = {
                "cli": (release / "fszero", ["codemode"]),
                "mcp": (release / "fszero-mcp", []),
                "codemode": (release / "fszero-codemode", []),
            }
        else:
            roles = {
                "cli": (release / "fszero", ["codemode"]),
                "mcp": (release / "fszero", ["--mode=mcp"]),
                "codemode": (release / "fszero", ["--mode=codemode"]),
            }
        for role, (source, argv_prefix) in roles.items():
            if not source.is_file():
                artifacts[role] = {"available": False, "reason": f"built artifact missing: {source.name}"}
                continue
            destination = frozen_dir / f"{role}-{source.name}"
            info = freeze_binary(source, destination)
            artifacts[role] = {"available": True, "argv_prefix": argv_prefix, **info}

        manifest = {
            "schema": SCHEMA,
            "revision_requested": revision,
            "commit": commit,
            "split_surface_artifacts": split,
            "build_commands": commands,
            "cargo_version": run_checked(["cargo", "--version"], cwd=worktree),
            "rustc_version": run_checked(["rustc", "--version", "--verbose"], cwd=worktree),
            "artifacts": artifacts,
        }
        frozen_dir.mkdir(parents=True, exist_ok=True)
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        return manifest
    finally:
        if worktree.exists():
            subprocess.run(
                ["git", "worktree", "remove", "--force", str(worktree)],
                cwd=ROOT, capture_output=True, text=True,
            )


def verify_frozen_revision(manifest: dict[str, Any], expected_commit: str) -> None:
    if manifest.get("commit") != expected_commit:
        raise RuntimeError(f"frozen manifest commit mismatch: expected {expected_commit}")
    for role, artifact in manifest.get("artifacts", {}).items():
        if not artifact.get("available"):
            continue
        path = Path(artifact["path"])
        if not path.is_file():
            raise RuntimeError(f"frozen {role} binary missing: {path}")
        actual_size = path.stat().st_size
        actual_hash = sha256_file(path)
        if actual_size != artifact.get("size_bytes") or actual_hash != artifact.get("sha256"):
            raise RuntimeError(f"frozen {role} binary checksum mismatch: {path}")


def corpus_checksum(root: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    count = 0
    total = 0
    for path in sorted(p for p in root.rglob("*") if p.is_file() and ".git" not in p.parts):
        rel = path.relative_to(root).as_posix().encode()
        data = path.read_bytes()
        digest.update(len(rel).to_bytes(8, "big"))
        digest.update(rel)
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
        count += 1
        total += len(data)
    return {"sha256": digest.hexdigest(), "files": count, "bytes": total}


def generate_template(destination: Path, files: int, batch_width: int) -> dict[str, Any]:
    if destination.exists():
        shutil.rmtree(destination)
    subprocess.run(
        [sys.executable, str(GEN_CORPUS), "--files", str(files), "--out", str(destination), "--seed", "42"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    bench = destination / "bench"
    bench.mkdir()
    for lane in ("measure", "warmup"):
        for index in range(batch_width):
            suffix = f"{lane}_{index}"
            (bench / f"edit_{suffix}.txt").write_text(f"alpha-{suffix}\n")
            (bench / f"world_{suffix}.txt").write_text(f"before-{suffix}\n")
    return corpus_checksum(destination)


def clone_trial_root(template: Path, parent: Path, label: str) -> Path:
    trial = parent / label
    shutil.copytree(template, trial)
    env = os.environ.copy()
    env.update({
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    })
    subprocess.run(["git", "init", "-q"], cwd=trial, env=env, check=True)
    subprocess.run(["git", "config", "user.email", "bench@fszero.invalid"], cwd=trial, check=True)
    subprocess.run(["git", "config", "user.name", "FSZero E2E"], cwd=trial, check=True)
    subprocess.run(["git", "add", "."], cwd=trial, env=env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "seed"], cwd=trial, env=env, check=True)
    return trial


def benchmark_env(root: Path, *, startup_index: bool = True) -> dict[str, str]:
    env = os.environ.copy()
    env.update({
        "FSZERO_ROOT": str(root),
        "ZEROSTACK_STORE_ROOT": str(root / ".zerostack"),
        "ZERO_STACK_STORE_ROOT": str(root / ".zerostack"),
        "FSZERO_INDEX_MAX_FILES": "1000000",
    })
    if startup_index:
        env["FSZERO_STARTUP_INDEX"] = "1"
        env.pop("FSZERO_SKIP_STARTUP_INDEX", None)
    else:
        env["FSZERO_SKIP_STARTUP_INDEX"] = "1"
        env.pop("FSZERO_STARTUP_INDEX", None)
    return env


def profiler_command(command: list[str], metrics_file: Path) -> tuple[list[str], str]:
    time_bin = Path("/usr/bin/time")
    if not time_bin.is_file():
        return command, "unavailable"
    if sys.platform == "darwin":
        return [str(time_bin), "-l", "-o", str(metrics_file), *command], "darwin-time-l"
    probe = subprocess.run([str(time_bin), "-v", "/usr/bin/true"], capture_output=True, text=True)
    if probe.returncode == 0:
        return [str(time_bin), "-v", "-o", str(metrics_file), *command], "gnu-time-v"
    return command, "unavailable"


def parse_resources(text: str, profiler: str) -> dict[str, Any]:
    result: dict[str, Any] = {
        "profiler": profiler,
        "user_cpu_ms": None,
        "system_cpu_ms": None,
        "peak_rss_bytes": None,
    }
    if profiler == "darwin-time-l":
        user = re.search(r"([0-9]+(?:\.[0-9]+)?)\s+user\b", text)
        system = re.search(r"([0-9]+(?:\.[0-9]+)?)\s+sys\b", text)
        rss = re.search(r"([0-9]+)\s+maximum resident set size", text)
        if user:
            result["user_cpu_ms"] = float(user.group(1)) * 1000
        if system:
            result["system_cpu_ms"] = float(system.group(1)) * 1000
        if rss:
            result["peak_rss_bytes"] = int(rss.group(1))
    elif profiler == "gnu-time-v":
        user = re.search(r"^\s*User time \(seconds\):\s*([0-9]+(?:\.[0-9]+)?)\s*$", text, re.M)
        system = re.search(r"^\s*System time \(seconds\):\s*([0-9]+(?:\.[0-9]+)?)\s*$", text, re.M)
        rss = re.search(r"^\s*Maximum resident set size \(kbytes\):\s*([0-9]+)\s*$", text, re.M)
        if user:
            result["user_cpu_ms"] = float(user.group(1)) * 1000
        if system:
            result["system_cpu_ms"] = float(system.group(1)) * 1000
        if rss:
            result["peak_rss_bytes"] = int(rss.group(1)) * 1024
    if result["user_cpu_ms"] is not None and result["system_cpu_ms"] is not None:
        result["cpu_total_ms"] = result["user_cpu_ms"] + result["system_cpu_ms"]
    else:
        result["cpu_total_ms"] = None
    return result


def response_error(response: dict[str, Any]) -> str | None:
    if "error" in response:
        return canonical_json(response["error"])
    result = response.get("result")
    if not isinstance(result, dict):
        return "missing JSON-RPC result"
    if result.get("isError") is True:
        return canonical_json(result)
    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        if structured.get("ok") is False or structured.get("error") not in (None, False):
            return canonical_json(structured.get("error") or structured)
        payload = structured.get("payload")
        if isinstance(payload, dict) and payload.get("error") not in (None, False):
            return canonical_json(payload["error"])
    return None


class JsonRpcSession:
    def __init__(
        self, artifact: dict[str, Any], root: Path, surface: str, timeout: float,
        metrics_file: Path | None = None, *, startup_index: bool = True,
    ) -> None:
        self.surface = surface
        self.timeout = timeout
        base = [artifact["path"], *artifact.get("argv_prefix", [])]
        self.profiler = "unavailable"
        command = base
        if metrics_file is not None:
            command, self.profiler = profiler_command(base, metrics_file)
        self.metrics_file = metrics_file
        self.started = time.perf_counter()
        self.proc = subprocess.Popen(
            command, cwd=root, env=benchmark_env(root, startup_index=startup_index),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0,
        )
        self.next_id = 1
        self._send({
            "jsonrpc": "2.0", "id": self._id(), "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "shipped-e2e", "version": "1"}},
        })
        init = self._recv()
        if "error" in init:
            raise RuntimeError(f"initialize failed: {canonical_json(init['error'])}")
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _id(self) -> int:
        value = self.next_id
        self.next_id += 1
        return value

    def _send(self, message: Any) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write((canonical_json(message) + "\n").encode())
        self.proc.stdin.flush()

    def _recv(self) -> dict[str, Any]:
        assert self.proc.stdout is not None
        selector = selectors.DefaultSelector()
        selector.register(self.proc.stdout, selectors.EVENT_READ)
        try:
            if not selector.select(self.timeout):
                raise TimeoutError(f"JSON-RPC response timeout after {self.timeout}s")
            line = self.proc.stdout.readline()
        finally:
            selector.close()
        if not line:
            raise RuntimeError("JSON-RPC server closed stdout")
        value = json.loads(line)
        if not isinstance(value, dict):
            raise RuntimeError(f"expected JSON-RPC object, got {type(value).__name__}")
        return value

    def tools(self) -> list[str]:
        request_id = self._id()
        self._send({"jsonrpc": "2.0", "id": request_id, "method": "tools/list", "params": {}})
        response = self._recv()
        error = response_error(response)
        if error:
            raise RuntimeError(f"tools/list failed: {error}")
        tools = response["result"].get("tools", [])
        return sorted(t["name"] for t in tools if isinstance(t, dict) and isinstance(t.get("name"), str))

    def call_mcp(self, calls: list[dict[str, Any]]) -> list[dict[str, Any]]:
        requests = []
        for call in calls:
            requests.append({
                "jsonrpc": "2.0", "id": self._id(), "method": "tools/call",
                "params": {"name": mcp_tool(call["method"]), "arguments": call["args"]},
            })
        for request in requests:
            self._send(request)
        return [self._recv() for _ in requests]

    def call_plan(self, plan: str) -> dict[str, Any]:
        request_id = self._id()
        self._send({
            "jsonrpc": "2.0", "id": request_id, "method": "tools/call",
            "params": {"name": "fz_execute_code", "arguments": {"plan": plan, "envelope": "v1"}},
        })
        return self._recv()

    def close(self) -> tuple[int, bytes, float]:
        if self.proc.stdin is not None and not self.proc.stdin.closed:
            self.proc.stdin.close()
        try:
            returncode = self.proc.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            returncode = self.proc.wait(timeout=5)
        assert self.proc.stderr is not None
        stderr = self.proc.stderr.read()
        return returncode, stderr, (time.perf_counter() - self.started) * 1000


def mcp_tool(method: str) -> str:
    aliases = {
        "fs.read": "fszero.read", "fs.search": "fszero.search", "fs.ls": "fszero.ls",
        "fs.write": "fszero.write", "fs.edit": "fszero.edit", "fs.world": "fszero.world",
        "fs.history": "fszero.history", "fs.undo": "fszero.undo", "fs.expand": "fszero.expand",
    }
    return aliases[method]


def js_call(call: dict[str, Any], name: str) -> str:
    method = call["method"]
    args = canonical_json(call["args"])
    return (
        f"const {name}=await zero.{method}({args});"
        f"if({name}.ok===false)return{{error:{name}.detail||{name}.error||'{method} failed'}};"
    )


def code_plan(setup: Iterable[dict[str, Any]], warmup: Iterable[dict[str, Any]], measured: Iterable[dict[str, Any]]) -> str:
    parts: list[str] = []
    refs: list[str] = []
    counter = 0
    for prefix, calls in (("s", setup), ("w", warmup), ("m", measured)):
        for call in calls:
            name = f"{prefix}{counter}"
            counter += 1
            parts.append(js_call(call, name))
            if prefix == "m":
                refs.append(f"{name}.ref||{name}.ack")
    parts.append(f"return{{ok:true,results:[{','.join(refs)}]}};")
    return "".join(parts)


def dag_plan(*groups: Iterable[dict[str, Any]]) -> str:
    """JSON-DAG form runs in the shipped CLI shim without the interpreter."""
    steps = []
    previous: str | None = None
    for index, call in enumerate(item for group in groups for item in group):
        step_id = f"s{index}"
        steps.append({
            "id": step_id, "call": call["method"], "args": call["args"],
            "needs": [previous] if previous is not None else [],
        })
        previous = step_id
    if not steps:
        raise ValueError("JSON-DAG plan requires at least one operation")
    return canonical_json({"label": "shipped-e2e", "transaction": "auto", "steps": steps})


def run_unprofiled_setup(
    artifact: dict[str, Any], surface: str, root: Path, calls: list[dict[str, Any]], timeout: float,
    *, startup_index: bool = False,
) -> None:
    if not calls:
        return
    if surface == "cli":
        plan = dag_plan(calls)
        command = [artifact["path"], *artifact.get("argv_prefix", []), plan, "--root", str(root)]
        proc = subprocess.run(
            command, cwd=root, env=benchmark_env(root, startup_index=startup_index),
            capture_output=True, text=True, timeout=timeout,
        )
        if proc.returncode != 0 or proc.stdout.lstrip().startswith("X0"):
            raise RuntimeError(f"setup failed: rc={proc.returncode} stdout={proc.stdout[-500:]} stderr={proc.stderr[-500:]}")
        return
    session = JsonRpcSession(artifact, root, surface, timeout, startup_index=startup_index)
    try:
        if surface == "mcp":
            responses = session.call_mcp(calls)
            errors = [response_error(response) for response in responses]
            if any(errors):
                raise RuntimeError(f"setup failed: {errors}")
        else:
            response = session.call_plan(code_plan(calls, [], []))
            if response_error(response):
                raise RuntimeError(f"setup failed: {response_error(response)}")
    finally:
        session.close()


def prewarm_store(artifact: dict[str, Any], surface: str, root: Path, timeout: float) -> None:
    """Build/open the index outside measurement without mutating workload targets."""
    if surface == "cli":
        run_unprofiled_setup(
            artifact, surface, root,
            [{"method": "fs.read", "args": {"path": "mod_000/sub_000/f_000.rs"}}],
            timeout, startup_index=True,
        )
        return
    session = JsonRpcSession(artifact, root, surface, timeout, startup_index=True)
    session.close()


def metric_error(stage: str, exc: BaseException, **extra: Any) -> dict[str, Any]:
    return {
        "class": type(exc).__name__, "stage": stage, "message": str(exc), **extra,
    }


def run_cell_trial(
    revision: str, artifact: dict[str, Any], cell: dict[str, Any], trial: int,
    root: Path, timeout: float,
) -> dict[str, Any]:
    base = {
        "schema": SCHEMA, "kind": "trial", "revision": revision,
        "cell_id": cell["id"], "operation": cell["operation"],
        "surface": cell["surface"], "temperature": cell["temperature"],
        "invocation": cell["invocation"], "trial": trial,
        "payload_sha256": cell["payload_sha256"],
    }
    metrics_file = root.parent / f"{root.name}.time"
    payload = cell["payload"]
    setup = list(payload["setup"])
    warmup_setup = list(payload["warmup_setup"]) if cell["temperature"] == "warm" else []
    warmup = list(payload["warmup"]) if cell["temperature"] == "warm" else []
    measure = list(payload["measure"])
    # Recovery explicitly proves durability across process lifetime. History and
    # undo setup also runs in a separate no-index process so setup is not timed
    # and cannot contaminate the cold index.
    separate_setup = setup if cell["operation"] in ("history", "undo", "recovery") else []
    inline_setup = [] if separate_setup else setup
    try:
        verify_artifact(artifact)
        run_unprofiled_setup(artifact, cell["surface"], root, separate_setup, timeout)
        if warmup:
            run_unprofiled_setup(
                artifact, cell["surface"], root, warmup_setup, timeout,
                startup_index=False,
            )
            prewarm_store(artifact, cell["surface"], root, timeout)
        if cell["surface"] == "cli":
            if warmup:
                run_unprofiled_setup(
                    artifact, "cli", root, [*inline_setup, *warmup], timeout,
                    startup_index=True,
                )
                inline_setup = setup if cell["operation"] == "ref_expand" else []
            plan = dag_plan(inline_setup, measure)
            command = [artifact["path"], *artifact.get("argv_prefix", []), plan, "--root", str(root)]
            profiled, profiler = profiler_command(command, metrics_file)
            start = time.perf_counter()
            proc = subprocess.run(
                profiled, cwd=root, env=benchmark_env(root, startup_index=True),
                capture_output=True, text=True, timeout=timeout,
            )
            process_wall_ms = (time.perf_counter() - start) * 1000
            operation_wall_ms = process_wall_ms
            wall_ms = process_wall_ms
            if proc.returncode != 0 or proc.stdout.lstrip().startswith("X0"):
                raise RuntimeError(f"CLI operation failed: rc={proc.returncode} stdout={proc.stdout[-800:]} stderr={proc.stderr[-800:]}")
            response_bytes = proc.stdout.encode()
            time_text = metrics_file.read_text() if metrics_file.is_file() else ""
            resources = parse_resources(time_text, profiler)
        else:
            session = JsonRpcSession(artifact, root, cell["surface"], timeout, metrics_file)
            try:
                if inline_setup:
                    if cell["surface"] == "mcp":
                        errors = [response_error(r) for r in session.call_mcp(inline_setup)]
                        if any(errors):
                            raise RuntimeError(f"inline setup failed: {errors}")
                    else:
                        response = session.call_plan(code_plan(inline_setup, [], []))
                        if response_error(response):
                            raise RuntimeError(f"inline setup failed: {response_error(response)}")
                if warmup:
                    if cell["surface"] == "mcp":
                        errors = [response_error(r) for r in session.call_mcp(warmup)]
                        if any(errors):
                            raise RuntimeError(f"warmup failed: {errors}")
                    else:
                        response = session.call_plan(code_plan([], warmup, []))
                        if response_error(response):
                            raise RuntimeError(f"warmup failed: {response_error(response)}")
                start = time.perf_counter()
                if cell["surface"] == "mcp":
                    responses = session.call_mcp(measure)
                    errors = [response_error(response) for response in responses]
                    operation_wall_ms = (time.perf_counter() - start) * 1000
                    if any(errors):
                        raise RuntimeError(f"MCP operation failed: {errors}")
                    response_bytes = canonical_json(responses).encode()
                else:
                    response = session.call_plan(code_plan([], [], measure))
                    operation_wall_ms = (time.perf_counter() - start) * 1000
                    if response_error(response):
                        raise RuntimeError(f"CodeMode operation failed: {response_error(response)}")
                    response_bytes = canonical_json(response).encode()
            finally:
                returncode, stderr, process_wall_ms = session.close()
            if returncode != 0:
                raise RuntimeError(f"server exited {returncode}: {stderr[-800:].decode(errors='replace')}")
            time_text = metrics_file.read_text() if metrics_file.is_file() else ""
            resources = parse_resources(time_text, session.profiler)
            wall_ms = process_wall_ms if cell["temperature"] == "cold" else operation_wall_ms
        return {
            **base, "status": "ok",
            "metrics": {
                "wall_ms": wall_ms, "operation_wall_ms": operation_wall_ms,
                "process_wall_ms": process_wall_ms, **resources,
            },
            "response_sha256": sha256_bytes(response_bytes),
            "response_bytes": len(response_bytes),
        }
    except BaseException as exc:
        return {**base, "status": "error", "metrics": None, "error": metric_error("trial", exc)}
    finally:
        metrics_file.unlink(missing_ok=True)


def verify_artifact(artifact: dict[str, Any]) -> None:
    path = Path(artifact["path"])
    if not path.is_file():
        raise RuntimeError(f"frozen binary missing: {path}")
    if path.stat().st_size != artifact["size_bytes"] or sha256_file(path) != artifact["sha256"]:
        raise RuntimeError(f"frozen binary changed: {path}")


def probe_surface(artifact: dict[str, Any], surface: str, root: Path, timeout: float) -> dict[str, Any]:
    if not artifact.get("available"):
        return {"available": False, "reason": artifact.get("reason", "artifact unavailable")}
    try:
        verify_artifact(artifact)
        if surface == "cli":
            help_proc = subprocess.run([artifact["path"], "--help"], capture_output=True, text=True, timeout=timeout)
            if help_proc.returncode != 0:
                raise RuntimeError(f"--help exited {help_proc.returncode}")
            plan = dag_plan([operation_payload("read", 0)])
            command = [artifact["path"], *artifact.get("argv_prefix", []), plan, "--root", str(root)]
            smoke = subprocess.run(command, cwd=root, env=benchmark_env(root), capture_output=True, text=True, timeout=timeout)
            if smoke.returncode != 0 or smoke.stdout.lstrip().startswith("X0"):
                return {"available": False, "reason": "shipped CLI has no functional one-shot CodeMode operation path"}
            return {"available": True, "catalog": ["codemode-one-shot"]}
        session = JsonRpcSession(artifact, root, surface, timeout)
        try:
            tools = session.tools()
        finally:
            session.close()
        required = "fszero.read" if surface == "mcp" else "fz_execute_code"
        if required not in tools:
            return {"available": False, "reason": f"catalog lacks {required}", "catalog": tools}
        return {"available": True, "catalog": tools}
    except BaseException as exc:
        return {"available": False, "reason": str(exc), "error": metric_error("probe", exc)}


def parse_cpu_time(raw: str) -> float:
    value = raw.strip()
    days = 0
    if "-" in value:
        day, value = value.split("-", 1)
        days = int(day)
    fields = [float(part) for part in value.split(":")]
    if len(fields) == 3:
        hours, minutes, seconds = fields
    elif len(fields) == 2:
        hours, minutes, seconds = 0.0, fields[0], fields[1]
    else:
        hours, minutes, seconds = 0.0, 0.0, fields[0]
    return ((days * 24 + hours) * 3600 + minutes * 60 + seconds) * 1000


def process_cpu_ms(pid: int) -> float:
    output = subprocess.check_output(["ps", "-o", "time=", "-p", str(pid)], text=True)
    return parse_cpu_time(output)


def run_idle_trial(
    revision: str, artifact: dict[str, Any], surface: str, trial: int,
    root: Path, timeout: float, idle_seconds: float,
) -> dict[str, Any]:
    base = {"schema": SCHEMA, "kind": "warm_idle", "revision": revision,
            "surface": surface, "trial": trial}
    try:
        verify_artifact(artifact)
        session = JsonRpcSession(artifact, root, surface, timeout)
        try:
            warm = operation_payload("read", 0)
            if surface == "mcp":
                responses = session.call_mcp([warm])
                error = response_error(responses[0])
            else:
                response = session.call_plan(code_plan([], [warm], []))
                error = response_error(response)
            if error:
                raise RuntimeError(f"idle warmup failed: {error}")
            cpu_before = process_cpu_ms(session.proc.pid)
            wall_start = time.perf_counter()
            time.sleep(idle_seconds)
            wall_ms = (time.perf_counter() - wall_start) * 1000
            cpu_ms = max(0.0, process_cpu_ms(session.proc.pid) - cpu_before)
        finally:
            session.close()
        return {**base, "status": "ok", "metrics": {
            "idle_wall_ms": wall_ms, "idle_cpu_ms": cpu_ms,
            "idle_cpu_pct": (100.0 * cpu_ms / wall_ms) if wall_ms else None,
        }}
    except BaseException as exc:
        return {**base, "status": "error", "metrics": None, "error": metric_error("warm_idle", exc)}


MIN_MEASURED_RUNS = 20


def median(values: Iterable[float]) -> float:
    return float(statistics.median(values))


def percentiles(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)

    def at(fraction: float) -> float:
        rank = (len(ordered) - 1) * fraction
        lower = int(rank)
        upper = min(lower + 1, len(ordered) - 1)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {"p50": at(0.50), "p95": at(0.95), "p99": at(0.99)}


def comparison_entry(
    key: str, metric: str, baseline: list[float], candidate: list[float], max_regression_pct: float,
) -> dict[str, Any]:
    baseline_median = median(baseline)
    candidate_median = median(candidate)
    if baseline_median == 0:
        improvement = 0.0 if candidate_median == 0 else None
        regression = 0.0 if candidate_median == 0 else None
    else:
        improvement = 100.0 * (baseline_median - candidate_median) / baseline_median
        regression = -improvement
    return {
        "key": key, "metric": metric, "baseline_median": baseline_median,
        "candidate_median": candidate_median,
        "baseline_percentiles": percentiles(baseline),
        "candidate_percentiles": percentiles(candidate),
        "improvement_pct": improvement,
        "hit_90": improvement is not None and improvement >= 90.0,
        "hard_limit": {
            "kind": "max_regression_pct", "value": max_regression_pct,
            "observed_regression_pct": regression,
            "pass": regression is not None and regression <= max_regression_pct,
        },
        "baseline_trials": len(baseline), "candidate_trials": len(candidate),
    }


def summarize(
    records: list[dict[str, Any]], manifests: dict[str, dict[str, Any]],
    trials: int, max_regression_pct: float,
) -> dict[str, Any]:
    errors = [record for record in records if record.get("status") == "error"]
    comparisons: list[dict[str, Any]] = []
    metrics = (
        "wall_ms", "operation_wall_ms", "process_wall_ms", "user_cpu_ms",
        "system_cpu_ms", "cpu_total_ms", "peak_rss_bytes",
    )
    for cell_id in sorted({record["cell_id"] for record in records if record.get("kind") == "trial"}):
        grouped: dict[str, list[dict[str, Any]]] = {}
        for revision in ("baseline", "candidate"):
            grouped[revision] = [
                record for record in records
                if record.get("kind") == "trial" and record.get("cell_id") == cell_id
                and record.get("revision") == revision and record.get("status") == "ok"
            ]
        if any(len(grouped[revision]) != trials for revision in grouped):
            continue
        for metric in metrics:
            baseline = [r["metrics"][metric] for r in grouped["baseline"] if r["metrics"].get(metric) is not None]
            candidate = [r["metrics"][metric] for r in grouped["candidate"] if r["metrics"].get(metric) is not None]
            if len(baseline) == trials and len(candidate) == trials:
                comparisons.append(comparison_entry(cell_id, metric, baseline, candidate, max_regression_pct))
    for surface in PERSISTENT_SURFACES:
        grouped_idle = {
            revision: [r for r in records if r.get("kind") == "warm_idle" and r.get("surface") == surface
                       and r.get("revision") == revision and r.get("status") == "ok"]
            for revision in ("baseline", "candidate")
        }
        if all(len(rows) == trials for rows in grouped_idle.values()):
            for metric in ("idle_cpu_ms", "idle_cpu_pct"):
                comparisons.append(comparison_entry(
                    f"warm_idle/{surface}", metric,
                    [r["metrics"][metric] for r in grouped_idle["baseline"]],
                    [r["metrics"][metric] for r in grouped_idle["candidate"]],
                    max_regression_pct,
                ))
    for role in SURFACES:
        left = manifests["baseline"]["artifacts"].get(role, {})
        right = manifests["candidate"]["artifacts"].get(role, {})
        if left.get("available") and right.get("available"):
            comparisons.append(comparison_entry(
                f"binary/{role}", "size_bytes", [left["size_bytes"]], [right["size_bytes"]], max_regression_pct,
            ))
    return {
        "schema": SCHEMA,
        "integrity_status": "failed" if errors else "passed",
        "publishable": not errors,
        "error_count": len(errors),
        "comparison_count": len(comparisons),
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "measured_runs_per_cell": trials,
            "warmup_policy": "scenario prewarm is unmeasured; no measured trial excluded",
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered trial in raw-trials.jsonl",
        },
        "comparisons": comparisons if not errors else [],
        "suppressed_comparison_count": len(comparisons) if errors else 0,
        "hard_limit_failures": [] if errors else [
            {"key": c["key"], "metric": c["metric"], "hard_limit": c["hard_limit"]}
            for c in comparisons if not c["hard_limit"]["pass"]
        ],
    }


def write_jsonl(path: Path, rows: Iterable[dict[str, Any]]) -> None:
    with path.open("w") as handle:
        for row in rows:
            handle.write(canonical_json(row) + "\n")
            handle.flush()


def execute(args: argparse.Namespace) -> int:
    matrix = build_matrix(args.trials, args.batch_width)
    if args.dry_run:
        print(json.dumps({
            "schema": SCHEMA, "mode": "dry-run", "baseline_revision": args.baseline_rev,
            "candidate_revision": args.candidate_rev, "matrix": matrix,
            "cell_count": len(matrix["cells"]),
            "expected_trial_records": (
                len(matrix["cells"]) * args.trials * 2
                + len(PERSISTENT_SURFACES) * args.trials * 2
            ),
            "full_benchmark_executed": False,
        }, indent=2, sort_keys=True))
        return 0

    artifact_dir = args.artifact_dir.resolve()
    output_dir = args.output_dir.resolve()
    if output_dir.exists() and any(output_dir.iterdir()):
        raise RuntimeError(f"refusing to mix evidence in non-empty output directory: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)
    if args.skip_prepare:
        manifests = {}
        for label, revision in (("baseline", args.baseline_rev), ("candidate", args.candidate_rev)):
            commit = exact_commit(revision)
            path = artifact_dir / commit / "manifest.json"
            if not path.is_file():
                raise RuntimeError(f"missing frozen manifest for --skip-prepare: {path}")
            manifests[label] = json.loads(path.read_text())
            verify_frozen_revision(manifests[label], commit)
    else:
        manifests = {
            "baseline": prepare_revision(args.baseline_rev, artifact_dir),
            "candidate": prepare_revision(args.candidate_rev, artifact_dir),
        }
    provenance = {
        "schema": SCHEMA, "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "harness_path": str(Path(__file__).resolve()), "harness_sha256": sha256_file(Path(__file__)),
        "generator_path": str(GEN_CORPUS.resolve()), "generator_sha256": sha256_file(GEN_CORPUS),
        "matrix_sha256": matrix["matrix_sha256"], "revisions": manifests,
        "host": {"platform": sys.platform, "python": sys.version, "machine": os.uname().machine},
    }
    (output_dir / "matrix.json").write_text(json.dumps(matrix, indent=2, sort_keys=True) + "\n")
    (output_dir / "provenance.json").write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n")
    if args.prepare_only:
        return 0

    workspace = output_dir / "workspace"
    template = workspace / "template"
    roots = workspace / "trials"
    roots.mkdir(parents=True)
    corpus = generate_template(template, args.corpus_files, args.batch_width)
    provenance["corpus"] = {"seed": 42, "requested_generated_files": args.corpus_files, **corpus}
    (output_dir / "provenance.json").write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n")

    probes: dict[str, dict[str, Any]] = {"baseline": {}, "candidate": {}}
    records: list[dict[str, Any]] = []
    unsupported: list[dict[str, Any]] = []
    for revision in ("baseline", "candidate"):
        for surface in SURFACES:
            probe_root = clone_trial_root(template, roots, f"probe-{revision}-{surface}")
            artifact = manifests[revision]["artifacts"].get(surface, {"available": False, "reason": "role absent"})
            probes[revision][surface] = probe_surface(artifact, surface, probe_root, args.timeout)
            if not args.keep_trial_roots:
                shutil.rmtree(probe_root)
    (output_dir / "capabilities.json").write_text(json.dumps(probes, indent=2, sort_keys=True) + "\n")

    capability_errors = []
    for surface in SURFACES:
        baseline_available = probes["baseline"][surface]["available"]
        candidate_available = probes["candidate"][surface]["available"]
        if baseline_available == candidate_available:
            continue
        capability_errors.append({
            "schema": SCHEMA, "kind": "capability", "status": "error",
            "surface": surface, "metrics": None,
            "error": {
                "class": "asymmetric_capability", "stage": "probe",
                "message": "surface availability differs between baseline and candidate",
                "baseline_available": baseline_available,
                "candidate_available": candidate_available,
            },
        })
    records.extend(capability_errors)

    raw_path = output_dir / "raw-trials.jsonl"
    errors_path = output_dir / "errors.jsonl"
    with raw_path.open("w") as raw, errors_path.open("w") as errors:
        for record in capability_errors:
            raw.write(canonical_json(record) + "\n")
            errors.write(canonical_json(record) + "\n")
        raw.flush()
        errors.flush()
        for cell in matrix["cells"]:
            for revision in ("baseline", "candidate"):
                support = probes[revision][cell["surface"]]
                if not support["available"]:
                    item = {"cell_id": cell["id"], "revision": revision, "surface": cell["surface"],
                            "reason": support["reason"]}
                    unsupported.append(item)
                    for trial in range(1, args.trials + 1):
                        raw.write(canonical_json({
                            "schema": SCHEMA, "kind": "trial", **item, "trial": trial,
                            "status": "unsupported", "metrics": None,
                        }) + "\n")
                    raw.flush()
                    continue
                artifact = manifests[revision]["artifacts"][cell["surface"]]
                for trial in range(1, args.trials + 1):
                    label = f"{cell['id'].replace('/', '-')}-{revision}-{trial}"
                    trial_root = clone_trial_root(template, roots, label)
                    if corpus_checksum(trial_root) != corpus:
                        raise RuntimeError(f"trial corpus changed before execution: {label}")
                    record = run_cell_trial(revision, artifact, cell, trial, trial_root, args.timeout)
                    records.append(record)
                    raw.write(canonical_json(record) + "\n")
                    raw.flush()
                    if record["status"] == "error":
                        errors.write(canonical_json(record) + "\n")
                        errors.flush()
                    if not args.keep_trial_roots:
                        shutil.rmtree(trial_root)
        for surface in PERSISTENT_SURFACES:
            for revision in ("baseline", "candidate"):
                support = probes[revision][surface]
                if not support["available"]:
                    item = {"cell_id": f"warm_idle/{surface}", "revision": revision,
                            "surface": surface, "reason": support["reason"]}
                    unsupported.append(item)
                    for trial in range(1, args.trials + 1):
                        raw.write(canonical_json({
                            "schema": SCHEMA, "kind": "warm_idle", **item, "trial": trial,
                            "status": "unsupported", "metrics": None,
                        }) + "\n")
                    raw.flush()
                    continue
                artifact = manifests[revision]["artifacts"][surface]
                for trial in range(1, args.trials + 1):
                    trial_root = clone_trial_root(template, roots, f"idle-{surface}-{revision}-{trial}")
                    record = run_idle_trial(revision, artifact, surface, trial, trial_root,
                                            args.timeout, args.idle_seconds)
                    records.append(record)
                    raw.write(canonical_json(record) + "\n")
                    raw.flush()
                    if record["status"] == "error":
                        errors.write(canonical_json(record) + "\n")
                        errors.flush()
                    if not args.keep_trial_roots:
                        shutil.rmtree(trial_root)

    summary = summarize(records, manifests, args.trials, args.max_regression_pct)
    summary["residual_unsupported_cells"] = unsupported
    summary["residual_unsupported_count"] = len(unsupported)
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    if not args.keep_trial_roots:
        shutil.rmtree(workspace)
    return (
        0
        if summary["integrity_status"] == "passed" and not summary["hard_limit_failures"]
        else 1
    )


def parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--baseline-rev", default=DEFAULT_BASELINE,
                    help=f"baseline git revision (default: {DEFAULT_BASELINE})")
    ap.add_argument("--candidate-rev", default=DEFAULT_CANDIDATE,
                    help=f"candidate git revision (default: {DEFAULT_CANDIDATE})")
    ap.add_argument("--artifact-dir", type=Path, default=ROOT / ".benchmark-artifacts" / "shipped-e2e",
                    help="checksum-locked frozen binary directory")
    ap.add_argument("--output-dir", type=Path,
                    default=ROOT / "benchmarks" / "evidence" / f"shipped-e2e-{DEFAULT_BASELINE}-vs-{DEFAULT_CANDIDATE}")
    ap.add_argument(
        "--trials", type=int, default=MIN_MEASURED_RUNS,
        help=f"trials per matrix cell; minimum {MIN_MEASURED_RUNS}",
    )
    ap.add_argument("--batch-width", type=int, default=DEFAULT_BATCH_WIDTH)
    ap.add_argument("--corpus-files", type=int, default=1000)
    ap.add_argument("--timeout", type=float, default=300.0)
    ap.add_argument("--idle-seconds", type=float, default=1.0)
    ap.add_argument("--max-regression-pct", type=float, default=5.0,
                    help="hard limit applied to every lower-is-better summary metric")
    ap.add_argument("--dry-run", action="store_true",
                    help="validate and print the complete matrix without building or benchmarking")
    ap.add_argument("--prepare-only", action="store_true", help="freeze binaries and provenance, then stop")
    ap.add_argument("--skip-prepare", action="store_true", help="reuse checksum-verified frozen manifests")
    ap.add_argument("--keep-trial-roots", action="store_true")
    return ap


def main() -> None:
    args = parser().parse_args()
    if args.trials < MIN_MEASURED_RUNS:
        parser().error(f"--trials must be at least {MIN_MEASURED_RUNS}")
    if args.batch_width < 2:
        parser().error("--batch-width must be at least 2")
    if args.corpus_files < 1:
        parser().error("--corpus-files must be positive")
    if args.idle_seconds <= 0:
        parser().error("--idle-seconds must be positive")
    try:
        raise SystemExit(execute(args))
    except (RuntimeError, ValueError) as exc:
        print(canonical_json({"schema": SCHEMA, "status": "error", "error": metric_error("harness", exc)}), file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    main()
