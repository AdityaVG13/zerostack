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
    "fszero-mcp",
    "fszero-worker",
    "graphzero-mcp-compat",
    "graphzero-worker",
    "tokenzero-mcp-compat",
    "tokenzero-worker",
}
WORKER_PATH_MARKERS = ("-worker", "-codemode")
COMPAT_PACKAGE_SUFFIXES = ("-mcp", "-mcp-compat")
HUB_TRANSPORT_DEPENDENCY = "zero-mcp"
FORBIDDEN_DEPENDENCIES = {
    "fastmcp-rust",
    "rquickjs",
    "zerostack-machine-permit",
    "zero-codemode",
}
ENGINE_LOCAL_DEPENDENCIES = {
    "rquickjs",
    "zerostack-machine-permit",
    "zero-gate",
}
EXCLUDED_GUARD_PATH_PARTS = {
    "test",
    "tests",
    "bench",
    "benches",
    "example",
    "examples",
    "fixture",
    "fixtures",
    "fuzz",
    "fuzzing",
}
FORBIDDEN_ENGINE_SOURCE_PATTERNS = (
    (
        "rquickjs import",
        re.compile(r"(?:\buse\s+|\bextern\s+crate\s+)rquickjs\b|\brquickjs::"),
    ),
    (
        "QuickJS module/import",
        re.compile(r"\bmod\s+quickjs\s*;|\b(?:crate|super)::(?:\w+::)*quickjs::"),
    ),
    (
        "engine-local machine permit",
        re.compile(
            r"\bMachinePermit\b"
            r"|\bzerostack_machine_permit::(?:MachinePermit|scoped_permit_base)\b"
        ),
    ),
    (
        "engine-local host permit",
        re.compile(
            r"\bmod\s+host_permit\s*;|\b(?:crate|super)::(?:\w+::)*host_permit(?:::|\b)"
        ),
    ),
    (
        "engine-local MCP envelope framing",
        re.compile(
            r"\bfn\s+(?:mcp_(?:success|error)_envelope|parse_mcp_envelope)\b"
        ),
    ),
    (
        "engine-local process lifecycle",
        re.compile(
            r"\b(?:pub\s+)?struct\s+(?:VerifiedChild|ChildBinding)\b"
            r"|\bpub\s+fn\s+escalate_detached\b"
        ),
    ),
)
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
QUICKJS_CFG_RE = re.compile(
    r"\#\s*\[\s*cfg(?:_attr)?\s*\([^\]]*feature\s*=\s*[\"'][^\"']*quickjs[^\"']*[\"']",
    re.DOTALL | re.IGNORECASE,
)


class SurfaceGateError(AssertionError):
    pass


def hub_surface_path(root: Path) -> Path:
    return root / "crates" / "zero-abi" / "src" / "surface.rs"


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


def all_rust_files(root: Path):
    """Yield tracked and non-ignored untracked Rust sources."""
    if (root / ".git").exists():
        try:
            result = subprocess.run(
                [
                    "git",
                    "-C",
                    str(root),
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "--",
                    "*.rs",
                ],
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


def _declared_surface_features(root: Path) -> set[str]:
    declared: set[str] = set()
    for manifest in root.rglob("Cargo.toml"):
        if any(part in {".git", "target", ".ee"} for part in manifest.parts):
            continue
        try:
            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError):
            continue
        features = data.get("features", {})
        declared.update(
            feature
            for feature in ("surface-mcp", "surface-codemode")
            if feature in features
        )
    return declared


def check_exclusive_features(root: Path) -> list[str]:
    if _declared_surface_features(root) != {"surface-mcp", "surface-codemode"}:
        return []
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


def _dependency_package_name(name: str, value: Any) -> str:
    if isinstance(value, dict):
        package = value.get("package")
        if isinstance(package, str):
            return package
    return name


def _table_has_package(values: dict[str, Any], package: str) -> bool:
    return any(
        _dependency_package_name(name, value) == package
        for name, value in values.items()
    )


def _manifest_dependency_tables(data: dict[str, Any]):
    for section in DEPENDENCY_SECTIONS:
        yield section, data.get(section, {})
    for target, target_data in data.get("target", {}).items():
        if not isinstance(target_data, dict):
            continue
        for section in DEPENDENCY_SECTIONS:
            yield f"target.{target}.{section}", target_data.get(section, {})


def _is_excluded_guard_path(path: Path, root: Path) -> bool:
    relative = path.relative_to(root)
    if any(part in EXCLUDED_GUARD_PATH_PARTS for part in relative.parts):
        return True
    return path.stem.endswith(("_test", "_tests"))


