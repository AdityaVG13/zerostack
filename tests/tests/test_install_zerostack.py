from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "install_zerostack", ROOT / "scripts" / "install_zerostack.py"
)
assert SPEC is not None and SPEC.loader is not None
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)

HEADS = {
    "ZeroStack": "1" * 40,
    "FSZero": "2" * 40,
    "GraphZero": "3" * 40,
    "TokenZero": "4" * 40,
}


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def make_bundle(root: Path, version: str, output: str) -> Path:
    bundle = root / f"bundle-{version}"
    binary = bundle / "bin" / "zsx"
    binary.parent.mkdir(parents=True)
    payload = f"#!/bin/sh\nprintf '%s\\n' {json.dumps(output)}\n".encode()
    binary.write_bytes(payload)
    binary.chmod(0o755)
    manifest = {
        "schema": installer.SCHEMA,
        "version": version,
        "platform": installer.current_platform(),
        "source_heads": HEADS,
        "artifacts": [
            {
                "path": "bin/zsx",
                "sha256": digest(payload),
                "size_bytes": len(payload),
                "executable": True,
            }
        ],
        "entrypoints": {"zsx": "bin/zsx"},
    }
    (bundle / installer.MANIFEST).write_text(
        f"{json.dumps(manifest, indent=2, sort_keys=True)}\n", encoding="utf-8"
    )
    return bundle


