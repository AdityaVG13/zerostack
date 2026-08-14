# V6 vs GraphZero Grade-Name Mapping (decision record)

Status: **decided 2026-08-14** (hygiene bead zerostack-mayx). Applies to
`racc/v6/` specs and the GraphZero truth authority.

## The two vocabularies

| V6 canonical (racc/v6) | GraphZero (graphzero-engine) | Meaning |
|---|---|---|
| `Proved` | `Complete` | full proof of the property |
| `BoundedComplete` | `SoundOverapproximation` | complete within a declared bound |
| `Observed` | `ObservedOnly` | observed on the current project state, no bound |
| `Unknown` | `Unknown` | no usable evidence; terminal-epistemic |

## Divergence (why they are NOT free aliases)

GraphZero is STRICTER than V6 in one authority-relevant direction:
`SoundOverapproximation` **cannot certify absence** (a sound over-approximation
may over-report, so "no matches found" is not a certified absence), whereas V6
`BoundedComplete` permits absence certification within the declared bound
(`ProtectedScopeObligationsV1::equivalent_claim_permitted` admits
`BoundedComplete`).

Mapping rule:
- `Complete -> Proved`, `ObservedOnly -> Observed`, `Unknown -> Unknown` are
  lossless.
- `SoundOverapproximation -> BoundedComplete` is lossy for ABSENCE
  certification: a GraphZero `SoundOverapproximation` grade may be mapped to
  V6 `BoundedComplete` for POSITIVE claims only. Absence claims certified
  under `BoundedComplete` must NOT be fed back into GraphZero as
  `SoundOverapproximation`-certified unless the declared bound is present and
  the over-approximation is exact for the queried absence.
- V6 `Observed` never maps to GraphZero `Complete`/`SoundOverapproximation`
  (no promotion), and `Unknown` never maps to anything else (terminal).

## Enforcement points

- Hub side: `ProtectedScopeObligationsV1` (zero-abi `identity.rs`) keeps the
  V6 vocabulary and its `check_equivalent_claim` law.
- GraphZero side: `graphzero-engine` keeps its own grade names; the mapping
  above is the documented bridge, applied ONLY at composition boundaries
  (zsx-core aggregate connector), never inside either authority.

## Pending

- A cross-repo conformance vector (V6 grade JSON -> GraphZero grade JSON) is
  deferred until the aggregate connector consumes GraphZero grades in a
  production path (W2 wiring follow-up).
