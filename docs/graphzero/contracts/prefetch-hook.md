# TokenZero PrefetchHook contract (cross-engine)

GraphZero produces ranked next-fault candidates from blast radius; TokenZero's
working-set cache consumes them to prefetch. This is the wire contract between
the two engines.

Evidence for the ranking: `benchmarks/blast_prefetch_oracle/`
(methodology, replay corpus, published report).

## Direction

GraphZero (producer) -> TokenZero working-set `PrefetchHook` (consumer). One-way;
GraphZero does not read cache state back.

## Payload

```json
{
  "schema_version": 1,
  "source": "graphzero.blast_prefetch_oracle",
  "k": 5,
  "touched_symbols": ["parse_intent"],
  "candidates": [
    {
      "path": "crates/graphzero-query/tests/dispatcher_coverage.rs",
      "score": 12.5,
      "callers": 5,
      "proximity": 0.25
    }
  ]
}
```

| Field | Contract |
|---|---|
| `source` | producer identity; consumers may key policy off it |
| `k` | how many candidates the producer intends to be prefetched |
| `touched_symbols` | the edit that triggered scoring, for attribution |
| `candidates` | descending `score`, ties broken by `path` ascending |
| `path` | repo-relative, forward-slash |
| `score` | `callers x proximity x (1 + recent_change_frequency)` |
| `callers` | break sites resolving to this path |
| `proximity` | `max(confidence / (1 + hop))`, in `(0, 1]` |

## Consumer obligations

- Treat candidates as **advisory**. A prefetch miss must never be an error; the
  measured fault-rate is 0.4143, so most events still surface unpredicted files.
- Honour ordering. The ranking is the signal; re-sorting discards it.
- Cap admission at `k`. The producer may send more candidates than `k` so the
  consumer can extend its window, but the published metrics only hold at `k`.
- Prefetch must be evictable under normal cache policy. The hook adds hints, not
  pins.

## Producer obligations

- Deterministic for a given snapshot and event: same input, same order.
- `score` is comparable only within one payload; it is not an absolute scale.
- Candidates are graph-derived and may be stale relative to the working tree if
  the index has not been refreshed.

## Baseline and gate

The temporal-only LRU policy (most-recently-faulted, no graph signal) is the
baseline at 0.3000. The graph oracle is gated at >=20% relative lift over it and
currently measures 38.10%. If the lift regresses below the gate,
`cargo test -p graphzero-cli --test blast_prefetch_oracle_gate` fails and the hook
should be considered unproven rather than silently degraded.
