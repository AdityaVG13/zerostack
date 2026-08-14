# RACC V6 Corpus Coverage Audit -- Coverage Auditor Scout

Release root: `/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release`
Sources read: `current/model_ingest/ALL_INFORMATION_CATALOG.md`, `current/model_ingest/CORPUS_INDEX.csv`, `CUMULATIVE_COMPLETENESS_AUDIT_V6.md`, `archive/ORIGINAL_RELEASES_MANIFEST.md`, `integrity/INTEGRITY_VALIDATION_REPORT.md`, plus `current/model_ingest/00_START_HERE_FOR_IMPLEMENTATION.md` (Phase A/B/C/D), `READ_ORDER.json`, and the four pack headers for `## SOURCE:` coverage maps.

## SUMMARY

- **695 readable records** indexed; **432 unique** (empty `duplicate_of`), **263 byte-identical duplicates** retained for provenance (integrity: PASS, 920 files, 0 digest mismatches).
- **Unique by authority: 67 current / 365 historical.**
- Unique current breakdown: 64 Draft 6 + 1 Draft 5 (pack 02 itself) + 2 Historical/unspecified (CITATION.cff, RELEASE_MANIFEST_V6.json).
- Unique historical breakdown: Draft 1: 49, Draft 2: 37, Draft 3: 29, Draft 4: 6, Draft 5: 77, v0.1: 33, v0.3: 69, Waves 2-5: 42, archive manifests: 3. (Category mix: 184 archive, 90 paper, 37 validation, 17 implementation, 16 schema, 13 lineage, 8 research.)
- **Coverage layers (union of `## SOURCE:` sets + canonical docs):**
  - Pack 00 `00_CANONICAL_IMPLEMENTATION_PACK.md`: 14 sources (README, COMPLETENESS_AUDIT, 00_START_HERE, 01_AGENT_PROMPT, V6_CANONICAL_SYSTEM_SPEC, ZERO_EXECUTE_SEMANTIC_ABI_V6, Q99_CAUSAL_CACHE_IMPLEMENTATION_SPEC_V6, V6_IMPLEMENTATION_MASTER_PLAN, IMPLEMENTATION_BACKLOG_V6_SUMMARY, THEOREM_TO_RUNTIME_MAP_V6, AUTHORITY_AND_SUPERSESSION_RULES, CORRECTIONS_AND_SUPERSESSIONS, CLAIM_LEDGER_V6.md, TEST_BENCHMARK_AND_FAULT_PROGRAM_V6) -- all current.
  - Pack 01 `01_CURRENT_PAPERS_TEXT.md`: 5 Draft 6 paper texts (00-04).
  - Pack 02 `02_DRAFT5_DETAIL_PACK.md`: 6 Draft 5 sources (impl requirements, claim ledger, theorem-to-program map, resolution matrix, Q99 paper text, impl-requirements paper text) -- all historical.
  - Pack 03 `03_HISTORY_CORRECTIONS_AND_AUXILIARY_PACK.md`: 8 sources (CLAIM_LINEAGE_D1_D6.md, FORMULA_INDEX_D1_D6.md, THEOREM_LEDGER_DRAFT3.md, draft4 Q99 text, PROVIDER_CACHE_FACT_SHEET_2026-08-14, MISSING_EVIDENCE_REGISTER_V6, V6_RESEARCH_AGENDA, ORIGINAL_RELEASES_MANIFEST.md) -- 5 current + 3 historical.
  - **Phase A canonical docs** (9, per `00_START_HERE_FOR_IMPLEMENTATION.md`): all inside pack 00 **except `current/implementation/IMPLEMENTATION_BACKLOG_V6.csv`** (pack 00 carries only the 661-word summary; Phase A requires the full 6,528-word CSV).
  - **Draft 6 papers**: pack 01 covers 00-04; the **anthology `.../current/papers/ZeroStack_RACC_Cumulative_Research_Series_Draft6_Anthology.txt`** (8,542 words) is covered only via the Draft 6 papers layer.
- **Unique current covered: 26 of 67. Unique current NOT covered: 41 (listed below). 26 + 41 = 67 = 100%.**
- Authority rules consulted: `current/lineage/AUTHORITY_AND_SUPERSESSION_RULES.md` (in pack 00), `CUMULATIVE_COMPLETENESS_AUDIT_V6.md` authority order, `READ_ORDER.json` stages 3-4, and Phase C/D of the start-here doc.

## FILES_NEEDING_DIRECT_READ

