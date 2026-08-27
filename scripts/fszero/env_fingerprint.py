#!/usr/bin/env python3
"""Emit a bounded machine-readable fingerprint for FSZero performance runs."""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_VERSION = "fszero.perf-fingerprint.v1"
CACHE_STATES = ("cold", "warm", "mixed", "varies")


def redact_private(value: str | None, root: Path) -> str | None:
    if value is None:
        return None
    replacements = [(str(Path.home()), "$HOME")]
    if root != Path(root.anchor):
        replacements.append((str(root), "$ROOT"))
    replacements.sort(key=lambda item: len(item[0]), reverse=True)
    for private, label in replacements:
        if private:
            value = value.replace(private, label)
    return value


def command_value(argv: list[str], *, cwd: Path | None = None) -> tuple[str | None, str | None]:
    try:
        run = subprocess.run(
            argv,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except FileNotFoundError:
        return None, "command_not_found"
    except subprocess.TimeoutExpired:
        return None, "timeout"
    if run.returncode != 0:
        return None, f"exit_{run.returncode}"
    value = run.stdout.strip()
    return (value, None) if value else (None, "empty_output")


def cpu_model() -> tuple[str | None, str | None]:
    if platform.system() == "Darwin":
        return command_value(["sysctl", "-n", "machdep.cpu.brand_string"])
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text(errors="replace").splitlines():
            key, separator, value = line.partition(":")
            if separator and key.strip() in {"model name", "Hardware"}:
                return value.strip(), None
    value = platform.processor().strip() or platform.machine().strip()
    return (value, None) if value else (None, "unavailable")


def power_mode() -> dict[str, object]:
    if platform.system() == "Linux":
        path = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        if path.is_file():
            value = path.read_text(errors="replace").strip()
            return {"kind": "scaling_governor", "value": value or None,
                    "status": "available" if value else "unavailable"}
        return {"kind": "scaling_governor", "value": None, "status": "unavailable"}
    if platform.system() == "Darwin":
        value, error = command_value(["pmset", "-g", "custom"])
        match = re.search(r"^\s*lowpowermode\s+(\S+)", value or "", re.MULTILINE)
        return {
            "kind": "macos_low_power_mode",
            "value": match.group(1) if match else None,
            "status": "available" if match else "unavailable",
            "reason": None if match else error or "lowpowermode_not_reported",
        }
    return {"kind": "platform_power_mode", "value": None,
            "status": "not_applicable"}


def filesystem(root: Path) -> dict[str, object]:
    mount_text, mount_error = command_value(["df", "-P", str(root)])
    mount_source = None
    mount_point = None
    if mount_text:
        fields = mount_text.splitlines()[-1].split(maxsplit=5)
        if len(fields) == 6:
            mount_source, mount_point = fields[0], fields[5]
        else:
            mount_error = "unexpected_df_shape"

    if platform.system() == "Darwin":
        fs_type = None
        fs_error = "mount_entry_not_found"
        mounts, mounts_error = command_value(["mount"])
        if mounts and mount_source and mount_point:
            prefix = f"{mount_source} on {mount_point} ("
            entry = next((line for line in mounts.splitlines() if line.startswith(prefix)), None)
            match = re.search(r"\(([^,\s)]+)", entry or "")
            if match:
                fs_type, fs_error = match.group(1), None
            elif mounts_error:
                fs_error = mounts_error
    else:
        fs_type, fs_error = command_value(["stat", "-f", "-c", "%T", str(root)])
    mount_source = redact_private(mount_source, root)
    mount_point = redact_private(mount_point, root)
    return {
        "type": fs_type,
        "type_status": "available" if fs_type else "unavailable",
        "type_reason": fs_error,
        "mount_source": mount_source,
        "mount_point": mount_point,
        "mount_status": "available" if mount_source and mount_point else "unavailable",
        "mount_reason": mount_error,
    }

def git_state(root: Path) -> dict[str, object]:
    sha, sha_error = command_value(["git", "rev-parse", "HEAD"], cwd=root)
    status, status_error = command_value(
        ["git", "status", "--porcelain", "--untracked-files=no"], cwd=root
    )
    # Empty output is the valid clean state, unlike other command probes.
    if status_error == "empty_output":
        status, status_error = "", None
    return {
        "git_sha": sha,
        "git_dirty": bool(status) if status_error is None else None,
        "status": "available" if sha and status_error is None else "unavailable",
        "reason": sha_error or status_error,
    }



def derive_host_class(cpu_model_value: str | None) -> dict[str, object]:
    """Stable host-class label for same-host gate discipline.

    Absolute latency comparisons and gates require matching host_class.
    Published local baselines use labels like local-m5-max; GitHub Actions
    runners use gha-* labels. Override with FSZERO_PERF_HOST_CLASS when the
    auto label is wrong (e.g. a non-Max M-series host that still publishes).
    """
    override = os.environ.get("FSZERO_PERF_HOST_CLASS", "").strip()
    if override:
        return {
            "host_class": override,
            "host_class_source": "env:FSZERO_PERF_HOST_CLASS",
            "status": "provided",
        }

    system = platform.system()
    machine = platform.machine() or "unknown"
    model = (cpu_model_value or "").strip()

    if os.environ.get("GITHUB_ACTIONS") == "true":
        # Prefer the workflow runs-on label when callers stamp it; else OS+arch.
        runs_on = os.environ.get("FSZERO_PERF_RUNS_ON", "").strip()
        if runs_on:
            label = f"gha-{runs_on}"
            source = "env:FSZERO_PERF_RUNS_ON"
        else:
            # ImageOS is e.g. ubuntu22/macos14; fall back to platform.
            image_os = os.environ.get("ImageOS", "").strip().lower()
            if image_os:
                label = f"gha-{image_os}-{machine.lower()}"
                source = "env:ImageOS+machine"
            else:
                label = f"gha-{system.lower()}-{machine.lower()}"
                source = "github_actions+platform"
        return {
            "host_class": label,
            "host_class_source": source,
            "status": "derived",
        }

    # Local Darwin published baselines historically label Apple M5 Max.
    if system == "Darwin" and model:
        compact = model.lower().replace(" ", "-")
        if "apple" in compact and "m5" in compact and "max" in compact:
            label = "local-m5-max"
        elif "apple" in compact:
            # e.g. Apple M4 Pro -> local-apple-m4-pro
            label = f"local-{compact}"
        else:
            label = f"local-darwin-{machine.lower()}"
        return {
            "host_class": label,
            "host_class_source": "cpu_model",
            "status": "derived",
        }

    if system == "Linux":
        label = f"local-linux-{machine.lower()}"
        return {
            "host_class": label,
            "host_class_source": "platform",
            "status": "derived",
        }

    label = f"local-{system.lower()}-{machine.lower()}"
    return {
        "host_class": label,
        "host_class_source": "platform",
        "status": "derived",
    }


def runner_image_provenance() -> dict[str, object]:
    """CI runner image identity for artifact provenance (GHA ImageOS/ImageVersion)."""
    if os.environ.get("GITHUB_ACTIONS") != "true":
        return {
            "runner_class": None,
            "image_os": None,
            "image_version": None,
            "runs_on": os.environ.get("FSZERO_PERF_RUNS_ON"),
            "status": "not_ci",
            "reason": "GITHUB_ACTIONS not set",
        }
    image_os = os.environ.get("ImageOS")
    image_version = os.environ.get("ImageVersion")
    runs_on = os.environ.get("FSZERO_PERF_RUNS_ON")
    runner_name = os.environ.get("RUNNER_NAME")
    runner_os = os.environ.get("RUNNER_OS")
    runner_arch = os.environ.get("RUNNER_ARCH")
    present = any([image_os, image_version, runs_on, runner_name])
    return {
        "runner_class": runs_on or (
            f"gha-{(image_os or (runner_os or 'unknown')).lower()}"
        ),
        "image_os": image_os,
        "image_version": image_version,
        "runs_on": runs_on,
        "runner_name": runner_name,
        "runner_os": runner_os,
        "runner_arch": runner_arch,
        "status": "available" if present else "unavailable",
        "reason": None if present else "GHA image env vars not set",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT,
                        help="repository whose git and filesystem state is captured")
    parser.add_argument("--run-id", default=os.environ.get("FSZERO_PERF_RUN_ID"))
    parser.add_argument(
        "--cache-state",
        choices=CACHE_STATES,
        default=os.environ.get("FSZERO_PERF_CACHE_STATE", "varies"),
    )
    parser.add_argument(
        "--cargo-profile",
        default=os.environ.get("FSZERO_PERF_CARGO_PROFILE", "unspecified"),
    )
    parser.add_argument("--isolation-note",
                        default=os.environ.get("FSZERO_PERF_ISOLATION_NOTE"))
    args = parser.parse_args()
    root = args.root.resolve()
    if not root.is_dir():
        parser.error(f"--root is not a directory: {root}")

    captured = datetime.now(timezone.utc)
    run_id = args.run_id or f"{captured.strftime('%Y%m%dT%H%M%SZ')}-{os.getpid()}"
    model, model_error = cpu_model()
    rustc, rustc_error = command_value(["rustc", "--version", "--verbose"])
    cargo, cargo_error = command_value(["cargo", "--version", "--verbose"])
    rustc = redact_private(rustc, root)
    cargo = redact_private(cargo, root)
    isolation_note = redact_private(args.isolation_note, root)
    document = {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "captured_at_utc": captured.isoformat().replace("+00:00", "Z"),
        "cache_state": args.cache_state,
        "repository": git_state(root),
        "cpu": {
            "model": model,
            "model_status": "available" if model else "unavailable",
            "model_reason": model_error,
            "logical_cores": os.cpu_count(),
        },
        "power": power_mode(),
        "kernel": {
            "system": platform.system(),
            "release": platform.release(),
            "version": platform.version(),
            "machine": platform.machine(),
        },
        "toolchain": {
            "rustc": rustc,
            "rustc_status": "available" if rustc else "unavailable",
            "rustc_reason": rustc_error,
            "cargo": cargo,
            "cargo_status": "available" if cargo else "unavailable",
            "cargo_reason": cargo_error,
            "cargo_profile": args.cargo_profile,
            "python": platform.python_version(),
        },
        "filesystem": filesystem(root),
        "isolation": {
            "note": isolation_note,
            "status": "provided" if isolation_note else "not_provided",
        },
        "host": {
            **derive_host_class(model),
            "runner_image": runner_image_provenance(),
        },
    }
    json.dump(document, fp=sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
