"""Mutation tests for the semantic ownership inventory validator."""
from __future__ import annotations
import copy
import importlib.util
import json
import sys
import unittest
from pathlib import Path
SCRIPT = Path(__file__).parents[1] / "scripts" / "check_semantic_ownership.py"
spec = importlib.util.spec_from_file_location("check_semantic_ownership", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
DATA = Path(__file__).parents[1] / "data"

class SemanticOwnershipTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.semantic = json.loads((DATA / "semantic_ownership_inventory_v1.json").read_text())
        cls.loc = json.loads((DATA / "loc_ownership_v1.json").read_text())
        cls.ledger = json.loads((DATA / "temporary_adoption_ledger_v1.json").read_text())

    def validate(self, semantic=None, loc=None, ledger=None):
        return module.validate_documents(semantic or self.semantic, loc or self.loc, ledger or self.ledger)

    def test_reviewed_documents_are_internally_consistent(self) -> None:
        self.assertEqual(self.validate(), [])

    def test_duplicate_and_uncovered_records_fail_closed(self) -> None:
        duplicate = copy.deepcopy(self.semantic)
        duplicate["records"].append(copy.deepcopy(duplicate["records"][0]))
        self.assertTrue(any("duplicate semantic" in error for error in self.validate(semantic=duplicate)))
        uncovered = copy.deepcopy(self.semantic)
        uncovered["records"] = uncovered["records"][1:]
        self.assertTrue(any("uncovered tracked implementation" in error for error in self.validate(semantic=uncovered)))

    def test_digest_action_and_target_mutations_fail_closed(self) -> None:
        changed = copy.deepcopy(self.semantic)
        changed["records"][0]["blob_digest"] = "0" * 40
        self.assertTrue(any("blob digest differs" in error for error in self.validate(semantic=changed)))
        changed = copy.deepcopy(self.semantic)
        changed["records"][0]["action"] = "invented-action"
        self.assertTrue(any("invalid action" in error for error in self.validate(semantic=changed)))
        changed = copy.deepcopy(self.semantic)
        record = next(item for item in changed["records"] if item["action"] != "keep-domain")
        record["hub_target"] = ""
        self.assertTrue(any("requires a hub target" in error for error in self.validate(semantic=changed)))

    def test_temporary_ledger_is_exact(self) -> None:
        missing = copy.deepcopy(self.ledger)
        missing["entries"] = missing["entries"][1:]
        self.assertTrue(any("temporary duplicate missing" in error for error in self.validate(ledger=missing)))
        extra = copy.deepcopy(self.ledger)
        extra_entry = copy.deepcopy(extra["entries"][0])
        extra_entry["path"] = "not/a/real/path.rs"
        extra["entries"].append(extra_entry)
        self.assertTrue(any("no semantic record" in error for error in self.validate(ledger=extra)))

    def test_split_boundaries_are_bounded(self) -> None:
        changed = copy.deepcopy(self.semantic)
        record = next(item for item in changed["records"] if item["split_boundaries"])
        record["split_boundaries"][0]["end_line"] = 0
        self.assertTrue(any("invalid split boundary" in error for error in self.validate(semantic=changed)))

if __name__ == "__main__": unittest.main()
