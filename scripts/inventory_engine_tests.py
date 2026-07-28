#!/usr/bin/env python3
"""Generate a deterministic static inventory of the three engine test trees."""
from __future__ import annotations

import argparse
import ast
from collections import Counter
from dataclasses import dataclass
import json
from pathlib import Path
import re
from typing import Final, Iterable, Sequence

ENGINE_ORDER: Final = {"FSZero": 0, "TokenZero": 1, "GraphZero": 2}
EXCLUDED: Final = frozenset({".git", ".venv", ".zerostack", "__pycache__", "artifacts", "corpus", "corpora", "generated", "node_modules", "pareto", "target", "test-corpus", "vendor", "venv"})
FUNCTION_RE: Final = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{;]*>)?\s*\(")
PY_TEST_RE: Final = re.compile(r"^test_[A-Za-z0-9_]*$")

@dataclass(frozen=True, slots=True)
class Promise:
    promise_id: str
    text: str
    sources: tuple[str, ...]
    terms: tuple[str, ...]

# Defined from public docs and interfaces before inspecting test names.
PROMISES: Final = (
    Promise("security", "Users can rely on engine boundaries rejecting unsafe, unauthorized, or secret-exposing behavior.", ("README.md Security", "documented machine-permit and sandbox boundaries"), ("security", "secret", "sandbox", "permit", "unauthor", "traversal", "symlink")),
    Promise("integrity-corruption", "Users can rely on corrupted or invalid stored data being detected and never silently served as valid.", ("FSZero/docs/durability.md", "GraphZero durable-store documentation"), ("corrupt", "checksum", "digest", "integrity", "tamper", "malformed")),
    Promise("durability-crash", "Users can rely on acknowledged durable state surviving crashes and recovery without silent data loss.", ("FSZero/docs/durability.md", "GraphZero/docs/adr/001-fastcodemode-durable-runtime.md"), ("durab", "crash", "recovery", "journal", "fsync", "power_fail", "powerfail", "atomic")),
    Promise("cli-contract", "Users can rely on documented CLI commands, help, output, and exit behavior.", ("README.md command examples", "docs/contracts/cli-exit-codes.md where present"), ("cli", "command", "exit_code", "help_contract", "subcommand")),
    Promise("packaging-install", "Users can rely on documented packages installing and launching through their supported lifecycle.", ("README.md installation sections", "package/npm/README.md where present"), ("packag", "install", "npm", "wheel", "lifecycle", "distribution")),
    Promise("readme-contract", "Users can rely on checked README commands and claims matching shipped behavior.", ("README.md", "docs/contracts/readme-command-manifest.json where present"), ("readme", "doc_claim", "documentation_claim")),
    Promise("codemode-contract", "Users can rely on documented CodeMode operations, envelopes, and recoverable references.", ("docs/codemode.md",), ("codemode", "code_mode", "compound", "planner")),
    Promise("mcp-contract", "Users can rely on the documented MCP adapter contract when that deployment mode is selected.", ("docs/mcp.md",), ("mcp", "jsonrpc", "json_rpc")),
    Promise("wire-compatibility", "Users can rely on documented wire, schema, ABI, and reference formats remaining compatible.", ("README.md protocol sections", "docs/contracts/zeroref-fixture-cli.md where present"), ("wire", "schema", "protocol", "compat", "abi", "zeroref", "zero_ref", "envelope")),
)
FAMILIES: Final = (
    ("packaging_lifecycle", ("packaging_lifecycle", "package_lifecycle")),
    ("packaging_e2e", ("packaging_e2e", "package_e2e")),
    ("racc_durability_matrix", ("racc_durability_matrix", "durability_matrix")),
    ("readme_claims", ("readme_claims",)),
    ("readme_command_audit", ("readme_command_audit",)),
)

@dataclass(frozen=True, slots=True)
class Test:
    engine: str
    file: str
    name: str
    framework: str
    line: int

@dataclass(frozen=True, slots=True)
class Row:
    engine: str
    file: str
    name: str
    framework: str
    line: int
    promise_id: str | None
    promise_text: str | None
    decision: str
    confidence: str
    rationale: str
    duplicate_family: str | None

    def as_dict(self) -> dict[str, str | int | None]:
        return {"engine": self.engine, "file": self.file, "name": self.name, "framework": self.framework, "line": self.line, "promise_id": self.promise_id, "promise_text": self.promise_text, "decision": self.decision, "confidence": self.confidence, "rationale": self.rationale, "duplicate_family": self.duplicate_family}

def source_files(root: Path, suffix: str) -> Iterable[Path]:
    for path in sorted(root.rglob(f"*{suffix}"), key=lambda item: item.as_posix()):
        if path.is_file() and not any(part.lower() in EXCLUDED for part in path.relative_to(root).parts):
            yield path

def attribute_blocks(lines: Sequence[str]) -> dict[int, str]:
    blocks: dict[int, str] = {}
    index = 0
    while index < len(lines):
        stripped = lines[index].lstrip()
        if not stripped.startswith("#["):
            index += 1
            continue
        start = index
        parts = [stripped]
        depth = stripped.count("[") - stripped.count("]")
        while depth > 0 and index + 1 < len(lines):
            index += 1
            part = lines[index].strip()
            parts.append(part)
            depth += part.count("[") - part.count("]")
        blocks[start] = " ".join(parts)
        index += 1
    return blocks

def proptest_ranges(lines: Sequence[str]) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    for start, line in enumerate(lines):
        if not re.search(r"\b(?:proptest::)?proptest!\s*\{", line):
            continue
        depth = 0
        opened = False
        end = start
        for end in range(start, len(lines)):
            scan = lines[end]
            opened = opened or "{" in scan
            if opened:
                depth += scan.count("{") - scan.count("}")
                if depth <= 0:
                    break
        ranges.append((start, end))
    return ranges

def discover_rust(engine: str, root: Path, path: Path) -> list[Test]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except UnicodeDecodeError:
        return []
    attrs = attribute_blocks(lines)
    ranges = proptest_ranges(lines)
    relative = path.relative_to(root).as_posix()
    found: list[Test] = []
    seen: set[tuple[int, str]] = set()
    pending: tuple[int, str] | None = None
    for index, line in enumerate(lines):
        if index in attrs:
            normalized = re.sub(r"\s+", "", attrs[index])
            if re.search(r"#\[(?:[A-Za-z_][A-Za-z0-9_]*::)*test(?:\([^]]*\))?\]", normalized):
                pending = (index, "tokio::test" if "tokio::test" in normalized else "rust-test")
            continue
        if pending is None:
            continue
        if not line.strip() or line.lstrip().startswith(("#[", "//", "/*", "*")):
            continue
        match = FUNCTION_RE.match(line)
        if match:
            framework = "proptest" if any(start <= index <= end for start, end in ranges) else pending[1]
            found.append(Test(engine, relative, match.group(1), framework, index + 1))
            seen.add((index, match.group(1)))
        pending = None
    for start, end in ranges:
        cases = 0
        for index in range(start, min(end + 1, len(lines))):
            match = FUNCTION_RE.match(lines[index])
            if match:
                cases += 1
                if (index, match.group(1)) not in seen:
                    found.append(Test(engine, relative, match.group(1), "proptest", index + 1))
                    seen.add((index, match.group(1)))
        if cases == 0:
            found.append(Test(engine, relative, f"proptest_macro_case_L{start + 1}", "proptest-macro-synthetic", start + 1))
    return found

def discover_python(engine: str, root: Path, path: Path) -> list[Test]:
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=path.as_posix())
    except (SyntaxError, UnicodeDecodeError):
        return []
    relative = path.relative_to(root).as_posix()
    found: list[Test] = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and PY_TEST_RE.match(node.name):
            found.append(Test(engine, relative, node.name, "python-pytest", node.lineno))
        elif isinstance(node, ast.ClassDef) and node.name.startswith("Test"):
            is_unittest = any(isinstance(base, (ast.Name, ast.Attribute)) and (base.id if isinstance(base, ast.Name) else base.attr) == "TestCase" for base in node.bases)
            for child in node.body:
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)) and PY_TEST_RE.match(child.name):
                    found.append(Test(engine, relative, f"{node.name}.{child.name}", "python-unittest" if is_unittest else "python-pytest", child.lineno))
    return found

