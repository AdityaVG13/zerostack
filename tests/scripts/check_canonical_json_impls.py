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

# Calling the canonical zero-abi encoder makes a function a thin delegating
# wrapper, not a second implementation: it cannot diverge because the bytes it
# emits come from zero-abi itself.
DELEGATE_MARKER = "zero_abi::canonical_json"

# Real Rust test attributes, matched exactly. Qualified forms (e.g.
# `#[tokio::test]`) are test registrations too. Anything else that merely
# contains the substring "test" (e.g. `#[cfg(feature = "latest")]`) is NOT a
# test attribute and must not exempt a canonical-JSON encoder.
TEST_ATTR_RE = re.compile(
    r"^#\[\s*(?:(?:[A-Za-z_][A-Za-z0-9_]*::)*test|cfg\(test\))\s*\]$"
)

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


def is_test_fn(text: str, fn_pos: int) -> bool:
    """True when the `fn` at fn_pos is test-only, never a digest encoder.

    Covers both `#[test]`-annotated helpers in src files and functions inside
    a `#[cfg(test)] mod` block. Test helpers may be named *_canonical_json* to
    assert against the golden form, but nothing outside the test binary ever
    hashes their output, so they cannot cause cross-engine identity drift.
    """
    head = text[:fn_pos]
    lines = head.splitlines()
    attrs = []
    i = len(lines) - 1
    while i >= 0:
        line = lines[i].strip()
        if not line or line.startswith("///") or line.startswith("//!"):
            i -= 1
            continue
        if line.startswith("#["):
            attrs.append(line)
            i -= 1
            continue
        break
    if any(TEST_ATTR_RE.fullmatch(attr) for attr in attrs):
        return True
    cfg_idx = head.rfind("#[cfg(test)]")
    if cfg_idx >= 0:
        segment = text[cfg_idx:fn_pos]
        if segment.count("{") - segment.count("}") >= 1:
            return True
    return False


def fn_body(text: str, fn_pos: int) -> str | None:
    """Return the brace-delimited body of the fn at fn_pos, or None."""
    rest = text[fn_pos:]
    depth = 0
    for idx, ch in enumerate(rest):
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return rest[: idx + 1]
    return None


def delegates_to_zero_abi(text: str, fn_pos: int) -> bool:
    """True when the fn at fn_pos calls zero_abi::canonical_json in its body."""
    body = fn_body(text, fn_pos)
    return body is not None and DELEGATE_MARKER in body


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
                fn_pos = match.start()
                if is_test_fn(text, fn_pos) or delegates_to_zero_abi(text, fn_pos):
                    continue
                line = text[:fn_pos].count("\n") + 1
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
