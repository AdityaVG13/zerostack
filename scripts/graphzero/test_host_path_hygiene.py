#!/usr/bin/env python3
"""Durable regression fixture for host-path hygiene (macOS/Linux/Windows).

Covers the qh33x acceptance gap: a retained test that injects representative
home paths for all three platforms and proves the gate fails, plus the
scrub dirty-check -> scrub -> clean/idempotent cycle. Stdlib only.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO = pathlib.Path(__file__).resolve().parents[2]

# Build host-path shapes dynamically so this file itself stays clean for
# scripts/check_no_host_paths.py (which scans tracked files for /Users/[A-Za-z] etc.).
MAC_PATH = "/Us" + "ers/alice/AI/GraphZero/.beads/issues.jsonl"
LINUX_PATH = "/ho" + "me/bob/projects/GraphZero"
WIN_PATH = "C:" + "\\Users\\bob\\GraphZero"
WIN_FWD_PATH = "C:" + "/Us" + "ers/bob/GraphZero"

HOST_SHAPES = [MAC_PATH, LINUX_PATH, WIN_PATH, WIN_FWD_PATH]


class TestHostPathHygiene(unittest.TestCase):
    def test_gate_regex_catches_all_three_platform_shapes(self):
        """HOST_PATH in check_no_host_paths.py must flag macOS, Linux, Windows."""
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "check_no_host_paths", REPO / "scripts" / "check_no_host_paths.py"
        )
        mod = importlib.util.module_from_spec(spec)  # type: ignore[arg-type]
        assert spec and spec.loader
        spec.loader.exec_module(mod)  # type: ignore[union-attr]

        for shape in HOST_SHAPES:
            with self.subTest(shape=shape):
                self.assertIsNotNone(
                    mod.HOST_PATH.search(shape), f"HOST_PATH should match {shape!r}"
                )
        # ~/ must stay clean
        self.assertIsNone(mod.HOST_PATH.search("~/AI/GraphZero"))
        self.assertIsNone(mod.HOST_PATH.search("/Users"))

    def test_scrub_relativizes_all_three_shapes(self):
        """scrub relativize must rewrite macOS/Linux/Windows homes to ~/."""
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "scrub_beads_export", REPO / "scripts" / "scrub_beads_export.py"
        )
        mod = importlib.util.module_from_spec(spec)  # type: ignore[arg-type]
        assert spec and spec.loader
        spec.loader.exec_module(mod)  # type: ignore[union-attr]

        for shape in HOST_SHAPES:
            with self.subTest(shape=shape):
                out = mod.relativize(f"prefix {shape} suffix")
                self.assertNotIn(shape, out)
                self.assertIn("~", out)

    def test_scrub_dirty_check_scrub_clean_idempotent(self):
        """Dirty --check fails, scrub fixes, second --check and scrub are clean/idempotent."""
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "scrub_beads_export2", REPO / "scripts" / "scrub_beads_export.py"
        )
        mod = importlib.util.module_from_spec(spec)  # type: ignore[arg-type]
        assert spec and spec.loader
        spec.loader.exec_module(mod)  # type: ignore[union-attr]

        dirty_record = {
            "id": "smoke-1",
            "source_repo_path": MAC_PATH,
            "description": f"hit {LINUX_PATH}",
            "notes": f"see {WIN_PATH}",
            "comments": [{"body": WIN_FWD_PATH}],
            "title": "keep title intact /Us" + "ers/alice should not leak via non-allowlisted field",
        }
        clean_record = {
            "id": "smoke-2",
            "source_repo_path": "~/AI/GraphZero",
            "description": "clean",
            "notes": "ok",
        }

        with tempfile.TemporaryDirectory() as td:
            p = pathlib.Path(td) / "issues.jsonl"
            p.write_text(
                json.dumps(dirty_record, ensure_ascii=False, separators=(",", ":"))
                + "\n"
                + json.dumps(clean_record, ensure_ascii=False, separators=(",", ":"))
                + "\n",
                encoding="utf-8",
            )

            # --check must fail on dirty
            rc_check_dirty = mod.scrub_file(p, check_only=True)
            self.assertEqual(rc_check_dirty, 1)
            # file unchanged after --check
            self.assertIn("alice", p.read_text(encoding="utf-8"))

            # scrub must fix one record and exit 0
            rc_scrub = mod.scrub_file(p, check_only=False)
            self.assertEqual(rc_scrub, 0)
            text_after = p.read_text(encoding="utf-8")
            first = json.loads(text_after.splitlines()[0])
            # Canonical policy scrubs every string and normalizes source_repo_path.
            self.assertEqual(first["source_repo_path"], "issues.jsonl")
            self.assertNotIn("/Us" + "ers/alice", first["source_repo_path"])
            self.assertNotIn("/ho" + "me/bob", first["description"])
            self.assertNotIn("C:" + "\\Us" + "ers", first["notes"])
            self.assertNotIn("C:" + "/Us" + "ers", first["comments"][0]["body"])
            self.assertIn("~", first["notes"])
            # Non-routing string fields are scrubbed too; privacy is recursive.
            self.assertNotIn("/Us" + "ers/alice", first["title"])

            # second --check must be clean
            rc_check_clean = mod.scrub_file(p, check_only=True)
            self.assertEqual(rc_check_clean, 0)

            # idempotent second scrub (no dirty)
            before = p.read_text(encoding="utf-8")
            rc_second = mod.scrub_file(p, check_only=False)
            self.assertEqual(rc_second, 0)
            self.assertEqual(p.read_text(encoding="utf-8"), before)

    def test_check_no_host_paths_gate_invocation(self):
        """End-to-end privacy gate still passes on the clean checkout."""
        r = subprocess.run(
            [sys.executable, str(REPO / "scripts" / "check_no_host_paths.py")],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 0, f"{r.stdout} {r.stderr}")


if __name__ == "__main__":
    unittest.main()
