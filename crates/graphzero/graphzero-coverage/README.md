# graphzero-coverage

GraphZero P0.3 — Coverage Machinery.

## Three-Answer Model

Every query returns one of three mutually exclusive variants, each carrying a
`CoverageCertificate`:

- **PRESENT** — symbol found, with `evidence_ref` and coverage percentage.
- **ABSENT** — proven absent at 100 % fresh coverage.
- **UNKNOWN** — unproven absence; gap list names unindexed or stale blobs.

## Types

- `Bitmap` / `Bitmap` — dense per-blob per-tier bitset (64 categories per tier).
- `CoverageCertificate` — per-tier percentage, freshness flag, gap list, timestamp.
- `QueryResult` — `Present { evidence_ref, certificate }`, `Absent { certificate }`, `Unknown { certificate }`.
- `CoverageIndex` trait — integration surface with P0.1 snapshot store.

## Freshness

Lazy content-hash comparison at query time. `freshness_check` compares live blob
bytes to the stored `ContentHash`. Stale blobs downgrade `ABSENT` to `UNKNOWN`.

## Usage

```rust
use graphzero_coverage::{QueryResultBuilder, Tier, MockCoverageIndex};

let mut index = MockCoverageIndex::new();
// ... populate index ...

let result = QueryResultBuilder::new(&index, Tier::A)
    .found("gz://blob/abc#B10-20".into())
    .build(&EmptyProvider);
```

## Tests

```bash
cargo test -p graphzero-coverage --all-targets
cargo bench -p graphzero-coverage --bench coverage_bench
cargo clippy -p graphzero-coverage -- -D warnings
```
