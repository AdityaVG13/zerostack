# Protobuf 3.7 migration plan (`graphzero-scip`)

Status: **plan only**. This bead does not bump dependencies.

## Current state

- Crate: `crates/graphzero-scip`
- Dependency: `protobuf = "3.7"` in `crates/graphzero-scip/Cargo.toml`
- Also depends on `scip = "0.8.1"` (SCIP types generated against the
  protobuf crate API)
- Decode entry point: `crates/graphzero-scip/src/decode.rs` uses
  `protobuf::Message` and `scip_types::Index::parse_from_bytes`
- Purpose: Tier-B SCIP ingest -- decode SCIP index protobuf into
  witness-labeled edges (`lib.rs` module docs)

### LOC estimate (measured 2026-08-15, source only)

| Path | LOC (`wc -l`) |
| --- | ---: |
| `src/decode.rs` | 106 |
| `src/ingest.rs` | 544 |
| `src/publish.rs` | 92 |
| `src/types.rs` | 76 |
| `src/lsp.rs` | 39 |
| `src/lib.rs` | 12 |
| **Total `src/*.rs`** | **869** |

Migration blast radius is mostly `decode.rs` plus any `Message` / generated
type touchpoints in `ingest.rs`. Expect roughly **~100–200 LOC** of direct
decode/API churn if staying on protobuf-rs v4, or a similar band if switching
to `prost` (plus regenerated SCIP bindings). Full-crate rewrite is not
required; ingest/publish logic can stay once decode returns equivalent
`Index`-shaped data.

## Options

### A. Stay on protobuf-rs, move 3.7 → 4.x

- Pros: smallest conceptual change if `scip` crate publishes protobuf-v4
  compatible types.
- Cons: blocked on upstream `scip` / generated stubs; protobuf 3.x is
  legacy and will keep dragging advisories/tooling friction.

### B. Migrate decode to `prost` (+ regenerated SCIP `.proto` stubs)

- Pros: idiomatic modern Rust protobuf path; clearer long-term ownership of
  generated code in-tree or via a thin crate.
- Cons: need owned/regenerated SCIP message types; must preserve byte-level
  decode compatibility with existing SCIP index fixtures.

**Recommendation (advisory):** prefer **B** if upstream `scip 0.8.x` stays
on protobuf 3.x; prefer **A** only when a protobuf-v4-capable SCIP crate
release exists and is pin-auditable. Do not mix both stacks in one binary
without an explicit dual-decode shim.

## Decode compatibility test strategy

Do **not** treat a green unit compile as proof. Required gates before any
dependency bump lands:

1. **Fixture golden (existing):** keep `tests/scip/fixture_golden.rs`
   (`[[test]] name = "scip_fixture_golden"`) passing bit-for-bit on committed
   SCIP fixtures -- document count, symbol count, relationship count from
   `ScipDecoded`, and edge/witness labels produced by ingest.
2. **Byte decode parity (new, when migrating):** for each fixture under
   `tests/scip/` (and any committed `.scip` / protobuf blobs), decode with
   old and new stacks (or before/after in a temporary dual-path) and assert
   equal `ScipDecoded` summaries plus stable digest of normalized document /
   symbol / relationship walks.
3. **Round-trip smoke (optional):** where fixtures include encode helpers in
   `decode.rs` tests, ensure `write_to_bytes` / `parse_from_bytes` still
   round-trip under the new stack.
4. **No silent schema drift:** fail closed if `scip` / proto package versions
   change message field numbers; record package versions in the migration PR.

Out of scope for this plan doc: actually changing `Cargo.toml`, regenerating
bindings, or claiming advisory clearance.

## Non-goals

- No dependency bump in this change set.
- No performance claims about decode throughput (unmeasured).