def discover_engine(engine: str, root: Path) -> list[Test]:
    if not root.is_dir():
        raise FileNotFoundError(f"{engine} root is not a directory: {root}")
    found: list[Test] = []
    for path in source_files(root, ".rs"):
        found.extend(discover_rust(engine, root, path))
    for path in source_files(root, ".py"):
        found.extend(discover_python(engine, root, path))
    return sorted(found, key=lambda row: (row.file, row.line, row.name, row.framework))

def duplicate_family(test: Test) -> str | None:
    evidence = f"{test.file} {test.name}".lower()
    return next((family for family, markers in FAMILIES if any(marker in evidence for marker in markers)), None)

def mapped_promise(test: Test) -> Promise | None:
    evidence = f"{test.file} {test.name}".lower()
    return next((promise for promise in PROMISES if any(term in evidence for term in promise.terms)), None)

def classify(test: Test) -> Row:
    family = duplicate_family(test)
    promise = mapped_promise(test)
    evidence = f"{test.file} {test.name}".lower()
    cuts = ("roundtrip", "round_trip", "enum_agreement", "enum_and_", "internal_enum", "self_agreement", "encoders_disagree", "private_representation")
    if family:
        if promise is None:
            fallback = {"packaging_lifecycle": "packaging-install", "packaging_e2e": "packaging-install", "racc_durability_matrix": "durability-crash", "readme_claims": "readme-contract", "readme_command_audit": "readme-contract"}[family]
            promise = next(item for item in PROMISES if item.promise_id == fallback)
        decision, confidence = "move", "medium"
        rationale = f"Epic duplicate family {family}; documented {promise.promise_id} boundary is user-visible, but shared-owner migration still requires review."
    elif promise:
        decision, confidence = "keep", "medium"
        rationale = f"Name/path maps to independently documented {promise.promise_id} promise; static inventory does not prove full boundary coverage."
    elif any(marker in evidence for marker in cuts):
        decision, confidence = "cut_candidate", "medium"
        rationale = "Pure roundtrip/internal self-agreement signal with no independently mapped public promise; candidate only, never an automatic deletion."
    else:
        decision, confidence = "needs_review", "low"
        rationale = "No independent public-promise mapping from static name/path evidence; inspect assertion and docs before action."
    return Row(test.engine, test.file, test.name, test.framework, test.line, promise.promise_id if promise else None, promise.text if promise else None, decision, confidence, rationale, family)

