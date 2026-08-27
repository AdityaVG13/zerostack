#!/usr/bin/env python3
"""Fail-closed proptest seed/replay contract checker (graphzero-8gx5).

Enforces docs/proptest.md:

- Proptest sites keep random generation: no fixed seed requirement, and no
  `failure_persistence: None` or `FileFailurePersistence::Off`. Library sites
  use SourceParallel; integration sites use only the exact crate-local Direct
  path because SourceParallel cannot resolve their crate root.
- `PROPTEST_DISABLE_FAILURE_PERSISTENCE` is forbidden in proptest sites and in
  scripts/CI references checked here.
- Manual `TestRunner` construction must be explicit: `TestRunner::default()`
  fails, and every `TestRunner::new(<config>)` argument text must contain
  `source_file` and `file!()`.
- Every affected crate (a crate containing a proptest site) must commit
  `proptest-regressions/README.md` with the required contract phrases, and
  `docs/proptest.md` must carry the required contract phrases.
- Tracked regression files (via read-only `git ls-files`) must live under a
  crate-local `crates/<crate>/proptest-regressions/` directory; tracked
  `*.proptest-regressions` fallback files next to test files are forbidden;
  tracked files inside the layout must be `README.md` or a `.txt` mirroring an
  existing source path under that crate.

This checker is static: it never runs cargo or rustc. It calls `git ls-files`
read-only and fails closed if git is unavailable. Exit codes: 0 = valid,
1 = contract violations, 2 = I/O or argument error.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

#: Repository root: parent of the directory containing this script.
REPO_ROOT = Path(__file__).resolve().parents[2]

CRATES_DIR = REPO_ROOT / "crates"
DOCS = REPO_ROOT / "docs" / "proptest.md"

#: Environment override that silently discards failure evidence.
FORBIDDEN_ENV = "PROPTEST_DISABLE_FAILURE_PERSISTENCE"

#: Text that marks a Rust file as a proptest site.
SITE_MARKERS = (
    "proptest!",
    "TestRunner::",
    "FileFailurePersistence",
    "failure_persistence",
    FORBIDDEN_ENV,
)

#: Persistence-disabling constructs forbidden in every proptest site.
FORBIDDEN_PERSISTENCE = (
    "failure_persistence: None",
    "FileFailurePersistence::Off",
)

#: The only accepted FileFailurePersistence::Direct form: the exact crate-local
#: manifest path for an integration-test property. ``<name>`` is the test file stem.
SAFE_DIRECT_RE = re.compile(
    r"FileFailurePersistence::Direct\s*\(\s*concat!\s*\(\s*env!\s*\(\s*"
    r"\"CARGO_MANIFEST_DIR\"\s*\)\s*,\s*\"/proptest-regressions/tests/([^\"]+\.txt)\""
    r"\s*\)\s*\)"
)
_DIRECT_ANY = re.compile(r"FileFailurePersistence::Direct\s*\(")

#: Required contract phrases in each crate README and in docs/proptest.md.
REQUIRED_README_PHRASES = (
    "proptest-regressions",
    "never hand-edit",
    "empty layout",
    "PROPTEST_RNG_SEED",
    "PROPTEST_DISABLE_FAILURE_PERSISTENCE",
)
REQUIRED_DOC_PHRASES = (
    "PROPTEST_RNG_SEED",
    "PROPTEST_DISABLE_FAILURE_PERSISTENCE",
    "proptest-regressions",
    "hand-edit",
    "CARGO_TARGET_DIR=/tmp/rch_target_graphzero",
    "rch exec",
)

_TEST_RUNNER_DEFAULT = re.compile(r"TestRunner::default\s*\(")
_TEST_RUNNER_NEW = re.compile(r"TestRunner::new\s*\(")

class ContractError(Exception):
    """Unreadable or structurally unusable input."""


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ContractError(f"cannot read {path}: {exc}") from exc


def _crate_of(rel_path: str) -> str | None:
    parts = Path(rel_path).parts
    if len(parts) >= 2 and parts[0] == "crates":
        return parts[1]
    return None


def proptest_sites(crates_dir: Path = CRATES_DIR) -> list[Path]:
    """Return every Rust file under ``crates/`` that contains a proptest marker."""
    sites: list[Path] = []
    if not crates_dir.is_dir():
        raise ContractError(f"missing crates directory: {crates_dir}")
    for path in sorted(crates_dir.rglob("*.rs")):
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as exc:
            raise ContractError(f"cannot read {path}: {exc}") from exc
        if any(marker in text for marker in SITE_MARKERS):
            sites.append(path)
    return sites


def _balanced_arg(text: str, start: int) -> str:
    """Return the balanced-paren argument starting at ``start`` (which points at '(')."""
    depth = 0
    for idx in range(start, len(text)):
        ch = text[idx]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[start + 1 : idx]
    raise ContractError("unbalanced TestRunner::new argument")


def is_integration_site(rel_path: str) -> bool:
    """True when the site lives at ``crates/<crate>/tests/<file>.rs``."""
    parts = Path(rel_path).parts
    return len(parts) >= 4 and parts[0] == "crates" and parts[2] == "tests"


def check_site_text(
    path: Path, text: str, problems: list[str], is_integration: bool
) -> None:
    """Validate one proptest-site file's constructs."""
    for forbidden in FORBIDDEN_PERSISTENCE:
        if forbidden in text:
            problems.append(
                f"{path}: forbidden persistence-disabling construct {forbidden!r}"
            )
    direct_count = len(_DIRECT_ANY.findall(text))
    if is_integration:
        # Integration tests cannot use SourceParallel (no lib.rs above tests/);
        # they must pin the exact crate-local Direct persistence path.
        safe_matches = list(SAFE_DIRECT_RE.finditer(text))
        if direct_count == 0 or len(safe_matches) != direct_count:
            problems.append(
                f"{path}: integration proptest site must configure "
                "FileFailurePersistence::Direct(concat!(env!(\"CARGO_MANIFEST_DIR\"), "
                "\"/proptest-regressions/tests/<file>.txt\")) and no other Direct"
            )
        else:
            expected = f"{path.stem}.txt"
            for match in safe_matches:
                if match.group(1) != expected:
                    problems.append(
                        f"{path}: safe Direct target {match.group(1)!r} does not "
                        f"match site stem {expected}"
                    )
    elif direct_count:
        problems.append(
            f"{path}: library proptest site must use the SourceParallel layout, "
            "not FileFailurePersistence::Direct"
        )
    if FORBIDDEN_ENV in text:
        problems.append(f"{path}: forbidden {FORBIDDEN_ENV} reference")
    for match in _TEST_RUNNER_DEFAULT.finditer(text):
        problems.append(
            f"{path}:{text.count(chr(10), 0, match.start()) + 1}: "
            "TestRunner::default() lacks an explicit source_file config"
        )
    for match in _TEST_RUNNER_NEW.finditer(text):
        arg = _balanced_arg(text, match.end() - 1)
        if "source_file" not in arg or "file!(" not in arg:
            problems.append(
                f"{path}:{text.count(chr(10), 0, match.start()) + 1}: "
                "TestRunner::new config must set source_file: Some(file!())"
            )


