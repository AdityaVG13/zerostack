"""Mutation coverage for the tracked implementation LOC gate."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts" / "check_loc_majority.py"
spec = importlib.util.spec_from_file_location("check_loc_majority", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class LocMajorityTests(unittest.TestCase):
    def test_self_test_covers_required_mutations(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--self-test"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertIn("provenance", result.stdout)
        self.assertIn("threshold mutations", result.stdout)
        self.assertIn("cross-repo duplicate", result.stdout)

    def test_language_set_is_exact_and_extensionless_python_is_content_bound(self) -> None:
        self.assertEqual(
            set(module.LANGUAGES),
            {
                "Rust",
                "Python",
                "TypeScript",
                "JavaScript",
                "C",
                "C++",
                "Shell",
                "PowerShell",
                "Zsh",
                "Ruby",
                "Go",
                "Swift",
            },
        )
        self.assertEqual(module.language_for("scripts/zs", b"#!/usr/bin/env python3\n"), "Python")
        self.assertIsNone(module.language_for("README", b"plain text\n"))

    def test_threshold_equality_is_not_a_pass(self) -> None:
        self.assertFalse(module._threshold(50, 100))
        self.assertTrue(module._threshold(51, 100))

    def test_missing_or_omitted_denominator_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = []
            for repo_id in module.REPOSITORIES:
                repo_root = root / repo_id
                module._tiny_repo(repo_root, {"src/domain.rs": f"fn {repo_id}() {{}}\n"})
                repos.append(module.Repo(repo_id, repo_root))
            inventory = module.build_inventory(repos)
            with self.assertRaises(module.GateError):
                module.measure(repos[:3], inventory)
            with self.assertRaises(module.GateError):
                module.measure(
                    [module.Repo("tokenzero", root / "missing"), *repos[1:]],
                    inventory,
                )

    def test_inventory_rejects_uncovered_duplicate_and_domain_mutations(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = []
            for repo_id in module.REPOSITORIES:
                repo_root = root / repo_id
                module._tiny_repo(repo_root, {"src/domain.rs": f"fn {repo_id}() {{}}\n"})
                repos.append(module.Repo(repo_id, repo_root))
            inventory = module.build_inventory(repos)

            uncovered = json.loads(json.dumps(inventory))
            uncovered["files"] = uncovered["files"][1:]
            self.assertTrue(module.validate_inventory(uncovered, repos))

            multiply = json.loads(json.dumps(inventory))
            multiply["files"].append(dict(multiply["files"][0]))
            self.assertTrue(module.validate_inventory(multiply, repos))

            misclassified = json.loads(json.dumps(inventory))
            misclassified["files"][0]["classification"] = "domain-local"
            self.assertTrue(module.validate_inventory(misclassified, repos))

    def test_fixture_tree_inflation_and_hub_engine_copy_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = []
            for repo_id in module.REPOSITORIES:
                repo_root = root / repo_id
                module._tiny_repo(repo_root, {"src/domain.rs": f"fn {repo_id}() {{}}\n"})
                repos.append(module.Repo(repo_id, repo_root))
            inventory = module.build_inventory(repos)
            baseline = module.measure(repos, inventory)
            fixture = repos[0].root / "fixtures/giant.rs"
            fixture.parent.mkdir(parents=True)
            fixture.write_text("".join(f"fn giant_{i}() {{}}\n" for i in range(1000)), encoding="utf-8")
            module._run(["git", "-C", str(repos[0].root), "add", "fixtures/giant.rs"])
            module._run(["git", "-C", str(repos[0].root), "commit", "-qm", "fixture-padding"])
            fixture_result = module.measure(repos, module.build_inventory(repos))
            self.assertEqual(fixture_result["denominator"], baseline["denominator"])
            self.assertEqual(fixture_result["hub_share"], baseline["hub_share"])

            copied = repos[0].root / "src/copied_engine.rs"
            copied.write_bytes((repos[1].root / "src/domain.rs").read_bytes())
            module._run(["git", "-C", str(repos[0].root), "add", "src/copied_engine.rs"])
            module._run(["git", "-C", str(repos[0].root), "commit", "-qm", "duplicate-engine"])
            result = module.measure(repos, module.build_inventory(repos))
            self.assertFalse(result["pass"])
            self.assertGreater(result["violations"]["count"], 0)
            self.assertEqual(result["hub"]["implementation_loc"], baseline["hub"]["implementation_loc"])
            self.assertLess(result["hub_share"], baseline["hub_share"])

    def test_untracked_only_gain_does_not_change_head_bound_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = []
            for repo_id in module.REPOSITORIES:
                repo_root = root / repo_id
                module._tiny_repo(repo_root, {"src/domain.rs": f"fn {repo_id}() {{}}\n"})
                repos.append(module.Repo(repo_id, repo_root))
            inventory = module.build_inventory(repos)
            before = module.measure(repos, inventory)
            (repos[0].root / "src/untracked.rs").write_text("fn padding() {}\n", encoding="utf-8")
            after = module.measure(repos, inventory)
            self.assertEqual(before["source_heads"], after["source_heads"])
            self.assertEqual(before["denominator"], after["denominator"])
            self.assertEqual(before["hub_share"], after["hub_share"])


if __name__ == "__main__":
    unittest.main()