def _rust_code_only(text: str) -> str:
    """Replace comments and string literals while retaining Rust code shape."""
    output: list[str] = []
    index = 0
    block_depth = 0
    while index < len(text):
        if block_depth:
            if text.startswith("/*", index):
                block_depth += 1
                index += 2
            elif text.startswith("*/", index):
                block_depth -= 1
                index += 2
            else:
                output.append("\n" if text[index] == "\n" else " ")
                index += 1
            continue
        if text.startswith("//", index):
            end = text.find("\n", index)
            if end == -1:
                output.extend(" " for _ in text[index:])
                break
            output.extend(" " for _ in text[index:end])
            output.append("\n")
            index = end + 1
            continue
        if text.startswith("/*", index):
            block_depth = 1
            output.extend((" ", " "))
            index += 2
            continue
        raw_match = re.match(r'r(\#*)"', text[index:])
        if raw_match:
            delimiter = '"' + raw_match.group(1)
            end = text.find(delimiter, index + len(raw_match.group(0)))
            end = len(text) if end == -1 else end + len(delimiter)
            output.extend("\n" if char == "\n" else " " for char in text[index:end])
            index = end
            continue
        if text[index] == '"':
            end = index + 1
            while end < len(text):
                if text[end] == "\\":
                    end += 2
                    continue
                end += 1
                if text[end - 1] == '"':
                    break
            output.extend("\n" if char == "\n" else " " for char in text[index:end])
            index = end
            continue
        output.append(text[index])
        index += 1
    return "".join(output)


def check_engine_sources(root: Path, strict: bool) -> list[str]:
    """Reject engine-owned runtime/permit implementation from production Rust."""
    if not strict:
        return []
    errors: list[str] = []
    for path in all_rust_files(root):
        if _is_excluded_guard_path(path, root):
            continue
        lowered_stem = path.stem.lower()
        if lowered_stem.startswith("quickjs"):
            errors.append(f"{path}: forbidden engine-local QuickJS source module")
        if lowered_stem == "host_permit":
            errors.append(f"{path}: forbidden engine-local host permit source module")
        if lowered_stem == "mcp_frame":
            errors.append(f"{path}: forbidden engine-local MCP framing source module")
        text = path.read_text(encoding="utf-8", errors="replace")
        if QUICKJS_CFG_RE.search(text):
            errors.append(f"{path}: forbidden engine-local QuickJS feature gate")
        code = _rust_code_only(text)
        for label, pattern in FORBIDDEN_ENGINE_SOURCE_PATTERNS:
            if pattern.search(code):
                errors.append(f"{path}: forbidden {label}")
    return errors


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
            not section.endswith("dev-dependencies")
            and _table_has_package(values, "fastmcp-rust")
            for section, values in tables
        )
        if strict:
            for section, values in tables:
                if section.endswith("dev-dependencies"):
                    continue
                for dependency_name, value in values.items():
                    dependency = _dependency_package_name(dependency_name, value)
                    if dependency not in ENGINE_LOCAL_DEPENDENCIES:
                        continue
                    alias = (
                        f" (aliased as {dependency_name!r})"
                        if dependency_name != dependency
                        else ""
                    )
                    errors.append(
                        f"{manifest}: engine production manifest {section!r} directly "
                        f"depends on forbidden {dependency!r}{alias}"
                    )
            workspace_dependencies = data.get("workspace", {}).get("dependencies", {})
            for dependency_name, value in workspace_dependencies.items():
                dependency = _dependency_package_name(dependency_name, value)
                if dependency not in ENGINE_LOCAL_DEPENDENCIES:
                    continue
                errors.append(
                    f"{manifest}: engine workspace retains forbidden dependency "
                    f"declaration {dependency!r}"
                )
            features = data.get("features", {})
            for feature, members in features.items():
                values = members if isinstance(members, list) else []
                tokens = [feature, *(value for value in values if isinstance(value, str))]
                if any(
                    re.search(
                        r"(?:^|[-_/])(quickjs|codemode-js|javascript-runtime|host-permit|machine-permit)(?:$|[-_/])",
                        token,
                    )
                    or "rquickjs" in token
                    for token in tokens
                ):
                    errors.append(
                        f"{manifest}: engine feature {feature!r} retains forbidden "
                        "runtime/permit wiring"
                    )
        if is_compat_package:
            if strict and not _table_has_package(dependencies, HUB_TRANSPORT_DEPENDENCY):
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
            if not _table_has_package(dependencies, HUB_TRANSPORT_DEPENDENCY):
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
            for dependency_name, value in values.items():
                dependency = _dependency_package_name(dependency_name, value)
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
    for root in roots[1:]:
        errors.extend(check_exclusive_features(root))
        errors.extend(check_worker_dependencies(root, strict_engines))
        errors.extend(check_engine_sources(root, strict_engines))
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
