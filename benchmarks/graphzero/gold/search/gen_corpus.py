#!/usr/bin/env python3
"""Deterministic search-scale gold corpus generator (graphzero-aluu).

Produces a pinned Rust-like tree under benchmarks/gold/search/corpus/ with:
  - rare planted symbol (exactly one dedicated hit site)
  - dense common substring names (parse_*)
  - absent needle never present in any name/path/body

Integrity: seeded RNG; same seed+params => byte-identical tree. The harness
verifies corpus.sha256 (sorted path + content digests). This is search gold —
not the synthetic search_bigram_spike fixture, and not edge-accuracy excerpts.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = Path(__file__).resolve().parent / "manifest.json"
DEFAULT_OUT = Path(__file__).resolve().parent / "corpus"
SHA_OUT = Path(__file__).resolve().parent / "corpus.sha256"

WORDS = (
    "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima "
    "mike november oscar papa quebec romeo sierra tango uniform victor whiskey "
    "xray yankee zulu index store walk merge commit journal blob ref orient"
).split()


def load_manifest() -> dict:
    return json.loads(MANIFEST.read_text())


def gen_file(
    rng: random.Random,
    *,
    module: str,
    idx: int,
    fns_per_file: int,
    pad_bytes: int,
    common_prefix: str,
    plant_rare: str | None,
) -> str:
    lines: list[str] = [
        f"//! Search-gold module {module}, file {idx} (graphzero-aluu).",
        "",
    ]
    for _ in range(rng.randint(1, 3)):
        lines.append(f"use crate::{rng.choice(WORDS)}::{rng.choice(WORDS)};")
    lines.append("")

    fn_names: list[str] = []
    for i in range(fns_per_file):
        if i % 3 == 0:
            name = f"{common_prefix}{rng.choice(WORDS)}_{rng.randint(0, 9999)}"
        else:
            name = f"{rng.choice(WORDS)}_{rng.choice(WORDS)}_{rng.randint(0, 9999)}"
        fn_names.append(name)

    for name in fn_names:
        lines.append(f"pub fn {name}(input: &str) -> usize {{")
        for _ in range(rng.randint(1, 4)):
            a, b = rng.choice(WORDS), rng.randint(1, 9999)
            lines.append(f"    let {a} = input.len().wrapping_add({b});")
        callee = rng.choice(fn_names)
        lines.append(f"    {callee}_helper(input.len())")
        lines.append("}")
        lines.append(f"fn {name}_helper(n: usize) -> usize {{ n.wrapping_mul(31) }}")
        lines.append("")

    if plant_rare:
        lines.append(f"pub fn {plant_rare}() -> &'static str {{")
        lines.append(f'    "{plant_rare}"')
        lines.append("}")
        lines.append("")

    body = "\n".join(lines) + "\n"
    encoded = body.encode()
    if pad_bytes > len(encoded):
        need = pad_bytes - len(encoded)
        chunk = ("// " + ("pad " * 16) + "\n").encode()
        reps = max(1, (need + len(chunk) - 1) // len(chunk))
        body = body + (chunk * reps).decode()
    return body


def generate(out: Path, manifest: dict) -> dict[str, object]:
    gen = manifest["generator"]
    planted = manifest["planted"]
    seed = int(gen["seed"])
    files = int(gen["files"])
    fns_per_file = int(gen["fns_per_file"])
    pad = int(gen["pad_comment_bytes_per_file"])
    common_prefix = planted["common_name_prefix"]
    rare = planted["rare"]
    absent = planted["absent"]

    if out.exists():
        for p in sorted(out.rglob("*"), reverse=True):
            if p.is_file():
                p.unlink()
            elif p.is_dir():
                p.rmdir()
    out.mkdir(parents=True, exist_ok=True)

    rng = random.Random(seed)
    rare_file_idx = rng.randint(0, files - 1)
    written = 0
    total_bytes = 0
    leaf = 0
    per_leaf = 40

    while written < files:
        top = f"mod_{leaf // 32:03d}"
        sub = f"sub_{leaf % 32:03d}"
        d = out / top / sub
        d.mkdir(parents=True, exist_ok=True)
        for i in range(min(per_leaf, files - written)):
            plant = rare if written == rare_file_idx else None
            text = gen_file(
                rng,
                module=f"{top}::{sub}",
                idx=i,
                fns_per_file=fns_per_file,
                pad_bytes=pad,
                common_prefix=common_prefix,
                plant_rare=plant,
            )
            if absent in text:
                raise RuntimeError(f"absent needle leaked into generated file {written}")
            path = d / f"f_{i:03d}.rs"
            path.write_text(text)
            total_bytes += len(text.encode())
            written += 1
        leaf += 1

    digest = corpus_sha256(out)
    SHA_OUT.write_text(digest + "\n")
    return {
        "files": written,
        "bytes": total_bytes,
        "rare_file_idx": rare_file_idx,
        "corpus_sha256": digest,
        "seed": seed,
    }


def corpus_sha256(out: Path) -> str:
    h = hashlib.sha256()
    for path in sorted(p for p in out.rglob("*") if p.is_file()):
        rel = path.relative_to(out).as_posix().encode()
        data = path.read_bytes()
        h.update(len(rel).to_bytes(8, "big"))
        h.update(rel)
        h.update(len(data).to_bytes(8, "big"))
        h.update(data)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--check", action="store_true", help="Verify corpus.sha256 only")
    args = ap.parse_args()
    manifest = load_manifest()
    if args.check:
        want = SHA_OUT.read_text().strip()
        got = corpus_sha256(args.out)
        if got != want:
            print(f"SHA mismatch: got={got} want={want}", file=sys.stderr)
            return 1
        print(f"ok {got}")
        return 0
    stats = generate(args.out, manifest)
    gates = manifest["scale_gates"]
    ok = (
        stats["files"] >= gates["min_files"]
        and stats["bytes"] >= gates["min_corpus_bytes"]
    )
    print(json.dumps({"ok": ok, **stats}, indent=2))
    return 0 if ok else 2


if __name__ == "__main__":
    raise SystemExit(main())
