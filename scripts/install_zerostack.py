#!/usr/bin/env python3
"""Install, upgrade, verify, roll back, or uninstall a prebuilt ZeroStack bundle."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import json
import os
import platform as host_platform
from pathlib import Path, PurePosixPath
import shutil
import shlex
import subprocess
import tarfile
import tempfile
from typing import Any, Iterator
from urllib.parse import urlparse
from urllib.request import urlopen
import zipfile

SCHEMA = "zerostack.release_bundle.v1"
STATE_SCHEMA = "zerostack.install_state.v1"
MANIFEST = "manifest.json"
SIGNATURE = "manifest.json.minisig"
SUPPORTED_PLATFORMS = {
    "darwin-arm64",
    "darwin-x86_64",
    "linux-arm64",
    "linux-x86_64",
    "windows-x86_64",
}
MAX_BUNDLE_BYTES = 1024 * 1024 * 1024
MAX_BUNDLE_MEMBERS = 4096


def current_platform() -> str:
    system = host_platform.system().lower()
    machine = host_platform.machine().lower()
    architecture = {
        "aarch64": "arm64",
        "arm64": "arm64",
        "amd64": "x86_64",
        "x86_64": "x86_64",
    }.get(machine)
    operating_system = {"darwin": "darwin", "linux": "linux", "windows": "windows"}.get(system)
    if operating_system is None or architecture is None:
        raise InstallError(f"unsupported install host: {system}-{machine}")
    return f"{operating_system}-{architecture}"


class InstallError(RuntimeError):
    """Fail-closed install error suitable for CLI display."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_write(path: Path, data: bytes, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp = Path(temporary)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        temp.chmod(mode)
        os.replace(temp, path)
        try:
            directory = os.open(path.parent, os.O_RDONLY)
        except OSError:
            return
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temp.unlink(missing_ok=True)


def copy_bounded(source: Any, output: Any, maximum: int, label: str) -> int:
    copied = 0
    while True:
        chunk = source.read(min(1024 * 1024, maximum - copied + 1))
        if not chunk:
            return copied
        copied += len(chunk)
        if copied > maximum:
            raise InstallError(f"{label} exceeds the {maximum}-byte safety limit")
        output.write(chunk)


def check_archive_limits(sizes: list[int]) -> None:
    if len(sizes) > MAX_BUNDLE_MEMBERS:
        raise InstallError(f"bundle has more than {MAX_BUNDLE_MEMBERS} archive members")
    if any(size < 0 for size in sizes) or sum(sizes) > MAX_BUNDLE_BYTES:
        raise InstallError(f"expanded bundle exceeds the {MAX_BUNDLE_BYTES}-byte safety limit")


def safe_relative(raw: str) -> Path:
    if "\\" in raw or ":" in raw or "\0" in raw:
        raise InstallError(f"bundle path uses a forbidden platform path form: {raw!r}")
    pure = PurePosixPath(raw)
    if pure.is_absolute() or not pure.parts or any(part in ("", ".", "..") for part in pure.parts):
        raise InstallError(f"bundle path is not a safe relative path: {raw!r}")
    return Path(*pure.parts)


