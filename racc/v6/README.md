# RACC V6 -- current authority docs

Curated current-authority subset of the **ZeroStack RACC Cumulative V6 Release**
(Draft 6, 2026-08-14). This directory supersedes the `racc-r-handoff` pointer
(RACC-R V1 + Q99 / V3 R5 / V2) as the spec authority for implementation work.

## Provenance

- Source: `~/Downloads/ZeroStack_RACC_Cumulative_V6_Release.zip`
- SHA-256: `3944abece0e8cfd04b7099a8f01557f0754cfda12ef100fe9fb6eacf2059d47a`
- Full archive (109 MB: Draft 6 PDFs, historical Draft 1-5 releases, model-ingest
  corpus with 695 records / 120 PDF extractions) stays in the zip. Only the
  current-authority text lands here.

## Reading order (from the corpus READ_ORDER)

1. `lineage/AUTHORITY_AND_SUPERSESSION_RULES.md` -- what is current vs superseded
2. `implementation/V6_CANONICAL_SYSTEM_SPEC.md` -- canonical semantics
3. `implementation/ZERO_EXECUTE_SEMANTIC_ABI_V6.md` -- transport-neutral ABI
4. `implementation/Q99_CAUSAL_CACHE_IMPLEMENTATION_SPEC_V6.md` -- L1/L2/L3 cache law
5. `implementation/V6_IMPLEMENTATION_MASTER_PLAN.md` -- phases 0-10 with gates
6. `implementation/IMPLEMENTATION_BACKLOG_V6.csv` -- 130 requirements (77 P0 / 47 P1 / 6 P2)
7. `implementation/THEOREM_TO_RUNTIME_MAP_V6.md` -- invariant -> checker -> falsifier
8. `benchmarks/TEST_BENCHMARK_AND_FAULT_PROGRAM_V6.md`
9. `research/MISSING_EVIDENCE_REGISTER_V6.md` -- evidence that must come from this repo

Historical PDFs and Drafts 1-5 are lineage, not authority. Consult the archive
only through `lineage/CLAIM_LINEAGE_D1_D6.csv` when a proof or counterexample is
needed.

## Packs and distillations (second landing)

- `packs/01_CURRENT_PAPERS_TEXT.md` -- Draft 6 papers 00-04 as text (proof detail,
  side conditions, 4 process theorems not in the theorem-to-runtime map).
- `packs/02_DRAFT5_DETAIL_PACK.md` -- Draft 5 detail: the 110 requirements with
  their **acceptance tests**, 65-claim ledger, theorem-to-program map,
  research-resolution matrix, full D5 Q99 paper, 15-state Zero Execute machine
  with 5 forbidden transitions, 31 record types, 22 checkers, mandatory test
  suites, minimum vertical slice, first-release acceptance criteria.
- `packs/03_HISTORY_CORRECTIONS_AND_AUXILIARY_PACK.md` -- D1-D4 lineage,
  retained auxiliary results (cache-break taxonomy, No Project-Amnesia,
  branch-local reuse...), superseded framings, failed avenues.
- `distilled/scout_*.md` -- five scout audit reports mapping every pack and the
  whole corpus catalog against the Phase A docs (what is additive, what is
  superseded, 67/67 unique current-authority records accounted for).
- `01_IMPLEMENTATION_AGENT_PROMPT.md` -- required first-response contract for an
  implementing agent (5-value status taxonomy incl. `conflicting`).
- `CUMULATIVE_COMPLETENESS_AUDIT_V6.md` -- corpus completeness + non-claims.

Key scout findings an implementer must not miss:

1. **Four process theorems are unmapped** (Explanation Evidence Preservation,
   Decision-Delimited Refactor d+1, Port Nonregression, Greenfield Strategy
   Preservation) -- they exist only in paper 01 text and have no
   theorem-to-runtime rows or checkers yet.
2. **Acceptance tests for the 110 D5-inherited requirements live in the V6
   backlog CSV and pack 02 only**; V6 canonical docs restate zero of them.
3. **D5's 5 forbidden state transitions** (e.g. Unknown -> Authorized,
   Executing -> Committed directly) are the authoritative safety constraints;
   V6's 14-state chain does not restate them.
4. Exactly two lineage rows are hard-superseded (D4 rewrite-break pair) -- use
   the exact LCP residual `s + b - r` everywhere.
5. Result envelope: V6 ABI schema has 6 result kinds; D5 requirement
   ZS-ADAPTER-003 lists 8 (adds Cancelled, FailedNoAuthority). Resolve at
   implementation time; the JSON schema is canonical for the wire shape.

## Ground rules the corpus itself imposes

- **Audit before implementing** (Phase 0 gate): no semantic implementation until
  every P0 requirement is mapped to existing / partial / conflicting / missing
  code with evidence paths. The backlog CSV `status` column is a corpus default
  ("Not implemented / audit required"), **not** a statement about this repo.
- Baseline is always `same model + same harness + same reasoning + native tools`.
- `Unknown` never aliases `Safe`; authority only from trusted checkers.
- First milestone: read-only Zero Execute (snapshot -> lens -> decision view ->
  receipt -> fallback). No autonomous editing first.

## Status in this repo

- [x] Authority docs landed (this directory)
- [ ] Crosswalk audit: requirement -> code/tests evidence (`CROSSWALK.md`, pending)
- [ ] Gap beads generated from audited P0 gaps
- [ ] Milestone-1 read-only Zero Execute gate mapped onto current `zsx`