class PrebuiltInstallerTests(unittest.TestCase):
    def test_cli_installs_and_verifies_prebuilt_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = make_bundle(root, "1.0.0", "cli")
            prefix = root / "install"
            command = [
                sys.executable,
                str(ROOT / "scripts" / "install_zerostack.py"),
            ]
            installed = subprocess.run(
                command
                + [
                    "install",
                    "--bundle",
                    str(bundle),
                    "--prefix",
                    str(prefix),
                    "--allow-unsigned",
                    "--json",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertTrue(json.loads(installed.stdout)["ok"])
            verified = subprocess.run(
                command + ["verify", "--prefix", str(prefix), "--json"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(verified.returncode, 0, verified.stderr)
            self.assertTrue(json.loads(verified.stdout)["ok"])

    def test_install_upgrade_verify_rollback_and_uninstall(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "install"
            first = make_bundle(root, "1.0.0", "one")
            second = make_bundle(root, "1.1.0", "two")

            state = installer.install_bundle(prefix, first, None, True)
            self.assertEqual(state["current"], f"1.0.0-{installer.current_platform()}")
            self.assertEqual(installer.verify_install(prefix), state)
            self.assertEqual(
                subprocess.check_output([prefix / "bin" / "zsx"], text=True).strip(), "one"
            )

            state = installer.install_bundle(prefix, second, None, True)
            self.assertEqual(state["previous"], f"1.0.0-{installer.current_platform()}")
            self.assertEqual(
                subprocess.check_output([prefix / "bin" / "zsx"], text=True).strip(), "two"
            )

            state = installer.rollback(prefix)
            self.assertEqual(state["current"], f"1.0.0-{installer.current_platform()}")
            self.assertEqual(
                subprocess.check_output([prefix / "bin" / "zsx"], text=True).strip(), "one"
            )

            installer.uninstall(prefix)
            self.assertFalse((prefix / "install-state.json").exists())
            self.assertFalse((prefix / "bin" / "zsx").exists())
            self.assertFalse((prefix / "current").exists())
            self.assertTrue((prefix / "releases").is_dir(), "uninstall preserves rollback data")

    def test_tampered_artifact_fails_before_install(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "install"
            bundle = make_bundle(root, "1.0.0", "one")
            binary = bundle / "bin" / "zsx"
            original = binary.read_bytes()
            binary.write_bytes(bytes([original[0] ^ 1]) + original[1:])
            with self.assertRaisesRegex(installer.InstallError, "digest mismatch"):
                installer.install_bundle(prefix, bundle, None, True)
            self.assertFalse(installer.state_path(prefix).exists())

    def test_same_release_id_cannot_replace_or_misbind_content(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "install"
            first = make_bundle(root / "one", "1.0.0", "one")
            republished = make_bundle(root / "two", "1.0.0", "different")
            installer.install_bundle(prefix, first, None, True)
            with self.assertRaisesRegex(installer.InstallError, "release id collision"):
                installer.install_bundle(prefix, republished, None, True)
            self.assertEqual(
                subprocess.check_output([prefix / "bin" / "zsx"], text=True).strip(), "one"
            )

            state = installer.read_state(prefix)
            state["manifest_sha256"] = "0" * 64
            installer.write_state(prefix, state)
            with self.assertRaisesRegex(installer.InstallError, "state manifest digest"):
                installer.verify_install(prefix)

    def test_signature_is_required_and_minisign_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bundle = make_bundle(Path(temporary), "1.0.0", "one")
            with self.assertRaisesRegex(installer.InstallError, "public-key"):
                installer.verify_signature(bundle, None, False)
            (bundle / installer.SIGNATURE).write_text("fixture", encoding="utf-8")
            with patch.object(installer.shutil, "which", return_value="/usr/bin/minisign"), patch.object(
                installer.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 1, "", "bad signature"),
            ):
                with self.assertRaisesRegex(installer.InstallError, "verification failed"):
                    installer.verify_signature(bundle, "RWfixture", False)

    def test_safe_zip_bundle_installs_and_traversal_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = make_bundle(root, "1.0.0", "archive")
            archive = root / "bundle.zip"
            with zipfile.ZipFile(archive, "w") as package:
                for path in bundle.rglob("*"):
                    if path.is_file():
                        package.write(path, Path("release") / path.relative_to(bundle))
            with installer.materialized_bundle(str(archive)) as extracted:
                state = installer.install_bundle(root / "install", extracted, None, True)
            self.assertEqual(state["current"], f"1.0.0-{installer.current_platform()}")

            malicious = root / "malicious.zip"
            with zipfile.ZipFile(malicious, "w") as package:
                package.writestr("../escape", "no")
            destination = root / "extract"
            destination.mkdir()
            with self.assertRaisesRegex(installer.InstallError, "safe relative path"):
                installer.extract_archive(malicious, destination)
            self.assertFalse((root / "escape").exists())

            oversized = root / "oversized.zip"
            with zipfile.ZipFile(oversized, "w") as package:
                package.writestr("large", "four")
            with patch.object(installer, "MAX_BUNDLE_BYTES", 3), self.assertRaisesRegex(
                installer.InstallError, "expanded bundle exceeds"
            ):
                installer.extract_archive(oversized, destination)

    def test_tar_bundle_installs_and_links_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = make_bundle(root, "1.0.0", "tar")
            archive = root / "bundle.tar.gz"
            with tarfile.open(archive, "w:gz") as package:
                package.add(bundle, arcname="release")
            with installer.materialized_bundle(str(archive)) as extracted:
                state = installer.install_bundle(root / "install", extracted, None, True)
            self.assertEqual(state["current"], f"1.0.0-{installer.current_platform()}")

            malicious = root / "link.tar"
            with tarfile.open(malicious, "w") as package:
                member = tarfile.TarInfo("link")
                member.type = tarfile.SYMTYPE
                member.linkname = "../escape"
                package.addfile(member)
            destination = root / "bad-tar"
            destination.mkdir()
            with self.assertRaisesRegex(installer.InstallError, "special files are forbidden"):
                installer.extract_archive(malicious, destination)

    def test_manifest_binds_all_source_heads_and_platform(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bundle = make_bundle(Path(temporary), "1.0.0", "one")
            manifest_path = bundle / installer.MANIFEST
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["source_heads"].pop("GraphZero")
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(installer.InstallError, "all four repositories"):
                installer.load_manifest(bundle)

    def test_installer_never_invokes_source_build_tools(self) -> None:
        source = (ROOT / "scripts" / "install_zerostack.py").read_text(encoding="utf-8")
        self.assertNotIn("cargo build", source)
        self.assertNotIn("git clone", source)
        self.assertNotIn("pip install", source)

    def test_windows_launchers_follow_atomic_current_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            prefix = Path(temporary)
            manifest = {"entrypoints": {"zsx": "bin/zsx.exe"}}
            release = prefix / "releases" / "1.0.0-windows-x86_64"
            with patch.object(installer.os, "name", "nt"), patch.object(
                installer, "atomic_write"
            ) as atomic_write:
                installer.write_launchers(prefix, manifest)
                wrapper = atomic_write.call_args.args[1].decode()
                self.assertEqual(atomic_write.call_args.args[0], prefix / "bin" / "zsx.cmd")
                self.assertIn("set /p ZEROSTACK_CURRENT=", wrapper)
                self.assertIn('"%ZEROSTACK_CURRENT%\\bin\\zsx.exe" %*', wrapper)
                atomic_write.reset_mock()
                installer.switch_current(prefix, "1.0.0-windows-x86_64")
                self.assertEqual(atomic_write.call_args.args[0], prefix / "current.txt")
                self.assertEqual(atomic_write.call_args.args[1].decode().strip(), str(release))
            with patch.object(installer.os, "name", "nt"), self.assertRaisesRegex(
                installer.InstallError, "must not contain"
            ):
                installer.write_launchers(prefix / "100%", manifest)


if __name__ == "__main__":
    unittest.main()
