#!/usr/bin/env python3
"""Generate, verify, and clean bounded disposable performance corpora."""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
MANIFEST_PATH = REPO / "tests" / "perf-corpus-manifest.json"
PERF_ROOT = REPO / "tests" / "artifacts" / "perf"


def load_manifest() -> dict[str, Any]:
    return json.loads(MANIFEST_PATH.read_text())


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def base_lines(functions: int) -> Iterator[str]:
    yield "// synth\n"
    for index in range(functions):
        yield f"pub fn item_{index}() -> u32 {{ {index} }}\n"


def expected_files(manifest: dict[str, Any]) -> dict[str, tuple[int, str]]:
    expected = {
        manifest["small"]["path"]: (
            manifest["small"]["size"], manifest["small"]["sha256"]
        ),
        manifest["base"]["path"]: (
            manifest["base"]["size"], manifest["base"]["sha256"]
        ),
    }
    hashes = manifest["variant_sha256"]
    base_size = int(manifest["base"]["size"])
    for group, indices in manifest["variant_groups"].items():
        for index in indices:
            expected[f"{group}/f{index}.rs"] = (
                base_size + len(f"\n//v{index}\n".encode()), hashes[index]
            )
    return expected


def verify(root: Path, manifest: dict[str, Any]) -> dict[str, Any]:
    failures: list[str] = []
    expected = expected_files(manifest)
    observed = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and path.relative_to(root).parts[0] != "caches"
    }
    unexpected = sorted(observed - set(expected))
    if unexpected:
        failures.append(f"unexpected files: {unexpected}")
    total = 0
    for relative, (size, digest) in expected.items():
        path = root / relative
        if not path.is_file():
            failures.append(f"missing: {relative}")
            continue
        observed_size = path.stat().st_size
        total += observed_size
        if observed_size != size:
            failures.append(f"size {relative}: {observed_size} != {size}")
        observed_hash = sha256(path)
        if observed_hash != digest:
            failures.append(f"sha256 {relative}: {observed_hash} != {digest}")
    if total > int(manifest["max_bytes"]):
        failures.append(f"corpus bytes {total} exceed {manifest['max_bytes']}")
    return {
        "status": "pass" if not failures else "fail",
        "root": root.relative_to(REPO).as_posix(),
        "files": len(expected),
        "bytes": total,
        "failures": failures,
    }


def generate(force: bool) -> dict[str, Any]:
    manifest = load_manifest()
    destination = REPO / manifest["root"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() and not force:
        raise SystemExit(f"{destination} exists; pass --force after verify or clean")
    with tempfile.TemporaryDirectory(prefix="perf-corpus-", dir=destination.parent) as tmp:
        generated = Path(tmp)
        (generated / manifest["small"]["path"]).write_text(manifest["small"]["content"])
        base = generated / manifest["base"]["path"]
        with base.open("w") as handle:
            handle.writelines(base_lines(int(manifest["base"]["functions"])))
        for group, indices in manifest["variant_groups"].items():
            group_dir = generated / group
            group_dir.mkdir()
            for index in indices:
                target = group_dir / f"f{index}.rs"
                shutil.copyfile(base, target)
                with target.open("a") as handle:
                    handle.write(f"\n//v{index}\n")
        report = verify(generated, manifest)
        if report["status"] != "pass":
            raise SystemExit(json.dumps(report, indent=2))
        if destination.exists():
            shutil.rmtree(destination)
        Path(tmp).replace(destination)
    return verify(destination, manifest)


def clean(all_files: bool, max_age_hours: float | None) -> dict[str, Any]:
    manifest = load_manifest()
    cutoff_hours = float(max_age_hours or manifest["max_age_hours"])
    cutoff = time.time() - cutoff_hours * 3600
    removed: list[str] = []
    if not PERF_ROOT.exists():
        return {"status": "pass", "removed": removed}
    for path in list(PERF_ROOT.iterdir()):
        if all_files or path.stat().st_mtime < cutoff:
            relative = path.relative_to(REPO).as_posix()
            shutil.rmtree(path) if path.is_dir() else path.unlink()
            removed.append(relative)
    if PERF_ROOT.exists() and not any(PERF_ROOT.iterdir()):
        PERF_ROOT.rmdir()
    return {"status": "pass", "removed": sorted(removed), "cutoff_hours": cutoff_hours}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("generate", "verify", "clean"))
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--max-age-hours", type=float)
    args = parser.parse_args()
    manifest = load_manifest()
    if args.action == "generate":
        report = generate(args.force)
    elif args.action == "verify":
        report = verify(REPO / manifest["root"], manifest)
    else:
        report = clean(args.all, args.max_age_hours)
    print(json.dumps(report, sort_keys=True))
    if report["status"] != "pass":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
