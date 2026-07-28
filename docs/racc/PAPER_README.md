# RACC Exact Causal Frontier v1 — read first

## What is proved

The formal package proves a task-indexed zero-degradation frontier, not a universal fixed compression percentage.

For a locked agent, environment, action interface, tokenizer, query algebra, router class, and resource gauge, define

\[
K^0(\omega;\Lambda)
=
\min\{\text{LM-visible input communication}:\text{byte, policy, and task distortion are zero}\}.
\]

The optimal exact saving is

\[
S^{0,*}_\omega=1-K^0(\omega;\Lambda)/R_\tau(\omega).
\]

The master theorem proves that a proof-carrying TokenZero runtime preserves the complete action law whenever every committed compressed decision has a sound sufficiency certificate and raw context is used otherwise. If a fixed certificate hierarchy approximates the exact optimum by

\[
K_H\le \chi K^0+\eta,
\]

then direct certified construction costs at most

\[
\chi K^0+\eta+G,
\]

and deterministic model-visible geometric expansion costs strictly less than

\[
4\chi K^0+4\eta+q\bigl(1+\lceil\log_2((\chi K^0+\eta)/b_0)\rceil\bigr)+G.
\]

Whenever the relevant bound is at most \(\varepsilon R\), the exact saving is at least \(1-\varepsilon\). Thus 97%, 99%, and 99.9% are obtained by setting \(\varepsilon\) to 0.03, 0.01, and 0.001.

The package additionally proves:

- exact action-law simulation and universal sound fallback;
- a canonical minimal finite action quotient and token lower bounds;
- impossibility of a universal fixed percentage;
- undecidability of unrestricted exact-sufficiency verification;
- an exact independent sparse-demand rate;
- an exact replay/exposure identity;
- bounded/sublinear cumulative phase laws;
- the sharp deterministic factor 4 for black-box nested expansion;
- NP-hardness of minimum certificate selection;
- exact dependency-closure, streaming-query, and causal-cut constructions; and
- a task-success no-regret transactional wrapper for verifiable tasks.

## What remains conjectural

RADC-D asserts that broad real coding workloads have sublinear exact causal certificate complexity, a bounded-quality fixed hierarchy, and sufficiently low fallback. The mathematical implication from those premises to savings tending to 100% is proved. The prevalence of the premises in real workloads must be established experimentally.

## Reproduce

```bash
./RUN_ALL.sh
```

Current artifact results:

- Python exact suite: **214 PASS, 0 FAIL**;
- independent C++20 checker: **all checks pass**;
- Rust semantic contract: included, but `rustc` was unavailable in this container, so no Rust compiler attestation is claimed.
