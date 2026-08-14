# Scout Pack 03 -- History, Corrections, and Auxiliary Results

**Scout:** subagent `scout_pack03` | **Date:** 2026-08-14 | **Read scope:** `03_HISTORY_CORRECTIONS_AND_AUXILIARY_PACK.md` (1704 lines, 8 sources, read fully in 4 sequential chunks to EOF) + `current/lineage/CLAIM_LINEAGE_D1_D6.md` (317 lines, 283 claims, read fully).

---

## SUMMARY

The pack consolidates the full claim lineage D1->D6 and the Draft-4 causal-caching paper (the cache-specific mathematical core), plus the Draft-3 theorem ledger, formula index, provider cache fact sheet, missing evidence register, V6 research agenda, and the original-releases manifest. Key takeaways for an implementer:

1. **Only V6 claims (`V6-C01..V6-I01`, 30 records) are current canonical authority.** Every older claim is preserved for lineage but disposition-controlled: D5 = "current detailed predecessor; retained unless V6 narrows/extends" (consult pack 02 for proof detail), D4 = "historical auxiliary result; retained when compatible", D3 = "retained/strengthened" except D3-Q1/Q2 (supplementary stochastic model, NOT universal authority), D2 = "architecture partially superseded by harness correction", D1 = "foundational/historical; consult current scope".
2. **Exactly two lineage rows are explicitly SUPERSEDED:** `D4-02-3_2` (Retrospective Rewrite Cache-Break Theorem) and `D4-02-3_3` (Invalidated cached suffix mass), both superseded by the Draft5/V6 exact LCP characterization (`V6-Q01`).
3. **The Draft-4 paper contributes ~11 retained auxiliary results not surfaced in Phase A packs 00/01** (cache-break taxonomy, No Project-Amnesia, Reference-First Amortization, Branch-Local Reuse, Sequential-to-Causal Topology Transposition, Two-Tier Nonincrease, No Self-Induced Prefix Write Amplification, Idle-time independence, cache-layer coordinates, Executable Causal-Frontier Sufficiency checker, bounded-invalidation/windowed Q99 formal definition). Most of the rest already landed in Phase A as V6-Q01..Q12 / canonical spec sections 9-11.
4. **Epistemic rule (pack line ~417-423):** historical novelty is unresolved (`D5-R10`) unless an independent literature review establishes it; runtime authority never depends on novelty -- only proved mathematics, exact rooted evidence, and a conforming implementation.
5. **Measurement discipline:** "Q99" must name its coordinate (cached provider tokens / indexed object mass / avoided model-visible tokens / avoided primitive calls / complete work); provider policies are time-indexed operational context (fact sheet), not mathematical premises.

---

## OBLIGATIONS

Retained auxiliary results worth implementing or documenting. "ALREADY_IN_PHASE_A" judged against packs 00 (canonical implementation pack) and 01 (current papers) via targeted grep; items marked `no` have zero/nonexhaustive coverage there.

### A. Implement (runtime behavior)

1. **Cache-break taxonomy with four distinct break events and per-event responses.**
   Source: `03_HISTORY..._PACK.md` lines ~1273-1306 (Draft4 §15) and the taxonomy table (prefix-identity / provider-residency / causal-validity / physical-retention break). Response: prefix-identity -> CCNF, append, rerender edge layer only; residency -> rehydrate compact view from L2, never rediscover; causal-validity -> dependency-complete cone invalidation (invalidate is success, never serve stale); physical-retention -> recover from lower durable tier or rebuild, report a true ZeroStack miss. Severity: high (error-classification contract for the runtime; each event has bounded blast radius, none may silently convert stale state into authority).
   ALREADY_IN_PHASE_A: **no** (0 hits in packs 00/01).

2. **No Project-Amnesia on Provider Miss (Thm 15.1).** A provider prefix-cache miss must NOT trigger primitive project rediscovery while the demanded causal closure is retained and valid; it may rerender/reprocess the compact decision view only. This is the L1/L2 decoupling guarantee.
   ALREADY_IN_PHASE_A: **no** (0 hits).

