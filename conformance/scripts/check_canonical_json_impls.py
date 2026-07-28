#!/usr/bin/env python3
"""Fail when a second canonical-JSON implementation appears outside zero-abi.

Deliverable 4 of zerostack-graphzero-canonical-json-drift-881.

Canonical JSON feeds the contract digest. It is the single most
drift-forbidden primitive in the stack, and it already drifted once without
anyone noticing: GraphZero's graphzero-pack carried its own
unsigned_canonical_json that emitted struct declaration order while zero-abi
emits sorted keys. Both produced valid JSON and both hashed cleanly, so
nothing failed; the manifests and the contract digests simply disagreed about
identity.

The only cure for a silent divergence is a loud gate. This script walks the
engine checkouts and fails on any function that looks like a canonical-JSON
encoder and does not live in zero-abi.

Usage:
    check_canonical_json_impls.py [engine_root ...]

With no arguments it checks the sibling engine checkouts next to this repo.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# Names that mean "this function encodes canonical/deterministic JSON".
SUSPECT_FN = re.compile(
    r"\bfn\s+([a-z0-9_]*canonical[a-z0-9_]*json[a-z0-9_]*|"
    r"[a-z0-9_]*json[a-z0-9_]*canonical[a-z0-9_]*)\s*[(<]",
    re.IGNORECASE,
)

# zero-abi is the one legitimate home.
ALLOWED_PATH_PARTS = ("zero-abi", "zero_abi")

# Known divergences with a bead already tracking them. Remove an entry when
# its bead closes; leaving it is how the gate silently stops working.
KNOWN_EXCEPTIONS = {
    (
        "crates/graphzero-pack/src/manifest.rs",
        "unsigned_canonical_json",
    ): "zerostack-t76",
}

SKIP_DIRS = {
    "target",
    ".git",
    "node_modules",
    ".beads",
    ".rotation",
    "archive",
    ".zerostack",
}


def iter_rust_files(root: Path):
    for path in root.rglob("*.rs"):
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        yield path


def is_allowed(path: Path) -> bool:
    if any(part in str(path) for part in ALLOWED_PATH_PARTS):
        return True
    # Test-local helpers named canonical_* are golden-output normalizers, not
    # digest encoders: nothing outside the test binary ever hashes their
    # output, so they cannot cause cross-engine identity drift. TokenZero's
    # crates/tokenzero/tests/golden_outputs.rs is the motivating case; it
    # pretty-prints and scrubs volatile fields, which is the opposite of a
    # canonical encoding.
    parts = path.parts
    return "tests" in parts or "benches" in parts or path.name.endswith("_test.rs")


def known_exception(path: Path, fn: str):
    for (suffix, name), bead in KNOWN_EXCEPTIONS.items():
        if name == fn and str(path).endswith(suffix):
            return bead
    return None


def scan_roots(roots: list[Path]) -> tuple[list[str], list[str], int]:
    """Walk all roots; return violations, excused, files checked."""
    violations: list[str] = []
    excused: list[str] = []
    checked = 0
    for root in roots:
        for path in iter_rust_files(root):
            checked += 1
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            if "canonical" not in text.lower():
                continue
            for match in SUSPECT_FN.finditer(text):
                fn = match.group(1)
                if is_allowed(path):
                    continue
                line = text[: match.start()].count("\n") + 1
                bead = known_exception(path, fn)
                where = "%s:%d: fn %s" % (path, line, fn)
                if bead:
                    excused.append("%s  (known, tracked by %s)" % (where, bead))
                else:
                    violations.append(where)
    return violations, excused, checked


def main(argv):
    if len(argv) > 1:
        roots = [Path(a).resolve() for a in argv[1:]]
    else:
        siblings = Path(__file__).resolve().parents[3]
        roots = []
        for name in ("TokenZero", "FSZero", "GraphZero", "ZeroStack"):
            candidate = siblings / name
            if candidate.is_dir():
                roots.append(candidate)
    if not roots:
        print("no engine roots to check", file=sys.stderr)
        return 1
    violations, excused, checked = scan_roots(roots)
    for note in excused:
        print("known divergence: %s" % note)
    if not violations:
        print(
            "canonical-JSON guard: ok (%d files, %d tracked divergence(s), 0 new)"
            % (checked, len(excused))
        )
        return 0
    print(
        "\ncanonical-JSON guard: FAIL - implementation(s) outside zero-abi:",
        file=sys.stderr,
    )
    for violation in violations:
        print("  - %s" % violation, file=sys.stderr)
    print(
        "\nCanonical JSON feeds the contract digest. A second implementation\n"
        "disagrees silently: both sides emit valid JSON and hash cleanly, so\n"
        "nothing fails until two engines disagree about identity.\n"
        "Call zero_abi::canonical_json, or add a KNOWN_EXCEPTIONS entry with a\n"
        "bead that says when it will be removed.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
