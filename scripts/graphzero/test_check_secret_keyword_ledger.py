"""Focused fixture tests for scripts/check_secret_keyword_ledger.py.

Run: python3 -m unittest scripts.test_check_secret_keyword_ledger -v
"""

from __future__ import annotations

import hashlib
import importlib.util
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_secret_keyword_ledger.py")
spec = importlib.util.spec_from_file_location("graphzero_check_secret_keyword_ledger", MODULE_PATH)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)

# A literal that matches the api-key-literal pattern without being a real
# secret: fixture-style assignment with a placeholder value. The seed file
# content adds a trailing newline, which git grep -n -z emits as part of the
# matched line, so the recorded digest covers exactly those bytes.
HIT_LINE = 'let api_key = "placeholder-abcdef1234567890";'
SWAP_LINE = 'let api_key = "replacement-abcdef1234567890";'
SEED_CONTENT = HIT_LINE + "\n"


def digest_of(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def api_key_row(
    path: str,
    line: int,
    cls: str = "fixture",
    content: str = SEED_CONTENT,
) -> tuple[str, str, int, str, str]:
    return ("api-key-literal", path, line, cls, digest_of(content))


API_KEY_ROW = api_key_row("src/fixture.rs", 1)


def ledger_source(rows: list[tuple[str, str, int, str, str]]) -> str:
    lines = ["# fixture ledger"]
    for row in rows:
        lines.append("\t".join(str(part) for part in row))
    return "\n".join(lines) + "\n"


class LedgerCheckerTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        self.repo = self._git_init(self.dir)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    @staticmethod
    def _git_init(path: Path) -> Path:
        repo = path / "repo"
        repo.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.email", "test@example.invalid"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.name", "test"], cwd=repo, check=True)
        return repo

    def seed(self, files: dict[str, str]) -> None:
        for rel, content in files.items():
            target = self.repo / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=self.repo, check=True)
        subprocess.run(["git", "commit", "-qm", "seed"], cwd=self.repo, check=True)

    def write_ledger(self, rows: list[tuple[str, str, int, str, str]]) -> Path:
        ledger = self.dir / "ledger.tsv"
        ledger.write_text(ledger_source(rows), encoding="utf-8")
        return ledger

    def run_main(self, ledger: Path) -> int:
        return checker.main(["--root", str(self.repo), "--ledger", str(ledger)])

    def run_checks(self, ledger: Path) -> list[str]:
        return checker.run_checks(self.repo, ledger)


class KnownPassTests(LedgerCheckerTestBase):
    def test_classified_fixture_hit_passes(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        self.assertEqual(self.run_main(ledger), 0)
        self.assertEqual(self.run_checks(ledger), [])

    def test_source_without_trailing_newline_passes(self) -> None:
        self.seed({"src/fixture.rs": HIT_LINE})
        ledger = self.write_ledger([API_KEY_ROW])
        self.assertEqual(self.run_checks(ledger), [])


class NewHitFailTests(LedgerCheckerTestBase):
    def test_unclassified_hit_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([])
        errors = self.run_checks(ledger)
        self.assertEqual(self.run_main(ledger), 1)
        self.assertTrue(
            any("unclassified hit: api-key-literal src/fixture.rs:1" in e for e in errors),
            errors,
        )

    def test_second_new_hit_fails_when_only_first_classified(self) -> None:
        self.seed(
            {
                "src/fixture.rs": SEED_CONTENT,
                "src/fixture2.rs": SEED_CONTENT,
            }
        )
        ledger = self.write_ledger([API_KEY_ROW])
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("unclassified hit: api-key-literal src/fixture2.rs:1" in e for e in errors),
            errors,
        )


