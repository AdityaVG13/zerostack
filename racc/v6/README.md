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
