# Design: daemon-held IndexData delta-apply for 1-file watch reindex

Status: accepted-as-design-residual (implementation is graphzero-93570).  
Related: materialize_index_data_from_sidecar full rebuild cost on watch batches.

## Problem

`collect_changed_paths` only reparses changed paths, but
`materialize_index_data_from_sidecar` walks **every** sidecar file and clones all
defs/tier_a/scan edges into a fresh `IndexData` before `write_snapshot`. A
one-file watch therefore pays O(|repo files| + |defs| + |edges|).

## Goals

1. 1-file watch reindex does **not** re-clone all prior defs/edges.
2. Correctness: known_sig refresh remains sound; snapshot after delta equals
   full materialize within declared truth scope.
3. Fail closed: if prior IndexData is missing or sig-skewed, fall back to full
   materialize (never silent under-apply).

## Non-goals

- Minimal exact invalidation without support certificates (belongs to zkxo1/bm4s).
- Changing write_snapshot on-disk format.

## Design

### Held state (daemon / long-lived indexer)

```text
struct IncrementalIndexState {
  known_sig: String,
  data: IndexData,                 // last published materialization
  path_to_blob: HashMap<PathBuf, ContentHash>,
  // optional reverse: blob -> def/edge ranges for O(1) remove
}
```

- Populated after cold `index_repo` or first full materialize.
- Cleared on process restart (fresh open must full-build once).

### Per-path remove/apply

On watch batch `changed: Vec<Path>`:

1. For each path removed/deleted: drop defs/edges whose blob path maps to it;
   remove blob meta.
2. For each path created/modified: re-parse sidecar (or extract), insert new
   defs/edges for that blob only.
3. Re-run **global** appenders only when their inputs may change:
   - cargo/API/bead edges: re-run if any Cargo.toml / public API path changed,
     else reuse prior appends.
   - tier-C git history: unchanged unless git identity/bookmark changed.
4. If `known_sig` drifts vs prior: **full** materialize (fail-closed refresh).

### Correctness tests (must ship with 93570)

| Test | Assert |
|------|--------|
| 1-file edit | def/edge clone count for unchanged files is 0 (counter or mock) |
| parity | IndexData after delta == IndexData after full materialize on same tree |
| known_sig skew | forces full rebuild |
| delete path | edges/defs for that path absent after batch |

### When full rebuild is still required

- Process start / empty held state
- known_sig mismatch
- Sidecar format version bump
- Corruption / missing path map

## Acceptance for this design bead

- This document committed under `docs/design/`.
- graphzero-93570 remains blocked until an implementation bead lands against
  this design (or this design is revised by operator).
