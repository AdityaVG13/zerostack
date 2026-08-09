import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts" / "check_surface_substrate.py"
spec = importlib.util.spec_from_file_location("check_surface_substrate", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SurfaceSubstrateGuardTests(unittest.TestCase):
    def _root(self, surface: str, worker_manifest: str = "") -> Path:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        surface_path = root / "crates/zero-codemode/src/surface.rs"
        surface_path.parent.mkdir(parents=True)
        surface_path.write_text(surface, encoding="utf-8")
        if worker_manifest:
            manifest = root / "crates/fszero-codemode/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text(worker_manifest, encoding="utf-8")
        return root

    @staticmethod
    def valid_surface() -> str:
        return """\
        #[serde(deny_unknown_fields)]
        enum SurfaceKind {}
        struct DomainAdapterRegistration;
        struct SurfaceRegistration;
        fn global_registration() {}
        enum WrongSurface {}
        """

    def test_hub_contract_stays_framework_neutral(self):
        self.assertEqual(module.check_hub_surface(self._root(self.valid_surface())), [])

    def test_runtime_or_transport_marker_fails(self):
        errors = module.check_hub_surface(
            self._root(self.valid_surface() + "// rquickjs must never enter this module")
        )
        self.assertTrue(any("rquickjs" in error for error in errors))

    def test_feature_exclusivity_requires_production_guard_shape(self):
        root = self._root(self.valid_surface())
        self.assertTrue(module.check_exclusive_features(root))
        guard = root / "src/guard.rs"
        guard.parent.mkdir(parents=True, exist_ok=True)
        guard.write_text(
            '#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]\n'
            'compile_error!("mutually exclusive");',
            encoding="utf-8",
        )
        self.assertEqual(module.check_exclusive_features(root), [])

    def test_git_scan_ignores_test_file_spoof_without_production_guard(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        tests = root / "tests"
        tests.mkdir(parents=True)
        baseline = tests / "packaging_e2e.rs"
        baseline.write_text(
            '#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]\n'
            'compile_error!("test spoof");',
            encoding="utf-8",
        )
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", "tests/packaging_e2e.rs"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "baseline",
            ],
            check=True,
        )
        self.assertTrue(module.check_exclusive_features(root))
        production = root / "src/packaging.rs"
        production.parent.mkdir(parents=True)
        production.write_text(
            '#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]\n'
            'compile_error!("production guard");',
            encoding="utf-8",
        )
        subprocess.run(["git", "-C", str(root), "add", "src/packaging.rs"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "production-guard",
            ],
            check=True,
        )
        self.assertEqual(module.check_exclusive_features(root), [])

    def test_git_scan_ignores_untracked_exclusivity_guard(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        source = root / "src"
        source.mkdir(parents=True)
        baseline = source / "baseline.rs"
        baseline.write_text("fn baseline() {}\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", "src/baseline.rs"], check=True)
        subprocess.run(
            ["git", "-C", str(root), "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "baseline"],
            check=True,
        )
        untracked = source / "untracked_guard.rs"
        untracked.write_text(
            '#[cfg(all(feature = "surface-mcp", feature = "surface-codemode"))]\n'
            'compile_error!("mutually exclusive");',
            encoding="utf-8",
        )
        self.assertTrue(module.check_exclusive_features(root))

    def test_worker_optional_compatibility_dependency_is_adoption_debt(self):
        manifest = """
        [package]
        name = "fszero-worker"
        [dependencies]
        fastmcp-rust = { optional = true }
        """
        root = self._root(self.valid_surface(), manifest)
        self.assertEqual(module.check_worker_dependencies(root, False), [])
        self.assertTrue(module.check_worker_dependencies(root, True))

    def test_worker_direct_runtime_dependency_fails_in_default_mode(self):
        manifest = """
        [package]
        name = "fszero-worker"
        [dependencies]
        rquickjs = "0.12"
        """
        root = self._root(self.valid_surface(), manifest)
        errors = module.check_worker_dependencies(root, False)
        self.assertTrue(any("rquickjs" in error for error in errors))

    def test_worker_path_marker_is_scanned_even_with_different_package_name(self):
        manifest = """
        [package]
        name = "graphzero-worker"
        [dependencies]
        zerostack-machine-permit = "1"
        """
        root = self._root(self.valid_surface(), manifest)
        self.assertTrue(module.check_worker_dependencies(root, False))


if __name__ == "__main__":
    unittest.main()