Exactly the 41 unique current-authority records not covered by any pack source, any Phase A canonical doc, or the Draft 6 papers. Absolute paths; one per line with reason.

### Tier 1 -- genuine unique content, no equivalent elsewhere (MUST read)

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/causal_residency_plan_v6.schema.json     - Draft 6 canonical schema; only copy in corpus; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/decision_view_v6.schema.json              - Draft 6 canonical schema; only copy; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/implementation_requirement_v6.schema.json - Draft 6 canonical schema; only copy; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/zero_execute_request_v6.example.json      - Draft 6 semantic ABI example payload; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/zero_execute_request_v6.schema.json       - Draft 6 semantic ABI request contract; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/schemas/zero_execute_result_v6.schema.json        - Draft 6 semantic ABI result contract; not in any pack
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/validators/validate_v6.py                         - Draft 6 finite validation checker source; defines what "validated" means
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/validators/validate_package_v6.py                 - Draft 6 package validation checker source
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/validators/VALIDATION_REPORT.md                   - Draft 6 validation evidence (1,027,205 checks, 0 failures claimed)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/validators/PACKAGE_VALIDATION_REPORT.md           - Draft 6 package validation evidence
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/validators/validation_output.json                 - Draft 6 validation run output (evidence payload)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/BUILD_VALIDATION.md                     - Draft 6 build evidence; unique current artifact record
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/lineage/CLAIM_LINEAGE_D1_D6.csv                   - 7,936 words; explicitly REQUIRED by Phase C ("Use ... to locate the exact source and current disposition of a claim"); pack 03 carries only the .md twin
```

### Tier 2 -- machine-readable twins of covered docs (read only for field-level/schema fidelity)

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/implementation/IMPLEMENTATION_BACKLOG_V6.json - JSON twin of Phase A CSV; 7,689 words; read if JSON structure matters
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/lineage/CLAIM_LEDGER_V6.csv                  - CSV twin of CLAIM_LEDGER_V6.md (pack 00); read for exact fields/status values
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/lineage/CLAIM_LEDGER_V6.json                 - JSON twin of CLAIM_LEDGER_V6.md (pack 00); read for exact fields/status values
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/lineage/CLAIM_LINEAGE_D1_D6.json             - JSON twin of CLAIM_LINEAGE_D1_D6.md (pack 03) / .csv above
```

