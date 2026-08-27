"""Focused fixture tests for scripts/check_proptest_contract.py.

Run: uv run python -m unittest scripts.test_check_proptest_contract -v
"""

from __future__ import annotations

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_proptest_contract.py")
spec = importlib.util.spec_from_file_location(
    "graphzero_check_proptest_contract", MODULE_PATH
)
checker = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(checker)

REPO = checker.REPO_ROOT

SITE_SOURCE = """use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_round_trips(data in any::<u8>()) {
        let _ = data;
    }
}
"""

SITE_INTEGRATION_SAFE = """use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/proptest-regressions/tests/x_proptest.txt"
            )),
        )),
        ..ProptestConfig::with_cases(32)
    })]

    #[test]
    fn prop_round_trips(data in any::<u8>()) {
        let _ = data;
    }
}
"""

SITE_INTEGRATION_UNSAFE = """use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "/tmp/outside-layout.txt"
            ),
        )),
        ..ProptestConfig::with_cases(32)
    })]

    #[test]
    fn prop_round_trips(data in any::<u8>()) {
        let _ = data;
    }
}
"""

GREEN_README = """# proptest-regressions (gz)

Committed shrunk proptest regressions for gz. The empty layout means no
failing seed is persisted; it proves no broader absence. Never hand-edit
regression lines; proptest writes them, one failing seed per line. Replay with
PROPTEST_RNG_SEED=<emitted>. PROPTEST_DISABLE_FAILURE_PERSISTENCE is
forbidden in CI and release checks. See docs/proptest.md.
"""

GREEN_DOCS = """# Proptest contract

Replay a failing seed exactly with PROPTEST_RNG_SEED. The
PROPTEST_DISABLE_FAILURE_PERSISTENCE override is forbidden. Committed shrunk
failures live under each crate's proptest-regressions/ layout. Never
hand-edit regression lines. Run replay through rch exec with
CARGO_TARGET_DIR=/tmp/rch_target_graphzero.
"""


class FixtureBase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def write_fixture(
        self,
        *,
        site: str = SITE_SOURCE,
        readme: str = GREEN_README,
        docs: str = GREEN_DOCS,
        extra_tracked: list[str] | None = None,
        git: bool = True,
        site_path: str = "crates/gz/src/lib.rs",
    ) -> Path:
        root = self.dir
        (root / "crates" / "gz" / "src" / "store").mkdir(parents=True, exist_ok=True)
        site_file = root / site_path
        site_file.parent.mkdir(parents=True, exist_ok=True)
        site_file.write_text(site, encoding="utf-8")
        (root / "crates" / "gz" / "src" / "store" / "q.rs").write_text(
            "pub fn q() {}\n", encoding="utf-8"
        )
        (root / "crates" / "gz" / "proptest-regressions").mkdir(parents=True, exist_ok=True)
        (root / "crates" / "gz" / "proptest-regressions" / "README.md").write_text(
            readme, encoding="utf-8"
        )
        (root / "docs").mkdir(parents=True, exist_ok=True)
        (root / "docs" / "proptest.md").write_text(docs, encoding="utf-8")
        for rel in extra_tracked or []:
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("x\n", encoding="utf-8")
        if git:
            self.git_init_commit()
        return root

    def git_init_commit(self) -> None:
        subprocess.run(["git", "-C", str(self.dir), "init", "-q"], check=True)
        subprocess.run(
            ["git", "-C", str(self.dir), "add", "-A"], check=True, capture_output=True
        )
        subprocess.run(
            [
                "git", "-C", str(self.dir), "-c", "user.name=t", "-c", "user.email=t@t",
                "commit", "-q", "-m", "fixture",
            ],
            check=True,
            capture_output=True,
        )

    def run_checker(self, root: Path) -> tuple[int, list[str]]:
        return checker.run(repo_root=root, crates_dir=root / "crates")


class GreenFixtureTests(FixtureBase):
    def test_green_fixture_passes(self) -> None:
        root = self.write_fixture()
        code, problems = self.run_checker(root)
        self.assertEqual(code, 0, problems)

    def test_real_repository_passes(self) -> None:
        code, problems = checker.run()
        self.assertEqual(code, 0, problems)
        self.assertEqual(problems, [])

    def test_no_proptest_sites_fails(self) -> None:
        root = self.write_fixture(site="pub fn not_proptest() {}\n")
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("no proptest sites" in p for p in problems), problems)

    def test_git_unavailable_fails_closed(self) -> None:
        root = self.write_fixture(git=False)
        code, problems = self.run_checker(root)
        self.assertEqual(code, 2, problems)
        self.assertTrue(any("git" in p for p in problems), problems)


