# DEFINE: Checkout CAS dedup (shared-store second checkout)

Scenario card for `benchmarks/cas_dedup.py` / artifact `benchmarks/cas-dedup.json`.
Related seeds: fszero-zjt, fszero-qhz (acceptance history).

## Name

**cas-dedup** — two identical checkouts share one explicit store root with CAS
opt-in (`blobs/` pre-created). Checkout A cold-ingests first (all blobs minted);
checkout B repeats the same corpus on the shared store. B must mint zero new CAS
objects.

## Inputs

| Input | Value |
|---|---|
| Corpus generator | `benchmarks/gen_corpus.py` |
| Default file count | `2000` (`--files`) |
| Seed | `42` (deterministic) |
| Store | Temp `shared_store` with `blobs/` mkdir = **shared-store / CAS opt-in** |
| Binary | `$FSZERO_BIN` or `target/release-perf/fszero` |
| Ingest drive | `fszero codemode 'return{ok:true}'` with `FSZERO_STARTUP_INDEX=1` |

Two tree copies: `checkout_a`, `checkout_b` (mtime-preserving `copytree` of the
same generated corpus). Both set `ZEROSTACK_STORE_ROOT` to the same shared store.

## Expected outputs (integrity)

| Field | Contract |
|---|---|
| `objects_after_a` | Object count under `shared_store/blobs/sha256/**` after A |
| `objects_after_b` | Same path after B |
| `new_objects_from_b` | **Must be `0`** (`objects_after_b - objects_after_a`) |
| Integrity gate | Runner `SystemExit` if `objects_after_b != objects_after_a` |

Published JSON also records `git_commit`, `date`, `files`, wall times, and an
`honest_note` on warm-start accounting (B still may pay hashing/index work; the
gate is object-count identity, not zero work).

## Success metrics

### Product gate (enforced)

- **Integrity only:** `new_objects_from_b == 0` (B objects == A objects).
- No other numeric field fails the process.

### Wall time / throughput (observational — not a product budget)

| Metric | Meaning | Gate |
|---|---|---|
| `checkout_a_wall_s` | Cold ingest wall for A | **Observational** — no absolute or ratio budget |
| `checkout_b_wall_s` | Second checkout wall for B | **Observational** — no absolute or ratio budget |

Wall times are published for human comparison and research (example artifact:
A ~15.6s, B ~0.04s on M-class host at 2000 files). They are **not** CI or
release gates. A product wall budget (e.g. B ≤ X% of A) would require a
separate bead if one is intended.

## Reproduce

```bash
# release-perf binary required
./scripts/profile_build.sh -p fs-zero --bin fszero
python3 benchmarks/cas_dedup.py [--files 2000]
# writes benchmarks/cas-dedup.json and prints the same payload
```

## Non-goals

- Cross-machine artifact import (see `docs/design/team-shared-warm-store.md`; designed, not built).
- Claiming B does zero CPU — only zero *new CAS objects*.
- Encoding a wall-time pass/fail in the runner without an explicit product decision.

## Reference artifact shape

See committed `benchmarks/cas-dedup.json` for a historical run (files=2000,
`new_objects_from_b=0`, honest_note present). Numbers there are fingerprints of
one machine/commit, not success thresholds.
