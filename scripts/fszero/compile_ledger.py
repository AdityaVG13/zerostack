#!/usr/bin/env python3
"""Same-machine clean/touched-incremental Cargo build ledger.

The harness compares two checkouts without sharing target directories. It records
all trial observations as raw JSON; it intentionally does not turn timings into
a benchmark claim. Run at least three trials and summarize externally only after
reviewing the raw observations.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

SCHEMA = "fszero.compile-ledger.v1"
MIN_TRIALS = 3


@dataclass(frozen=True)
class Product:
    name: str
    package: str | None
    binary: str
    features: tuple[str, ...]
    default_features: bool
    touch: str
    engine_features: tuple[str, ...]

    def cargo_args(self, profile: str) -> list[str]:
        args = ["cargo", "build"]
        if profile != "debug":
            args.extend(["--profile", profile])
        if self.package:
            args.extend(["--package", self.package])
        else:
            args.extend(["--bin", self.binary])
        if not self.default_features:
            args.append("--no-default-features")
        if self.features:
            args.extend(["--features", ",".join(self.features)])
        return args


def products_for(root: Path) -> tuple[str, list[Product]]:
    """Return the compile contract for the checkout's detected architecture."""
    if (root / "crates/fszero-mcp/Cargo.toml").is_file():
        return "workspace-products-v1", [
            Product("mcp", "fszero-mcp", "fszero-mcp", ("sqlite-system",), False, "crates/fszero-mcp/src/main.rs", ("fszero-ast-sgrep", "surface-mcp", "sqlite-system")),
            Product("worker", "fszero-worker", "fszero-codemode", (), False, "crates/fszero-codemode/src/main.rs", ()),
            Product("cli", "fszero-cli", "fszero", ("sqlite-system",), False, "crates/fszero-shim/src/main.rs", ("sqlite-system",)),
        ]
    return "dense-package-v1", [
        Product("mcp", None, "fszero-mcp", ("fszero-ast-sgrep", "surface-mcp"), False, "src/bin/fszero_mcp.rs", ("fszero-ast-sgrep", "surface-mcp", "watch", "rusqlite/bundled")),
        Product("codemode", None, "fszero-codemode", ("fszero-ast-sgrep", "surface-codemode"), False, "src/bin/fszero_codemode.rs", ("fszero-ast-sgrep", "surface-codemode", "watch", "rusqlite/bundled")),
        Product("shim", None, "fszero", (), True, "src/bin/fszero.rs", ("fszero-ast-sgrep", "watch", "dev-harness", "mcp-http", "rusqlite/bundled")),
    ]


def command_output(argv: list[str], cwd: Path) -> str:
    proc = subprocess.run(argv, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    return proc.stdout.strip() if proc.returncode == 0 else f"unavailable (exit {proc.returncode}): {proc.stdout.strip()}"


def manifest_contract(root: Path) -> dict[str, Any]:
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version=1", "--no-deps"],
        cwd=root,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"cargo metadata failed in {root}: {proc.stderr.strip()}")
    doc = json.loads(proc.stdout)
    packages: dict[str, Any] = {}
    for package in doc["packages"]:
        packages[package["name"]] = {
            "version": package["version"],
            "features": package["features"],
            "dependencies": [
                {
                    "name": dep["name"],
                    "uses_default_features": dep["uses_default_features"],
                    "features": dep["features"],
                }
                for dep in package["dependencies"]
                if dep.get("path") is not None
            ],
        }
    return {"workspace_root": doc["workspace_root"], "packages": packages}


def machine_contract() -> dict[str, str]:
    return {
        "node": platform.node(),
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "python": platform.python_version(),
    }


def phase_contract(label: str, root: Path, profile: str, dry_run: bool) -> dict[str, Any]:
    architecture, products = products_for(root)
    result: dict[str, Any] = {
        "label": label,
        "root": str(root.resolve()),
        "architecture": architecture,
        "git_head": None,
        "manifest": None,
        "products": [
            {
                **asdict(product),
                "features": list(product.features),
                "engine_features": list(product.engine_features),
                "command": product.cargo_args(profile),
            }
            for product in products
        ],
    }
    if not dry_run:
        result["git_head"] = command_output(["git", "rev-parse", "HEAD"], root)
        result["manifest"] = manifest_contract(root)
    return result


def touch_for_rebuild(path: Path) -> tuple[int, int]:
    stat = path.stat()
    bumped = max(time.time_ns(), stat.st_mtime_ns + 2_000_000_000)
    os.utime(path, ns=(stat.st_atime_ns, bumped))
    return stat.st_atime_ns, stat.st_mtime_ns