def tracked_regression_paths(repo_root: Path) -> list[str]:
    """Return tracked paths that are regression-related, via read-only git ls-files."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "ls-files"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise ContractError(f"git unavailable; cannot verify tracked layout: {exc}") from exc
    if proc.returncode != 0:
        raise ContractError(
            f"git ls-files failed (exit {proc.returncode}): {proc.stderr.strip()}"
        )
    tracked = [line for line in proc.stdout.splitlines() if line.strip()]
    return [
        path
        for path in tracked
        if "proptest-regressions" in path or path.endswith(".proptest-regressions")
    ]


def check_tracked_layout(
    repo_root: Path, tracked: list[str], problems: list[str]
) -> None:
    """Validate tracked regression paths against the crate-local layout."""
    layout_prefix = re.compile(r"^crates/[^/]+/proptest-regressions/")
    for rel in sorted(tracked):
        rel_path = Path(rel)
        if rel_path.suffix == ".proptest-regressions" and "proptest-regressions/" not in rel:
            # Fallback file next to an integration test: outside the layout.
            problems.append(
                f"{rel}: tracked proptest fallback file outside the crate-local "
                "proptest-regressions layout"
            )
            continue
        match = layout_prefix.match(rel)
        if not match:
            problems.append(
                f"{rel}: tracked regression file outside crates/<crate>/proptest-regressions/"
            )
            continue
        crate = rel_path.parts[1]
        inner = rel_path.relative_to(Path("crates") / crate / "proptest-regressions")
        if inner.name == "README.md":
            continue
        if inner.suffix != ".txt":
            problems.append(
                f"{rel}: non-README entry in proptest-regressions layout must be a .txt"
            )
            continue
        mirror = inner.with_suffix(".rs")
        candidates = [
            repo_root / "crates" / crate / "src" / mirror,
            repo_root / "crates" / crate / mirror,
        ]
        if not any(candidate.is_file() for candidate in candidates):
            problems.append(
                f"{rel}: tracked regression .txt does not mirror an existing "
                f"source file under crates/{crate}"
            )


def check_required_phrases(
    path: Path, phrases: tuple[str, ...], problems: list[str], what: str
) -> None:
    if not path.is_file():
        problems.append(f"{path}: missing {what}")
        return
    text = _read_text(path).lower()
    for phrase in phrases:
        if phrase.lower() not in text:
            problems.append(f"{path}: {what} lacks required phrase {phrase!r}")


def run(
    repo_root: Path = REPO_ROOT,
    crates_dir: Path | None = None,
) -> tuple[int, list[str]]:
    """Validate the contract; returns ``(exit_code, problems)``."""
    crates_dir = crates_dir or repo_root / "crates"
    problems: list[str] = []
    try:
        sites = proptest_sites(crates_dir)
        if not sites:
            return 1, [f"{crates_dir}: no proptest sites found"]
        affected_crates: set[str] = set()
        for site in sites:
            rel = site.relative_to(repo_root).as_posix()
            crate = _crate_of(rel)
            if crate is None:
                problems.append(f"{rel}: proptest site outside crates/<crate>/ layout")
                continue
            affected_crates.add(crate)
            check_site_text(
                site, _read_text(site), problems, is_integration_site(rel)
            )

        check_required_phrases(repo_root / "docs" / "proptest.md", REQUIRED_DOC_PHRASES, problems, "docs/proptest.md")
        for crate in sorted(affected_crates):
            readme = repo_root / "crates" / crate / "proptest-regressions" / "README.md"
            check_required_phrases(
                readme, REQUIRED_README_PHRASES, problems, f"{crate} proptest-regressions README"
            )

        tracked = tracked_regression_paths(repo_root)
        check_tracked_layout(repo_root, tracked, problems)
    except ContractError as exc:
        return 2, [str(exc)]

    return (1 if problems else 0), problems


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail-closed proptest seed/replay contract checker."
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root (default: derived from this script's location)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    code, problems = run(repo_root=args.repo_root)
    if problems:
        for problem in problems:
            print(f"proptest-contract: {problem}", file=sys.stderr)
    if code == 0:
        print("proptest-contract: valid")
    return code


if __name__ == "__main__":
    sys.exit(main())
