#!/usr/bin/env python3
"""Focused regression tests for Senpi/ZeroStack artifact provenance."""

from __future__ import annotations

import argparse
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("senpi_zerostack_run", HERE / "run.py")
assert SPEC is not None and SPEC.loader is not None
run = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = run
SPEC.loader.exec_module(run)

SENPI_REVISION = "a" * 40
ZEROSTACK_REVISION = "b" * 40
CONFIG = {
    "comparison": {
        "assembly_manifest": {
            "senpi_revision": SENPI_REVISION,
            "zerostack_revision": ZEROSTACK_REVISION,
        }
    }
}


def fixture_args(*, zerostack_revision: str | None = None) -> argparse.Namespace:
    return argparse.Namespace(
        senpi_root=Path("/fixture/senpi"),
        zerostack_root=Path("/fixture/zerostack"),
        zerostack_revision=zerostack_revision,
        zerostack_host=Path("/fixture/zsx"),
        driver=Path("/fixture/driver.ts"),
        identity=Path("/fixture/identity.json"),
    )


class ArtifactFactTests(unittest.TestCase):
    @patch.object(run, "run_text", return_value="zsx 1.0")
    @patch.object(run, "digest_file", return_value="c" * 64)
    @patch.object(run, "host_facts", return_value={"system": "fixture"})
    @patch.object(
        run,
        "git_fact",
        side_effect=[
            {"head": SENPI_REVISION, "tracked_dirty": False},
            {"head": ZEROSTACK_REVISION, "tracked_dirty": False},
        ],
    )
    def test_matching_revisions_reach_artifact_collection(
        self,
        git_fact,
        _host_facts,
        _digest_file,
        _run_text,
    ) -> None:
        facts = run.collect_artifact_facts(fixture_args(), CONFIG)

        self.assertEqual(git_fact.call_args_list[1].args, (Path("/fixture/zerostack"),))
        self.assertEqual(facts["zerostack"]["head"], ZEROSTACK_REVISION)
        self.assertEqual(facts["zerostack"]["binary_sha256"], "c" * 64)

    @patch.object(
        run,
        "git_fact",
        side_effect=[
            {"head": SENPI_REVISION, "tracked_dirty": False},
            {"head": "d" * 40, "tracked_dirty": False},
        ],
    )
    def test_mismatched_zerostack_revision_fails_before_artifact_collection(self, _git_fact) -> None:
        with self.assertRaisesRegex(RuntimeError, "ZeroStack revision"):
            run.collect_artifact_facts(fixture_args(), CONFIG)

    @patch.object(run, "run_text", return_value="zsx 1.0")
    @patch.object(run, "digest_file", return_value="c" * 64)
    @patch.object(run, "host_facts", return_value={"system": "fixture"})
    @patch.object(
        run,
        "git_fact",
        side_effect=[
            {"head": SENPI_REVISION, "tracked_dirty": False},
            {"head": "e" * 40, "tracked_dirty": False},
        ],
    )
    def test_explicit_zerostack_revision_override_matches(
        self,
        _git_fact,
        _host_facts,
        _digest_file,
        _run_text,
    ) -> None:
        facts = run.collect_artifact_facts(fixture_args(zerostack_revision="e" * 40), CONFIG)
        self.assertEqual(facts["zerostack"]["head"], "e" * 40)


class StartArmsCleanupTests(unittest.TestCase):
    def test_handshake_failure_reaps_both_children(self) -> None:
        senpi = Mock()
        zero = Mock()
        for process in (senpi, zero):
            process.poll.return_value = None
            process.stdin = io.StringIO()
            process.stdout = io.StringIO("")
            process.stderr = io.StringIO("")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tsx = root / "node_modules/.bin/tsx"
            context = root / "packages/senpi-codemode/src/kernels/js/context-manager.ts"
            tsx.parent.mkdir(parents=True)
            context.parent.mkdir(parents=True)
            tsx.write_text("tsx\n", encoding="utf-8")
            context.write_text("context\n", encoding="utf-8")
            args = argparse.Namespace(
                senpi_root=root,
                driver=root / "driver.ts",
                zerostack_host=root / "zsx",
            )
            with (
                patch.object(run.subprocess, "Popen", side_effect=[senpi, zero]),
                patch.object(run.SenpiArm, "read", side_effect=RuntimeError("no ready")),
            ):
                with self.assertRaisesRegex(RuntimeError, "no ready"):
                    run.start_arms(args, root / "scratch")
        senpi.kill.assert_called_once()
        zero.kill.assert_called_once()
        senpi.wait.assert_called_once()
        zero.wait.assert_called_once()


if __name__ == "__main__":
    unittest.main()