3. **Reference-First Amortization threshold (Thm 6.1):** reference-first representation reduces cumulative model-visible provider input iff `E < (n-s) * sum_t alpha_t`. Charge every expansion (never free behind a handle) and never assume provider caching makes externalization irrelevant (cached tokens still cost, consume context, can expire). This is the accounting rule for capsule-vs-raw-first exposure decisions.
   ALREADY_IN_PHASE_A: **no** (pack 01 "amortiz" hits are campaign-work amortization, not this theorem).

4. **No Self-Induced Prefix Write Amplification (Thm 8.9) + stable-prefix write-amplification metric (`A_rewrite = sum omega_i W_i`, Def 8.8).** Under append-only CCNF, ZeroStack performs no provider-visible rewrite of an already-emitted capsule; new suffixes, provider eviction, and external prefix-contract changes remain separately chargeable. Worth a runtime telemetry metric to prove the zero claim.
   ALREADY_IN_PHASE_A: **no** (0 hits).

5. **Executable Causal-Frontier Sufficiency checker (Thm 12.1):** `C is sufficient for O  <=>  O subset cl(K union C)` (least executable closure under installed deterministic constructors). A trusted kernel checks the closure; a planner (heuristics, learning) proposes frontiers against `argmin sum r(v) + g(C)` -- optimization policy is NOT proof authority. This is the exactness separation that must hold in the L3 cache planner.
   ALREADY_IN_PHASE_A: **partial/yes** -- frontier closure is canonical (V6-C09) and "frontier" appears in both packs, but the deterministic closure-sufficiency checker + planning/authority split as a checkable kernel is only spelled out here. Treat as **yes** for concept, implementer must consult Draft4 §12 for the checker form.

