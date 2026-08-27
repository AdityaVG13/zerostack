# Reserve API `.clone()` audit (no code changes)

Scan date: **2026-08-15**. Method: count `.clone()` in
`crates/graphzero-reserve/src/**/*.rs` (production sources; test-module clones
called out where present).

**This bead does not change reserve code.** No speculative clone removal
without a measured allocation profile. The reservation golden gate remains
unchanged.

## Counts

| File | `.clone()` total | Prod (approx) | In `#[cfg(test)]` (approx) |
| --- | ---: | ---: | ---: |
| `service.rs` | 19 | 19 | 0 |
| `ledger.rs` | 9 | 5 | 4 |
| `footprint.rs` | 7 | 7 | 0 |
| `schema.rs` / `lib.rs` | 0 | 0 | 0 |
| **Total** | **35** | **~31** | **~4** |

Crate LOC (`wc -l` `src/*.rs`): **1191** (of which `service.rs` 580).

## Classification (advisory; unmeasured)

### Likely unavoidable ownership (API / ledger semantics)

These clones feed owned response structs, HashMap keys, or ledger tip
snapshots where the callee returns `'static`-free owned data to callers:

- **Response construction in `service.rs`:** cloning `footprint_ref`,
  `evidence_refs`, `contract_nodes`, `reservation_id`, `agent_id`, and
  conflict `node` / `evidence_ref` into declare/check/conflict payloads.
  Callers outside the crate need owned strings/vecs; returning references
  would expand lifetimes across the service API.
- **Ledger tip index in `ledger.rs`:**
  `tip.insert(rec.reservation_id.clone(), rec.clone())` (and refresh paths)
  keep an owned latest-record map beside the append-only log. Removing the
  record clone requires a different storage shape (e.g. indices into an
  arena), not a one-line `&` swap.
- **Footprint aggregation in `footprint.rs`:** inserting owned node/evidence
  strings into sets while walking ops -- typical owned-collection building.

Treat these as **default keep** until a measured pass shows they dominate
allocator traffic on declare/check/release hot paths.

### Possibly removable (candidates only)

Candidates for a *future* measured pass -- **do not remove in this bead**:

- Intermediate `existing.clone()` / `refreshed` copies in refresh paths if
  mutation can happen in place behind the ledger lock.
- Cloning both a map key and a full `ReservationRecord` when an
  `Arc<ReservationRecord>` or interned id table would suffice.
- Building `evidence_refs` via `edges.iter().map(|e| e.evidence_ref.clone())`
  if the response could borrow from an arena tied to the request.

Any removal must preserve:

1. Ledger replay determinism (`replay_ledger` / tip hash).
2. Conflict reporting fields (blocking reservation ids, evidence refs).
3. **Golden gate unchanged:** `tests/cli/reservation_golden_gate.rs` +
   `tests/cli/reservation_golden.jsonl` (see also `docs/benchmarks.md`
   reservation golden gate section).

## Golden gate status

**Unchanged.** This audit is documentation only; no reserve source edits,
no golden JSONL edits, no gate threshold edits.

## Next measurement (out of scope here)

On Spark with `/tmp/rch_target_graphzero`, profile
`ReserveService::{declare,check,release}` under the existing
`crates/graphzero-reserve/benches/reserve_check.rs` / contract tests before
landing clone removals. Numbers today: **unmeasured / advisory**.
