from __future__ import annotations

import importlib.util
import os
import subprocess
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).parents[1] / "scripts" / "run_shared_suite.py"
spec = importlib.util.spec_from_file_location("run_shared_suite", SCRIPT)
assert spec and spec.loader
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class SharedSuiteRunnerTests(unittest.TestCase):
    def test_reference_route_uses_reference_adapter_without_process_binary(self):
        with patch.object(runner.subprocess, "call", return_value=0) as call:
            self.assertEqual(runner.main(["--reference"]), 0)
        command, kwargs = call.call_args
        self.assertIn("reference_adapter.py", " ".join(command[0]))
        self.assertIn("--suite", command[0])
        self.assertEqual(kwargs["cwd"], runner.TESTS_ROOT)

    def test_selected_engine_requires_explicit_binary(self):
        error = StringIO()
        with patch.dict(os.environ, {}, clear=True), redirect_stderr(error):
            with self.assertRaises(SystemExit) as raised:
                runner.main(["fszero"])
        self.assertEqual(raised.exception.code, 2)
        self.assertIn("missing explicit binary", error.getvalue())

    def test_selected_engine_uses_explicit_binary_and_descriptor_namespace(self):
        with patch.object(runner.subprocess, "call", return_value=0) as call:
            self.assertEqual(runner.main(["fszero", "--fszero-bin", "/tmp/fszero-codemode"]), 0)
        command, kwargs = call.call_args
        self.assertEqual(command[0][0], "zerostack-shared-conformance")
        self.assertIn("--ns", command[0])
        self.assertEqual(command[0][command[0].index("--ns") + 1], "fz")
        self.assertEqual(kwargs["cwd"], runner.REPO_ROOT)

    def test_all_runs_adapters_in_descriptor_order_with_explicit_binaries(self):
        commands = []

        def call(command, **kwargs):
            commands.append(command)
            return 0

        with patch.object(runner.subprocess, "call", side_effect=call):
            result = runner.main(
                [
                    "--all",
                    "--fszero-bin",
                    "/tmp/fszero",
                    "--graphzero-bin",
                    "/tmp/graphzero",
                    "--tokenzero-bin",
                    "/tmp/tokenzero",
                ]
            )

        self.assertEqual(result, 0)
        self.assertEqual(len(commands), 4)
        self.assertEqual(
            [command[command.index("--ns") + 1] for command in commands[:3]],
            ["fz", "gz", "tz"],
        )
        self.assertIn("reference_adapter.py", " ".join(commands[3]))

    def test_schema_pairs_use_canonical_contracts_directory(self):
        schema_pairs = Path(__file__).parents[1] / "scripts" / "check_schema_pairs.py"
        spec = importlib.util.spec_from_file_location("check_schema_pairs", schema_pairs)
        assert spec and spec.loader
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        self.assertEqual(checker.SCHEMA_DIR, Path(__file__).parents[1] / "contracts")
        self.assertTrue(checker.SCHEMA_DIR.is_dir())
        self.assertTrue((checker.SCHEMA_DIR / "capability-manifest.schema.json").is_file())

    def test_all_continues_after_engine_failure_and_returns_nonzero(self):
        statuses = iter([7, 0, 3, 0])
        commands = []

        def call(command, **kwargs):
            commands.append(command)
            return next(statuses)

        with patch.object(runner.subprocess, "call", side_effect=call):
            result = runner.main(
                [
                    "--all",
                    "--fszero-bin",
                    "/tmp/fszero",
                    "--graphzero-bin",
                    "/tmp/graphzero",
                    "--tokenzero-bin",
                    "/tmp/tokenzero",
                ]
            )

        self.assertEqual(result, 7)
        self.assertEqual(len(commands), 4)

    def test_schema_pair_checker_runs_against_canonical_contracts(self):
        result = subprocess.run(
            ["python3", str(Path(__file__).parents[1] / "scripts" / "check_schema_pairs.py")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ok:", result.stdout)


if __name__ == "__main__":
    unittest.main()
