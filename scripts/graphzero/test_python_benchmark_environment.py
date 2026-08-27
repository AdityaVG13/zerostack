"""Contract test: benchmark Python drivers are stdlib-only and locked.

Proves the environment contract declared by `.python-version`, `pyproject.toml`,
and `uv.lock`:

- every import reachable from the four benchmark entry modules
  (`scripts/benchmark_driver.py`, `scripts/bench_ratchet.py`,
  `benchmarks/rebaseline/run.py`, `benchmarks/rebaseline/stats.py`) resolves to
  the standard library or the local module `stats`;
- `.python-version` pins interpreter minor 3.13;
- `pyproject.toml` declares `requires-python = ">=3.13,<3.14"` and zero
  dependencies (non-package project);
- `uv.lock` records the same Python bound (`==3.13.*`) and contains no
  third-party package entries (only the virtual project itself).

Run: python3 -m unittest scripts.test_python_benchmark_environment -v
"""

from __future__ import annotations

import ast
import sys
import tomllib
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PYPROJECT = REPO_ROOT / "pyproject.toml"
UV_LOCK = REPO_ROOT / "uv.lock"
PY_VERSION_FILE = REPO_ROOT / ".python-version"

#: Entry modules whose full import closure must stay stdlib-only plus `stats`.
ENTRY_MODULES = (
    "scripts/benchmark_driver.py",
    "scripts/bench_ratchet.py",
    "benchmarks/rebaseline/run.py",
    "benchmarks/rebaseline/stats.py",
)

#: Local-only module that may be imported (sibling of run.py in
#: benchmarks/rebaseline); the drivers put that directory on sys.path.
ALLOWED_LOCAL = {"stats"}

STDLIB = frozenset(sys.stdlib_module_names)


def import_closure(entries: tuple[str, ...], repo_root: Path) -> set[str]:
    """Return every top-level import name reachable from the entry modules."""
    seen_files: set[Path] = set()
    imports: set[str] = set()

    def enqueue(name: str) -> None:
        # Local modules live either at the repo root (scripts importable as
        # scripts.*) or as a sibling of run.py (benchmarks/rebaseline adds
        # itself to sys.path); stdlib paths simply do not exist on disk.
        rel = name.replace(".", "/") + ".py"
        candidates = [repo_root / rel, repo_root / "benchmarks" / "rebaseline" / rel]
        for candidate in candidates:
            if candidate.is_file():
                queue.append(candidate)

    queue = [repo_root / entry for entry in entries]
    while queue:
        path = queue.pop()
        path = path.resolve()
        if path in seen_files:
            continue
        seen_files.add(path)
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    top = alias.name.split(".")[0]
                    imports.add(top)
                    enqueue(alias.name)
            elif isinstance(node, ast.ImportFrom):
                module = node.module or ""
                if module:
                    top = module.split(".")[0]
                    imports.add(top)
                    enqueue(module)
    return imports


class PythonBenchmarkEnvironmentTests(unittest.TestCase):
    def test_import_closure_is_stdlib_or_local_stats(self) -> None:
        imports = import_closure(ENTRY_MODULES, REPO_ROOT)
        unexpected = sorted(imports - STDLIB - ALLOWED_LOCAL)
        self.assertEqual(
            unexpected,
            [],
            f"non-stdlib imports reachable from benchmark drivers: {unexpected}",
        )
        self.assertIn("stats", imports)  # the intended local module is used

    def test_python_version_file_pins_minor_313(self) -> None:
        self.assertTrue(PY_VERSION_FILE.is_file(), f"missing {PY_VERSION_FILE}")
        self.assertEqual(PY_VERSION_FILE.read_text(encoding="utf-8").strip(), "3.13")

    def test_pyproject_declares_bound_and_no_dependencies(self) -> None:
        self.assertTrue(PYPROJECT.is_file(), f"missing {PYPROJECT}")
        with PYPROJECT.open("rb") as fh:
            data = tomllib.load(fh)
        project = data["project"]
        self.assertEqual(project["requires-python"], ">=3.13,<3.14")
        self.assertEqual(project["dependencies"], [])
        self.assertEqual(data["tool"]["uv"]["package"], False)

    def test_uv_lock_has_no_third_party_packages(self) -> None:
        self.assertTrue(UV_LOCK.is_file(), f"missing {UV_LOCK}")
        with UV_LOCK.open("rb") as fh:
            lock = tomllib.load(fh)
        self.assertEqual(lock["requires-python"], "==3.13.*")
        packages = lock.get("package", [])
        # Only the virtual project itself may appear; any other entry would be
        # a third-party dependency, which the contract forbids.
        names = [p.get("name") for p in packages]
        self.assertEqual(
            names,
            ["graphzero-python-tools"],
            f"unexpected packages in uv.lock: {names}",
        )


if __name__ == "__main__":
    unittest.main()
