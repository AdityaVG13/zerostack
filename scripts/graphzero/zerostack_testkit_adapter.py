#!/usr/bin/env python3
"""GraphZero thin adapter for canonical ZeroStack test-policy scripts."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from types import ModuleType

ENGINE_ROOT = Path(__file__).resolve().parents[2]


def hub_root() -> Path:
    """Resolve the sibling ZeroStack source selected by Cargo metadata."""
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=ENGINE_ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    try:
        packages = json.loads(result.stdout)["packages"]  # ubs:ignore — JSONDecodeError is converted below.
    except (json.JSONDecodeError, KeyError) as error:
        raise RuntimeError("Cargo metadata returned malformed JSON") from error
    for package in packages:
        if package["name"] != "zero-abi":
            continue
        root = Path(package["manifest_path"]).resolve().parents[3]
        if (root / "scripts" / "check-portability.sh").is_file():
            return root
    raise RuntimeError("Cargo metadata does not contain a usable sibling ZeroStack source")


def load_hub_script(script_name: str, module_name: str) -> ModuleType:
    """Load one canonical script without copying its policy implementation."""
    path = hub_root() / "scripts" / script_name
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load canonical ZeroStack script: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module
