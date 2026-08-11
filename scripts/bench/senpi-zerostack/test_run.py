#!/usr/bin/env python3
"""Focused regression tests for Senpi/ZeroStack artifact provenance."""

from __future__ import annotations

import argparse
import importlib.util
import sys
import unittest
from pathlib import Path
from unittest.mock import patch


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


if __name__ == "__main__":
    unittest.main()