class ForbiddenConstructTests(FixtureBase):
    def test_failure_persistence_none_fails(self) -> None:
        root = self.write_fixture(
            site="use proptest::test_runner::Config;\n"
            "let _ = Config { failure_persistence: None, ..Config::default() };\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("failure_persistence: None" in p for p in problems), problems)

    def test_file_persistence_off_fails(self) -> None:
        root = self.write_fixture(
            site="use proptest::test_runner::FileFailurePersistence;\n"
            "let _ = FileFailurePersistence::Off;\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("FileFailurePersistence::Off" in p for p in problems), problems)

    def test_disable_failure_persistence_env_fails(self) -> None:
        root = self.write_fixture(
            site="// PROPTEST_DISABLE_FAILURE_PERSISTENCE must not appear\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("PROPTEST_DISABLE_FAILURE_PERSISTENCE" in p for p in problems), problems)

    def test_integration_site_without_safe_direct_fails(self) -> None:
        root = self.write_fixture(
            site=SITE_SOURCE, site_path="crates/gz/tests/x_proptest.rs"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("integration proptest site" in p for p in problems), problems)

    def test_integration_site_with_unsafe_direct_fails(self) -> None:
        root = self.write_fixture(
            site=SITE_INTEGRATION_UNSAFE, site_path="crates/gz/tests/x_proptest.rs"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("integration proptest site" in p for p in problems), problems)

    def test_integration_site_safe_direct_wrong_stem_fails(self) -> None:
        wrong = SITE_INTEGRATION_SAFE.replace("x_proptest.txt", "other.txt")
        root = self.write_fixture(
            site=wrong, site_path="crates/gz/tests/x_proptest.rs"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("does not match site stem" in p for p in problems), problems)

    def test_integration_site_with_safe_direct_passes(self) -> None:
        root = self.write_fixture(
            site=SITE_INTEGRATION_SAFE, site_path="crates/gz/tests/x_proptest.rs"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 0, problems)

    def test_library_site_with_unsafe_direct_fails(self) -> None:
        root = self.write_fixture(
            site=SITE_INTEGRATION_UNSAFE.replace(
                "/tmp/outside-layout.txt", "/tmp/lib-out.txt"
            )
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("SourceParallel layout" in p for p in problems), problems)

    def test_library_site_with_safe_direct_fails(self) -> None:
        # Direct persistence is reserved for integration-test sites only.
        root = self.write_fixture(site=SITE_INTEGRATION_SAFE)
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("SourceParallel layout" in p for p in problems), problems)


class ManualRunnerTests(FixtureBase):
    def test_test_runner_default_fails(self) -> None:
        root = self.write_fixture(
            site="use proptest::test_runner::TestRunner;\n"
            "fn p() { let mut r = TestRunner::default(); let _ = &mut r; }\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("TestRunner::default()" in p for p in problems), problems)

    def test_test_runner_new_without_source_file_fails(self) -> None:
        root = self.write_fixture(
            site="use proptest::test_runner::{Config, TestRunner};\n"
            "fn p() { let mut r = TestRunner::new(Config::default()); let _ = &mut r; }\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("source_file" in p for p in problems), problems)

    def test_test_runner_new_with_inline_source_file_passes(self) -> None:
        root = self.write_fixture(
            site="use proptest::test_runner::{Config, TestRunner};\n"
            "fn p() { let mut r = TestRunner::new(Config {\n"
            "    source_file: Some(file!()),\n"
            "    ..Config::default()\n"
            "}); let _ = &mut r; }\n"
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 0, problems)


class LayoutAndDocsTests(FixtureBase):
    def test_missing_readme_fails(self) -> None:
        root = self.write_fixture()
        (root / "crates" / "gz" / "proptest-regressions" / "README.md").unlink()
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("README" in p for p in problems), problems)

    def test_readme_missing_phrase_fails(self) -> None:
        root = self.write_fixture(readme=GREEN_README.replace("PROPTEST_RNG_SEED", "SEED_VAR"))
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("PROPTEST_RNG_SEED" in p for p in problems), problems)

    def test_docs_missing_phrase_fails(self) -> None:
        root = self.write_fixture(docs=GREEN_DOCS.replace("rch exec", "remote exec"))
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("rch exec" in p for p in problems), problems)

    def test_tracked_fallback_file_fails(self) -> None:
        root = self.write_fixture(
            extra_tracked=["crates/gz/tests/x.proptest-regressions"], git=True
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("fallback file" in p for p in problems), problems)

    def test_tracked_orphan_txt_in_layout_fails(self) -> None:
        root = self.write_fixture(
            extra_tracked=["crates/gz/proptest-regressions/nope/q.txt"], git=True
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 1, problems)
        self.assertTrue(any("does not mirror" in p for p in problems), problems)

    def test_tracked_mirrored_txt_passes(self) -> None:
        root = self.write_fixture(
            extra_tracked=["crates/gz/proptest-regressions/store/q.txt"], git=True
        )
        code, problems = self.run_checker(root)
        self.assertEqual(code, 0, problems)


if __name__ == "__main__":
    unittest.main()
