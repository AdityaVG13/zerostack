from __future__ import annotations

import json
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[1]
MANIFEST_PATH = ROOT.parent / "docs/release-cleanup-manifest-v1.json"
TOPOLOGY_PATH = ROOT.parent / "conformance/engine-topology-v1.json"


def load(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise AssertionError(f"{path} must contain an object")
    return value


class ReleaseCleanupManifestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.manifest = load(MANIFEST_PATH)
        cls.topology = load(TOPOLOGY_PATH)

    def test_scope_baselines_and_heads_are_complete(self) -> None:
        scope = self.manifest["scope"]
        repositories = {item["id"]: item for item in self.manifest["repositories"]}
        self.assertEqual(
            scope,
            ["zerostack", "tokenzero", "fszero", "graphzero", "prime-zerostack"],
        )
        self.assertEqual(set(repositories), set(scope))
        for repository_id, item in repositories.items():
            baseline = item["baseline"]
            self.assertGreater(baseline["inventory_files"], 0, repository_id)
            self.assertGreater(baseline["inventory_bytes"], 0, repository_id)
            if repository_id == "prime-zerostack":
                self.assertEqual(baseline["vcs_state"], "untracked-package")
                self.assertEqual(baseline["tracked_files"], 0)
            else:
                self.assertRegex(baseline["source_head"], r"^[0-9a-f]{40}$")
            self.assertGreater(baseline["source_and_doc_lines"], 0, repository_id)

    def test_engine_migration_candidates_equal_topology(self) -> None:
        batches = {item["id"]: item for item in self.manifest["batches"]}
        dispositions = {"move": "move", "merge": "consolidate", "retire-from-release": "archive"}
        for engine in self.topology["engines"]:
            engine_id = engine["id"]
            expected = {
                (
                    item["path"],
                    item["kind"],
                    item["name"],
                    dispositions[item["disposition"]],
                    item["target_package"],
                    item["target_binary"],
                )
                for item in engine["current_to_target"]
                if item["disposition"] != "keep"
            }
            actual = {
                (
                    item["path"],
                    item["kind"],
                    item["current_name"],
                    item["classification"],
                    item["replacement_package"],
                    item["replacement_binary"],
                )
                for item in batches[f"{engine_id}-topology-migration"]["entries"]
            }
            self.assertEqual(actual, expected, engine_id)

    def test_inventory_never_authorizes_deletion(self) -> None:
        self.assertEqual(self.manifest["status"], "inventory-only-no-deletion-authority")
        rules = self.manifest["rules"]
        self.assertFalse(rules["history_rewrite"])
        self.assertFalse(rules["deletions_performed"])
        self.assertEqual(rules["operator_approval_granted_batches"], [])
        allowed = {"keep", "move", "consolidate", "archive", "generate", "delete-from-public-release", "delete-before-first-track"}
        for batch in self.manifest["batches"]:
            self.assertFalse(batch["delete_now"], batch["id"])
            self.assertNotEqual(batch["approval_state"], "granted", batch["id"])
            self.assertTrue(batch["entries"], batch["id"])
            for item in batch["entries"]:
                self.assertIn(item["classification"], allowed)
                path = item["path"]
                self.assertTrue(path and not path.startswith(("/", "~", "\\")))
                self.assertNotIn("..", Path(path).parts)
                self.assertNotIn("\\", path)

    def test_manifest_contains_no_host_paths(self) -> None:
        encoded = json.dumps(self.manifest, sort_keys=True)
        for forbidden in ("/Users/", "\\Users\\", "aditya"):
            self.assertNotIn(forbidden, encoded)


if __name__ == "__main__":
    unittest.main()
