#!/usr/bin/env python3
"""Verify README benchmark claims against committed benchmark artifacts.

Enforces docs/benchmark-integrity.md (bead fszero-ltr): every number in a
claim-annotated README table row must match the committed artifact it cites,
and every cited artifact must carry provenance (git commit, hardware, date).

Claim annotation format, one or more per row:

    <!-- claim:benchmarks/demo-bench_results.json#results.cold_full_index_ms -->

A claim passes when some numeric literal in the row equals the artifact value
under a unit scale (raw, /1e3, /1e6, /1e9), within the rounding tolerance
implied by the literal's shown precision.

Exit code 0 = all claims verified, 1 = violations found.

Also audits competitive publish artifacts under benchmarks/ (fszero-tqz4):
bakeoff.json and other *bakeoff*.json / honest_gate_*.json companions must
carry provenance and git_dirty:false even when README has no claim citation.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

CLAIM_RE = re.compile(r"<!--\s*claim:([^#\s]+)#([A-Za-z0-9_.]+)\s*-->")
NUM_RE = re.compile(r"\d+(?:\.\d+)?")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
# Unit transforms a README cell may apply to a raw artifact value
# (e.g. bytes shown as MB).
SCALES = (1.0, 1e-3, 1e-6, 1e-9)


def resolve(doc: object, dotted: str) -> object:
    cur = doc
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            raise KeyError(dotted)
        cur = cur[part]
    return cur


def literal_matches(literal: str, value: float) -> bool:
    shown = float(literal)
    decimals = len(literal.split(".")[1]) if "." in literal else 0
    tol = 0.5 * 10**-decimals + 1e-12
    return any(abs(value * s - shown) <= tol for s in SCALES)


def commit_exists(root: Path, commit: str) -> bool | None:
    """True/False when git can answer, None when git is unavailable."""
    try:
        r = subprocess.run(
            ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
            cwd=root, capture_output=True, timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return r.returncode == 0


def audit(root: Path, readme: Path) -> tuple[list[str], int, int]:
    """Return (violations, claims_checked, artifacts_checked)."""
    violations: list[str] = []
    artifacts: dict[str, dict] = {}
    text = readme.read_text()
    checked = 0

    for lineno, line in enumerate(text.splitlines(), 1):
        claims = CLAIM_RE.findall(line)
        if not claims:
            continue
        row = CLAIM_RE.sub("", line)
        literals = NUM_RE.findall(row)
        for rel, pointer in claims:
            checked += 1
            where = f"{readme.name}:{lineno} {rel}#{pointer}"
            if rel not in artifacts:
                path = root / rel
                if not path.exists():
                    violations.append(f"{where}: artifact missing")
                    continue
                try:
                    artifacts[rel] = json.loads(path.read_text())
                except json.JSONDecodeError as e:
                    violations.append(f"{where}: artifact is not valid JSON ({e})")
                    continue
            try:
                value = resolve(artifacts[rel], pointer)
            except KeyError:
                violations.append(f"{where}: pointer does not resolve in artifact")
                continue
            if not isinstance(value, (int, float)):
                violations.append(f"{where}: pointer resolves to non-numeric value {value!r}")
                continue
            if not any(literal_matches(lit, float(value)) for lit in literals):
                violations.append(
                    f"{where}: artifact value {value} not shown in row "
                    f"(row literals: {literals})"
                )

    if checked == 0:
        violations.append(f"{readme.name}: no claim annotations found -- nothing is verified")

    for rel, doc in artifacts.items():
        for field in ("hardware", "date"):
            if not doc.get(field):
                violations.append(f"{rel}: provenance field {field!r} missing or empty")
        commit = doc.get("git_commit")
        if not (isinstance(commit, str) and COMMIT_RE.match(commit)):
            violations.append(f"{rel}: git_commit missing or not a full 40-hex sha")
        else:
            if commit[:12] not in text:
                violations.append(
                    f"{rel}: benchmark commit {commit[:12]} not cited in {readme.name}"
                )
            if commit_exists(root, commit) is False:
                violations.append(f"{rel}: git_commit {commit[:12]} not found in this repo")
        if doc.get("git_dirty") is not False:
            violations.append(
                f"{rel}: git_dirty must be false in a committed artifact "
                f"(got {doc.get('git_dirty')!r})"
            )
        date = doc.get("date")
        if date and date not in text:
            violations.append(f"{rel}: artifact date {date} not cited in {readme.name}")

    return violations, checked, len(artifacts)



def audit_competitive_benchmarks(root: Path) -> list[str]:
    """Provenance + dirty checks for competitive benchmarks/* publishes (fszero-tqz4).

    Inventory includes bakeoff.json (omitted from w2g.37 scaling suite list) and
    sibling *bakeoff*.json competitive artifacts. honest_gate_*.json are
    attestations, not measurement artifacts -- only checked for existence when
    a bakeoff measurement file is present.
    """
    violations: list[str] = []
    # Primary competitor table -- always required if benchmarks/ exists.
    bakeoff = root / "benchmarks" / "bakeoff.json"
    if not bakeoff.is_file():
        violations.append("benchmarks/bakeoff.json: missing competitive bakeoff artifact")
        return violations

    competitive = sorted(
        {
            *bakeoff.parent.glob("*bakeoff*.json"),
            bakeoff,
        }
    )
    # Exclude honest_gate_* from dirty/provenance measurement rules.
    measurement = [p for p in competitive if not p.name.startswith("honest_gate_")]

    for path in measurement:
        rel = path.relative_to(root).as_posix()
        try:
            doc = json.loads(path.read_text())
        except json.JSONDecodeError as e:
            violations.append(f"{rel}: artifact is not valid JSON ({e})")
            continue
        # Flat top-level (bakeoff.json) or nested provenance (watch-bakeoff.json).
        prov = doc.get("provenance") if isinstance(doc.get("provenance"), dict) else doc
        for field in ("hardware", "date"):
            if not prov.get(field):
                violations.append(f"{rel}: provenance field {field!r} missing or empty")
        commit = prov.get("git_commit")
        if not (isinstance(commit, str) and COMMIT_RE.match(commit)):
            violations.append(f"{rel}: git_commit missing or not a full 40-hex sha")
        else:
            if commit_exists(root, commit) is False:
                violations.append(f"{rel}: git_commit {commit[:12]} not found in this repo")
        if prov.get("git_dirty") is not False:
            violations.append(
                f"{rel}: git_dirty must be false in a committed artifact "
                f"(got {prov.get('git_dirty')!r})"
            )

    # Honest-gate attestation must sit beside bakeoff when present (policy).
    gate = root / "benchmarks" / "honest_gate_bakeoff.json"
    if not gate.is_file():
        violations.append(
            "benchmarks/honest_gate_bakeoff.json: missing honest-gate attestation "
            "beside bakeoff.json (docs/benchmark-integrity.md)"
        )
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    default_root = Path(__file__).resolve().parents[2]
    parser.add_argument("--root", type=Path, default=default_root)
    parser.add_argument("--readme", type=Path, default=None)
    args = parser.parse_args()
    root: Path = args.root.resolve()
    readme: Path = args.readme or root / "README.md"

    violations, checked, n_artifacts = audit(root, readme)
    competitive_violations = audit_competitive_benchmarks(root)
    violations.extend(competitive_violations)
    if violations:
        print(f"FAIL: {len(violations)} violation(s)")
        for v in violations:
            print(f"  - {v}")
        return 1
    print(
        f"OK: {checked} claim(s) verified against {n_artifacts} artifact(s); "
        f"competitive inventory clean"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