class ContentSwapFailTests(LedgerCheckerTestBase):
    def test_content_swap_same_key_fails(self) -> None:
        # Same path, line, and pattern but different matched content: the
        # live digest no longer matches the recorded sha256.
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        self.assertEqual(self.run_checks(ledger), [])
        (self.repo / "src/fixture.rs").write_text(SWAP_LINE + "\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=self.repo, check=True)
        subprocess.run(["git", "commit", "-qm", "swap content"], cwd=self.repo, check=True)
        errors = self.run_checks(ledger)
        self.assertEqual(self.run_main(ledger), 1)
        self.assertTrue(
            any("content changed at key: api-key-literal src/fixture.rs:1" in e for e in errors),
            errors,
        )


class ColonFilenameTests(LedgerCheckerTestBase):
    def test_colon_filename_passes(self) -> None:
        # -z parsing must keep a colon in the path instead of splitting on it.
        self.seed({"src/we:ird.rs": SEED_CONTENT})
        ledger = self.write_ledger([api_key_row("src/we:ird.rs", 1)])
        self.assertEqual(self.run_main(ledger), 0)
        self.assertEqual(self.run_checks(ledger), [])


class StaleEntryFailTests(LedgerCheckerTestBase):
    def test_stale_line_number_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        # Ledger names line 2 but the hit is on line 1.
        ledger = self.write_ledger([api_key_row("src/fixture.rs", 2)])
        errors = self.run_checks(ledger)
        self.assertEqual(self.run_main(ledger), 1)
        self.assertTrue(
            any("stale or deleted ledger entry: api-key-literal src/fixture.rs:2" in e for e in errors),
            errors,
        )

    def test_removed_file_entry_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        # The hit was removed before this run; the entry is now stale.
        (self.repo / "src/fixture.rs").write_text("fn no_secret() {}\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=self.repo, check=True)
        subprocess.run(["git", "commit", "-qm", "remove hit"], cwd=self.repo, check=True)
        ledger = self.write_ledger([API_KEY_ROW])
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("stale or deleted ledger entry: api-key-literal src/fixture.rs:1" in e for e in errors),
            errors,
        )


class RealRiskFailTests(LedgerCheckerTestBase):
    def test_real_risk_class_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([api_key_row("src/fixture.rs", 1, cls="real-risk")])
        errors = self.run_checks(ledger)
        self.assertEqual(self.run_main(ledger), 1)
        self.assertTrue(
            any("real-risk" in e and "forbidden" in e for e in errors),
            errors,
        )


class StructuralFailTests(LedgerCheckerTestBase):
    def test_duplicate_entry_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW, API_KEY_ROW])
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("duplicate entry for api-key-literal src/fixture.rs:1" in e for e in errors),
            errors,
        )

    def test_invalid_class_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([api_key_row("src/fixture.rs", 1, cls="production")])
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("invalid class 'production'" in e for e in errors),
            errors,
        )

    def test_non_integer_line_number_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.dir / "ledger.tsv"
        ledger.write_text(
            "api-key-literal\tsrc/fixture.rs\tone\tfixture\t"
            f"{digest_of(SEED_CONTENT)}\n",
            encoding="utf-8",
        )
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("line number must be an integer" in e for e in errors),
            errors,
        )

    def test_unknown_pattern_id_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger(
            [("totally-new-pattern", "src/fixture.rs", 1, "fixture", digest_of(SEED_CONTENT))]
        )
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("unknown pattern id 'totally-new-pattern'" in e for e in errors),
            errors,
        )

    def test_invalid_digest_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger(
            [("api-key-literal", "src/fixture.rs", 1, "fixture", "not-a-sha256")]
        )
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("sha256" in e and "digest" in e for e in errors),
            errors,
        )

    def test_uppercase_digest_fails(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger(
            [("api-key-literal", "src/fixture.rs", 1, "fixture", digest_of(SEED_CONTENT).upper())]
        )
        errors = self.run_checks(ledger)
        self.assertTrue(
            any("lowercase" in e and "digest" in e for e in errors),
            errors,
        )


class OperationalFailureTests(LedgerCheckerTestBase):
    def test_missing_git_binary_is_typed_error_without_traceback(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        with unittest.mock.patch.object(
            checker.subprocess, "run", side_effect=FileNotFoundError("git")
        ):
            with self.assertRaises(checker.LedgerError) as ctx:
                checker.run_checks(self.repo, ledger)
            # main converts the LedgerError into exit code 2 with a single
            # stderr line, never a traceback.
            rc = checker.main(["--root", str(self.repo), "--ledger", str(ledger)])
        self.assertIn("cannot spawn git", str(ctx.exception))
        self.assertEqual(rc, 2)

    def test_missing_ledger_exit_2(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        missing = self.dir / "no-such-ledger.tsv"
        with self.assertRaises(checker.LedgerError) as ctx:
            checker.run_checks(self.repo, missing)
        self.assertIn("missing ledger", str(ctx.exception))
        self.assertEqual(
            checker.main(["--root", str(self.repo), "--ledger", str(missing)]), 2
        )

    def test_invalid_utf8_ledger_exit_2(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.dir / "invalid-ledger.tsv"
        ledger.write_bytes(b"\xff\xfe")
        with self.assertRaises(checker.LedgerError) as ctx:
            checker.run_checks(self.repo, ledger)
        self.assertIn("cannot read ledger", str(ctx.exception))
        self.assertEqual(
            checker.main(["--root", str(self.repo), "--ledger", str(ledger)]), 2
        )

    def test_grep_rc128_exit_2_omits_stderr(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        fake = subprocess.CompletedProcess(args=[], returncode=128, stdout=b"", stderr=b"boom")
        with unittest.mock.patch.object(checker.subprocess, "run", return_value=fake):
            with self.assertRaises(checker.LedgerError) as ctx:
                checker.run_checks(self.repo, ledger)
            rc = checker.main(["--root", str(self.repo), "--ledger", str(ledger)])
        message = str(ctx.exception)
        self.assertIn("rc=128", message)
        # git stderr is never echoed into the error.
        self.assertNotIn("boom", message)
        self.assertNotIn(HIT_LINE, message)
        self.assertEqual(rc, 2)

    def test_malformed_grep_output_omits_secret(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        # 2 parts is not a multiple of 3 -> malformed record.
        fake = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=b"src/x.rs\0", stderr=b""
        )
        with unittest.mock.patch.object(checker.subprocess, "run", return_value=fake):
            with self.assertRaises(checker.LedgerError) as ctx:
                checker.run_checks(self.repo, ledger)
        message = str(ctx.exception)
        self.assertIn("malformed git grep output", message)
        self.assertNotIn(HIT_LINE, message)

    def test_non_utf8_path_raises_redacted_error(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        fake = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=b"\xff\xfe\0 1\0 " + SEED_CONTENT.encode(), stderr=b""
        )
        with unittest.mock.patch.object(checker.subprocess, "run", return_value=fake):
            with self.assertRaises(checker.LedgerError) as ctx:
                checker.run_checks(self.repo, ledger)
        message = str(ctx.exception)
        self.assertIn("non-UTF8 path", message)
        self.assertNotIn(HIT_LINE, message)

    def test_tsv_unsafe_path_raises_redacted_error(self) -> None:
        self.seed({"src/fixture.rs": SEED_CONTENT})
        ledger = self.write_ledger([API_KEY_ROW])
        fake = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=b"src/unsafe\nname.rs\0 1\0 " + SEED_CONTENT.encode(), stderr=b""
        )
        with unittest.mock.patch.object(checker.subprocess, "run", return_value=fake):
            with self.assertRaises(checker.LedgerError) as ctx:
                checker.run_checks(self.repo, ledger)
        message = str(ctx.exception)
        self.assertIn("cannot be represented safely in TSV", message)
        self.assertNotIn("unsafe", message)
        self.assertNotIn(HIT_LINE, message)


if __name__ == "__main__":
    unittest.main()
