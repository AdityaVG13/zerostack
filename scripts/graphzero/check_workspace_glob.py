#!/usr/bin/env python3
"""Regression check: workspace members must not ingest hidden crates/* directories."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
ROOT_MANIFEST = REPO_ROOT / "Cargo.toml"


def write_crate(root: Path, crate_path: str, name: str) -> None:
    crate = root / crate_path
    (crate / "src").mkdir(parents=True)
    (crate / "Cargo.toml").write_text(
        f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\n',
        encoding="utf-8",
    )
    (crate / "src" / "lib.rs").write_text("pub fn marker() {}\n", encoding="utf-8")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="graphzero-workspace-glob-") as tmp:
        root = Path(tmp)
        shutil.copyfile(ROOT_MANIFEST, root / "Cargo.toml")
        write_crate(root, "crates/visible_fixture", "visible_fixture")
        write_crate(root, "crates/.hidden_fixture", "hidden_fixture")

        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(root / "Cargo.toml"),
                "--no-deps",
                "--format-version",
                "1",
            ],
            check=True,
            text=True,
            capture_output=True,
        )
        names = {package["name"] for package in json.loads(result.stdout)["packages"]}
        assert "visible_fixture" in names, names
        assert "hidden_fixture" not in names, names


if __name__ == "__main__":
    main()