def count_rows(counter: Counter[str], keys: Sequence[str] | None = None) -> list[str]:
    return [f"| {key} | {counter.get(key, 0)} |" for key in (keys or sorted(counter))]

def render_summary(rows: Sequence[Row], discovered: Sequence[Test]) -> str:
    engines = Counter(row.engine for row in rows)
    direct = Counter(row.engine for row in discovered)
    decisions = Counter(row.decision for row in rows)
    families = Counter(row.duplicate_family for row in rows if row.duplicate_family)
    frameworks = Counter(row.framework for row in rows)
    promises = Counter(row.promise_id for row in rows if row.promise_id)
    gaps = Counter(row.engine for row in rows if not row.promise_id)
    candidates = Counter(row.engine for row in rows if row.decision == "cut_candidate")
    historical = {"FSZero": 633, "TokenZero": 987, "GraphZero": 1845}
    out = ["# Engine test inventory summary", "", "Generated by scripts/inventory_engine_tests.py; do not hand-edit the JSONL.", "Static source discovery is not an expanded Cargo test-harness listing.", "", "## Reconciliation", "", "| Engine | Inventory rows | Direct parser discovery | Delta | Historical | Drift |", "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for engine in ENGINE_ORDER:
        out.append(f"| {engine} | {engines[engine]} | {direct[engine]} | {engines[engine]-direct[engine]} | {historical[engine]} | {engines[engine]-historical[engine]:+d} |")
    out += [f"| **Total** | **{len(rows)}** | **{len(discovered)}** | **{len(rows)-len(discovered)}** | **{sum(historical.values())}** | **{len(rows)-sum(historical.values()):+d}** |", "", "Rows reconcile exactly because each canonical row is produced from one direct parser case before classification. Historical 633/987/1,845 counts used an earlier broader harness-era measurement. Drift combines repository evolution with scope: this parser counts statically named Rust test attributes, proptest cases, and Python tests; it excludes build/generated/artifact/vendor trees and does not expand test-generating macros.", "", "## Decisions", "", "| Decision | Count |", "| --- | ---: |", *count_rows(decisions, ("keep", "move", "cut_candidate", "needs_review")), "", "A cut_candidate is a review queue, not a deletion recommendation. Ambiguous cases remain needs_review.", "", "## Duplicate families", "", "| Epic family | Count |", "| --- | ---: |", *count_rows(families, tuple(f for f, _ in FAMILIES)), "", "## Frameworks", "", "| Framework | Count |", "| --- | ---: |", *count_rows(frameworks), "", "## Independent promise catalog", "", "Derived from README, CLI, durability, MCP, CodeMode, wire, packaging, and security docs before mapping names. It is not backfit from assertions.", "", "| Promise ID | Promise | Public evidence | Mapped tests |", "| --- | --- | --- | ---: |"]
    for promise in PROMISES:
        out.append(f"| {promise.promise_id} | {promise.text} | {'; '.join(promise.sources)} | {promises[promise.promise_id]} |")
    out += ["", "## Promise gaps", "", "Unmapped tests need assertion/doc review; absence of a name mapping is not evidence that no promise exists.", "", "| Engine | Unmapped tests |", "| --- | ---: |", *count_rows(gaps, tuple(ENGINE_ORDER)), "", "Catalog-level gaps: generic search/query semantics, performance budgets, cancellation, observability, and platform support are not inferred without a single inspected public contract suitable for deterministic mapping.", "", "## 20-30 hypothesis comparison", "", "| Engine | Derived cut candidates | Compared with 20-30 |", "| --- | ---: | --- |"]
    for engine in ENGINE_ORDER:
        count = candidates[engine]
        comparison = "within" if 20 <= count <= 30 else ("below" if count < 20 else "above")
        out.append(f"| {engine} | {count} | {comparison} hypothesis range |")
    out += ["", "The 20-30 figure is a hypothesis, not a quota. Only explicit pure roundtrip/internal self-agreement signals become candidates; forcing a target could remove a promise's only guard.", "", "## Static-discovery limitations", "", "- Inline cfg(test) modules are included because all eligible Rust sources are parsed.", "- Statically named proptest functions retain exact names. A block with no visible function gets one stable proptest_macro_case_L<line> row.", "- Other macro-generated harness names require expansion. No names are fabricated, so historical harness totals need not match.", "- Parsing is deliberately conservative; commented or dynamically generated cases are not executable tests.", ""]
    return "\n".join(out)

def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    hub = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fszero-root", type=Path, default=hub.parent / "FSZero")
    parser.add_argument("--tokenzero-root", type=Path, default=hub.parent / "TokenZero")
    parser.add_argument("--graphzero-root", type=Path, default=hub.parent / "GraphZero")
    parser.add_argument("--inventory", type=Path, default=hub / "benchmarks/testkit/engine-test-inventory.jsonl")
    parser.add_argument("--summary", type=Path, default=hub / "benchmarks/testkit/engine-test-inventory-summary.md")
    return parser.parse_args(argv)

def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    roots = (("FSZero", args.fszero_root.resolve()), ("TokenZero", args.tokenzero_root.resolve()), ("GraphZero", args.graphzero_root.resolve()))
    discovered = [test for engine, root in roots for test in discover_engine(engine, root)]
    discovered.sort(key=lambda row: (ENGINE_ORDER[row.engine], row.file, row.line, row.name, row.framework))
    rows = [classify(test) for test in discovered]
    inventory = "".join(json.dumps(row.as_dict(), ensure_ascii=False, separators=(",", ":")) + "\n" for row in rows)
    args.inventory.parent.mkdir(parents=True, exist_ok=True)
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.inventory.write_text(inventory, encoding="utf-8")
    args.summary.write_text(render_summary(rows, discovered), encoding="utf-8")
    print(" ".join(f"{engine}={sum(row.engine == engine for row in rows)}" for engine in ENGINE_ORDER) + f" total={len(rows)}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
