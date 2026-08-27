"""Focused tests for benchmark entrypoint shell scripts' portable failure policy.

Covers scripts/benchmark.sh, scripts/criterion.sh, and
scripts/build-profilable.sh: required-tool checks (missing tool exits 2 with an
actionable stderr reason) and exact exit-status propagation from failing
subcommands (no pipeline masking).

Every scenario runs the real scripts with PATH set to a temporary stub
directory ONLY -- the host PATH contents are never consulted. The stub dir
always contains a tiny ``dirname`` implemented with pure shell parameter
expansion and builtins (benchmark.sh uses ``dirname "$0"`` to locate the repo),
plus optional ``uv``/``cargo`` stubs. Missing-tool scenarios are proven by the
absence of any executable ``uv``/``cargo`` in that controlled PATH. No cargo,
rust, benchmark, or network is ever invoked. Bash is resolved once before test
setup and invoked by absolute path.

Run: python3 -m unittest scripts.test_benchmark_shell_policy -v
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = {
    "benchmark": REPO_ROOT / "scripts" / "benchmark.sh",
    "criterion": REPO_ROOT / "scripts" / "criterion.sh",
    "build-profilable": REPO_ROOT / "scripts" / "build-profilable.sh",
}
# Resolved exactly once, before any test manipulates PATH; the child process is
# invoked with this absolute path so it never depends on PATH lookup for bash.
BASH = shutil.which("bash")

_DIRNAME_STUB = """#!/bin/sh
case "$1" in
  */*) printf '%s\\n' "${1%/*}" ;;
  *) printf '%s\\n' "." ;;
esac
"""


def _chmod_exec(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def write_stub(bin_dir: Path, name: str, exit_code: int) -> Path:
    """Write an executable stub that exits with the given status."""
    path = bin_dir / name
    path.write_text(f"#!/bin/sh\nexit {exit_code}\n", encoding="utf-8")
    _chmod_exec(path)
    return path


def write_dirname_stub(bin_dir: Path) -> Path:
    """Write an executable dirname stub using only shell builtins.

    Required because benchmark.sh computes SCRIPT_DIR via ``dirname "$0"`` and
    the stub-only PATH must not fall back to host tools.
    """
    path = bin_dir / "dirname"
    path.write_text(_DIRNAME_STUB, encoding="utf-8")
    _chmod_exec(path)
    return path


class BenchmarkShellPolicyTests(unittest.TestCase):
    def run_script(
        self,
        script: Path,
        *,
        tools: dict[str, int] | None = None,
        env_extra: dict[str, str] | None = None,
        args: list[str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """Run a real script with PATH restricted to a fresh stub dir."""
        with tempfile.TemporaryDirectory() as tmp:
            stub_dir = Path(tmp)
            write_dirname_stub(stub_dir)
            for name, exit_code in (tools or {}).items():
                write_stub(stub_dir, name, exit_code)
            env = dict(os.environ)
            env["PATH"] = str(stub_dir)
            if env_extra:
                env.update(env_extra)
            return subprocess.run(
                [BASH, str(script), *(args or [])],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
            )

    # ---- benchmark.sh ----

    def test_benchmark_missing_uv_exits_2_with_reason(self) -> None:
        proc = self.run_script(SCRIPTS["benchmark"])
        self.assertEqual(proc.returncode, 2)
        self.assertIn("uv is required", proc.stderr)

    def test_benchmark_uv_failure_propagates_exact_exit(self) -> None:
        proc = self.run_script(SCRIPTS["benchmark"], tools={"uv": 42})
        self.assertEqual(proc.returncode, 42)

    def test_benchmark_uv_success_passes_through(self) -> None:
        proc = self.run_script(SCRIPTS["benchmark"], tools={"uv": 0})
        self.assertEqual(proc.returncode, 0)

    def test_benchmark_invalid_profile_exits_2(self) -> None:
        proc = self.run_script(
            SCRIPTS["benchmark"],
            env_extra={"GRAPHZERO_BENCH_PROFILE": "not-a-profile"},
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("unsupported GRAPHZERO_BENCH_PROFILE", proc.stderr)

    # ---- criterion.sh ----

    def test_criterion_missing_cargo_exits_2_with_reason(self) -> None:
        proc = self.run_script(SCRIPTS["criterion"])
        self.assertEqual(proc.returncode, 2)
        self.assertIn("cargo is required", proc.stderr)

    def test_criterion_cargo_failure_propagates_exact_exit(self) -> None:
        proc = self.run_script(SCRIPTS["criterion"], tools={"cargo": 17})
        self.assertEqual(proc.returncode, 17)

    def test_criterion_cargo_success_passes_through(self) -> None:
        proc = self.run_script(SCRIPTS["criterion"], tools={"cargo": 0})
        self.assertEqual(proc.returncode, 0)

    # ---- build-profilable.sh ----

    def test_build_profilable_missing_cargo_exits_2_with_reason(self) -> None:
        proc = self.run_script(SCRIPTS["build-profilable"])
        self.assertEqual(proc.returncode, 2)
        self.assertIn("cargo is required", proc.stderr)

    def test_build_profilable_cargo_failure_propagates_exact_exit(self) -> None:
        proc = self.run_script(SCRIPTS["build-profilable"], tools={"cargo": 23})
        self.assertEqual(proc.returncode, 23)

    def test_build_profilable_cargo_success_passes_through(self) -> None:
        proc = self.run_script(SCRIPTS["build-profilable"], tools={"cargo": 0})
        self.assertEqual(proc.returncode, 0)


if __name__ == "__main__":
    unittest.main()
