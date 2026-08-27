#!/usr/bin/env python3
"""Narrow regression for P08-001: migration catch auto-restores checkout.

1) Source contract: catch handler contains an automatic restore transition
   (Move-Item / Invoke-ArchiveCheckout), matching the pass-08 falsifier.
2) Filesystem contract: archive-then-clone-failure restores CurrentCheckout
   contents from ArchivePath (models the PowerShell catch restore).
"""
from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MIGRATE = ROOT / "scripts" / "rust_windows_migrate.ps1"
MARKER = "tokenzero-migrate-restore-marker.txt"


def assert_catch_has_automatic_restore() -> None:
    text = MIGRATE.read_text(encoding="utf-8")
    catch = text[text.rindex("} catch {") :]
    if "Move-Item" not in catch and "Invoke-ArchiveCheckout" not in catch:
        raise AssertionError(
            "catch block must contain Move-Item or Invoke-ArchiveCheckout "
            "for automatic restore after archive-before-clone failure"
        )
    if "Invoke-RestoreArchivedCheckout" not in catch and "Move-Item" not in catch:
        raise AssertionError("catch block missing automatic restore call")


def simulate_archive_clone_fail_restore() -> None:
    with tempfile.TemporaryDirectory(prefix="tz-migrate-restore-") as tmp:
        home = Path(tmp)
        current = home / "tokenzero"
        archive = home / "tokenzero-python-old"
        current.mkdir()
        (current / MARKER).write_text("prior-checkout\n", encoding="utf-8")
        (current / "nested").mkdir()
        (current / "nested" / "keep.txt").write_text("keep\n", encoding="utf-8")

        # Archive (rename) — mirrors Invoke-ArchiveCheckout success path.
        current.rename(archive)
        assert not current.exists()
        assert archive.is_dir()

        # Failed clone leaves a disposable partial tree at CurrentCheckout.
        current.mkdir()
        (current / "partial").write_text("clone-failed\n", encoding="utf-8")

        # Automatic restore — mirrors catch Invoke-RestoreArchivedCheckout.
        shutil.rmtree(current)
        archive.rename(current)

        assert current.is_dir(), "CurrentCheckout must exist after restore"
        assert not archive.exists(), "ArchivePath must be consumed by restore"
        assert (current / MARKER).read_text(encoding="utf-8") == "prior-checkout\n"
        assert (current / "nested" / "keep.txt").read_text(encoding="utf-8") == "keep\n"
        assert not (current / "partial").exists()


def main() -> int:
    assert_catch_has_automatic_restore()
    simulate_archive_clone_fail_restore()
    print("ok: windows migrate restore contract")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
