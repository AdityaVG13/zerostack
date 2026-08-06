#!/usr/bin/env python3
from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
lean_files = sorted(root.rglob("*.lean"))
forbidden = {
    "placeholder proof": re.compile(r"\bsor" + r"ry\b"),
    "admitted proof": re.compile(r"(?:^\s*|\bby\s+)ad" + r"mit\b", re.MULTILINE),
    "user axiom": re.compile(r"^\s*axi" + r"om\b", re.MULTILINE),
    "native decision oracle": re.compile(r"\bnative_" + r"decide\b"),
}
errors: list[str] = []
for path in lean_files:
    text = path.read_text()
    for label, pattern in forbidden.items():
        if pattern.search(text):
            errors.append(f"{path.relative_to(root)}: {label}")

for release_root in [
    root / "ZeroRacc.lean",
    root / "ZeroRacc" / "All.lean",
    root / "ZeroRacc" / "V2All.lean",
    root / "ZeroRacc" / "V3All.lean",
    root / "ZeroRacc" / "RaccRCore.lean",
]:
    if "Conjectures" in release_root.read_text():
        errors.append(f"{release_root.relative_to(root)}: imports conjectures")

if errors:
    raise SystemExit("trust scan failed:\n" + "\n".join(errors))
print(f"trust scan passed: {len(lean_files)} Lean files; conjectures isolated")
