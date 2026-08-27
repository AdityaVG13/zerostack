"""Focused fixture tests for scripts/check_spec_tags.py.

Run: uv run python -m unittest scripts.test_check_spec_tags
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("check_spec_tags.py")
spec = importlib.util.spec_from_file_location("graphzero_check_spec_tags", MODULE_PATH)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)

REPO = Path(__file__).resolve().parents[2]

TEST_NAMES = [
    "all_five_modes_use_stable_names",
    "report_mapping_is_lossless_and_failure_only",
    "canonical_bytes_ignore_input_and_object_key_order",
    "round_trip_and_reject_malformed_contracts",
    "reject_digest_empty_failure_evidence_and_unsorted_refs",
    "stable_sort_retains_duplicate_failures",
]


def harness_source(names: list[str] | None = None) -> str:
    names = TEST_NAMES if names is None else names
    blocks: list[str] = []
    for name in names:
        blocks.append(f"#[test]\nfn {name}() {{ let _ = 1; }}\n")
    return "use graphzero_query::oracle::OracleMode;\n" + "\n".join(blocks)


def ledger_source(
    names: list[str] | None = None,
    *,
    status: str = "VERIFIED",
    id_prefix: str = "SPEC-GZ",
    source_ref: str = "src/oracle.rs::OracleMode",
    verifier_file: str | None = None,
    requirement: str = "requirement text",
) -> str:
    names = TEST_NAMES if names is None else names
    verifier_file = verifier_file or "tests/oracle_harness.rs"
    rows = []
    for idx, name in enumerate(names, 1):
        rows.append(
            f"| {id_prefix}-{idx:03d} | {requirement} | {source_ref} | "
            f"{verifier_file}::{name} | {status} |"
        )
    return (
        "| ID | Requirement | Source | Verifier | Status |\n"
        "|---|---|---|---|---|\n"
        + "\n".join(rows)
        + "\n"
    )


class CheckerTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def write(
        self,
        ledger: str | None = None,
        harness: str | None = None,
        ledger_path: str = "docs/spec-tags.md",
        harness_path: str = "tests/oracle_harness.rs",
    ) -> tuple[Path, Path]:
        if ledger is not None:
            (self.dir / ledger_path).parent.mkdir(parents=True, exist_ok=True)
            (self.dir / ledger_path).write_text(ledger, encoding="utf-8")
        if harness is not None:
            (self.dir / harness_path).parent.mkdir(parents=True, exist_ok=True)
            (self.dir / harness_path).write_text(harness, encoding="utf-8")
        # Stub the default source ref so green fixtures have a file to check.
        stub = self.dir / "src" / "oracle.rs"
        if ledger is not None and not stub.exists():
            stub.parent.mkdir(parents=True, exist_ok=True)
            stub.write_text("pub enum OracleMode { Gold }\n", encoding="utf-8")
        return self.dir / ledger_path, self.dir / harness_path

    def run_checker(self, ledger: Path, harness: Path) -> tuple[int, list[str]]:
        return checker.run(ledger, harness)


class GreenLedgerTests(CheckerTestBase):
    def test_valid_ledger_and_harness_pass(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(), harness=harness_source()
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 0, problems)

    def test_real_repository_passes(self) -> None:
        code, problems = checker.run(REPO / "docs/spec-tags.md", REPO / checker.ORACLE_HARNESS)
        self.assertEqual(code, 0, problems)
        self.assertEqual(problems, [])


class OrphanTests(CheckerTestBase):
    def test_harness_test_absent_from_ledger_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(names=TEST_NAMES[:-1]),
            harness=harness_source(),
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("orphan" in p for p in problems), problems)

    def test_empty_ledger_fails(self) -> None:
        ledger, harness = self.write(
            ledger="| ID | Requirement | Source | Verifier | Status |\n|---|---|---|---|---|\n",
            harness=harness_source(),
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("no data rows" in p for p in problems), problems)


class DuplicateTests(CheckerTestBase):
    def test_duplicate_id_fails(self) -> None:
        lines = ledger_source().splitlines()
        lines[3] = lines[2]  # row 2 now repeats SPEC-GZ-001
        ledger, harness = self.write(ledger="\n".join(lines) + "\n", harness=harness_source())
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("duplicate id" in p for p in problems), problems)

    def test_duplicate_verifier_fails(self) -> None:
        lines = ledger_source().splitlines()
        # Second row reuses the first row's verifier test function.
        lines[3] = lines[2].replace("SPEC-GZ-001", "SPEC-GZ-002")
        ledger, harness = self.write(ledger="\n".join(lines) + "\n", harness=harness_source())
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("duplicate verifier" in p for p in problems), problems)


class StaleReferenceTests(CheckerTestBase):
    def test_missing_source_file_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(source_ref="src/absent.rs::OracleMode"),
            harness=harness_source(),
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("source file does not exist" in p for p in problems), problems)

    def test_missing_source_symbol_fails(self) -> None:
        harness = harness_source()
        # Source file exists but lacks the symbol.
        (self.dir / "src").mkdir(parents=True)
        (self.dir / "src" / "oracle.rs").write_text("pub fn other() {}", encoding="utf-8")
        ledger, _ = self.write(
            ledger=ledger_source(source_ref="src/oracle.rs::OracleMode"),
            harness=harness,
        )
        code, problems = self.run_checker(ledger, self.dir / "tests" / "oracle_harness.rs")
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("source symbol" in p for p in problems), problems)

    def test_verifier_not_in_declared_harness_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(verifier_file="tests/other.rs"),
            harness=harness_source(),
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("not the declared oracle harness" in p for p in problems), problems)

    def test_verifier_test_function_missing_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(names=TEST_NAMES[:-1] + ["not_a_real_test"]),
            harness=harness_source(),
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("is not a #[test] function" in p for p in problems), problems)


class StatusAndShapeTests(CheckerTestBase):
    def test_missing_status_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(status="MISSING"), harness=harness_source()
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("MISSING" in p for p in problems), problems)

    def test_unknown_status_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(status="DONE"), harness=harness_source()
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("unknown status" in p for p in problems), problems)

    def test_non_spec_gz_id_fails(self) -> None:
        ledger, harness = self.write(
            ledger=ledger_source(id_prefix="SPEC-X"), harness=harness_source()
        )
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("SPEC-GZ-" in p for p in problems), problems)

    def test_malformed_row_column_count_fails(self) -> None:
        lines = ledger_source().splitlines()
        lines[2] = "| SPEC-GZ-001 | a | b | c | d | e |"  # six cells
        ledger, harness = self.write(ledger="\n".join(lines) + "\n", harness=harness_source())
        code, problems = self.run_checker(ledger, harness)
        self.assertEqual(code, 2, problems)
        self.assertTrue(any("malformed row" in p for p in problems), problems)

    def test_missing_ledger_file_fails(self) -> None:
        _, harness = self.write(harness=harness_source())
        code, problems = self.run_checker(self.dir / "absent.md", harness)
        self.assertEqual(code, 2, problems)
        self.assertTrue(any("missing ledger" in p for p in problems), problems)


if __name__ == "__main__":
    unittest.main()
