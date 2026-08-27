#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("compile_ledger.py")
SPEC = importlib.util.spec_from_file_location("compile_ledger", SCRIPT)
assert SPEC and SPEC.loader
compile_ledger = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = compile_ledger
SPEC.loader.exec_module(compile_ledger)


class CompileLedgerContractTest(unittest.TestCase):
    def test_workspace_contract_is_surface_and_fixture_narrow(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "crates/fszero-mcp").mkdir(parents=True)
            (root / "crates/fszero-mcp/Cargo.toml").write_text("[package]\nname='fszero-mcp'\nversion='0.1.0'\n")
            architecture, products = compile_ledger.products_for(root)
        self.assertEqual(architecture, "workspace-products-v1")
        self.assertEqual([p.package for p in products], ["fszero-mcp", "fszero-worker", "fszero-cli"])
        for product in products:
            command = product.cargo_args("debug")
            self.assertIn("--no-default-features", command)
            rendered = " ".join(command)
            self.assertNotIn("dev-harness", rendered)
            self.assertNotIn("mcp-http", rendered)
        worker = products[1]
        self.assertEqual(worker.features, ())
        self.assertEqual(worker.engine_features, ())
        self.assertNotIn("surface-codemode", " ".join(worker.cargo_args("debug")))
        self.assertEqual(products[0].features, ("sqlite-system",))
        self.assertEqual(products[2].features, ("sqlite-system",))
        self.assertEqual(products[0].touch, "crates/fszero-mcp/src/main.rs")
        self.assertIn("surface-mcp", products[0].engine_features)

    def test_dense_baseline_contract_records_exact_surface_features(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            architecture, products = compile_ledger.products_for(Path(tmp))
        self.assertEqual(architecture, "dense-package-v1")
        self.assertEqual(products[0].features, ("fszero-ast-sgrep", "surface-mcp"))
        self.assertEqual(products[1].features, ("fszero-ast-sgrep", "surface-codemode"))
        self.assertTrue(products[2].default_features)

    def test_dry_run_emits_three_trial_raw_plan_without_running_cargo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "crates/fszero-mcp").mkdir(parents=True)
            (root / "crates/fszero-mcp/Cargo.toml").write_text("marker")
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), "--before", str(root), "--after", str(root), "--trials", "3", "--dry-run"],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        ledger = json.loads(proc.stdout)
        self.assertEqual(ledger["schema"], "fszero.compile-ledger.v1")
        self.assertTrue(ledger["dry_run"])
        self.assertEqual(ledger["trials"], 3)
        self.assertEqual(len(ledger["measurements"]), 36)
        self.assertEqual({row["kind"] for row in ledger["measurements"]}, {"clean", "touched-incremental"})
        self.assertTrue(all("requested_features" in row for row in ledger["measurements"]))
        self.assertTrue(all("engine_features" in row for row in ledger["measurements"]))

    def test_trials_below_three_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), "--before", tmp, "--after", tmp, "--trials", "2", "--dry-run"],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("--trials must be >= 3", proc.stderr)


if __name__ == "__main__":
    unittest.main()
