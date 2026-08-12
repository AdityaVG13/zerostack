from __future__ import annotations

import importlib.util
import json
from pathlib import Path, PurePosixPath
import unittest

ROOT = Path(__file__).resolve().parents[2]
MATRIX_PATH = ROOT / "release" / "release-matrix-v1.json"
INSTALLER_SPEC = importlib.util.spec_from_file_location(
    "install_zerostack_matrix", ROOT / "scripts" / "install_zerostack.py"
)
assert INSTALLER_SPEC is not None and INSTALLER_SPEC.loader is not None
installer = importlib.util.module_from_spec(INSTALLER_SPEC)
INSTALLER_SPEC.loader.exec_module(installer)

EXPECTED_PLATFORMS = {
    "darwin-arm64",
    "darwin-x86_64",
    "linux-arm64",
    "linux-x86_64",
    "windows-x86_64",
}


class ReleaseMatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.matrix = json.loads(MATRIX_PATH.read_text(encoding="utf-8"))

    def test_matrix_matches_installer_platform_support(self) -> None:
        self.assertEqual(self.matrix["schema"], "zerostack.release_matrix.v1")
        self.assertEqual(
            self.matrix["policy"],
            {
                "purpose": "local_conformance_only",
                "publication_allowed": False,
                "tagging_allowed": False,
                "pi_zsx": "internal_unpublished",
            },
        )
        platforms = {entry["id"] for entry in self.matrix["platforms"]}
        self.assertEqual(platforms, EXPECTED_PLATFORMS)
        self.assertEqual(platforms, installer.SUPPORTED_PLATFORMS)

    def test_default_is_codemode_and_compatibility_is_explicit_and_safe(self) -> None:
        self.assertEqual(self.matrix["default_surface"], "codemode")
        startup = self.matrix["startup"]
        self.assertEqual(startup["schema"], "zerostack.startup_argv.v1")
        self.assertIs(startup["shell"], False)
        self.assertEqual(startup["argv"], ["exec", "-C", "<project-root>"])
        self.assertIn(" startup ", startup["generator"])
        compatibility = self.matrix["compatibility"]
        self.assertEqual(compatibility["surface"], "mcp")
        self.assertEqual(compatibility["installer_flag"], "--compat-mcp")
        self.assertEqual(compatibility["warning_channel"], "stderr")
        self.assertIn("maintenance-only", compatibility["warning"])

    def test_every_platform_has_portable_prebuilt_paths(self) -> None:
        required = {"id", "rust_target", "archive", "cli", "node_addon"}
        for entry in self.matrix["platforms"]:
            self.assertEqual(set(entry), required)
            self.assertIn(entry["archive"], ("tar.gz", "zip"))
            for key in ("cli", "node_addon"):
                path = PurePosixPath(entry[key])
                self.assertFalse(path.is_absolute())
                self.assertNotIn("..", path.parts)
                self.assertNotIn("\\", entry[key])
            self.assertTrue(entry["node_addon"].endswith("/zsx_node.node"))
            if entry["id"].startswith("windows-"):
                self.assertTrue(entry["cli"].endswith(".exe"))
                self.assertEqual(entry["archive"], "zip")
            else:
                self.assertEqual(entry["cli"], "bin/zsx")
                self.assertEqual(entry["archive"], "tar.gz")

    def test_reproducibility_binds_all_repositories_and_signed_digests(self) -> None:
        reproducibility = self.matrix["reproducibility"]
        self.assertEqual(
            set(reproducibility["source_heads"]),
            {"ZeroStack", "FSZero", "GraphZero", "TokenZero"},
        )
        self.assertIs(reproducibility["cargo_locked"], True)
        self.assertEqual(reproducibility["artifact_hash"], "sha256")
        self.assertEqual(reproducibility["manifest_signature"], "minisign")

    def test_matrix_contains_no_host_paths_or_credentials(self) -> None:
        serialized = json.dumps(self.matrix)
        forbidden = ("/Users/", "/home/", "C:\\\\Users\\", "token=", "ghp_", "github_pat_")
        for marker in forbidden:
            self.assertNotIn(marker, serialized)


if __name__ == "__main__":
    unittest.main()
