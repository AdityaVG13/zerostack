#!/usr/bin/env python3
"""Guard the hub-owned execution-surface substrate.

The shared registration contract is metadata and validation only. It must not
silently grow a second JavaScript runtime, MCP transport, or engine adapter.
Engine packaging must also fail closed when both install-time faces are built.

Usage:
    check_surface_substrate.py [--strict-engines] [repo_root ...]

Without ``--strict-engines``, optional compatibility dependencies remain
reported as migration debt rather than blocking the hub-only gate. Strict mode
is the release gate used after all engine adapters have migrated.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

FORBIDDEN_HUB_TOKENS = (
    "fastmcp",
    "rquickjs",
    "quickjs",
    "machine-permit",
    "zerostack-machine-permit",
    "mcp_catalog",
    "mcp-catalog",
)
REQUIRED_HUB_MARKERS = (
    "SurfaceKind",
    "DomainAdapterRegistration",
    "SurfaceRegistration",
    "global_registration",
    "WrongSurface",
    "#[serde(deny_unknown_fields)]",
)
WORKER_PACKAGES = {
    "fs-zero",
    "fszero-mcp",
    "fszero-worker",
    "graphzero-mcp-compat",
    "graphzero-worker",
    "tokenzero-mcp-compat",
    "tokenzero-worker",
}
WORKER_PATH_MARKERS = ("-worker", "-codemode")
COMPAT_PACKAGE_SUFFIXES = ("-mcp", "-mcp-compat")
HUB_TRANSPORT_DEPENDENCY = "zero-codemode"
FORBIDDEN_DEPENDENCIES = {
    "fastmcp-rust",
    "rquickjs",
    "zerostack-machine-permit",
    "zero-codemode",
}
EXCLUDED_GUARD_PATH_PARTS = {
    "test",
    "tests",
    "bench",
    "benches",
    "example",
    "examples",
}
EXCLUSIVITY_GUARD_RE = re.compile(
    r"""
    \#\[\s*cfg\s*\(\s*all\s*\(
      (?=[^)]*feature\s*=\s*[\"']surface-mcp[\"'])
      (?=[^)]*feature\s*=\s*[\"']surface-codemode[\"'])
      [^)]*
    \)\s*\)\s*\]\s*compile_error\s*!\s*\(
    """,
    re.DOTALL | re.VERBOSE,
)
CODEMODE_FEATURE_GUARD_RE = re.compile(
    r"""
    \#\[\s*cfg\s*\(\s*all\s*\(
      (?=[^)]*feature\s*=\s*[\"']fastmcp[\"'])
      (?=[^)]*feature\s*=\s*[\"']quickjs[\"'])
      [^)]*
    \)\s*\)\s*\]\s*compile_error\s*!\s*\(
    """,
    re.DOTALL | re.VERBOSE,
)


class SurfaceGateError(AssertionError):
    pass


def hub_surface_path(root: Path) -> Path:
    return root / "crates" / "zero-codemode" / "src" / "surface.rs"


def check_hub_surface(root: Path) -> list[str]:
    path = hub_surface_path(root)
    if not path.is_file():
        return [f"missing hub surface contract: {path}"]
    text = path.read_text(encoding="utf-8")
    errors = [
        f"{path}: missing required marker {marker!r}"
        for marker in REQUIRED_HUB_MARKERS
        if marker not in text
    ]
    lowered = text.lower()
    errors.extend(
        f"{path}: hub surface imports forbidden runtime/transport token {token!r}"
        for token in FORBIDDEN_HUB_TOKENS
        if token in lowered
    )
    return errors


def rust_files(root: Path):
    """Yield Rust sources tracked by git, or all sources for a temp root."""
    git_dir = root / ".git"
    if git_dir.exists():
        try:
            result = subprocess.run(
                ["git", "-C", str(root), "ls-files", "--", "*.rs"],
                check=True,
                capture_output=True,
                text=True,
                timeout=10,
            )
        except (OSError, subprocess.SubprocessError):
            return
        for line in result.stdout.splitlines():
            path = root / line
            if path.is_file():
                yield path
        return
    for path in root.rglob("*.rs"):
        if any(part in {".git", "target", ".ee"} for part in path.parts):
            continue
        yield path


def check_codemode_feature_exclusivity(root: Path) -> list[str]:
    path = root / "crates" / "zero-codemode" / "src" / "lib.rs"
    if not path.is_file():
        return [f"missing CodeMode feature guard source: {path}"]
    text = path.read_text(encoding="utf-8", errors="replace")
    if CODEMODE_FEATURE_GUARD_RE.search(text):
        return []
    return [
        f"{path}: no cfg+compile_error! guard rejects simultaneous fastmcp and quickjs"
    ]


def check_exclusive_features(root: Path) -> list[str]:
    for path in rust_files(root):
        if any(part in EXCLUDED_GUARD_PATH_PARTS for part in path.relative_to(root).parts):
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        if EXCLUSIVITY_GUARD_RE.search(text):
            return []
    return [
        f"{root}: no production cfg+compile_error! guard rejects simultaneous surface-mcp and surface-codemode"
    ]


DEPENDENCY_SECTIONS = ("dependencies", "build-dependencies", "dev-dependencies")


def _dependency_is_optional(value: Any) -> bool:
    return isinstance(value, dict) and value.get("optional") is True


def _manifest_dependency_tables(data: dict[str, Any]):
    for section in DEPENDENCY_SECTIONS:
        yield section, data.get(section, {})
    for target, target_data in data.get("target", {}).items():
        if not isinstance(target_data, dict):
            continue
        for section in DEPENDENCY_SECTIONS:
            yield f"target.{target}.{section}", target_data.get(section, {})


def check_worker_dependencies(root: Path, strict: bool) -> list[str]:
    errors: list[str] = []
    for manifest in sorted(root.rglob("Cargo.toml")):
        if any(part in {".git", "target", ".ee"} for part in manifest.parts):
            continue
        try:
            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            errors.append(f"{manifest}: cannot parse Cargo manifest: {error}")
            continue
        package = data.get("package", {})
        package_name = package.get("name", "")
        is_compat_package = package_name.endswith(COMPAT_PACKAGE_SUFFIXES)
        is_worker_package = package_name in WORKER_PACKAGES or any(
            marker in manifest.parent.name for marker in WORKER_PATH_MARKERS
        )
        dependencies = data.get("dependencies", {})
        tables = list(_manifest_dependency_tables(data))
        direct_fastmcp = any(
            not section.endswith("dev-dependencies") and "fastmcp-rust" in values
            for section, values in tables
        )
        if is_compat_package:
            if strict and HUB_TRANSPORT_DEPENDENCY not in dependencies:
                errors.append(
                    f"{manifest}: compatibility package must depend on hub transport "
                    f"{HUB_TRANSPORT_DEPENDENCY!r}"
                )
            if strict and direct_fastmcp:
                errors.append(
                    f"{manifest}: compatibility package directly depends on "
                    "forbidden 'fastmcp-rust'; use the hub transport"
                )
            continue
        if strict and direct_fastmcp:
            if HUB_TRANSPORT_DEPENDENCY not in dependencies:
                errors.append(
                    f"{manifest}: engine production manifest must depend on hub transport "
                    f"{HUB_TRANSPORT_DEPENDENCY!r} when declaring 'fastmcp-rust'"
                )
            errors.append(
                f"{manifest}: engine production manifest directly depends on forbidden "
                "'fastmcp-rust'; use the hub transport"
            )
        if not is_worker_package:
            continue
        for section, values in tables:
            for dependency, value in values.items():
                if dependency not in FORBIDDEN_DEPENDENCIES:
                    continue
                if section.endswith("dev-dependencies"):
                    continue
                if _dependency_is_optional(value) and not strict:
                    continue
                errors.append(
                    f"{manifest}: worker directly depends on forbidden {dependency!r}"
                )
    return errors


def scan_roots(roots: list[Path], strict_engines: bool = False) -> list[str]:
    errors: list[str] = []
    if not roots:
        return ["no repository roots supplied"]
    # The first root is ZeroStack. Sibling roots do not contain the hub module;
    # they are checked only for worker dependency and feature exclusivity rules.
    errors.extend(check_hub_surface(roots[0]))
    errors.extend(check_codemode_feature_exclusivity(roots[0]))
    for root in roots[1:]:
        errors.extend(check_exclusive_features(root))
        errors.extend(check_worker_dependencies(root, strict_engines))
    return errors


def default_roots() -> list[Path]:
    hub = Path(__file__).resolve().parents[2]
    roots = [hub]
    for name in ("FSZero", "GraphZero", "TokenZero"):
        sibling = hub.parent / name
        if sibling.is_dir():
            roots.append(sibling)
    return roots


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--strict-engines",
        action="store_true",
        help="reject optional compatibility dependencies in worker manifests",
    )
    parser.add_argument("roots", nargs="*", type=Path)
    args = parser.parse_args(argv)
    roots = [path.resolve() for path in args.roots] or default_roots()
    errors = scan_roots(roots, args.strict_engines)
    if errors:
        print("surface substrate guard: FAIL", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    mode = "strict" if args.strict_engines else "hub/adoption"
    print(f"surface substrate guard: ok ({mode}; {len(roots)} repositories)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
