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

    def test_shared_candidate_rules_are_conservative(self) -> None:
        domain_local = (
            ("fszero", "src/core/session.rs"),
            ("graphzero", "crates/graphzero-store/src/store/session.rs"),
            ("tokenzero", "crates/tokenzero-codemode/src/journal.rs"),
            ("fszero", "src/packaging/release_smoke.rs"),
            ("graphzero", "crates/graphzero-cli/src/packaging.rs"),
            ("tokenzero", "crates/tokenzero-mcp-compat/src/capability_descriptor.rs"),
        )
        for repo, path in domain_local:
            with self.subTest(repo=repo, path=path):
                classification = module.classify_path(repo, path)
                self.assertEqual(classification.kind, "domain-local")
                self.assertEqual(classification.hub_target, None)
                self.assertTrue(classification.justification)

        shared = (
            ("fszero", "src/session_persist.rs", "session-discovery"),
            ("graphzero", "src/discovery.rs", "session-discovery"),
            ("fszero", "src/durable_journal.rs", "store-cas"),
            ("graphzero", "src/journal_helper.rs", "store-cas"),
            ("graphzero", "src/codemode/host.rs", "codemode-host"),
        )
        for repo, path, rule in shared:
            with self.subTest(repo=repo, path=path):
                classification = module.classify_path(repo, path)
                self.assertEqual(classification.kind, "shared-candidate")
                self.assertEqual(classification.rule, rule)

    def test_hub_docs_and_formal_target_zero_testkit(self) -> None:
        for path in ("docs/evidence.rs", "formal/proof.py"):
            with self.subTest(path=path):
                classification = module.classify_path("zerostack", path)
                self.assertEqual(classification.hub_target, "zero-testkit")

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

    def test_inventory_survives_metadata_only_successor_but_rejects_source_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repos = []
            for repo_id in module.REPOSITORIES:
                repo_root = root / repo_id
                module._tiny_repo(repo_root, {"src/domain.rs": f"fn {repo_id}() {{}}\n"})
                repos.append(module.Repo(repo_id, repo_root))
            inventory = module.build_inventory(repos)

            # A successor commit changing only an untracked metadata file must
            # not invalidate unchanged source bindings.
            note = repos[0].root / "NOTE.txt"
            note.write_text("inventory successor\n", encoding="utf-8")
            module._run(["git", "-C", str(repos[0].root), "add", "NOTE.txt"])
            module._run(["git", "-C", str(repos[0].root), "commit", "-qm", "metadata-only successor"])
            self.assertEqual(module.validate_inventory(inventory, repos), [])

            changed = repos[0].root / "src/domain.rs"
            changed.write_text("fn changed() {}\n", encoding="utf-8")
            module._run(["git", "-C", str(repos[0].root), "add", "src/domain.rs"])
            module._run(["git", "-C", str(repos[0].root), "commit", "-qm", "source change"])
            errors = module.validate_inventory(inventory, repos)
            self.assertTrue(any("blob digest mismatch" in error for error in errors))

            added = repos[0].root / "src/added.rs"
            added.write_text("fn added() {}\n", encoding="utf-8")
            module._run(["git", "-C", str(repos[0].root), "add", "src/added.rs"])
            module._run(["git", "-C", str(repos[0].root), "commit", "-qm", "source addition"])
            errors = module.validate_inventory(inventory, repos)
            self.assertTrue(any("uncovered tracked allowed-language file" in error for error in errors))

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
