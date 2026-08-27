# ADR 010 — Entity-addressed refs: dedup knowledge, not bytes

- **Status:** Accepted (foundation + index-time minting + view emission + entity-aware SeenProvider + cross-view dedup ledger + TokenZero shared novelty fusion)
- **Bead:** `graphzero-entity-refs-lfoo` / `.1` / `.2` / `.3` / `.4` / `.5`
- **Spec:** ZeroStack `2026-07-15-marshot-decision-entropy-ACTIVE.md` §5
- **Implementation:** `crates/graphzero-store/src/store/entity.rs`, `GzRef::Entity`,
  index publish in `indexer::write_snapshot`, session novelty in `session.rs`,
  dedup ledger + `REPEAT_ENCOUNTER_PCT` gate,
  shared novelty fusion in `entity_novelty_fusion.rs` (`zerostack.entity-novelty`)

## Decision

GraphZero gains an engine-owned **entity layer** over the CAS. Refs address
**facts** (symbol X at defining content digest D), not presentation costumes.
Byte-level views (`gz://blob/…#B…`, grep/diff/trace capsules, `gz://node/…`)
link to their entity. Novelty upgrades from "seen these bytes?" to "know this
fact?".

Portable ZeroRef v1 remains **blob-only**. `gz://entity/<64-hex>` is
GraphZero-owned, like `gz://node/…` and `gz://mem/…`. Foreign engines must
treat it as `unsupported`.

## Grammar

```text
gz://entity/<sha256>
```

- `<sha256>` is exactly 64 lowercase hex digits (SHA-256 of the canonical
  entity key preimage).
- No fragments. Fragments belong on byte views, not on the fact identity.

## Identity

Canonical key (versioned):

```text
v1 \0 {kind} \0 {symbol} \0 {content_digest}
```

- `kind`: `symbol` (v1)
- `symbol`: qualified GraphZero symbol spelling
- `content_digest`: full lowercase SHA-256 of the defining content (blob or
  span bytes)

`EntityId = SHA-256(preimage)` as lowercase hex. Same symbol + same defining
bytes ⇒ same entity across read, grep, diff, trace, and blast views.

## Index-time minting (`.1`)

At snapshot publish (`write_snapshot`), each def span mints an entity from
`symbol` + SHA-256 of defining bytes (`block_start..block_end` when present,
else name span). Links:

- `gz://node/<symbol>` → `EntityViewKind::Node`
- `gz://blob/<hash>#B<name_start>-<name_end>` → `EntityViewKind::Read`

Sidecar: `shards/entities_{snapshot_id:08}.json` (`graphzero.entities.v1`).
Process registry is updated at mint; `expand gz://entity/<id>` hydrates from
the latest sidecar when the process registry misses.

## Novelty billing

- First encounter of an entity bills the full view token cost.
- Later encounters of the **same entity** (any view) bill at most
  `ceil(first * 10 / 100)` tokens (Mars target ≤10% of first encounter).
- Byte-level `SeenKey` remains a blob + `#B` span identity. `EntityAwareSeenProvider`
  composes `EntityNovelty` with `LocalSeenProvider` / TokenZero adapter so
  destination ranking and session dedup treat know-this-fact in addition to
  seen-these-bytes.

## Cross-view dedup ledger (`.4`)

`EntityDedupLedger` (`graphzero.dedup_ledger.v1`, path
`telemetry/dedup_ledger.json`) tracks naive token mass, mass after byte dedup,
and mass after entity novelty. Rates sit side-by-side:

- `byte_dedup_rate_pct`
- `entity_cross_view_dedup_rate_pct`
- `max_repeat_encounter_pct` (gate: ≤ `REPEAT_ENCOUNTER_PCT` = 10)

Session `apply_seen_to_destinations` classifies removals as `byte_deduped` vs
`entity_deduped`. Capsule JSON ledger embeds the rate fields. Focused gate:
`cargo test -p graphzero-store --test entity_cross_view_dedup_gate`.

## Expand

`graphzero expand gz://entity/<id>` returns the `EntityRecord` JSON when the
entity has been linked (process registry or published sidecar); otherwise
`entity_not_found`. Expand does not invent bytes from the id alone.

## Non-goals (remaining children)

- ~~Automatic linking from query / snap / trace emission (`.2`)~~ **Done**:
  `link_emitted_view` / `link_emitted_symbol_view` at read capsule, grep hit,
  delta outline (diff), trace record/callpath, and blast node emission.
- ~~Upgrading `SeenProvider` to entity-aware novelty (`.3`)~~ **Done**:
  `EntityAwareSeenProvider` is the default; TokenZero adapter shares the same
  composition. Byte `SeenKey` preserved.
- ~~Cross-view dedup ledger metric and ≤10% gate (`.4`)~~ **Done**:
  `EntityDedupLedger` + `REPEAT_ENCOUNTER_PCT` gate.
- ~~Cross-engine TokenZero fusion of the novelty ledger (`.5`)~~ **Done**:
  `zerostack.entity-novelty` shared pointer under
  `<store-root>/shared/entity-novelty/v1/` + optional SharedCas snapshot
  (`cas_digest`). Entity ids remain GraphZero `EntityId` hex only;
  refs stay `gz://entity/<id>` (no `tz://entity/`). TokenZero owns the
  freeze at `tokenzero/schemas/entity-novelty/v1/`; GraphZero
  `TokenZeroSeenAdapter` hydrates/flushes when `ZEROSTACK_STORE_ROOT` is set.

## Consequences

- Agents can hold one fact id and recover linked views without re-paying for
  the same knowledge under a new costume (once emission is wired).
- Cross-view dedup rate is measurable alongside byte dedup.
- Entity refs must never be advertised as ZeroRef v1 portable blobs.
- TokenZero and GraphZero share known-entity novelty over the same store/CAS
  without a second entity namespace.
