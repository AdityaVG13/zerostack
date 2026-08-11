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
        root = self._root(
            self.valid_surface(),
            '[package]\nname = "engine"\n[features]\nsurface-mcp = []\nsurface-codemode = []\n',
        )
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
        (root / "Cargo.toml").write_text(
            '[package]\nname = "engine"\n[features]\nsurface-mcp = []\nsurface-codemode = []\n',
            encoding="utf-8",
        )
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
        (root / "Cargo.toml").write_text(
            '[package]\nname = "engine"\n[features]\nsurface-mcp = []\nsurface-codemode = []\n',
            encoding="utf-8",
        )
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

    def test_feature_exclusivity_skips_single_surface_manifest(self):
        root = self._root(
            self.valid_surface(),
            '[package]\nname = "engine"\n[features]\nsurface-mcp = []\n',
        )
        self.assertEqual(module.check_exclusive_features(root), [])

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

    def test_fs_domain_package_is_not_misclassified_as_raw_worker(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / "Cargo.toml").write_text(
            """
            [package]
            name = "fs-zero"
            [dependencies]
            zero-codemode = { optional = true }
            zerostack-machine-permit = { optional = true }
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertFalse(any("worker directly depends" in error for error in errors))

    def test_removed_quickjs_feature_needs_no_exclusivity_guard(self):
        root = self._root(self.valid_surface())
        codemode = root / "crates/zero-codemode/src/lib.rs"
        codemode.write_text(
            '#[cfg(feature = "fastmcp")]\nfn transport() {}',
            encoding="utf-8",
        )
        self.assertEqual(module.scan_roots([root]), [])

    def test_strict_compatibility_packages_must_use_hub_transport(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        manifest = root / "crates/graphzero-mcp-compat/Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(
            """
            [package]
            name = "graphzero-mcp-compat"
            [dependencies]
            fastmcp-rust = "0.3"
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("hub transport" in error for error in errors))
        self.assertTrue(any("fastmcp-rust" in error for error in errors))

        manifest.write_text(
            """
            [package]
            name = "graphzero-mcp-compat"
            [dependencies]
            zero-mcp = { path = "../../../ZeroStack/crates/zero-mcp", features = ["fastmcp"] }
            """,
            encoding="utf-8",
        )
        self.assertEqual(module.check_worker_dependencies(root, True), [])

    def test_target_dev_dependency_and_workspace_declaration_are_not_production_edges(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        manifest = root / "Cargo.toml"
        manifest.write_text(
            """
            [package]
            name = "fszero-worker"
            [target.'cfg(unix)'.dev-dependencies]
            fastmcp-rust = "0.3"
            [workspace.dependencies]
            fastmcp-rust = "0.3"
            """,
            encoding="utf-8",
        )
        self.assertEqual(module.check_worker_dependencies(root, True), [])

        manifest.write_text(
            """
            [package]
            name = "fszero-worker"
            [target.'cfg(unix)'.dependencies]
            fastmcp-rust = "0.3"
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("fastmcp-rust" in error for error in errors))

    def test_strict_engine_fastmcp_dependency_is_rejected_without_hub_carrier(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        manifest = root / "Cargo.toml"
        manifest.write_text(
            """
            [package]
            name = "fs-zero"
            [dependencies]
            fastmcp-rust = { optional = true }
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("engine production manifest" in error for error in errors))
        self.assertTrue(any("forbidden 'fastmcp-rust'" in error for error in errors))

        manifest.write_text(
            """
            [package]
            name = "fs-zero"
            [dependencies]
            zero-codemode = { path = "../ZeroStack/crates/zero-codemode" }
            fastmcp-rust = { optional = true }
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("forbidden 'fastmcp-rust'" in error for error in errors))

    def test_strict_rejects_engine_runtime_and_permit_manifest_wiring(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / "Cargo.toml").write_text(
            """
            [package]
            name = "domain-engine"
            [features]
            quickjs-runtime = ["dep:js"]
            host-permit = ["dep:permit"]
            [dependencies]
            js = { package = "rquickjs", version = "1", optional = true }
            permit = { package = "zerostack-machine-permit", version = "1", optional = true }
            gate = { package = "zero-gate", version = "1" }
            """,
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("forbidden 'rquickjs'" in error for error in errors))
        self.assertTrue(any("forbidden 'zerostack-machine-permit'" in error for error in errors))
        self.assertTrue(any("forbidden 'zero-gate'" in error for error in errors))
        self.assertTrue(any("quickjs-runtime" in error for error in errors))
        self.assertTrue(any("host-permit" in error for error in errors))

    def test_strict_rejects_engine_runtime_and_permit_sources(self):
        root = self._root(self.valid_surface())
        source = root / "src"
        source.mkdir()
        (source / "runtime.rs").write_text(
            '#[cfg(feature = "quickjs-runtime")]\nuse rquickjs::Runtime;\n',
            encoding="utf-8",
        )
        (source / "quickjs.rs").write_text("fn dormant() {}\n", encoding="utf-8")
        (source / "permit.rs").write_text(
            "use zerostack_machine_permit::MachinePermit;\n",
            encoding="utf-8",
        )
        (source / "host_permit.rs").write_text("fn acquire() {}\n", encoding="utf-8")
        (source / "mcp_frame.rs").write_text(
            "pub fn mcp_success_envelope() {}\n",
            encoding="utf-8",
        )
        (source / "child_identity.rs").write_text(
            "pub struct VerifiedChild;\n"
            "pub struct ChildBinding;\n"
            "pub fn escalate_detached() {}\n",
            encoding="utf-8",
        )
        errors = module.check_engine_sources(root, True)
        self.assertTrue(any("rquickjs import" in error for error in errors))
        self.assertTrue(any("QuickJS feature gate" in error for error in errors))
        self.assertTrue(any("QuickJS source module" in error for error in errors))
        self.assertTrue(any("machine permit" in error for error in errors))
        self.assertTrue(any("host permit source module" in error for error in errors))
        self.assertTrue(any("MCP framing source module" in error for error in errors))
        self.assertTrue(any("MCP envelope framing" in error for error in errors))
        self.assertTrue(any("process lifecycle" in error for error in errors))

    def test_strict_rejects_direct_machine_permit_even_for_session_identity(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        source = root / "src"
        source.mkdir()
        (root / "Cargo.toml").write_text(
            """
            [package]
            name = "domain-identity"
            [dependencies]
            zerostack-machine-permit = "1"
            """,
            encoding="utf-8",
        )
        (source / "lib.rs").write_text(
            "pub use zerostack_machine_permit::session_owner::ProcessIdentity;\n",
            encoding="utf-8",
        )
        errors = module.check_worker_dependencies(root, True)
        self.assertTrue(any("zerostack-machine-permit" in error for error in errors))
        self.assertEqual(module.check_engine_sources(root, True), [])

    def test_strict_source_scan_excludes_tests_and_allows_thin_adapter(self):
        root = self._root(self.valid_surface())
        tests = root / "tests"
        tests.mkdir()
        (tests / "legacy.rs").write_text("use rquickjs::Runtime;\n", encoding="utf-8")
        source = root / "src"
        source.mkdir()
        (source / "legacy_tests.rs").write_text(
            "use zerostack_machine_permit::MachinePermit;\n",
            encoding="utf-8",
        )
        (source / "adapter.rs").write_text(
            "use zero_codemode::RawWorkerClient;\n"
            'const MESSAGE: &str = "QuickJS host permit migration";\n',
            encoding="utf-8",
        )
        ignored = root / ".zerostack/mirror"
        ignored.mkdir(parents=True)
        (ignored / "runtime.rs").write_text("use rquickjs::Runtime;\n", encoding="utf-8")
        (root / ".gitignore").write_text(".zerostack/\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        self.assertEqual(module.check_engine_sources(root, True), [])


if __name__ == "__main__":

    unittest.main()
