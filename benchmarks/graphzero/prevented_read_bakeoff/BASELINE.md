# Prevented-read bake-off — BASELINE

Graph-guided navigation vs grep+read loops vs hybrid, on fixed gold tasks
(`gold.json`, gold_version 1) over an isolated copy of this repository's
`crates/` + `docs/` + `README.md` (the harness and gold set are excluded from
the corpus so they cannot contaminate results).

- Corpus commit: `8f1956e75389ae956301c19dca4a56a7c2b6e232`
- Hardware: arm64 / Darwin (arm)
- Regenerate: `cargo build --release -p graphzero-cli && python3 benchmarks/prevented_read_bakeoff/run.py --write`
- Verify reproducibility: `python3 benchmarks/prevented_read_bakeoff/run.py --check` (byte metrics are deterministic; the check requires exact equality)

## Totals (4 tasks)

| Arm | Bytes read | Visible tokens | Files opened | Turns | Correct |
| --- | ---: | ---: | ---: | ---: | ---: |
| rg_read | 227,562 | 56,893 | 12 | 16 | 4/4 |
| graph_only | 72,007 | 18,004 | 4 | 9 | 3/4 |
| hybrid | 72,667 | 18,168 | 6 | 7 | 4/4 |

## Per task (bytes read / correct)

| Task | rg_read | graph_only | hybrid |
| --- | ---: | ---: | ---: |
| def_blast_radius | 129,742 (ok) | 721 (ok) | 560 (ok) |
| callers_apply_fragment | 47,985 (ok) | 1,167 (ok) | 1,871 (ok) |
| blast_apply_fragment_edit | 47,985 (ok) | 69,925 (ok) | 70,114 (ok) |
| word_rare_literal | 1,850 (ok) | 194 (WRONG) | 122 (ok) |

## Where the graph wins

- `def_blast_radius`: 721 B vs 129,742 B — a budgeted snap + one widened
  evidence window replaces reading every grep candidate file (~180x fewer bytes).
- `callers_apply_fragment`: 1,167 B vs 47,985 B — caller edges with byte-span
  evidence replace opening three whole files (~41x fewer bytes).

## Losses (published, not hidden)

- **word_rare_literal** — loser `graph_only`: rg wins on rare literals: one candidate file, byte-minimal; the graph word surface is symbol-oriented and does not return this hit
- **blast_apply_fragment_edit** — loser `graph_only`: the budget-8 blast JSON (break sites + per-hop provenance) costs more bytes than a shallow grep scan; the graph pays for transitive impact evidence this gold does not require
- **(all)** — loser `graph_only`: one-time index cost is not free; reported in index_cost and amortized only across repeated queries

## Integrity

- Gold tasks are versioned in `gold.json` and were fixed before measurement;
  the harness records every executed command in `report.json`.
- All three arms answer the same gold tasks; correctness is asserted against
  the same gold facts per arm.
- The graph arms include the visible JSON they consume; the one-time index
  cost is reported separately in `report.json` (`index_cost`).