def load_manifest(root: Path) -> dict[str, Any]:
    try:
        value = json.loads((root / MANIFEST).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise InstallError(f"cannot read {MANIFEST}: {error}") from error
    if not isinstance(value, dict) or value.get("schema") != SCHEMA:
        raise InstallError(f"{MANIFEST} must use {SCHEMA}")
    expected_keys = {"schema", "version", "platform", "source_heads", "artifacts", "entrypoints"}
    if set(value) != expected_keys:
        raise InstallError("manifest must contain exactly the release-bundle-v1 fields")
    if not isinstance(value.get("version"), str) or not value["version"].strip():
        raise InstallError("manifest version must be a nonempty string")
    if not isinstance(value.get("platform"), str) or not value["platform"].strip():
        raise InstallError("manifest platform must be a nonempty string")
    if value["platform"] not in SUPPORTED_PLATFORMS:
        raise InstallError(f"manifest platform is unsupported: {value['platform']!r}")
    source_heads = value.get("source_heads")
    if not isinstance(source_heads, dict) or set(source_heads) != {
        "ZeroStack",
        "FSZero",
        "GraphZero",
        "TokenZero",
    }:
        raise InstallError("manifest source_heads must bind all four repositories exactly")
    for repository, head in source_heads.items():
        if not isinstance(head, str) or len(head) != 40 or any(
            character not in "0123456789abcdef" for character in head
        ):
            raise InstallError(f"manifest source head is invalid: {repository}")
    artifacts = value.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise InstallError("manifest artifacts must be a nonempty array")
    entrypoints = value.get("entrypoints")
    if not isinstance(entrypoints, dict) or not entrypoints:
        raise InstallError("manifest entrypoints must be a nonempty object")
    return value


def verify_signature(root: Path, public_key: str | None, allow_unsigned: bool) -> str:
    signature = root / SIGNATURE
    if allow_unsigned:
        return "unsigned-development-override"
    if not public_key:
        raise InstallError("signed release requires --public-key or ZEROSTACK_RELEASE_PUBLIC_KEY")
    if not signature.is_file():
        raise InstallError(f"signed release is missing {SIGNATURE}")
    minisign = shutil.which("minisign")
    if minisign is None:
        raise InstallError("minisign is required to verify release signatures")
    completed = subprocess.run(
        [minisign, "-Vm", str(root / MANIFEST), "-x", str(signature), "-P", public_key],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        evidence = (completed.stderr or completed.stdout).strip().splitlines()
        raise InstallError(f"release signature verification failed: {(evidence or ['unknown error'])[-1]}")
    return "minisign-verified"


def verify_artifacts(root: Path, manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    verified: dict[str, dict[str, Any]] = {}
    if len(manifest["artifacts"]) > MAX_BUNDLE_MEMBERS:
        raise InstallError(f"manifest has more than {MAX_BUNDLE_MEMBERS} artifacts")
    total_size = 0
    for index, artifact in enumerate(manifest["artifacts"]):
        if not isinstance(artifact, dict):
            raise InstallError(f"artifact {index} must be an object")
        if set(artifact) != {"path", "sha256", "size_bytes", "executable"}:
            raise InstallError(f"artifact {index} must contain exactly the v1 artifact fields")
        if not isinstance(artifact.get("path"), str):
            raise InstallError(f"artifact {index} path must be a string")
        if type(artifact.get("executable")) is not bool:
            raise InstallError(f"artifact {index} executable must be a boolean")
        relative = safe_relative(artifact["path"])
        if relative.as_posix() in verified:
            raise InstallError(f"artifact path is duplicated: {relative.as_posix()}")
        source = root / relative
        if not source.is_file() or source.is_symlink():
            raise InstallError(f"artifact is missing or not a regular file: {relative.as_posix()}")
        expected_digest = artifact.get("sha256")
        expected_size = artifact.get("size_bytes")
        if (
            not isinstance(expected_digest, str)
            or len(expected_digest) != 64
            or any(character not in "0123456789abcdef" for character in expected_digest)
        ):
            raise InstallError(f"artifact digest is invalid: {relative.as_posix()}")
        if type(expected_size) is not int or expected_size < 0:
            raise InstallError(f"artifact size is invalid: {relative.as_posix()}")
        total_size += expected_size
        if total_size > MAX_BUNDLE_BYTES:
            raise InstallError(f"manifest artifacts exceed the {MAX_BUNDLE_BYTES}-byte safety limit")
        if source.stat().st_size != expected_size:
            raise InstallError(f"artifact size mismatch: {relative.as_posix()}")
        actual_digest = sha256_file(source)
        if actual_digest != expected_digest:
            raise InstallError(f"artifact digest mismatch: {relative.as_posix()}")
        verified[relative.as_posix()] = {
            "path": relative,
            "source": source,
            "sha256": actual_digest,
            "size_bytes": expected_size,
            "executable": artifact.get("executable") is True,
        }
    for name, raw_path in manifest["entrypoints"].items():
        if (
            not isinstance(name, str)
            or not name
            or name in (".", "..")
            or any(character in name for character in "/\\:\0")
        ):
            raise InstallError(f"entrypoint name is invalid: {name!r}")
        if not isinstance(raw_path, str):
            raise InstallError(f"entrypoint path must be a string: {name!r}")
        relative = safe_relative(raw_path)
        if relative.as_posix() not in verified:
            raise InstallError(f"entrypoint {name!r} does not name a verified artifact")
    return verified


def _archive_member_path(root: Path, name: str) -> Path:
    relative = safe_relative(name.rstrip("/"))
    candidate = (root / relative).resolve()
    if not candidate.is_relative_to(root.resolve()):
        raise InstallError(f"archive member escapes extraction root: {name!r}")
    return candidate


def extract_archive(archive: Path, destination: Path) -> None:
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as package:
            members = package.infolist()
            check_archive_limits([member.file_size for member in members])
            for member in members:
                target = _archive_member_path(destination, member.filename)
                if member.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                with package.open(member) as source, target.open("wb") as output:
                    copy_bounded(source, output, member.file_size, f"archive member {member.filename!r}")
        return
    try:
        package = tarfile.open(archive, "r:*")
    except tarfile.TarError as error:
        raise InstallError(f"unsupported or corrupt bundle archive: {error}") from error
    with package:
        members = package.getmembers()
        check_archive_limits([member.size for member in members])
        for member in members:
            target = _archive_member_path(destination, member.name)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise InstallError(f"archive links and special files are forbidden: {member.name!r}")
            source = package.extractfile(member)
            if source is None:
                raise InstallError(f"cannot extract archive member: {member.name!r}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, target.open("wb") as output:
                copy_bounded(source, output, member.size, f"archive member {member.name!r}")


def _download(url: str, destination: Path) -> None:
    parsed = urlparse(url)
    if parsed.scheme != "https":
        raise InstallError("remote bundle URL must use https")
    try:
        with urlopen(url, timeout=60) as response, destination.open("wb") as output:  # noqa: S310
            if urlparse(response.geturl()).scheme != "https":
                raise InstallError("bundle download redirected to a non-HTTPS URL")
            content_length = response.headers.get("Content-Length")
            if content_length is not None:
                try:
                    declared_length = int(content_length)
                except ValueError as error:
                    raise InstallError("bundle download returned an invalid Content-Length") from error
                if declared_length < 0 or declared_length > MAX_BUNDLE_BYTES:
                    raise InstallError(f"bundle download exceeds the {MAX_BUNDLE_BYTES}-byte safety limit")
            copy_bounded(response, output, MAX_BUNDLE_BYTES, "bundle download")
    except OSError as error:
        raise InstallError(f"bundle download failed: {error}") from error


@contextmanager
def materialized_bundle(source: str) -> Iterator[Path]:
    candidate = Path(source).expanduser()
    if candidate.is_dir():
        yield candidate.resolve()
        return
    with tempfile.TemporaryDirectory(prefix="zerostack-bundle-") as temporary:
        temp = Path(temporary)
        archive = candidate
        if source.startswith("https://"):
            archive = temp / "bundle.download"
            _download(source, archive)
        elif not archive.is_file():
            raise InstallError(f"bundle does not exist: {source}")
        if archive.stat().st_size > MAX_BUNDLE_BYTES:
            raise InstallError(f"bundle archive exceeds the {MAX_BUNDLE_BYTES}-byte safety limit")
        root = temp / "extracted"
        root.mkdir()
        extract_archive(archive, root)
        children = [child for child in root.iterdir() if child.name != "__MACOSX"]
        bundle_root = children[0] if len(children) == 1 and children[0].is_dir() else root
        yield bundle_root


def state_path(prefix: Path) -> Path:
    return prefix / "install-state.json"


def read_state(prefix: Path) -> dict[str, Any]:
    path = state_path(prefix)
    if not path.is_file():
        return {"schema": STATE_SCHEMA, "current": None, "previous": None, "releases": {}}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise InstallError(f"cannot read install state: {error}") from error
    if not isinstance(value, dict) or value.get("schema") != STATE_SCHEMA:
        raise InstallError("install state schema is invalid")
    return value


def write_state(prefix: Path, state: dict[str, Any]) -> None:
    atomic_write(state_path(prefix), f"{json.dumps(state, indent=2, sort_keys=True)}\n".encode())


def release_id(manifest: dict[str, Any]) -> str:
    raw = f"{manifest['version']}-{manifest['platform']}"
    if any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-" for character in raw):
        raise InstallError("version and platform produce an unsafe release id")
    return raw


def write_launchers(prefix: Path, manifest: dict[str, Any]) -> None:
    launchers = prefix / "bin"
    launchers.mkdir(parents=True, exist_ok=True)
    if os.name == "nt" and "%" in str(prefix):
        raise InstallError("Windows install prefix must not contain the '%' command-expansion character")
    for name, relative in manifest["entrypoints"].items():
        destination = launchers / name
        if os.name == "nt":
            current_file = prefix / "current.txt"
            relative_windows = str(safe_relative(str(relative))).replace("/", "\\")
            wrapper = (
                f"@echo off\r\nset /p ZEROSTACK_CURRENT=<\"{current_file}\"\r\n"
                f'"%ZEROSTACK_CURRENT%\\{relative_windows}" %*\r\n'
            ).encode()
            atomic_write(destination.with_suffix(".cmd"), wrapper)
        else:
            current_target = prefix / "current" / safe_relative(str(relative))
            script = f"#!/bin/sh\nexec {shlex.quote(str(current_target))} \"$@\"\n".encode()
            atomic_write(destination, script, 0o755)


def switch_current(prefix: Path, identifier: str) -> None:
    release = prefix / "releases" / identifier
    if os.name == "nt":
        atomic_write(prefix / "current.txt", f"{release}\n".encode())
        return
    temporary = prefix / f".current.{os.getpid()}"
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(Path("releases") / identifier, target_is_directory=True)
    os.replace(temporary, prefix / "current")


def install_bundle(prefix: Path, root: Path, public_key: str | None, allow_unsigned: bool) -> dict[str, Any]:
    manifest = load_manifest(root)
    if manifest["platform"] != current_platform():
        raise InstallError(
            f"bundle platform {manifest['platform']!r} does not match host {current_platform()!r}"
        )
    signature_status = verify_signature(root, public_key, allow_unsigned)
    verified = verify_artifacts(root, manifest)
    incoming_manifest_sha256 = sha256_file(root / MANIFEST)
    identifier = release_id(manifest)
    releases = prefix / "releases"
    releases.mkdir(parents=True, exist_ok=True)
    destination = releases / identifier
    if destination.is_symlink():
        raise InstallError(f"release destination must not be a symbolic link: {destination}")
    if destination.exists():
        installed_manifest_path = destination / MANIFEST
        if not installed_manifest_path.is_file() or sha256_file(installed_manifest_path) != incoming_manifest_sha256:
            raise InstallError(f"release id collision has different manifest content: {identifier}")
    else:
        stage = Path(tempfile.mkdtemp(prefix=f".{identifier}.", dir=releases))
        try:
            for record in verified.values():
                target = stage / record["path"]
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(record["source"], target)
                target.chmod(0o755 if record["executable"] else 0o644)
            shutil.copyfile(root / MANIFEST, stage / MANIFEST)
            os.replace(stage, destination)
        finally:
            if stage.exists():
                shutil.rmtree(stage)
    installed_manifest = load_manifest(destination)
    verify_artifacts(destination, installed_manifest)
    old_state = read_state(prefix)
    old_current = old_state.get("current")
    previous = old_state.get("previous") if old_current == identifier else old_current
    release_states = old_state.get("releases")
    if not isinstance(release_states, dict):
        release_states = {}
    release_states = dict(release_states)
    release_states[identifier] = {
        "manifest_sha256": incoming_manifest_sha256,
        "signature_status": signature_status,
    }
    write_launchers(prefix, installed_manifest)
    state = {
        "schema": STATE_SCHEMA,
        "current": identifier,
        "previous": previous,
        "signature_status": signature_status,
        "manifest_sha256": incoming_manifest_sha256,
        "releases": release_states,
    }
    write_state(prefix, state)
    switch_current(prefix, identifier)
    return state


def verify_install(prefix: Path) -> dict[str, Any]:
    state = read_state(prefix)
    current = state.get("current")
    if not isinstance(current, str) or not current:
        raise InstallError("ZeroStack is not installed")
    release = prefix / "releases" / current
    if os.name == "nt":
        current_file = prefix / "current.txt"
        if not current_file.is_file() or current_file.read_text(encoding="utf-8").strip() != str(release):
            raise InstallError("atomic current-release pointer does not match install state")
    else:
        current_link = prefix / "current"
        if not current_link.is_symlink() or current_link.resolve() != release.resolve():
            raise InstallError("atomic current-release pointer does not match install state")
    manifest = load_manifest(release)
    verify_artifacts(release, manifest)
    installed_manifest_sha256 = sha256_file(release / MANIFEST)
    if state.get("manifest_sha256") != installed_manifest_sha256:
        raise InstallError("install state manifest digest does not match the active release")
    release_state = state.get("releases", {}).get(current, {})
    if release_state.get("manifest_sha256") != installed_manifest_sha256:
        raise InstallError("release history manifest digest does not match the active release")
    for name in manifest["entrypoints"]:
        launcher = prefix / "bin" / (f"{name}.cmd" if os.name == "nt" else name)
        if not launcher.is_file():
            raise InstallError(f"entrypoint launcher is missing: {launcher.name}")
    return state


def rollback(prefix: Path) -> dict[str, Any]:
    state = read_state(prefix)
    previous = state.get("previous")
    if not isinstance(previous, str) or not previous:
        raise InstallError("no previous ZeroStack release is available")
    release = prefix / "releases" / previous
    manifest = load_manifest(release)
    verify_artifacts(release, manifest)
    current = state.get("current")
    write_launchers(prefix, manifest)
    state["current"], state["previous"] = previous, current
    state["manifest_sha256"] = sha256_file(release / MANIFEST)
    release_state = state.get("releases", {}).get(previous, {})
    if release_state.get("manifest_sha256") != state["manifest_sha256"]:
        raise InstallError("previous release history does not match its manifest")
    state["signature_status"] = release_state.get("signature_status")
    write_state(prefix, state)
    switch_current(prefix, previous)
    return state


def uninstall(prefix: Path) -> None:
    state = read_state(prefix)
    manifest: dict[str, Any] | None = None
    current = state.get("current")
    if isinstance(current, str):
        release = prefix / "releases" / current
        if release.is_dir():
            manifest = load_manifest(release)
    if manifest is not None:
        for name in manifest["entrypoints"]:
            launcher = prefix / "bin" / (f"{name}.cmd" if os.name == "nt" else name)
            launcher.unlink(missing_ok=True)
    (prefix / "current").unlink(missing_ok=True)
    (prefix / "current.txt").unlink(missing_ok=True)
    state_path(prefix).unlink(missing_ok=True)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("action", choices=("install", "upgrade", "verify", "rollback", "status", "uninstall"))
    command.add_argument("--bundle", help="bundle directory, archive, or https URL")
    command.add_argument("--prefix", type=Path, default=Path.home() / ".local/share/zerostack")
    command.add_argument("--public-key", default=os.environ.get("ZEROSTACK_RELEASE_PUBLIC_KEY"))
    command.add_argument("--allow-unsigned", action="store_true", help="development fixtures only; never for releases")
    command.add_argument("--json", action="store_true")
    return command


def main() -> int:
    args = parser().parse_args()
    prefix = args.prefix.expanduser().resolve()
    try:
        if args.action in ("install", "upgrade"):
            if not args.bundle:
                raise InstallError(f"{args.action} requires --bundle")
            with materialized_bundle(args.bundle) as root:
                result: Any = install_bundle(prefix, root, args.public_key, args.allow_unsigned)
        elif args.action == "verify":
            result = verify_install(prefix)
        elif args.action == "rollback":
            result = rollback(prefix)
        elif args.action == "status":
            result = read_state(prefix)
        else:
            uninstall(prefix)
            result = {"schema": STATE_SCHEMA, "current": None, "uninstalled": True}
    except (InstallError, OSError) as error:
        if args.json:
            print(json.dumps({"ok": False, "error": str(error)}, sort_keys=True))
        else:
            print(f"install_zerostack: {error}", file=os.sys.stderr)
        return 1
    if args.json:
        print(json.dumps({"ok": True, "state": result}, sort_keys=True))
    else:
        print(f"{args.action}: {result.get('current') or 'not installed'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
