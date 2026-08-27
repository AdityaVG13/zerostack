# Shared derivation provenance — GraphZero emitter notes

Bead: `graphzero-iubq` (migrate after TokenZero freeze). Prior: `graphzero-3wbh` / `.1` / `.2` / `.3`.

GraphZero emits the TokenZero-owned freeze `zerostack.derivation-provenance`.
This directory is **not** a second freeze home; it documents GraphZero attach
paths and opt-in. Frozen `zerostack.cas-gc.legacy` is unchanged and orthogonal.

## Coordination (TokenZero)

- Freeze id: `zerostack.derivation-provenance`
- Freeze SHA: `04b9db5` (`feat(schema): freeze zerostack.derivation-provenance`)
- Canonical bundle: TokenZero `schemas/derivation-provenance/v1/`
- Tracking bead: `tokenzero-cas-gc-vnext-provenance-2yis`
- Frozen CAS-GC remains: TokenZero `schemas/shared-cas-gc/v1/` (`tokenzero-9ap`)
- Retired proposal tag: `zerostack.cas-gc.vnext-provenance` (do not emit)

## Record

`schema_version: "zerostack.derivation-provenance"`

`record_type: "derivation-provenance"`

Fields:

| Field | Meaning |
| --- | --- |
| `row_id` | 64-hex SHA-256 identity of the derivation |
| `derived_kind` | e.g. `graph_edge`, `outline_span`, `semantic_chunk`, `query_capsule` |
| `derived_ref` | expandable evidence ref (`gz://blob/<hash>#B…` or `gz://query/<id>`) |
| `source_blob_digest` | lowercase 64-hex source blob |
| `byte_span` | `{start,end}` byte offsets in the source blob |
| `line_span` | optional 1-based inclusive lines |
| `producing_engine` | `graphzero` |
| `producing_commit` | engine commit / package pin |
| `transform_id` | e.g. `graphzero.overlay.extract_edges.v1`, `graphzero.indexer.shard_edges.v1`, `graphzero.outline.extract_spans.v1`, `graphzero.semantic.extract_chunks.v1`, `graphzero.capsule.build.v1` |

## Storage

Engine-private (outside `gc/roots|pins|leases`):

`<store-root>/graphzero/provenance/<row_id>.json`

## Opt-in

`GRAPHZERO_PROVENANCE=1` or `ZEROSTACK_PROVENANCE=1`.

## Surfaces

- Attach: worktree overlay edges + defs; full-index shard/global CSR edges + def spans; query-capsule spills
- Query: `lookup_by_derived_ref` / `why_for_evidence_ref` (used by `verify`)
- Doctor: `orphaned_derivations` when source blob is missing