### Tier 3 -- paper LaTeX build sources (same intellectual content as pack 01 texts; read only for build/notation)

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/00_core/main.tex       - source of paper 00 (text covered by pack 01)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/01_harness/main.tex    - source of paper 01 (text covered by pack 01)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/02_q99/main.tex        - source of paper 02 (text covered by pack 01)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/03_impl/main.tex       - source of paper 03 (text covered by pack 01)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/04_agenda/main.tex     - source of paper 04 (text covered by pack 01)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/05_anthology/main.tex  - anthology LaTeX driver (127 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/BUILD_INSTRUCTIONS.md  - paper build instructions (102 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/references.bib         - bibliography (213 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/sources/shared/preamble.tex    - shared LaTeX preamble (181 words)
```

### Tier 4 -- trivial preflight stubs and packaging metadata (skim only)

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/00_ZeroStack_RACC_Cumulative_Core_Draft6.txt                    - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/01_Zero_Execute_Harness_Runtime_Draft6.txt                      - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/02_RACC_Q99_Causal_Residency_Draft6.txt                         - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/03_ZeroStack_Implementation_Conformance_Draft6.txt              - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/04_ZeroStack_RACC_Draft6_Research_Agenda.txt                    - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/artifacts/preflight/ZeroStack_RACC_Cumulative_Research_Series_Draft6_Anthology.txt  - 22-word preflight stub; no unique content
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/CITATION.cff                      - release citation metadata (60 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/PUBLICATION_STATUS.md            - publication status (88 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/RELEASE_MANIFEST_V6.json         - release manifest (265 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/READ_ORDER.json     - ingestion order spec; superseded-by-summary in 00_START_HERE (but unique record)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/build_corpus.py     - corpus generation script (tooling, not content)
```

### Self-referential -- the packs themselves (consumed as coverage vehicles, not separately read)

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/packs/00_CANONICAL_IMPLEMENTATION_PACK.md
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/packs/01_CURRENT_PAPERS_TEXT.md
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/packs/02_DRAFT5_DETAIL_PACK.md
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/packs/03_HISTORY_CORRECTIONS_AND_AUXILIARY_PACK.md
```

## RETAINED_HISTORICAL

Historical unique records the authority rules explicitly keep relevant (READ_ORDER stages 3-4, start-here Phase C/D, pack 02/03 sources, completeness-audit authority order items 3-5). **Core retained set (11)** -- these are the "detailed predecessor authority" and "historical proofs and auxiliary results":

```
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft5/implementation/ZEROSTACK_IMPLEMENTATION_REQUIREMENTS_DRAFT5.md  - stage 3 + Phase C + pack 02: detailed current predecessor (6,896 words)
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft5/claims/CLAIM_LEDGER_DRAFT5.md                                  - stage 3 + Phase C + pack 02
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft5/research/RESEARCH_AGENDA_RESOLUTION_MATRIX_DRAFT5.md            - stage 3 + Phase C + pack 02
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft5/implementation/THEOREM_TO_PROGRAM_MAP_DRAFT5.md                 - pack 02 source
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/text/pdf_text/archive/extracted/Draft5/papers/02_RACC_Q99_Causal_Caching_Draft5.txt  - Phase C Draft 5 Q99 paper + pack 02
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/current/model_ingest/text/pdf_text/archive/extracted/Draft5/papers/05_ZeroStack_Draft5_Implementation_Requirements.txt  - pack 02 source
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft3/THEOREM_LEDGER_DRAFT3.md                                        - stage 4 + Phase C + pack 03: harness correction + stochastic cache model
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/draft4/text/02_RACC_Q99_Causal_Caching_Draft4.txt                               - stage 4 + Phase C + pack 03: Draft 4 Q99 results omitted from Draft 5
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft2/CLAIM_LEDGER_DRAFT2.md                                          - stage 4 foundational lineage
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/extracted/Draft1/CLAIM_LEDGER.md                                                 - stage 4 foundational lineage
/tmp/racc_v6/ZeroStack_RACC_Cumulative_V6_Release/archive/ORIGINAL_RELEASES_MANIFEST.md                                                    - pack 03 source: byte-level provenance of all 12 original release ZIPs
```

**Phase D foundational lineage (41 files, retained but only after canonical architecture understood)** -- Draft 1 paper texts (12), Draft 2 paper texts (4), Wave 2 coding corpus (6), Wave 3 (3), Wave 3.5 (3), Wave 4 (6), Wave 5 (7): all under `current/model_ingest/text/pdf_text/archive/extracted/Draft1/`, `.../Draft2/papers/`, `.../full_originals/ZeroStack_RACC_Research_Release_v0.3/.../source-lineage/zerostack-wave2-s2/`, and `archive/extracted/Waves/zerostack-wave{3,3_5,4,5}/` (full list computed from CORPUS_INDEX.csv; the three normalized Wave master texts `current/model_ingest/text/pdf_text/archive/extracted/Waves/zerostack-wave{3,4,5}/...Master.txt` belong to the same lineage set). Superseded-by-notice: several earlier model-internal framings and literal one-token interpretations are superseded -- use `current/lineage/CORRECTIONS_AND_SUPERSESSIONS.md` (pack 00) as the tie-breaker.

Not retained for current work: all 263 byte-identical duplicates (read `duplicate_of` to skip), remaining Draft 5/4/3/2/1 packaging records (CITATION.cff, README, SHA256SUMS, validators, schemas, preflight stubs, LaTeX sources of superseded drafts) that neither the packs nor READ_ORDER stages reference.

## COVERAGE_CONFIRMATION

**CONFIRMED: packs 00-03 + Phase A canonical docs + Draft 6 papers + the 41-file direct-read list above = 100% (67/67) of unique current-authority readable content.**

- 67 unique current records = 26 covered by the coverage layers (pack 00: 14; pack 01: 5; pack 03: 5; Phase A adds `IMPLEMENTATION_BACKLOG_V6.csv`; Draft 6 papers add the anthology text) + 41 needing direct read.
- Numeric check re-derived from `CORPUS_INDEX.csv` (duplicate_of-empty rows): 432 unique = 67 current + 365 historical; coverage arithmetic 26 + 41 = 67. Integrity report PASS (920 hashed files, 0 mismatches) supports that no unique record is missing from disk.
- Caveat: "100% coverage" means every unique current record is assigned to a read channel. Tier 2/3/4 items are covered by reading their covered twins or by skim; only Tier 1 carries genuinely uncovered unique content. If the consuming agent reads only Tier 1 + the packs/Phase A/papers, effective content coverage is ~100% but not every *unique record* is visited -- flag for the orchestrator whether record-level completeness is required.
