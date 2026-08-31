# graphzero-coverage

GraphZero coverage certificates and three-answer query semantics.

## Three-Answer Model

Every query returns one of three mutually exclusive variants, each carrying a
`CoverageCertificate`:

- **PRESENT** — symbol found, with `evidence_ref` and coverage percentage.
- **ABSENT** — proven absent at 100 % fresh coverage.
- **UNKNOWN** — unproven absence; gap list names unindexed or stale blobs.

## Types

- `Bitmap`: dense per-blob, per-tier coverage bits.
- `CoverageCertificate` — per-tier percentage, freshness flag, gap list, timestamp.
- `QueryResult` — `Present { evidence_ref, certificate }`, `Absent { certificate }`, `Unknown { certificate }`.
- `CoverageIndex` trait: storage interface for coverage and content hashes.

## Freshness

Lazy content-hash comparison at query time. `freshness_check` compares live blob
bytes to the stored `ContentHash`. Stale blobs downgrade `ABSENT` to `UNKNOWN`.

## Usage

```rust
use graphzero_coverage::{
    MockCoverageIndex, QueryResultBuilder, Tier, freshness::EmptyProvider,
};

let index = MockCoverageIndex::new();
let result = QueryResultBuilder::new(&index, Tier::A)
    .found("z://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#B10-20".into())
    .build(&EmptyProvider);
```