6. **Branch-Local Reuse (Thm 10.1):** if every dependency/contract root of artifact v is equal across project states s, s', then `Kv(s) = Kv(s')` and the artifact is exactly reusable across conversations, harnesses, branches sharing unchanged modules, model changes (rerender but not reindex), and overlapping task lenses. This defines the L2 object-store reuse contract beyond session scope.
   ALREADY_IN_PHASE_A: **no** (0 hits for branch-local).

7. **Sequential-to-Causal Topology Transposition (Thm 9.1):** moving reusable project state from transcript sequence to causal DAG changes invalidation exposure from `W_seq(k)` (suffix after first changed token) to `W_dag(Delta) = w(D(q) ∩ Desc*(Delta))`; strictly more reusable state preserved iff `W_dag < W_seq`. Design implication: keep causal cones fine-grained (actual semantic influence), not file- or session-wide buckets.
   ALREADY_IN_PHASE_A: **no** (0 hits).

8. **Two-Tier Nonincrease (Thm 13.3):** with `0 <= rho <= 1` and `c <= B`, adding provider caching cannot increase model-visible input above `c` on a ZeroStack hit, and the ZeroStack hit cannot increase it above baseline `B`. Bounds the composed L1+L2 expectation `E[T_Z] = h_Z c (1 - h_P (1-rho)) + (1-h_Z) B`.
   ALREADY_IN_PHASE_A: **no** (0 hits; the formula itself appears in V6 Q99 paper).

9. **Idle-time independence (Cor 14.2):** conditional on retained exact storage and unchanged dependencies, `P(H_Z | I, R_Z) = 1` for any idle duration; wall-clock age alone never makes a project object semantically stale. Also **Provider-Controlled Residency Transposition (Thm 14.1):** durable L2 hit probability minus provider hit probability is `P(I ∩ (M_P ∩ R_P)^c) >= 0` under retained storage.
   ALREADY_IN_PHASE_A: **partial** -- Residency Transposition is in pack 01 (V6 Q99 paper §9) = **yes**; Idle-time independence = **no** (0 hits).

10. **Cache-layer coordinates (Def 2.1):** `H_P` (provider prefix hit), `H_T` (stable harness-history identity), `H_Z` (causally valid ZeroStack hit) are separate events; a provider miss does not imply a ZeroStack miss and vice versa. Every "Q99" claim must name its coordinate.
    ALREADY_IN_PHASE_A: **no** (three cache layers L1/L2/L3 are canonical in spec §10, but the three-event distinction and its measurement consequence are this pack's contribution).

### B. Document / formalize

11. **Windowed causal Q99 (Def 8.6 + Prop 8.7):** `inf over W of R_w(W) >= 0.99` is strictly stronger than aggregate average; service-level claims must be windowed. Bounded-invalidation sufficient condition: every window's dependency-complete invalidated demanded mass <= 1% and no physical retention miss on the remaining valid mass.
    ALREADY_IN_PHASE_A: **yes** (canonical spec §9 "Windowed and restoration Q99"; register lists sliding-window Q99 evidence). Implementer: use this pack's formal definition when writing the conformance test.

12. **Causal invalidation mass distribution reporting (Thm 8.3 + §8):** report measured `w(D(q) ∩ Desc*(Delta)) / w(D(q))` distributions rather than treating Q99 as architecture alone; a release should publish the invalidated-mass distribution. A runtime-discovered undeclared dependency is a **falsifier of the current graph certificate, not merely a cache miss** (also Draft4 §18.1).
    ALREADY_IN_PHASE_A: **yes** (V6-Q08 Weighted Causal Q99 canonical; missing-evidence register includes causal-graph omission/counterexample rate). The "falsifier not cache miss" severity framing is explicit here -- carry it into bug classification.

13. **Draft-3 ledger falsifier columns (pack lines 378-423):** each D3 claim carries a direct falsifier (e.g., D3-C1: "two raw observations share one view but admit different protected continuations"; D3-C2: "a compressed segment removes a baseline semantic choice"). The ledger is the quickest falsifier index for the harness/decision claims that V6 retained; useful as a test-suite checklist.
    ALREADY_IN_PHASE_A: **no** (the ledger lives only in this pack).

14. **Provider cache fact sheet as dated operational context (pack lines 1525-1571):** OpenAI (GPT-5.6 breakpoints, append-only guidance), Anthropic (5-min / 1-hour TTL), xAI (miss on first use/eviction/routing), Gemini (implicit vs explicit caching, 1-hour default TTL). Must be refreshed before any public claim; never treated as a theorem premise.
    ALREADY_IN_PHASE_A: **no** (fact sheet only in this pack; Draft4 references were quoted in the pack 01 paper text).

15. **Original-releases manifest integrity pins (pack lines 1685-1704):** 12 archives, all PASS with SHA-256 (e.g., Draft 5 `4287c751...`, Draft 3 `8c0be397...`, Wave 5 `0db15da0...`). Any future release must re-verify these pins; Draft 4 original bundle is a V6-created container (`6172a726...`) because a finalized Draft-4 ZIP was unavailable.
    ALREADY_IN_PHASE_A: **no**.

### C. Empirical targets (already canonical, carry forward)

16. **Draft-3 empirical conjectures D3-E1..E4** (low adaptive decision depth -> one-/two-call coverage; invalidated mass usually < 1%; compact views preserve/improve quality; complete-work Q99 on mature recurring work) are retained/strengthened through D5-R01..R09. They are the measurement targets; D5-R10 (novelty of complete composition) stays **unresolved**.
    ALREADY_IN_PHASE_A: **yes** (V6 research agenda + missing-evidence register cover the measurement targets).

---

## SUPERSESSIONS

Framings an implementer must NOT follow as current authority:

1. **D4-02-3_2 / D4-02-3_3 (Retrospective Rewrite Cache-Break Theorem + Invalidated cached suffix mass) -- SUPERSEDED.** Lineage disposition (lines ~104-105): "Superseded by Draft5/V6 exact LCP characterization." Use `V6-Q01`'s exact identity `lcp(A||X||B, A||S||B) = |A| + lcp(X||B, S||B)` (formula index, pack lines ~343-345) instead of the Draft-4 "first changed token position k" formulation (Draft4 Thm 3.2/Cor 3.3). Same mechanism, corrected exact form.
2. **D3-Q1 (Provider-Controlled Residency Transposition / Finite-TTL Identity) and D3-Q2 (Poisson Durable-Hit Advantage) -- supplementary stochastic model only.** Lineage disposition (lines ~198-199): "Supplementary stochastic model; not universal authority." The `lambda >= 99 mu` durable-Q99 condition (formula index line ~357; Draft4 Cor 14.4) holds only under exponential-arrival assumptions and must never be cited as a production Q99 guarantee. The general transposition theorem (Draft4 Thm 14.1) does not depend on the Poisson specialization.
3. **Draft 2 (L1-L8, F1-F3) architecture framing -- partially superseded by the harness correction.** Lineage disposition (lines ~216-231): "Canonical consolidation lineage; architecture partially superseded by harness correction." The D2 framing (right-congruence quotient machine, channel-capacity product, task-relative view exactness) is lineage, not current architecture; consult V6 canonical spec §§7-8 (decision-boundary compression, stable decision views) for current framing.
4. **Draft 1 "compression always wins" implications -- superseded by the cache-compaction paradox resolution.** Draft4 §1/§3.1: retrospective compaction can *increase* next-request cost; the correct move is reference-first capture (change *when and where* compaction occurs), not abandoning compaction. Naive "shorter prompt = cheaper" framing is explicitly rejected (Example 3.5: with `rho=0.1, u=1`, compaction helps only if `s < 0.1n - 0.9b`; 20k output followed by 10k cached suffix cannot be profitably rewritten).
5. **COMP-J4 (RACC-Native authenticated state ABI one-token invocation) -- NOT core architecture.** Lineage disposition (line ~299): "Optional future optimization; not core architecture." Do not scope or plan core Zero Execute/ABI work around it.
6. **Draft 1 claims as a whole -- foundational/historical.** Disposition: "Foundational/historical; consult current scope." They are preserved for lineage (RACC/OMEGA/INF/SOV/COMP/EPI/MARGIN/FORGE/CAPITAL series) and may only be used as motivation or detail, never as the authority for a runtime decision.
7. **Provider-residency/time-based validity framing -- superseded by causal validity.** Draft4 §14: validity is dependency-determined (`H_Z = I ∩ R_Z`), not wall-clock-determined; provider TTLs are L1 accelerators only (fact sheet "ZeroStack implication" line ~1566).

---

## NOTABLE

Failed avenues, counterexamples, and impossibilities worth preserving (all retained as legitimate negative results):

1. **No universal positive compression ratio (RACC-T5, D1, P/N).** Over arbitrary literal-output tasks, universal positive compression is impossible -- bounds every savings claim; savings claims must be workload-relative.
2. **Adversarial Q99 Impossibility (D5-Q17, proved lower bound).** Q99 cannot be guaranteed against adversarial workloads; the honest claim is a measured property of retained causally valid mass.
3. **Causal Q99 Impossibility under High-Impact Novelty (Draft4 Thm 8.5; canonical as V6 paper Cor 6.2).** If a change creates dependency-complete affected demanded mass > 1%, exact task-relative reuse is strictly below 0.99 until recomputation/verification completes -- no architecture can avoid this; response is rapid causal recomputation, never a misleading hit-rate definition. Related historical row: D4-03-4_4 "Impossible Q99 configuration."
4. **Capacity-Constrained Frontier Hardness (D5-Q14, proved by knapsack reduction).** Finding the cheapest cache frontier is combinatorial -- hence the exact closure *checker* + heuristic *planner* separation (Obligation 5), never a "proven optimal" planner claim.
5. **The shorter-prompt-costs-more counterexample (Draft4 Example 3.5)** -- the canonical worked failure of naive compaction; keep as a test fixture for the rewrite economics (V6-Q02/Q03 break-even/horizon).
6. **Falsifier catalog (Draft4 §18 + D3 ledger falsifier columns):** index incompleteness (undeclared dependency = certificate falsifier), nondeterminism (unbound randomness/time/concurrency/network must be bound as dependencies, represented distributionally, verified, or marked Unknown), decision-view insufficiency (capsule stable but insufficient -> expand and record counterexample), physical storage/maintenance cost (Q99 indexed reuse is not Q99 complete work), provider behavior (time-indexed operational context).
7. **Rendering fragmentation penalties (D5-Q23/Q24).** Fragmentation (rendering-related) is a proved penalty term in Q99/complete-work accounting; D5-Q22 Expected Prefix-Survival Ordering adds the ordering caveat (independent-volatility/commutative-block premises only).
8. **Zero-Failure Q99 Sample Bound (D5-Q21)** -- only under an independent Bernoulli model; and D5-Q19 Causal Hazard Bound (union bound). Both are model-scoped; do not generalize.
9. **D5-R10 / epistemic rule:** historical novelty of the complete composition is unresolved; runtime authority never depends on novelty. External prior art cited (TraceLab, Harness Effect, LCM, Context Compaction Theory, Leyline, Shake/Build Systems à la Carte, self-adjusting computation, leases) reinforces the mechanism but establishes no priority.
10. **Provider policies change (fact sheet, 2026-08-14):** any cached-TTL numbers quoted in Draft4 §14 (OpenAI 30-min GPT-5.6 breakpoints, Anthropic 5-min/1-hour, xAI) are dated operational inputs, already superseded in nuance by the fact sheet (Gemini explicit TTL default 1-hour).

---

## COVERAGE_CONFIRMATION

- `03_HISTORY_CORRECTIONS_AND_AUXILIARY_PACK.md`: **read fully to EOF** -- 1704/1704 lines in 4 sequential chunks (1-407, 408-857, 858-1307, 1308-1704); EOF verified via `sed -n '1704p'`; no skips. All 8 `## SOURCE` sections covered: CLAIM_LINEAGE_D1_D6 (L8-330), FORMULA_INDEX_D1_D6 (331-377), THEOREM_LEDGER_DRAFT3 (378-423 incl. epistemic rule), 02_RACC_Q99_Causal_Caching_Draft4.txt (424-1524, full paper incl. all 19 sections + references), PROVIDER_CACHE_FACT_SHEET_2026-08-14 (1525-1572), MISSING_EVIDENCE_REGISTER_V6 (1573-1628), V6_RESEARCH_AGENDA (1629-1684), ORIGINAL_RELEASES_MANIFEST (1685-1704).
- `current/lineage/CLAIM_LINEAGE_D1_D6.md`: **read fully** -- 317/317 lines; all 283 claims across Draft6 (30), Draft5 (69), Draft4 (74), Draft3 (24), Draft2 (11), Draft1 (75); dispositions captured above.
- Lineage dispositions that affect implementation: (a) only the 30 V6 rows are "Current canonical"; (b) D4-02-3_2/3_3 superseded (Supersession 1); (c) D3-Q1/Q2 supplementary-only (Supersession 2); (d) D2 rows partially superseded (Supersession 3); (e) COMP-J4 optional-not-core (Supersession 5); (f) every D5 row is "retained unless V6 narrows/extends" -> proof detail lives in pack 02 (DRAFT5_DETAIL_PACK), consult when implementing V6 claims; (g) D5-R10 novelty unresolved.
- Phase A grounding for ALREADY_IN_PHASE_A flags: grep cross-check against packs 00/01 (canonical spec + current V6 papers); items flagged `no` have 0 occurrences in both packs.
- Residual risk: ALREADY_IN_PHASE_A flags were inferred from pack 00/01 header/content greps, not from reading packs 00/01 fully; a scout that read those packs fully should confirm flags for items marked `yes`/`partial`.