def run_build(root: Path, product: Product, target: Path, profile: str, kind: str, trial: int, phase: str) -> dict[str, Any]:
    argv = product.cargo_args(profile)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target)
    env["CARGO_INCREMENTAL"] = "1"
    started_unix_ns = time.time_ns()
    started = time.monotonic_ns()
    proc = subprocess.run(argv, cwd=root, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    elapsed = time.monotonic_ns() - started
    binary = target / profile / product.binary
    record = {
        "phase": phase,
        "trial": trial,
        "product": product.name,
        "kind": kind,
        "command": argv,
        "requested_features": list(product.features),
        "default_features": product.default_features,
        "engine_features": list(product.engine_features),
        "touch": product.touch if kind == "touched-incremental" else None,
        "target_dir": str(target),
        "started_unix_ns": started_unix_ns,
        "duration_ns": elapsed,
        "returncode": proc.returncode,
        "binary": str(binary),
        "binary_size_bytes": binary.stat().st_size if binary.is_file() else None,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
    }
    if proc.returncode != 0:
        raise BuildFailure(record)
    return record


class BuildFailure(RuntimeError):
    def __init__(self, record: dict[str, Any]):
        super().__init__(f"{record['phase']} {record['product']} {record['kind']} failed")
        self.record = record


def planned_measurements(phases: list[dict[str, Any]], trials: int) -> list[dict[str, Any]]:
    return [
        {
            "phase": phase["label"],
            "trial": trial,
            "product": product["name"],
            "kind": kind,
            "command": product["command"],
            "requested_features": product["features"],
            "default_features": product["default_features"],
            "engine_features": product["engine_features"],
            "touch": product["touch"] if kind == "touched-incremental" else None,
        }
        for phase in phases
        for trial in range(1, trials + 1)
        for product in phase["products"]
        for kind in ("clean", "touched-incremental")
    ]


def write_json(doc: dict[str, Any], output: Path | None) -> None:
    text = json.dumps(doc, indent=2, sort_keys=True) + "\n"
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", required=True, type=Path, help="baseline checkout")
    parser.add_argument("--after", required=True, type=Path, help="candidate checkout")
    parser.add_argument("--trials", type=int, default=MIN_TRIALS)
    parser.add_argument(
        "--profile", choices=("debug", "release", "release-perf"), default="debug"
    )
    parser.add_argument("--output", type=Path, help="raw JSON ledger (default: stdout)")
    parser.add_argument("--scratch", type=Path, help="target-directory parent; retained after the run")
    parser.add_argument("--dry-run", action="store_true", help="emit the exact contract without running cargo/git")
    args = parser.parse_args(argv)
    if args.trials < MIN_TRIALS:
        parser.error(f"--trials must be >= {MIN_TRIALS}")
    for label in ("before", "after"):
        root = getattr(args, label)
        if not root.is_dir():
            parser.error(f"--{label} is not a directory: {root}")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    phases = [
        phase_contract("before", args.before, args.profile, args.dry_run),
        phase_contract("after", args.after, args.profile, args.dry_run),
    ]
    ledger: dict[str, Any] = {
        "schema": SCHEMA,
        "dry_run": args.dry_run,
        "profile": args.profile,
        "trials": args.trials,
        "same_machine": machine_contract(),
        "toolchain": None,
        "phases": phases,
        "measurements": planned_measurements(phases, args.trials) if args.dry_run else [],
    }
    if args.dry_run:
        write_json(ledger, args.output)
        return 0

    ledger["toolchain"] = {
        "cargo": command_output(["cargo", "--version", "--verbose"], args.after),
        "rustc": command_output(["rustc", "--version", "--verbose"], args.after),
    }
    owned_scratch = args.scratch is None
    scratch = Path(tempfile.mkdtemp(prefix="fszero-compile-ledger-")) if owned_scratch else args.scratch.resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    ledger["scratch"] = str(scratch)
    failed: dict[str, Any] | None = None
    try:
        for phase in phases:
            root = Path(phase["root"])
            _, products = products_for(root)
            for trial in range(1, args.trials + 1):
                for product in products:
                    target = scratch / phase["label"] / f"trial-{trial}" / product.name
                    ledger["measurements"].append(run_build(root, product, target, args.profile, "clean", trial, phase["label"]))
                    touch = root / product.touch
                    if not touch.is_file():
                        raise RuntimeError(f"touch contract missing: {touch}")
                    old_times = touch_for_rebuild(touch)
                    try:
                        ledger["measurements"].append(run_build(root, product, target, args.profile, "touched-incremental", trial, phase["label"]))
                    finally:
                        os.utime(touch, ns=old_times)
    except BuildFailure as exc:
        ledger["measurements"].append(exc.record)
        failed = exc.record
    finally:
        ledger["completed"] = failed is None
        ledger["failure"] = failed
        write_json(ledger, args.output)
        if owned_scratch:
            shutil.rmtree(scratch, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
