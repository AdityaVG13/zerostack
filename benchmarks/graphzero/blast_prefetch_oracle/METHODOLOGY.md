# Blast-as-prefetch oracle methodology

This benchmark tests whether GraphZero blast radius is a useful prefetch oracle:
given the symbols an edit just touched, can the graph predict which test files
are about to break better than a temporal-only (LRU) policy can?

## Corpus

`corpus.jsonl` holds 70 replay events captured by `build_corpus.py` from the
GraphZero self-repo git history. An event is one real commit that changed both
source and test files:

| Field | Meaning |
|---|---|
| `event_id` / `commit` / `date` | provenance back to the real commit |
| `touched_symbols` | `fn` names appearing in the commit's source-side diff hunks |
| `faults` | the test files that commit actually had to change (ground truth) |
| `graph_candidates` | blast break sites resolved to test files, with `callers` and `proximity` |

Faults are observed, not predicted: a commit that changed a test file is treated
as that test having needed to change. This is a proxy for "the test broke" - the
repo does not retain per-commit red/green CI history, and this proxy is the
honest available substitute. It is stated here rather than hidden in the score.

Candidates come from the live index: `build_corpus.py` calls
`graphzero blast --intent 'change signature of <symbol>'` and expands the
returned `q:` ref to a full `BlastRadiusCapsule`. Capture therefore needs a
built `target/release/graphzero` and a warm store; scoring does not.

### Why break sites and not `covering_tests`

The capsule's `covering_tests` field is degenerate on this repo: it returned the
same 5 paths for all 368 symbol lookups in the corpus, giving a 17.1% ceiling -
below the 30.0% temporal baseline. `break_sites` are symbol-specific and reach
an 82.9% ceiling once resolved to test files. The oracle therefore scores
break sites. This is a real limitation of `covering_tests`, recorded here rather
than worked around silently.

Resolution from break-site symbol to test file is textual: a test file that
mentions the symbol is treated as covering it. That over-collects (a comment
mentioning a symbol counts), which is why `callers` and `proximity` are needed
to rank rather than just filter.

Break sites whose `confidence / (1 + hop)` weight is zero are dropped at capture:
they carry no ranking signal and would enter the candidate list in arbitrary
order. Keeping them scored 0.4714 (57.14% lift); dropping them scores 0.4143
(38.10% lift). The lower, honest number is the published one.

## Arms

Both arms see only the current event's candidates plus history from strictly
earlier events, so there is no lookahead.

- `graph_blast_oracle` - ranks candidates by
  `callers x proximity x (1 + recent_change_frequency)`, where `callers` counts
  break sites in this event resolving to the file, `proximity` is
  `max(confidence / (1 + hop))` over those break sites, and frequency counts
  prior faults on that file. This is the bead's
  `callers x test_coverage x recent_change_frequency` ranking.
- `temporal_only` - the baseline: most-recently-faulted test files, LRU order,
  no graph signal.

## Metrics

- **fault-rate at k=5** - share of events whose actual faults intersect the
  top-5 prefetch set.
- **competitive ratio** - measured against the offline optimum, an omniscient
  policy that always prefetches the actual faults and so scores 1.0. The ratio
  is therefore each arm's fault-rate.

Published result: `graph_blast_oracle` 0.4143 vs `temporal_only` 0.3000, a
38.10% relative lift, against a >=20% gate.

Every event each arm missed is listed in that arm's `losses` array in
`report.json`. A report that dropped its losing events would be invalid.

## Reproducing

```bash
# scoring only, from the committed corpus (no graph, no binary needed)
python3 benchmarks/blast_prefetch_oracle/run.py

# gate: report must be fresh and still clear the lift bar
python3 benchmarks/blast_prefetch_oracle/run.py --check
cargo test -p graphzero-cli --test blast_prefetch_oracle_gate

# re-capture the corpus from git history (needs the release binary + warm index)
cargo build --release
python3 benchmarks/blast_prefetch_oracle/build_corpus.py
```

## Freshness manifest

`report.json` records the SHA-256 of the corpus, the capture script, the scorer,
and this methodology file. `--check` recomputes the whole report and compares it
byte-for-byte, so any change to corpus, scorer, or methodology fails the gate
until the report is regenerated.

## Consumer

The scored candidates feed the TokenZero working-set `PrefetchHook`; the
cross-engine contract is in `docs/contracts/prefetch-hook.md`.
