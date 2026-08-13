#!/usr/bin/env python3
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent
EXPECTED = {
    "01_racc_r_theory.tex",
    "02_q99_causal_cache.tex",
    "03_zerostack_systems.tex",
    "04_graphzero_fszero.tex",
    "05_tokenzero.tex",
    "06_zerobench_r.tex",
}
errors: list[str] = []
actual = {path.name for path in ROOT.glob("*.tex")}
if actual != EXPECTED:
    errors.append(f"paper set mismatch: expected {sorted(EXPECTED)}, found {sorted(actual)}")

bare_percent = re.compile(r"(?:9{2}|1(?:0){2})\\?%")
measured_abstract = re.compile(
    r"\bwe\s+(?:achieve|demonstrate|measure|measured|outperform|reduce|save|show)\b",
    re.IGNORECASE,
)
for path in sorted(ROOT.glob("*.tex")):
    text = path.read_text()
    if bare_percent.search(text):
        errors.append(f"{path.name}: bare percentage claim")
    if "No measured result is claimed." not in text:
        errors.append(f"{path.name}: missing scaffold claim status")
    abstract = text.partition(r"\begin{abstract}")[2].partition(r"\end{abstract}")[0]
    if not abstract:
        errors.append(f"{path.name}: missing abstract")
    elif measured_abstract.search(abstract):
        errors.append(f"{path.name}: abstract uses measured-result language")
    if r"\begin{theorem}" in text and "Conditional theorem target." not in text:
        errors.append(f"{path.name}: theorem target is not labeled conditional")

q99 = (ROOT / "02_q99_causal_cache.tex").read_text()
for label in ["Q99-State", "Q99-Input", "Q99-Total", "provider-reported cache hits", "exact reasoning continuation"]:
    if label not in q99:
        errors.append(f"02_q99_causal_cache.tex: missing {label}")

if errors:
    raise SystemExit("claim-language gate failed:\n" + "\n".join(errors))
print("claim-language gate passed: six scaffolds; labeled denominators; no measured-result abstracts")
